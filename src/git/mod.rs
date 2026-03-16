pub mod branch_names;
pub mod diff;
pub(crate) mod repository;
pub mod watcher;

pub use diff::{DiffResult, DiffMode, FileDiff, DiffLineType, get_diff_with_options, is_git_repo, get_file_contents_for_diff};
pub use repository::{
    create_worktree,
    remove_worktree,
    remove_worktree_fast,
    list_git_worktrees,
    get_available_branches_for_worktree,
    get_repo_root,
    has_uncommitted_changes,
    get_current_branch,
    get_default_branch,
    rebase_onto,
    merge_branch,
    stash_changes,
    stash_pop,
    fetch_all,
    delete_local_branch,
    delete_remote_branch,
    push_branch,
    count_unpushed_commits,
};

use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Cache TTL - how long before status is considered stale
const CACHE_TTL: Duration = Duration::from_secs(5);

/// Git status information for display in project header
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GitStatus {
    /// Current branch name (None if detached HEAD shows short commit hash)
    pub branch: Option<String>,
    /// Lines added in working directory (unstaged + staged)
    pub lines_added: usize,
    /// Lines removed in working directory (unstaged + staged)
    pub lines_removed: usize,
}

/// Per-file diff summary for popover display
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FileDiffSummary {
    /// File path (relative to repo root)
    pub path: String,
    /// Lines added
    pub added: usize,
    /// Lines removed
    pub removed: usize,
    /// Whether this is a new (untracked) file
    pub is_new: bool,
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
///
/// On cache miss, returns stale data if available (non-blocking for UI).
/// Use `refresh_git_status` for a blocking fresh fetch (e.g. from a background watcher).
pub fn get_git_status(path: &Path) -> Option<GitStatus> {
    let path_buf = path.to_path_buf();

    // Check cache — return fresh or stale data without blocking
    let (cached, is_fresh) = with_cache(|cache| {
        if let Some(entry) = cache.get(&path_buf) {
            (Some(entry.status.clone()), entry.is_fresh())
        } else {
            (None, false)
        }
    });

    if is_fresh {
        return cached.unwrap();
    }

    // Have stale data — return it immediately (watcher will refresh)
    if cached.is_some() {
        return cached.unwrap();
    }

    // No cached data at all — must do initial fetch (only happens once per path)
    let status = repository::get_status(path);

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

/// Fetch fresh git status and update the cache. Intended for background watchers.
pub fn refresh_git_status(path: &Path) -> Option<GitStatus> {
    let path_buf = path.to_path_buf();
    let status = repository::get_status(path);

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

/// Get per-file diff summary for a repository.
/// Returns a list of files with their add/remove counts.
pub fn get_diff_file_summary(path: &Path) -> Vec<FileDiffSummary> {
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return vec![],
    };

    let mut summaries = Vec::new();

    // Get tracked file changes with numstat
    let output = crate::process::safe_output(
        crate::process::command("git").args(["-C", path_str, "diff", "--numstat", "--no-color", "--no-ext-diff", "HEAD"]),
    )
    .ok();

    if let Some(output) = output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split('\t').collect();
                if parts.len() >= 3 {
                    // Binary files show "-" instead of numbers
                    let added = parts[0].parse::<usize>().unwrap_or(0);
                    let removed = parts[1].parse::<usize>().unwrap_or(0);
                    summaries.push(FileDiffSummary {
                        path: parts[2].to_string(),
                        added,
                        removed,
                        is_new: false,
                    });
                }
            }
        }
    }

    // Get untracked files
    let output = crate::process::safe_output(
        crate::process::command("git").args(["-C", path_str, "ls-files", "--others", "--exclude-standard"]),
    )
    .ok();

    if let Some(output) = output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for file in stdout.lines() {
                if !file.is_empty() {
                    // Count lines in untracked file
                    let file_path = path.join(file);
                    let added = std::fs::read_to_string(&file_path)
                        .map(|c| c.lines().count())
                        .unwrap_or(0);
                    summaries.push(FileDiffSummary {
                        path: file.to_string(),
                        added,
                        removed: 0,
                        is_new: true,
                    });
                }
            }
        }
    }

    // Sort by path
    summaries.sort_by(|a, b| a.path.cmp(&b.path));
    summaries
}
