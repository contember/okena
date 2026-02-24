//! VCS dispatch layer.
//!
//! Detects which VCS backend to use for a given path and delegates to the
//! appropriate module (git or jj).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::git::{self, GitStatus, FileDiffSummary};
use crate::git::diff::{DiffResult, DiffMode};
use crate::jj;

/// Which VCS backend is in use for a repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcsBackend {
    Git,
    Jujutsu,
}

/// Cached jj status entry.
struct CacheEntry {
    status: Option<GitStatus>,
    timestamp: Instant,
}

impl CacheEntry {
    fn is_fresh(&self) -> bool {
        self.timestamp.elapsed() < CACHE_TTL
    }
}

static JJ_CACHE: Mutex<Option<HashMap<PathBuf, CacheEntry>>> = Mutex::new(None);
const CACHE_TTL: Duration = Duration::from_secs(5);

fn with_jj_cache<F, R>(f: F) -> R
where
    F: FnOnce(&mut HashMap<PathBuf, CacheEntry>) -> R,
{
    let mut guard = JJ_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = guard.get_or_insert_with(HashMap::new);
    f(cache)
}

/// Detect which VCS is in use at `path`.
///
/// jj is checked first so that colocated repos (containing both `.jj/` and
/// `.git/`) are treated as Jujutsu repos.
pub fn detect_vcs(path: &Path) -> Option<VcsBackend> {
    if jj::is_jj_repo(path) {
        return Some(VcsBackend::Jujutsu);
    }
    if git::is_git_repo(path) {
        return Some(VcsBackend::Git);
    }
    None
}

/// Return `true` if `path` is inside any supported VCS repository.
pub fn is_vcs_repo(path: &Path) -> bool {
    detect_vcs(path).is_some()
}

/// Get the VCS status for `path`, using an appropriate cache.
///
/// For git repos the git module's own cache is used.
/// For jj repos a separate 5-second cache is maintained here.
pub fn get_vcs_status(path: &Path) -> Option<GitStatus> {
    match detect_vcs(path) {
        Some(VcsBackend::Git) => git::get_git_status(path),
        Some(VcsBackend::Jujutsu) => {
            let path_buf = path.to_path_buf();

            // Check jj cache first.
            let cached = with_jj_cache(|cache| {
                if let Some(entry) = cache.get(&path_buf) {
                    if entry.is_fresh() {
                        return Some(entry.status.clone());
                    }
                }
                None
            });

            if let Some(status) = cached {
                return status;
            }

            // Cache miss — fetch fresh.
            let status = jj::get_status(path);
            with_jj_cache(|cache| {
                cache.insert(
                    path_buf,
                    CacheEntry {
                        status: status.clone(),
                        timestamp: Instant::now(),
                    },
                );
            });
            status
        }
        None => None,
    }
}

/// Get a per-file diff summary for `path`.
pub fn get_diff_file_summary(path: &Path) -> Vec<FileDiffSummary> {
    match detect_vcs(path) {
        Some(VcsBackend::Git) => git::get_diff_file_summary(path),
        Some(VcsBackend::Jujutsu) => jj::get_diff_file_summary(path),
        None => vec![],
    }
}

/// Get the full diff for `path` with options.
pub fn get_diff_with_options(
    path: &Path,
    mode: DiffMode,
    ignore_whitespace: bool,
) -> Result<DiffResult, String> {
    match detect_vcs(path) {
        Some(VcsBackend::Git) => git::get_diff_with_options(path, mode, ignore_whitespace),
        Some(VcsBackend::Jujutsu) => jj::get_diff_with_options(path, mode, ignore_whitespace),
        None => Err("Not a version-controlled repository".to_string()),
    }
}

/// Get (old, new) file contents for a side-by-side diff view.
pub fn get_file_contents_for_diff(
    repo_path: &Path,
    file_path: &str,
    mode: DiffMode,
) -> (Option<String>, Option<String>) {
    match detect_vcs(repo_path) {
        Some(VcsBackend::Git) => git::get_file_contents_for_diff(repo_path, file_path, mode),
        Some(VcsBackend::Jujutsu) => jj::get_file_contents_for_diff(repo_path, file_path, mode),
        None => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_vcs_nonexistent_path() {
        assert!(detect_vcs(Path::new("/nonexistent/path")).is_none());
    }

    #[test]
    fn test_is_vcs_repo_nonexistent_path() {
        assert!(!is_vcs_repo(Path::new("/nonexistent/path")));
    }
}
