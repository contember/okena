pub mod diff;
mod repository;

pub use diff::{DiffResult, DiffMode, FileDiff, DiffLineType, get_diff, is_git_repo};
pub use repository::{
    create_worktree,
    remove_worktree,
    get_available_branches_for_worktree,
};

use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Cache TTL - how long before status is considered stale
const CACHE_TTL: Duration = Duration::from_secs(5);

/// Git status information for display in project header
#[derive(Clone, Debug, Default)]
pub struct GitStatus {
    /// Current branch name (None if detached HEAD shows short commit hash)
    pub branch: Option<String>,
    /// Lines added in working directory (unstaged + staged)
    pub lines_added: usize,
    /// Lines removed in working directory (unstaged + staged)
    pub lines_removed: usize,
}

impl GitStatus {
    pub fn has_changes(&self) -> bool {
        self.lines_added > 0 || self.lines_removed > 0
    }
}

/// Cached git status entry
struct CacheEntry {
    status: Option<GitStatus>,
    timestamp: Instant,
}

impl CacheEntry {
    fn is_fresh(&self) -> bool {
        self.timestamp.elapsed() < CACHE_TTL
    }
}

/// Global cache for git status
static CACHE: Mutex<Option<HashMap<PathBuf, CacheEntry>>> = Mutex::new(None);

fn with_cache<F, R>(f: F) -> R
where
    F: FnOnce(&mut HashMap<PathBuf, CacheEntry>) -> R,
{
    let mut guard = CACHE.lock();
    let cache = guard.get_or_insert_with(HashMap::new);
    f(cache)
}

/// Get git status for a directory path (with caching).
/// Returns None if the path is not inside a git repository.
pub fn get_git_status(path: &Path) -> Option<GitStatus> {
    let path_buf = path.to_path_buf();

    // Check cache first
    let cached = with_cache(|cache| {
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

    // Cache miss or stale - fetch fresh status
    let status = repository::get_status(path);

    // Update cache
    with_cache(|cache| {
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

/// Invalidate cache for a specific path (call when you know files changed)
#[allow(dead_code)]
pub fn invalidate_cache(path: &Path) {
    with_cache(|cache| {
        cache.remove(path);
    });
}

/// Clear entire cache
#[allow(dead_code)]
pub fn clear_cache() {
    with_cache(|cache| {
        cache.clear();
    });
}
