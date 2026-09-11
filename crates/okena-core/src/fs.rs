//! Filesystem helpers for files people and other tools also touch.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Expand a leading `~`, the form people naturally type into a path field.
pub fn expand_home(p: &str) -> PathBuf {
    let t = p.trim();
    if let Some(home) = dirs::home_dir() {
        if t == "~" {
            return home;
        }
        if let Some(rest) = t.strip_prefix("~/").or_else(|| t.strip_prefix("~\\")) {
            return home.join(rest);
        }
    }
    PathBuf::from(t)
}

/// The canonical form of an existing path, or the path unchanged.
///
/// Registries compare checkouts by canonical path, or the same checkout reached
/// through a symlink reads as two. The Windows verbatim prefix `canonicalize`
/// adds is stripped: nothing a person or another tool writes carries it, so an
/// entry holding it would never match.
pub fn canonical(p: &Path) -> PathBuf {
    match p.canonicalize() {
        Ok(real) => strip_verbatim(real),
        Err(_) => p.to_path_buf(),
    }
}

fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\")
        && !rest.starts_with("UNC\\")
    {
        return PathBuf::from(rest);
    }
    p
}

/// Write through a sibling temp file and rename, so a reader never sees a
/// half-written file. `private` makes the file owner-only on Unix.
pub fn write_atomically(path: &Path, content: &str, private: bool) -> std::io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // pid + time + a process counter: unique across processes and across
    // threads writing the same file in one instant.
    let tmp = dir.join(format!(
        ".{name}.{}.{nanos}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            if private {
                options.mode(0o600);
            }
        }
        #[cfg(not(unix))]
        let _ = private;
        let mut file = options.open(&tmp)?;
        file.write_all(content.as_bytes())?;
        match file.sync_all() {
            Err(e) if e.kind() == std::io::ErrorKind::Unsupported => {}
            other => other?,
        }
        drop(file);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_writes_replace_the_file_and_leave_no_temp_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested/registry.yaml");
        write_atomically(&path, "a", true).expect("first");
        write_atomically(&path, "b", true).expect("second");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "b");
        let names: Vec<_> = std::fs::read_dir(dir.path().join("nested"))
            .expect("dir")
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
    }

    #[cfg(unix)]
    #[test]
    fn private_atomic_writes_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("secret.yaml");
        write_atomically(&path, "x", true).expect("write");
        let mode = std::fs::metadata(&path).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn home_is_expanded_only_at_the_start() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        assert_eq!(expand_home("~"), home);
        assert_eq!(
            expand_home("  ~/knowledge/eng "),
            home.join("knowledge/eng")
        );
        assert_eq!(expand_home("/abs/~/x"), PathBuf::from("/abs/~/x"));
        assert_eq!(expand_home("~other/x"), PathBuf::from("~other/x"));
    }

    #[test]
    fn verbatim_prefixes_are_stripped_but_unc_paths_are_kept() {
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\C:\k\eng")),
            PathBuf::from(r"C:\k\eng")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\UNC\server\share")),
            PathBuf::from(r"\\?\UNC\server\share")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from("/k/eng")),
            PathBuf::from("/k/eng")
        );
    }
}
