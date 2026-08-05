//! Per-pane pointer to the pane's *current* slave pty.
//!
//! `$OKENA_TTY` is captured into a pane's environment when the pane is first
//! launched and can never be refreshed: under a session backend the shell — and
//! the agent running under it — outlives Okena, so after a restart the variable
//! names a pty that is gone or, worse, now belongs to a *different* pane. An
//! agent hook then writes its status into someone else's pane, where the `tid=`
//! guard correctly drops it: silent loss on both ends.
//!
//! What does survive a restart is the pane's **terminal id**. So key a small
//! file by that id and rewrite its contents on every spawn: the *path* is stable
//! enough to hand to the pane's environment once (`$OKENA_TTY_FILE`), while the
//! device it names is always the pane's current one. See `docs/agent-status.md`.
//!
//! Unix only — `$OKENA_TTY` itself is Unix-only.

use std::path::{Path, PathBuf};

/// Subdirectory of Okena's profile-scoped runtime directory holding the
/// pointers. Its own directory so the dtach socket GC and this one never see
/// each other's files.
const POINTER_DIR: &str = "ttys";

/// Bound on a terminal id we will build a path from. Ids are UUIDs; anything
/// longer is not one.
const MAX_ID_LEN: usize = 128;

/// Directory the pointers live in.
pub fn dir() -> PathBuf {
    crate::session_backend::okena_runtime_dir().join(POINTER_DIR)
}

/// Path of the pointer for `terminal_id`, or `None` when the id cannot safely
/// be used as a file name.
///
/// Terminal ids reach us from persisted workspace data and from remote clients,
/// so treat them as untrusted path components rather than trusting the UUID
/// convention.
pub fn path_for(terminal_id: &str) -> Option<PathBuf> {
    path_in(&dir(), terminal_id)
}

fn path_in(dir: &Path, terminal_id: &str) -> Option<PathBuf> {
    // No `.`: it keeps a pointer name from ever colliding with the `<id>.tmp`
    // a concurrent publish writes, and rules out `.`/`..` for free. Ids are
    // UUIDs, so nothing legitimate is excluded.
    let safe = !terminal_id.is_empty()
        && terminal_id.len() <= MAX_ID_LEN
        && terminal_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'));
    safe.then(|| dir.join(terminal_id))
}

/// Record `tty_path` as the pane's current device and return the pointer path
/// to export as `$OKENA_TTY_FILE`, or `None` if it could not be written.
pub fn publish(terminal_id: &str, tty_path: &str) -> Option<PathBuf> {
    publish_in(&dir(), terminal_id, tty_path)
}

fn publish_in(dir: &Path, terminal_id: &str, tty_path: &str) -> Option<PathBuf> {
    let path = path_in(dir, terminal_id)?;
    if let Err(error) = create_dir_private(dir) {
        log::warn!("failed to create tty pointer dir {dir:?}: {error}");
        return None;
    }
    // Write-then-rename: a hook reading concurrently either sees the previous
    // device or the new one, never a half-written path.
    let temp = dir.join(format!("{terminal_id}.tmp"));
    if let Err(error) = write_private(&temp, tty_path) {
        log::warn!("failed to write tty pointer {temp:?}: {error}");
        let _ = std::fs::remove_file(&temp);
        return None;
    }
    if let Err(error) = std::fs::rename(&temp, &path) {
        log::warn!("failed to publish tty pointer {path:?}: {error}");
        let _ = std::fs::remove_file(&temp);
        return None;
    }
    Some(path)
}

/// Drop the pointer for a pane whose session is being killed.
pub fn revoke(terminal_id: &str) {
    revoke_in(&dir(), terminal_id);
}

fn revoke_in(dir: &Path, terminal_id: &str) {
    if let Some(path) = path_in(dir, terminal_id) {
        let _ = std::fs::remove_file(path);
    }
}

/// Remove pointers left behind by a previous run.
///
/// Only removes a pointer whose device is **gone**, so this is safe to run from
/// any `PtyManager` — including the throwaway ones tests build while a real
/// Okena is running against the same runtime directory. A pointer whose pty
/// number has since been recycled survives, and is covered by the same `tid=`
/// guard that covers a stale `$OKENA_TTY`.
pub fn sweep_dead() {
    sweep_dead_in(&dir());
}

fn sweep_dead_in(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return; // no pointers yet — nothing to sweep
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        let device = std::fs::read_to_string(&path)
            .ok()
            .map(|content| content.trim().to_string());
        // A leftover `.tmp` (crash mid-publish) reads as an empty/absent device
        // and is swept with the rest.
        let dead = device.is_none_or(|device| device.is_empty() || !Path::new(&device).exists());
        if dead && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        log::info!("Cleaned up {removed} stale tty pointer(s) from {dir:?}");
    }
}

/// Create the pointer directory with owner-only access — it names devices other
/// processes can write terminal output into.
fn create_dir_private(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    writeln!(file, "{contents}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("okena-tty-pointer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn rejects_ids_that_are_not_safe_file_names() {
        let dir = PathBuf::from("/run/user/1000/okena/ttys");
        assert!(path_in(&dir, "").is_none());
        assert!(path_in(&dir, ".").is_none());
        assert!(path_in(&dir, "..").is_none());
        assert!(path_in(&dir, "../../etc/passwd").is_none());
        assert!(path_in(&dir, "a/b").is_none());
        assert!(path_in(&dir, "a\\b").is_none());
        assert!(path_in(&dir, &"x".repeat(MAX_ID_LEN + 1)).is_none());
        // A dotted id could name another pane's in-flight `<id>.tmp`.
        assert!(path_in(&dir, "term.tmp").is_none());
        assert_eq!(
            path_in(&dir, "ddcf395e-7f78-4536-af47-98d56ba36db9"),
            Some(dir.join("ddcf395e-7f78-4536-af47-98d56ba36db9"))
        );
    }

    #[test]
    fn publish_then_revoke_round_trips_the_device() {
        let dir = temp_dir("round-trip");
        let published = publish_in(&dir, "term-1", "/dev/pts/9").expect("publish");
        assert_eq!(published, dir.join("term-1"));
        assert_eq!(
            std::fs::read_to_string(&published).expect("read").trim(),
            "/dev/pts/9"
        );

        // A respawn overwrites the device in place — the path stays put, which
        // is what makes it safe to capture in a pane's environment once.
        let republished = publish_in(&dir, "term-1", "/dev/pts/3").expect("republish");
        assert_eq!(republished, published);
        assert_eq!(
            std::fs::read_to_string(&published).expect("read").trim(),
            "/dev/pts/3"
        );
        assert!(
            !dir.join("term-1.tmp").exists(),
            "temp file must not linger"
        );

        revoke_in(&dir, "term-1");
        assert!(!published.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sweep_removes_only_pointers_whose_device_is_gone() {
        let dir = temp_dir("sweep");
        publish_in(&dir, "live", "/dev/null").expect("publish live");
        publish_in(&dir, "dead", "/dev/pts/999999").expect("publish dead");
        publish_in(&dir, "empty", "").expect("publish empty");

        sweep_dead_in(&dir);

        assert!(dir.join("live").exists(), "a live device must be kept");
        assert!(!dir.join("dead").exists());
        assert!(!dir.join("empty").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sweep_is_a_no_op_when_the_directory_is_absent() {
        sweep_dead_in(&temp_dir("absent"));
    }
}
