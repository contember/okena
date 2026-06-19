//! macOS process introspection via `libproc` syscalls.
//!
//! Replaces fork+exec of `pgrep` / `lsof` / `ps` used for child-process
//! detection, dtach socket → pid resolution and port detection. Each external
//! command spawn costs tens of ms on macOS (dyld + AMFI/codesign checks), and
//! they used to sit on hot/UI paths. Linux reads `/proc` directly for the same
//! data; this is the in-process equivalent for macOS.
//!
//! Everything here is best-effort: a libproc error for one pid (e.g. a process
//! that exited mid-scan, or one we lack permission to inspect) is skipped, not
//! propagated — mirroring how the `pgrep`/`lsof` fallbacks silently ignored
//! such cases.

use libproc::libproc::bsd_info::BSDInfo;
use libproc::libproc::file_info::{pidfdinfo, ListFDs, ProcFDType};
use libproc::libproc::net_info::{SocketFDInfo, SocketInfoKind, TcpSIState};
use libproc::libproc::proc_pid::{listpidinfo, name, pidinfo};
use libproc::processes::{pids_by_type, ProcFilter};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

/// Direct child pids of `ppid` — equivalent to `pgrep -P <ppid>`.
pub fn child_pids(ppid: u32) -> Vec<u32> {
    pids_by_type(ProcFilter::ByParentProcess { ppid }).unwrap_or_default()
}

/// Whether `ppid` has any direct child process.
pub fn has_children(ppid: u32) -> bool {
    !child_pids(ppid).is_empty()
}

/// The first direct child pid of `ppid` (lowest pid, for determinism). Used to
/// walk from a dtach daemon down to the actual shell process.
pub fn first_child_pid(ppid: u32) -> Option<u32> {
    child_pids(ppid).into_iter().min()
}

/// Best-effort short accounting name of a process (e.g. "make", "vim").
pub fn process_name(pid: u32) -> Option<String> {
    name(pid as i32).ok().filter(|s| !s.is_empty())
}

/// pid → direct-children map for every process on the system — equivalent to a
/// single `ps -eo pid,ppid` building a process tree.
pub fn process_tree() -> HashMap<u32, Vec<u32>> {
    let mut tree: HashMap<u32, Vec<u32>> = HashMap::new();
    for pid in pids_by_type(ProcFilter::All).unwrap_or_default() {
        if let Ok(info) = pidinfo::<BSDInfo>(pid as i32, 0) {
            tree.entry(info.pbi_ppid).or_default().push(pid);
        }
    }
    tree
}

/// Map each given unix-socket path to the pids that have it open — equivalent to
/// `lsof <paths>`. Scans every process's socket fds and matches the bound
/// unix-domain address against the requested paths (exact match, like the lsof
/// fallback).
pub fn pids_holding_unix_sockets(socket_paths: &[PathBuf]) -> HashMap<PathBuf, Vec<u32>> {
    let mut result: HashMap<PathBuf, Vec<u32>> = HashMap::new();
    if socket_paths.is_empty() {
        return result;
    }
    for pid in pids_by_type(ProcFilter::All).unwrap_or_default() {
        for socket in socket_fds(pid) {
            if !matches!(SocketInfoKind::from(socket.psi.soi_kind), SocketInfoKind::Un) {
                continue;
            }
            // SAFETY: `soi_kind == Un` means `pri_un` is the active union arm.
            let un = unsafe { socket.psi.soi_proto.pri_un };
            // SAFETY: `ua_sun` is the active arm of the bound-address union for a
            // unix-domain socket.
            let path = unsafe { sun_path(&un.unsi_addr.ua_sun) };
            if let Some(path) = path
                && let Some(target) = socket_paths.iter().find(|p| **p == path)
            {
                result.entry(target.clone()).or_default().push(pid);
            }
        }
    }
    result
}

/// All `(pid, local_port)` pairs for TCP sockets in the LISTEN state —
/// equivalent to the `lsof -iTCP -sTCP:LISTEN` scan used for port detection.
pub fn listening_port_pairs() -> Vec<(u32, u16)> {
    let mut pairs = Vec::new();
    for pid in pids_by_type(ProcFilter::All).unwrap_or_default() {
        for socket in socket_fds(pid) {
            if !matches!(SocketInfoKind::from(socket.psi.soi_kind), SocketInfoKind::Tcp) {
                continue;
            }
            // SAFETY: `soi_kind == Tcp` means `pri_tcp` is the active union arm.
            let tcp = unsafe { socket.psi.soi_proto.pri_tcp };
            if !matches!(TcpSIState::from(tcp.tcpsi_state), TcpSIState::Listen) {
                continue;
            }
            // insi_lport is the port in network byte order, in the low 16 bits.
            let port = u16::from_be(tcp.tcpsi_ini.insi_lport as u16);
            if port != 0 {
                pairs.push((pid, port));
            }
        }
    }
    pairs
}

/// All open socket fds of `pid`, decoded. Best-effort: empty on any libproc
/// error (process gone, permission denied, …).
fn socket_fds(pid: u32) -> Vec<SocketFDInfo> {
    let pid = pid as i32;
    let Ok(info) = pidinfo::<BSDInfo>(pid, 0) else {
        return Vec::new();
    };
    let Ok(fds) = listpidinfo::<ListFDs>(pid, info.pbi_nfiles as usize) else {
        return Vec::new();
    };
    fds.into_iter()
        .filter(|fd| matches!(ProcFDType::from(fd.proc_fdtype), ProcFDType::Socket))
        .filter_map(|fd| pidfdinfo::<SocketFDInfo>(pid, fd.proc_fd).ok())
        .collect()
}

/// Decode a bound `sockaddr_un.sun_path` into a filesystem path. Returns `None`
/// for empty / abstract sockets.
///
/// SAFETY: caller must ensure `sun` is the active arm of the address union.
unsafe fn sun_path(sun: &libc::sockaddr_un) -> Option<PathBuf> {
    let raw = &sun.sun_path;
    let len = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
    if len == 0 {
        return None;
    }
    // c_char is i8 on macOS; reinterpret the NUL-terminated path bytes as u8.
    let bytes = unsafe { std::slice::from_raw_parts(raw.as_ptr().cast::<u8>(), len) };
    Some(PathBuf::from(OsStr::from_bytes(bytes)))
}
