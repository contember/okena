//! Who may tear down sessions in the shared dtach socket pool.
//!
//! Every Okena daemon of a user puts its `tm-*.sock` sessions into one pool,
//! keyed only by `XDG_RUNTIME_DIR` (else `/tmp/okena-<uid>`) and the profile id.
//! `HOME`, `TMPDIR` and `XDG_CONFIG_HOME` do not move it, so a second daemon
//! started from a scratch config (a test harness, a second checkout) lands in
//! the pool of the daemon the user is working in. On start a daemon kills the
//! pool sessions its own workspace does not know
//! ([`reconcile_dtach_sessions`](crate::session_backend::reconcile_dtach_sessions)),
//! so without a guard that second daemon kills every pane of the first one,
//! whether the first one is running or the user simply closed Okena.
//!
//! The guards, all independent of where a daemon keeps its control socket:
//!
//! - **Owner tags.** `tm-<id>.owner` next to the socket names the profile
//!   directory (config dir + profile) of the daemon the session belongs to
//!   ([`claim_session`]). It is written when a daemon spawns or reattaches the
//!   session, and for every untagged session its workspace holds when it
//!   reconciles. A live session's existing tag is never overwritten. A daemon
//!   never kills a session tagged by another owner, running or not.
//! - **No other daemon**, for untagged sessions (started before owner tags). A
//!   daemon reclaims those only while no other daemon is live: none holds a
//!   [`DtachPoolLease`] (an exclusively `flock`ed file in `<pool>/leases/`
//!   that the kernel releases when its process dies), none has a control
//!   socket in the pool, and no `okena --headless` / `okena-daemon` process of
//!   this user runs ([`other_daemon_process_live`], which also sees daemons from
//!   before leases).
//! - **Attached sessions.** A session with a live `dtach -A` client is in use by
//!   someone, so reconciliation never kills it ([`attached_dtach_sockets`]).

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

/// Who a pool session belongs to, read from its owner tag.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionOwner {
    /// Tagged with an identity this daemon may reclaim.
    Us,
    /// Tagged by another daemon, or the tag exists but cannot be read.
    Other,
    /// No tag: started before owner tags existed.
    Untagged,
}

/// The identity a daemon writes into owner tags, and the tags it may reclaim.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnerIdentity {
    /// The active profile's directory: unique per config dir and profile, and
    /// stable across restarts.
    tag: String,
    /// For the default profile, `<config_root>/profiles/`: its pool directory
    /// is the legacy root that named profiles of the same config dir still
    /// share, and its reconciliation already retains their known terminals.
    sibling_prefix: Option<String>,
}

#[cfg(unix)]
impl OwnerIdentity {
    pub(crate) fn current() -> Option<Self> {
        let profile = okena_core::profiles::try_current()?;
        let profiles_dir = profile.config_root.join("profiles");
        Some(Self {
            tag: profile.root.to_string_lossy().into_owned(),
            sibling_prefix: (profile.id == "default")
                .then(|| format!("{}/", profiles_dir.to_string_lossy())),
        })
    }

    /// The identity of profile `profile_id` under the active config dir.
    pub(crate) fn for_profile(profile_id: &str) -> Option<Self> {
        let profile = okena_core::profiles::try_current()?;
        let root = profile.config_root.join("profiles").join(profile_id);
        Some(Self::new(&root.to_string_lossy(), None))
    }

    pub(crate) fn new(tag: &str, sibling_prefix: Option<&str>) -> Self {
        Self {
            tag: tag.to_owned(),
            sibling_prefix: sibling_prefix.map(str::to_owned),
        }
    }

    pub(crate) fn tag(&self) -> &str {
        &self.tag
    }

    fn claims(&self, tag: &str) -> bool {
        tag == self.tag
            || self
                .sibling_prefix
                .as_deref()
                .is_some_and(|prefix| tag.starts_with(prefix))
    }
}

#[cfg(unix)]
pub(crate) fn owner_tag_path(socket_path: &Path) -> PathBuf {
    socket_path.with_extension("owner")
}

/// Tag the session at `socket_path` as `tag`'s, unless a live session there
/// already names an owner: first owner wins, so a daemon that reattaches
/// someone else's session (a sandbox seeded with a copy of the user's config)
/// does not take it over. A tag beside a dead or missing socket is stale and
/// is replaced.
#[cfg(unix)]
pub(crate) fn claim_session(socket_path: &Path, tag: &str) {
    let tag_path = owner_tag_path(socket_path);
    let live_tag_exists = std::fs::symlink_metadata(&tag_path).is_ok()
        && std::fs::symlink_metadata(socket_path).is_ok();
    if live_tag_exists {
        return;
    }
    if let Some(parent) = socket_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = write_tag(&tag_path, tag) {
        log::warn!("failed to record dtach session owner for {socket_path:?}: {error}");
    }
}

/// The pool can sit in shared `/tmp`, so tags are never written or read
/// through a symlink, and are private to the user.
#[cfg(unix)]
fn write_tag(tag_path: &Path, tag: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(tag_path)?
        .write_all(tag.as_bytes())
}

#[cfg(unix)]
fn read_tag(tag_path: &Path) -> std::io::Result<String> {
    use std::io::Read as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut tag = String::new();
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(tag_path)?
        .read_to_string(&mut tag)?;
    Ok(tag)
}

#[cfg(unix)]
pub(crate) fn session_owner(socket_path: &Path, identity: Option<&OwnerIdentity>) -> SessionOwner {
    match read_tag(&owner_tag_path(socket_path)) {
        Ok(tag) if identity.is_some_and(|identity| identity.claims(&tag)) => SessionOwner::Us,
        Ok(_) => SessionOwner::Other,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => SessionOwner::Untagged,
        // Includes a symlink planted in place of the tag (`ELOOP`).
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
#[cfg(unix)]
fn dtach_process_argvs() -> Option<Vec<(u32, Vec<OsString>)>> {
    user_processes(|name| name == "dtach")
}

/// Whether another Okena daemon of this user runs: an `okena-daemon`, or an
/// `okena` started with `--headless` (the desktop's own daemon is
/// `okena --headless --ui-owned`). Version-independent, so it also sees
/// daemons that predate pool leases. Unknown counts as live.
#[cfg(unix)]
pub(crate) fn other_daemon_process_live() -> bool {
    let Some(processes) = user_processes(|name| name.starts_with("okena")) else {
        return true;
    };
    processes
        .iter()
        .any(|(pid, argv)| *pid != std::process::id() && is_daemon_argv(argv))
}

#[cfg(unix)]
fn is_daemon_argv(argv: &[OsString]) -> bool {
    let program_is_daemon = argv
        .first()
        .and_then(|arg0| Path::new(arg0).file_name())
        .is_some_and(|name| name == "okena-daemon");
    program_is_daemon || argv.iter().skip(1).any(|arg| arg == "--headless")
}

/// `(pid, argv)` of every process of this user whose name matches. `None` when
/// the process table cannot be listed, or a matching process is still running
/// but its argv cannot be read.
#[cfg(target_os = "linux")]
fn user_processes(name_matches: impl Fn(&str) -> bool) -> Option<Vec<(u32, Vec<OsString>)>> {
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
        let matches = entry.metadata().is_ok_and(|meta| meta.uid() == my_uid)
            && std::fs::read_to_string(entry.path().join("comm"))
                .is_ok_and(|comm| name_matches(comm.trim_end()));
        if !matches {
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
fn user_processes(name_matches: impl Fn(&str) -> bool) -> Option<Vec<(u32, Vec<OsString>)>> {
    use crate::macos_proc::{all_pids, process_args, process_name, process_uid};
    // SAFETY: `getuid(2)` takes no arguments and never fails.
    let my_uid = unsafe { libc::getuid() };
    let mut found = Vec::new();
    for pid in all_pids()? {
        if process_uid(pid) != Some(my_uid)
            || !process_name(pid).is_some_and(|name| name_matches(&name))
        {
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
fn user_processes(_name_matches: impl Fn(&str) -> bool) -> Option<Vec<(u32, Vec<OsString>)>> {
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
        let socket = |name: &str| {
            let path = dir.join(format!("{name}.sock"));
            std::fs::write(&path, b"").unwrap();
            path
        };
        let ours = socket("tm-ours");
        let theirs = socket("tm-theirs");
        let sibling = socket("tm-sibling");
        let legacy = socket("tm-legacy");
        claim_session(&ours, "/cfg/profiles/default");
        claim_session(&theirs, "/sandbox/profiles/default");
        claim_session(&sibling, "/cfg/profiles/work");

        let me = OwnerIdentity::new("/cfg/profiles/default", None);
        assert_eq!(session_owner(&ours, Some(&me)), SessionOwner::Us);
        assert_eq!(session_owner(&theirs, Some(&me)), SessionOwner::Other);
        assert_eq!(session_owner(&sibling, Some(&me)), SessionOwner::Other);
        assert_eq!(session_owner(&legacy, Some(&me)), SessionOwner::Untagged);
        // Without an identity of our own, no tagged session is ours.
        assert_eq!(session_owner(&ours, None), SessionOwner::Other);

        // The default profile reclaims legacy sessions of its own config dir's
        // named profiles, never another config dir's.
        let default = OwnerIdentity::new("/cfg/profiles/default", Some("/cfg/profiles/"));
        assert_eq!(session_owner(&sibling, Some(&default)), SessionOwner::Us);
        assert_eq!(session_owner(&theirs, Some(&default)), SessionOwner::Other);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_live_sessions_owner_is_never_taken_over_but_a_stale_tag_is_replaced() {
        let dir = scratch_dir("claim");
        let socket = dir.join("tm-x.sock");
        std::fs::write(&socket, b"").unwrap();
        let user = OwnerIdentity::new("/cfg/profiles/default", None);

        claim_session(&socket, user.tag());
        // A sandbox seeded with a copy of the user's workspace reattaches it.
        claim_session(&socket, "/sandbox/profiles/default");
        assert_eq!(session_owner(&socket, Some(&user)), SessionOwner::Us);

        // The session died and a new one reuses the name.
        std::fs::remove_file(&socket).unwrap();
        claim_session(&socket, "/sandbox/profiles/default");
        assert_eq!(session_owner(&socket, Some(&user)), SessionOwner::Other);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_symlinked_tag_is_neither_followed_nor_trusted() {
        let dir = scratch_dir("symlink");
        let socket = dir.join("tm-x.sock");
        let victim = dir.join("victim");
        std::fs::write(&victim, b"precious").unwrap();
        std::os::unix::fs::symlink(&victim, owner_tag_path(&socket)).unwrap();

        // The socket is missing, so the tag counts as stale and is rewritten.
        claim_session(&socket, "/cfg/profiles/default");
        assert_eq!(std::fs::read(&victim).unwrap(), b"precious");
        let me = OwnerIdentity::new("/cfg/profiles/default", None);
        assert_eq!(session_owner(&socket, Some(&me)), SessionOwner::Other);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn daemon_processes_are_told_from_other_okena_processes() {
        assert!(is_daemon_argv(&argv(&[
            "/r/okena",
            "--headless",
            "--ui-owned"
        ])));
        assert!(is_daemon_argv(&argv(&["/usr/local/bin/okena-daemon"])));
        assert!(!is_daemon_argv(&argv(&["/r/okena"])));
        assert!(!is_daemon_argv(&argv(&["/r/okena", "--ai-proxy"])));
        assert!(!is_daemon_argv(&argv(&["okena", "ls"])));
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
