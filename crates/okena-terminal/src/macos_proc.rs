//! macOS process introspection via `libproc` and `sysctl` syscalls.
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
use libproc::libproc::file_info::{ListFDs, ProcFDType, pidfdinfo};
use libproc::libproc::net_info::{SocketFDInfo, SocketInfoKind, TcpSIState};
use libproc::libproc::proc_pid::{listpidinfo, name, pidinfo, pidpath};
use libproc::processes::{ProcFilter, pids_by_type};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

/// Every process currently visible to libproc.
pub fn all_pids() -> Option<Vec<u32>> {
    pids_by_type(ProcFilter::All).ok()
}

/// Direct child pids of `ppid` — equivalent to `pgrep -P <ppid>`.
pub fn child_pids(ppid: u32) -> Vec<u32> {
    pids_by_type(ProcFilter::ByParentProcess { ppid }).unwrap_or_default()
}

/// Direct child pids of `ppid`, `None` when libproc fails.
pub fn checked_child_pids(ppid: u32) -> Option<Vec<u32>> {
    pids_by_type(ProcFilter::ByParentProcess { ppid }).ok()
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

/// Best-effort short accounting name of a process (e.g. "make", "vim"): the
/// file name of its executable, symlinks resolved. For what the user ran, see
/// `terminal::process_name`.
pub fn process_name(pid: u32) -> Option<String> {
    name(pid as i32).ok().filter(|s| !s.is_empty())
}

/// Path of the executable `pid` runs, symlinks resolved.
pub fn process_path(pid: u32) -> Option<String> {
    pidpath(pid as i32).ok().filter(|path| !path.is_empty())
}

/// `argv[0]` of `pid` (via `sysctl` `KERN_PROCARGS2`), `None` when the kernel
/// refuses (another user's process) or the arguments are unreadable.
pub fn process_arg0(pid: u32) -> Option<String> {
    // Cached once read; a failed read is retried on the next call.
    static ARG_MAX: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let arg_max = match ARG_MAX.get() {
        Some(&known) => known,
        None => {
            let read = arg_max()?;
            *ARG_MAX.get_or_init(|| read)
        }
    };
    let mut buf = vec![0u8; arg_max];
    let mut size = buf.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    // SAFETY: `buf` is writable for `size` bytes; the kernel writes at most that.
    let ok = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    if !ok {
        return None;
    }
    buf.truncate(size);
    arg0_from_procargs2(&buf)
}

/// The kernel's limit on a process's arguments (`KERN_ARGMAX`), the buffer
/// `KERN_PROCARGS2` needs.
fn arg_max() -> Option<usize> {
    let mut arg_max: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: `mib` names an int-sized value and `size` holds its size.
    let ok = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&mut arg_max as *mut libc::c_int).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    (ok && arg_max > 0).then_some(arg_max as usize)
}

/// `argv[0]` from a `KERN_PROCARGS2` buffer: `argc` (a C int), the executable
/// path, NUL padding, then `argv` as NUL-terminated strings.
fn arg0_from_procargs2(buf: &[u8]) -> Option<String> {
    const ARGC: usize = std::mem::size_of::<libc::c_int>();
    let argc = libc::c_int::from_ne_bytes(buf.get(..ARGC)?.try_into().ok()?);
    if argc < 1 {
        return None;
    }
    let rest = &buf[ARGC..];
    let rest = &rest[rest.iter().position(|&b| b == 0)?..];
    let rest = &rest[rest.iter().position(|&b| b != 0)?..];
    let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    String::from_utf8(rest[..end].to_vec()).ok()
}

/// pid → direct-children map for every process on the system — equivalent to a
/// single `ps -eo pid,ppid` building a process tree.
pub fn process_tree() -> HashMap<u32, Vec<u32>> {
    let mut tree: HashMap<u32, Vec<u32>> = HashMap::new();
    for pid in all_pids().unwrap_or_default() {
        if let Ok(info) = pidinfo::<BSDInfo>(pid as i32, 0) {
            tree.entry(info.pbi_ppid).or_default().push(pid);
        }
    }
    tree
}

/// Stable process birth marker used to revalidate a PID before signalling it.
pub fn process_start_time(pid: u32) -> Option<(u64, u64)> {
    let info = pidinfo::<BSDInfo>(pid as i32, 0).ok()?;
    Some((info.pbi_start_tvsec, info.pbi_start_tvusec))
}

/// Whether the process is dead and waiting for its parent to reap it.
pub fn process_is_zombie(pid: u32) -> bool {
    pidinfo::<BSDInfo>(pid as i32, 0).is_ok_and(|info| info.pbi_status == libc::SZOMB)
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
            if !matches!(
                SocketInfoKind::from(socket.psi.soi_kind),
                SocketInfoKind::Un
            ) {
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
            if !matches!(
                SocketInfoKind::from(socket.psi.soi_kind),
                SocketInfoKind::Tcp
            ) {
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

#[cfg(test)]
mod procargs_tests {
    use super::arg0_from_procargs2;

    fn procargs(argc: i32, rest: &[u8]) -> Vec<u8> {
        let mut buf = argc.to_ne_bytes().to_vec();
        buf.extend_from_slice(rest);
        buf
    }

    #[test]
    fn arg0_is_the_first_argument_after_the_executable_path() {
        let buf = procargs(
            2,
            b"/Users/u/.local/share/claude/versions/2.1.316\0\0\0claude\0--resume\0HOME=/x\0",
        );
        assert_eq!(arg0_from_procargs2(&buf).as_deref(), Some("claude"));
        assert_eq!(arg0_from_procargs2(&procargs(0, b"/bin/x\0\0")), None);
        assert_eq!(arg0_from_procargs2(&procargs(1, b"/bin/x")), None);
        assert_eq!(arg0_from_procargs2(&[1, 0]), None);
    }
}
