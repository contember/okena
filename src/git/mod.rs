pub mod branch_names;
pub mod diff;
pub(crate) mod repository;
pub mod watcher;

pub use diff::{DiffResult, DiffMode, FileDiff, DiffLineType, get_diff_with_options, is_git_repo, batch_is_git_repo, get_file_contents_for_diff};
pub use repository::{
    create_worktree,
    move_worktree,
    remove_worktree,
    remove_worktree_fast,
    list_git_worktrees,
    list_template_worktrees,
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
    get_commit_graph,
    list_branches,
};

use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// PR state from GitHub
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PrState {
    Open,
    Merged,
    Closed,
    Draft,
}

impl PrState {
    /// Theme color for this PR state
    pub fn color(&self, t: &crate::theme::ThemeColors) -> u32 {
        match self {
            PrState::Open => t.term_green,
            PrState::Draft => t.text_muted,
            PrState::Merged => t.term_magenta,
            PrState::Closed => t.term_red,
        }
    }

    /// Display label for this PR state
    pub fn label(&self) -> &'static str {
        match self {
            PrState::Open => "Open",
            PrState::Draft => "Draft",
            PrState::Merged => "Merged",
            PrState::Closed => "Closed",
        }
    }
}

/// Overall CI check rollup status
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CiStatus {
    Success,
    Failure,
    Pending,
}

impl CiStatus {
    pub fn color(&self, t: &crate::theme::ThemeColors) -> u32 {
        match self {
            CiStatus::Success => t.term_green,
            CiStatus::Failure => t.term_red,
            CiStatus::Pending => t.term_yellow,
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            CiStatus::Success => "icons/check.svg",
            CiStatus::Failure => "icons/close.svg",
            CiStatus::Pending => "icons/refresh.svg",
        }
    }

    pub fn is_pending(&self) -> bool {
        matches!(self, CiStatus::Pending)
    }
}

/// Summary of CI check results
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CiCheckSummary {
    pub status: CiStatus,
    pub passed: usize,
    pub failed: usize,
    pub pending: usize,
    pub total: usize,
}

impl CiCheckSummary {
    pub fn tooltip_text(&self) -> String {
        match self.status {
            CiStatus::Success => format!("{}/{} checks passed", self.passed, self.total),
            CiStatus::Failure => format!("{} failed, {} passed of {} checks", self.failed, self.passed, self.total),
            CiStatus::Pending => format!("{} pending, {} passed of {} checks", self.pending, self.passed, self.total),
        }
    }
}

/// Pull request info
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PrInfo {
    pub url: String,
    pub state: PrState,
    pub number: u32,
    #[serde(default)]
    pub ci_checks: Option<CiCheckSummary>,
}

/// Git status information for display in project header
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GitStatus {
    /// Current branch name (None if detached HEAD shows short commit hash)
    pub branch: Option<String>,
    /// Lines added in working directory (unstaged + staged)
    pub lines_added: usize,
    /// Lines removed in working directory (unstaged + staged)
    pub lines_removed: usize,
    /// Pull request info for the current branch (if any)
    #[serde(default)]
    pub pr_info: Option<PrInfo>,
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

/// A single commit entry for the commit log popover.
#[derive(Clone, Debug)]
pub struct CommitLogEntry {
    /// Short hash (7 chars)
    pub hash: String,
    /// Commit subject (first line)
    pub message: String,
    /// Author name
    pub author: String,
    /// Unix timestamp of the commit
    pub timestamp: i64,
    /// Whether this is a merge commit (2+ parents)
    pub is_merge: bool,
    /// Graph prefix characters (e.g. "| * |")
    pub graph: String,
}

/// A row in the commit graph — either a commit or a graph connector line.
#[derive(Clone, Debug)]
pub enum GraphRow {
    Commit(CommitLogEntry),
    /// Graph-only connector line (e.g. "|\ ", "|/ ")
    Connector(String),
}

/// Format a Unix timestamp as compact relative time.
pub fn format_relative_time(timestamp: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let diff = (now - timestamp).max(0) as u64;
    if diff < 60 {
        "just now".to_string()
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else if diff < 604800 {
        format!("{}d ago", diff / 86400)
    } else {
        format!("{}w ago", diff / 604800)
    }
}

/// Global cache for git status
static CACHE: Mutex<Option<HashMap<PathBuf, Option<GitStatus>>>> = Mutex::new(None);

fn with_cache<F, R>(f: F) -> R
where
    F: FnOnce(&mut HashMap<PathBuf, Option<GitStatus>>) -> R,
{
    let mut guard = CACHE.lock();
    let cache = guard.get_or_insert_with(HashMap::new);
    f(cache)
}

/// Get git status for a directory path (with caching).
/// Returns None if the path is not inside a git repository or not yet cached.
///
/// Always non-blocking: returns cached data immediately.
/// Returns None on cache miss — the background watcher will populate it.
/// Use `refresh_git_status` for a blocking fresh fetch (e.g. from a background watcher).
pub fn get_git_status(path: &Path) -> Option<GitStatus> {
    with_cache(|cache| cache.get(path).cloned().flatten())
}

/// Fetch fresh git status and update the cache. Intended for background watchers.
pub fn refresh_git_status(path: &Path) -> Option<GitStatus> {
    let path_buf = path.to_path_buf();
    let status = repository::get_status(path);
    with_cache(|cache| { cache.insert(path_buf, status.clone()); });
    status
}

/// Invalidate cache for a specific path (call when you know files changed)
#[allow(dead_code)]
pub fn invalidate_cache(path: &Path) {
    with_cache(|cache| { cache.remove(path); });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_tooltip_all_passed() {
        let summary = CiCheckSummary { status: CiStatus::Success, passed: 4, failed: 0, pending: 0, total: 4 };
        assert_eq!(summary.tooltip_text(), "4/4 checks passed");
    }

    #[test]
    fn ci_tooltip_failure() {
        let summary = CiCheckSummary { status: CiStatus::Failure, passed: 3, failed: 1, pending: 0, total: 4 };
        assert_eq!(summary.tooltip_text(), "1 failed, 3 passed of 4 checks");
    }

    #[test]
    fn ci_tooltip_pending() {
        let summary = CiCheckSummary { status: CiStatus::Pending, passed: 1, failed: 0, pending: 2, total: 3 };
        assert_eq!(summary.tooltip_text(), "2 pending, 1 passed of 3 checks");
    }

    #[test]
    fn format_relative_time_just_now() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        assert_eq!(format_relative_time(now), "just now");
        assert_eq!(format_relative_time(now - 30), "just now");
    }

    #[test]
    fn format_relative_time_minutes() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        assert_eq!(format_relative_time(now - 60), "1m ago");
        assert_eq!(format_relative_time(now - 300), "5m ago");
        assert_eq!(format_relative_time(now - 3599), "59m ago");
    }

    #[test]
    fn format_relative_time_hours() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        assert_eq!(format_relative_time(now - 3600), "1h ago");
        assert_eq!(format_relative_time(now - 7200), "2h ago");
    }

    #[test]
    fn format_relative_time_days() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        assert_eq!(format_relative_time(now - 86400), "1d ago");
        assert_eq!(format_relative_time(now - 259200), "3d ago");
    }

    #[test]
    fn format_relative_time_weeks() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        assert_eq!(format_relative_time(now - 604800), "1w ago");
        assert_eq!(format_relative_time(now - 1209600), "2w ago");
    }
}
