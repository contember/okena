//! Who may tear down sessions in the shared dtach socket pool.
//!
//! Every Okena daemon of a user puts its `tm-*.sock` sessions into one pool,
//! keyed only by `XDG_RUNTIME_DIR` (else `/tmp/okena-<uid>`) and the profile id.
//! `HOME`, `TMPDIR` and `XDG_CONFIG_HOME` do not move it, so a second daemon
//! started from a scratch config (a test harness, a second checkout) lands in
//! the pool of the daemon the user is working in. On start a daemon kills the
//! pool sessions its own workspace does not know
//! ([`reconcile_dtach_sessions`](crate::session_backend::reconcile_dtach_sessions)),
//! so without a guard that second daemon kills every pane of the first one.
//!
//! Three guards, all independent of where a daemon keeps its control socket:
//!
//! - **Owner tags.** Every spawn or reattach writes `tm-<id>.owner` next to the
//!   socket, naming the profile directory (config dir + profile) of the daemon
//!   that runs it ([`record_session_owner`]). A daemon never kills a session
//!   tagged by another owner, whether that owner is running or not: the user's
//!   Okena may simply be closed while its panes live on.
//! - **Leases**, for sessions from before owner tags. Each daemon holds a [`DtachPoolLease`] for its lifetime: an
//!   exclusively `flock`ed file in `<pool>/leases/`. The kernel drops the lock
//!   when the process dies, however it dies, so a lock that cannot be taken
//!   belongs to a live daemon and untagged sessions are left alone.
//! - **Attached sessions.** A daemon that predates the leases holds none, but it
//!   keeps a `dtach -A <socket>` client attached to each pane it shows, and that
//!   client dies with the daemon's PTY. A session with a live client is in use
//!   by someone, so reconciliation never kills it
//!   ([`attached_dtach_sockets`]).

#[cfg(unix)]
use std::collections::{HashMap, HashSet};
#[cfg(unix)]
use std::ffi::OsString;
#[cfg(unix)]
use std::path::{Path, PathBuf};

/// Directory under the profile's pool that holds the lease files.
#[cfg(unix)]
const LEASES_DIR: &str = "leases";

/// Proof that this process takes part in the shared dtach pool. Held by the
/// daemon for its whole lifetime; dropping it unlinks the lease file, and the
/// lock goes with the file descriptor.
#[cfg(unix)]
#[derive(Debug)]
pub struct DtachPoolLease {
    path: PathBuf,
    _file: std::fs::File,
}

#[cfg(not(unix))]
#[derive(Debug)]
pub struct DtachPoolLease;

#[cfg(not(unix))]
impl DtachPoolLease {
    /// Windows has no dtach pool; the lease guards nothing.
    pub fn acquire() -> std::io::Result<Self> {
        Ok(Self)
    }
}

#[cfg(unix)]
impl DtachPoolLease {
    /// Take a lease in the active profile's pool. Per profile, so a running
    /// daemon of another profile does not stop this one from cleaning up. The
    /// default profile's reconciliation already retains every profile's known
    /// terminals among the legacy sessions it shares with them.
    pub fn acquire() -> std::io::Result<Self> {
        Self::acquire_in(&crate::session_backend::okena_runtime_dir().join(LEASES_DIR))
    }

    /// Take a lease in `dir`. The file is locked under a temporary name and only
    /// then renamed to `*.lock`, so a peer never sees an unlocked lease and
    /// mistakes it for a dead daemon's leftover.
    pub(crate) fn acquire_in(dir: &Path) -> std::io::Result<Self> {
        use std::os::unix::fs::DirBuilderExt as _;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let stem = format!("{}-{}", std::process::id(), uuid::Uuid::new_v4().simple());
        let staging = dir.join(format!(".{stem}.tmp"));
        let path = dir.join(format!("{stem}.lock"));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)?;
        let locked = file
            .try_lock()
            .map_err(std::io::Error::from)
            .and_then(|()| std::fs::rename(&staging, &path));
        if let Err(error) = locked {
            let _ = std::fs::remove_file(&staging);
            return Err(error);
        }
        Ok(Self { path, _file: file })
    }

    /// Leases in this pool held by live processes other than this lease.
    /// Lease files whose lock can be taken belong to a daemon that died
    /// without cleaning up and are removed. A lease that cannot be probed for
    /// another reason counts as live: standing down is the safe answer.
    pub(crate) fn live_peers(&self) -> Vec<PathBuf> {
        let Some(dir) = self.path.parent() else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            // Our own lease is in there; an unreadable directory hides peers.
            return vec![dir.to_path_buf()];
        };
        let mut live = Vec::new();
        for path in entries.flatten().map(|entry| entry.path()) {
            if path == self.path || path.extension().is_none_or(|ext| ext != "lock") {
                continue;
            }
            let file = match std::fs::File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    live.push(path);
                    continue;
                }
            };
            match file.try_lock() {
                Ok(()) => {
                    // Unlink while we hold the lock; the name is unique to the
                    // dead daemon, so nobody can be taking it right now.
                    let _ = std::fs::remove_file(&path);
                }
                Err(_) => live.push(path),
            }
        }
        live.sort();
        live
    }
}

#[cfg(unix)]
impl Drop for DtachPoolLease {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Who started a pool session, read from its owner tag.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionOwner {
    /// Tagged with this daemon's identity.
    Us,
    /// Tagged by another daemon, or the tag exists but cannot be read.
    Other,
    /// No tag: started before owner tags existed.
    Untagged,
}

/// Identity written into owner tags: the active profile's directory, which is
/// unique per config dir and profile and stable across restarts.
#[cfg(unix)]
pub(crate) fn owner_identity() -> Option<String> {
    okena_core::profiles::try_current().map(|profile| profile.root.to_string_lossy().into_owned())
}

#[cfg(unix)]
pub(crate) fn owner_tag_path(socket_path: &Path) -> PathBuf {
    socket_path.with_extension("owner")
}

/// Tag the session at `socket_path` as ours. Called on every spawn and
/// reattach, so sessions from before owner tags get tagged by the daemon that
/// picks them up.
#[cfg(unix)]
pub(crate) fn record_session_owner(socket_path: &Path, identity: &str) {
    if let Some(parent) = socket_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = std::fs::write(owner_tag_path(socket_path), identity) {
        log::warn!("failed to record dtach session owner for {socket_path:?}: {error}");
    }
}

#[cfg(unix)]
pub(crate) fn session_owner(socket_path: &Path, identity: Option<&str>) -> SessionOwner {
    match std::fs::read_to_string(owner_tag_path(socket_path)) {
        Ok(tag) if identity.is_some_and(|identity| tag == identity) => SessionOwner::Us,
        Ok(_) => SessionOwner::Other,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => SessionOwner::Untagged,
        Err(_) => SessionOwner::Other,
    }
}

/// The sockets among `socket_paths` that a live dtach client is attached to.
/// `None` when the process table cannot be read, in which case the caller must
/// treat every session as attached.
#[cfg(unix)]
pub(crate) fn attached_dtach_sockets(socket_paths: &[PathBuf]) -> Option<HashSet<PathBuf>> {
    if socket_paths.is_empty() {
        return Some(HashSet::new());
    }
    let argvs = dtach_process_argvs()?;
    let holders = crate::pty_manager::find_pids_for_unix_sockets(socket_paths);
    Some(attached_from(
        socket_paths,
        &holders,
        &argvs,
        std::process::id(),
    ))
}

/// Pure core of [`attached_dtach_sockets`].
#[cfg(unix)]
///
/// `dtach -A <socket>` forks the master off the client, so both carry the
/// socket path in their argv. The master is the process holding the bound
/// socket (`holders`); any other live dtach naming the socket is a client.
pub(crate) fn attached_from(
    socket_paths: &[PathBuf],
    holders: &HashMap<PathBuf, Vec<u32>>,
    dtach_argvs: &[(u32, Vec<OsString>)],
    my_pid: u32,
) -> HashSet<PathBuf> {
    let mut attached = HashSet::new();
    for (pid, argv) in dtach_argvs {
        if *pid == my_pid {
            continue;
        }
        for arg in argv.iter().skip(1) {
            let Some(socket) = socket_paths
                .iter()
                .find(|socket| same_path(socket, Path::new(arg)))
            else {
                continue;
            };
            let is_master = holders.get(socket).is_some_and(|pids| pids.contains(pid));
            if !is_master {
                attached.insert(socket.clone());
            }
        }
    }
    attached
}

/// Path equality that survives a symlinked prefix (`/tmp` → `/private/tmp`
/// on macOS) between how the pool was spelled when the session started and now.
#[cfg(unix)]
fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    if a.file_name() != b.file_name() {
        return false;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// `(pid, argv)` of every `dtach` process of this user. `None` when one of
/// them is still running but its argv cannot be read: that client could be
/// attached to any session.
#[cfg(target_os = "linux")]
fn dtach_process_argvs() -> Option<Vec<(u32, Vec<OsString>)>> {
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::MetadataExt as _;
    // SAFETY: `getuid(2)` takes no arguments and never fails.
    let my_uid = unsafe { libc::getuid() };
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let is_ours_and_dtach = entry.metadata().is_ok_and(|meta| meta.uid() == my_uid)
            && std::fs::read_to_string(entry.path().join("comm"))
                .is_ok_and(|comm| comm.trim_end() == "dtach");
        if !is_ours_and_dtach {
            continue;
        }
        let cmdline = match std::fs::read(entry.path().join("cmdline")) {
            Ok(cmdline) => cmdline,
            // Exited since the directory listing.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        };
        let argv = cmdline
            .split(|&b| b == 0)
            .filter(|arg| !arg.is_empty())
            .map(|arg| std::ffi::OsStr::from_bytes(arg).to_os_string())
            .collect();
        found.push((pid, argv));
    }
    Some(found)
}

#[cfg(target_os = "macos")]
fn dtach_process_argvs() -> Option<Vec<(u32, Vec<OsString>)>> {
    use crate::macos_proc::{all_pids, process_args, process_name, process_uid};
    // SAFETY: `getuid(2)` takes no arguments and never fails.
    let my_uid = unsafe { libc::getuid() };
    let mut found = Vec::new();
    for pid in all_pids()? {
        if process_uid(pid) != Some(my_uid) || process_name(pid).as_deref() != Some("dtach") {
            continue;
        }
        match process_args(pid) {
            Some(argv) => found.push((pid, argv)),
            // Exited since the listing.
            None if process_uid(pid).is_none() => {}
            None => return None,
        }
    }
    Some(found)
}

/// No process-table reader on other Unixes: report "unknown", which makes
/// reconciliation keep every session.
#[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
fn dtach_process_argvs() -> Option<Vec<(u32, Vec<OsString>)>> {
    None
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "okena-dtach-pool-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_held_lease_is_a_live_peer_until_dropped() {
        let dir = scratch_dir("lease");
        let first = DtachPoolLease::acquire_in(&dir).unwrap();
        let second = DtachPoolLease::acquire_in(&dir).unwrap();

        assert_eq!(second.live_peers(), vec![first.path.clone()]);
        assert_eq!(first.live_peers(), vec![second.path.clone()]);

        let first_path = first.path.clone();
        drop(first);
        assert!(!first_path.exists(), "a dropped lease unlinks its file");
        assert!(second.live_peers().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dead_daemons_lease_file_is_not_a_peer_and_is_removed() {
        let dir = scratch_dir("stale");
        // A daemon that died without unlinking: the file stays, the lock is gone.
        let stale = dir.join("4242-deadbeef.lock");
        std::fs::write(&stale, b"").unwrap();
        // Files that are not leases are ignored and left alone.
        let staging = dir.join(".4243-cafe.tmp");
        std::fs::write(&staging, b"").unwrap();

        let lease = DtachPoolLease::acquire_in(&dir).unwrap();
        assert!(lease.live_peers().is_empty());
        assert!(!stale.exists());
        assert!(staging.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn owner_tags_tell_our_sessions_from_others_and_untagged_ones() {
        let dir = scratch_dir("owner");
        let ours = dir.join("tm-ours.sock");
        let theirs = dir.join("tm-theirs.sock");
        let legacy = dir.join("tm-legacy.sock");
        record_session_owner(&ours, "/me/profiles/default/");
        record_session_owner(&theirs, "/sandbox/profiles/default/");

        let me = Some("/me/profiles/default/");
        assert_eq!(session_owner(&ours, me), SessionOwner::Us);
        assert_eq!(session_owner(&theirs, me), SessionOwner::Other);
        assert_eq!(session_owner(&legacy, me), SessionOwner::Untagged);
        // Without an identity of our own, no tagged session is ours.
        assert_eq!(session_owner(&ours, None), SessionOwner::Other);
        assert_eq!(owner_tag_path(&ours), dir.join("tm-ours.owner"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn argv(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn a_session_is_attached_only_when_a_dtach_other_than_its_master_names_it() {
        let attached = PathBuf::from("/pool/tm-attached.sock");
        let detached = PathBuf::from("/pool/tm-detached.sock");
        let sockets = [attached.clone(), detached.clone()];
        let holders = HashMap::from([(attached.clone(), vec![10]), (detached.clone(), vec![20])]);
        let argvs = vec![
            // Master of each session (holds the bound socket).
            (
                10,
                argv(&["dtach", "-A", "/pool/tm-attached.sock", "-E", "zsh"]),
            ),
            (
                20,
                argv(&["dtach", "-A", "/pool/tm-detached.sock", "-E", "zsh"]),
            ),
            // A client still attached from a live daemon's PTY.
            (
                11,
                argv(&["dtach", "-A", "/pool/tm-attached.sock", "-E", "zsh"]),
            ),
            // Our own process never counts.
            (99, argv(&["dtach", "-a", "/pool/tm-detached.sock"])),
        ];

        assert_eq!(
            attached_from(&sockets, &holders, &argvs, 99),
            HashSet::from([attached])
        );
    }

    #[test]
    fn an_unknown_master_counts_as_attached() {
        // When the socket holder lookup comes back empty, the master cannot be
        // told from a client. Keeping the session is the safe reading.
        let socket = PathBuf::from("/pool/tm-x.sock");
        let argvs = vec![(10, argv(&["dtach", "-A", "/pool/tm-x.sock", "zsh"]))];
        assert_eq!(
            attached_from(std::slice::from_ref(&socket), &HashMap::new(), &argvs, 99),
            HashSet::from([socket])
        );
    }
}
