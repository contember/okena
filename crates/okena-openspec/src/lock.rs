//! The store registry lock, compatible with the `openspec` CLI.
//!
//! OpenSpec serializes registry writes with an exclusive-create lock file next
//! to the registry (`registry.yaml.lock`) holding an ownership token
//! `<pid>:<uuid>`. A contender polls every 25 ms for up to 5 s and then gives
//! up; nobody ever steals a lock by age, because unlinking a "stale" lock can
//! race with its replacement and erase a live owner's. Release removes the file
//! only if it still holds our token. okena follows the same protocol so an
//! `openspec store register` running at the same moment cannot lose a write.

use crate::OpenSpecError;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const LOCK_DEADLINE: Duration = Duration::from_millis(5000);
const LOCK_POLL: Duration = Duration::from_millis(25);

/// A held registry lock. Released on drop.
#[derive(Debug)]
pub struct RegistryLock {
    path: PathBuf,
    token: String,
}

impl RegistryLock {
    pub fn acquire(path: &Path) -> Result<Self, OpenSpecError> {
        Self::acquire_within(path, LOCK_DEADLINE)
    }

    pub(crate) fn acquire_within(path: &Path, deadline: Duration) -> Result<Self, OpenSpecError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| create_failed(path, &e))?;
        }
        let started = Instant::now();
        loop {
            match open_exclusive(path) {
                Ok(mut file) => {
                    let token = format!("{}:{}", std::process::id(), uuid::Uuid::new_v4());
                    let written = file.write_all(token.as_bytes()).and_then(|()| {
                        // Some network filesystems support exclusive create but
                        // not fsync; the token is still visible to cooperating
                        // processes, so that is not a reason to fail.
                        match file.sync_all() {
                            Err(e) if e.kind() == std::io::ErrorKind::Unsupported => Ok(()),
                            other => other,
                        }
                    });
                    if let Err(e) = written {
                        drop(file);
                        let _ = std::fs::remove_file(path);
                        return Err(create_failed(path, &e));
                    }
                    return Ok(Self {
                        path: path.to_path_buf(),
                        token,
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if started.elapsed() >= deadline {
                        return Err(OpenSpecError::new(
                            "store_registry_busy",
                            "Store registry is busy.",
                        )
                        .with_fix(format!(
                            "Retry in a moment. If no openspec or okena command is running, \
                             delete the orphaned lock file {}.",
                            path.display()
                        )));
                    }
                    std::thread::sleep(LOCK_POLL);
                }
                Err(e) => return Err(create_failed(path, &e)),
            }
        }
    }
}

impl Drop for RegistryLock {
    fn drop(&mut self) {
        // Only remove what is still ours: if the file was replaced, it belongs
        // to someone else now.
        if std::fs::read_to_string(&self.path).is_ok_and(|t| t == self.token) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn open_exclusive(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn create_failed(path: &Path, e: &std::io::Error) -> OpenSpecError {
    OpenSpecError::new(
        "store_registry_lock_failed",
        format!(
            "Could not create the registry lock file {}: {e}",
            path.display()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_file_holds_a_pid_token_and_is_removed_on_release() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stores/registry.yaml.lock");
        {
            let _lock = RegistryLock::acquire(&path).unwrap();
            let token = std::fs::read_to_string(&path).unwrap();
            let (pid, uuid) = token.split_once(':').expect("pid:uuid");
            assert_eq!(pid, std::process::id().to_string());
            assert_eq!(uuid.len(), 36);
        }
        assert!(!path.exists());
    }

    #[test]
    fn a_held_lock_makes_a_contender_report_busy_rather_than_steal_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.yaml.lock");
        // Someone else's lock, e.g. a running `openspec store register`.
        std::fs::write(&path, "999:someone-else").unwrap();
        let err = RegistryLock::acquire_within(&path, Duration::from_millis(60)).unwrap_err();
        assert_eq!(err.code, "store_registry_busy");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "999:someone-else");
    }

    #[test]
    fn release_leaves_a_lock_that_was_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.yaml.lock");
        let lock = RegistryLock::acquire(&path).unwrap();
        std::fs::write(&path, "1:replaced").unwrap();
        drop(lock);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "1:replaced");
    }

    #[cfg(unix)]
    #[test]
    fn the_lock_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.yaml.lock");
        let _lock = RegistryLock::acquire(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
