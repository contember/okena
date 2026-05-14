#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

pub mod branch_names;
pub mod diff;
pub mod error;
pub(crate) mod gix_helpers;
pub mod repository;

pub use error::{GitError, GitResult};
pub use diff::{DiffResult, DiffMode, FileDiff, DiffLineType, get_diff_with_options, is_git_repo, get_file_contents_for_diff};
pub use repository::{
    create_worktree,
    remove_worktree,
    remove_worktree_fast,
    get_available_branches_for_worktree,
    get_repo_root,
    resolve_git_root_and_subdir,
    compute_target_paths,
    project_path_in_worktree,
    has_uncommitted_changes,
    get_current_branch,
    get_default_branch,
    rebase_onto,
    merge_branch,
    stash_changes,
    stash_pop,
    stage_file,
    unstage_file,
    discard_file_changes,
    fetch_all,
    delete_local_branch,
    delete_remote_branch,
    push_branch,
    count_unpushed_commits,
    count_ahead_behind,
    get_commit_graph,
    list_branches,
    list_branches_classified,
    BranchList,
    checkout_local_branch,
    checkout_remote_branch,
    create_and_checkout_branch,
};

/// Validate that a git ref (branch name, commit hash, revision) doesn't look
/// like a command-line flag.  Returns `Ok(name)` for safe values, or an error
/// for values starting with `-`.
pub fn validate_git_ref(name: &str) -> GitResult<&str> {
    if name.starts_with('-') {
        Err(GitError::InvalidRef(name.to_string()))
    } else {
        Ok(name)
    }
}

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
    /// Number of commits the local branch is ahead of its upstream.
    /// `None` when there is no upstream or HEAD is detached.
    #[serde(default)]
    pub ahead: Option<usize>,
    /// Number of commits the local branch is behind its upstream.
    /// `None` when there is no upstream or HEAD is detached.
    #[serde(default)]
    pub behind: Option<usize>,
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
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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
    #[allow(dead_code)] // used in tests
    pub is_merge: bool,
    /// Graph prefix characters (e.g. "| * |")
    pub graph: String,
    /// Ref decorations (e.g. "HEAD -> main", "origin/main", "tag: v1.0")
    pub refs: Vec<String>,
}

/// A row in the commit graph — either a commit or a graph connector line.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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
///
/// On transient failure (e.g. `git diff --numstat HEAD` exited non-zero or
/// the gix index walk briefly failed) the cache is left untouched and the
/// previous cached value is returned, so a single bad poll cycle doesn't
/// blank the +/- badge in the project header.
pub fn refresh_git_status(path: &Path) -> Option<GitStatus> {
    let path_buf = path.to_path_buf();
    match repository::get_status(path) {
        repository::StatusFetch::Status(s) => {
            with_cache(|cache| { cache.insert(path_buf, Some(s.clone())); });
            Some(s)
        }
        repository::StatusFetch::NotRepo => {
            with_cache(|cache| { cache.insert(path_buf, None); });
            None
        }
        repository::StatusFetch::Transient => {
            with_cache(|cache| cache.get(&path_buf).cloned().flatten())
        }
    }
}

/// Lightweight startup warmup: populate the cache with branch only (via gix —
/// no diff stats, no spawn). Skips paths that are already cached so it never
/// clobbers richer data from the polling watcher. Use for non-visible projects
/// we don't poll continuously, so the project switcher etc. can show a branch.
pub fn warm_branch_cache(path: &Path) {
    let path_buf = path.to_path_buf();
    let already_cached = with_cache(|cache| cache.contains_key(&path_buf));
    if already_cached {
        return;
    }
    let Some(branch) = repository::get_current_branch(path) else {
        return;
    };
    with_cache(|cache| {
        cache.entry(path_buf).or_insert_with(|| Some(GitStatus {
            branch: Some(branch),
            lines_added: 0,
            lines_removed: 0,
            pr_info: None,
            ahead: None,
            behind: None,
        }));
    });
}

/// Invalidate cache for a specific path (call when you know files changed)
#[allow(dead_code)]
pub fn invalidate_cache(path: &Path) {
    with_cache(|cache| { cache.remove(path); });
}

/// Get per-file diff summary for a repository.
/// Returns a list of files with their add/remove counts.
pub fn get_diff_file_summary(path: &Path) -> Vec<FileDiffSummary> {
    use okena_core::process::{command, safe_output};

    let path_str = match path.to_str() {
        Some(s) => s,
        None => return vec![],
    };

    let mut summaries = Vec::new();

    // Get tracked file changes with numstat
    let output = safe_output(
        command("git").args(["-C", path_str, "diff", "--numstat", "--no-color", "--no-ext-diff", "HEAD"]),
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

    // Get untracked files (best effort: silently skip on transient gix failure)
    for file in crate::gix_helpers::list_untracked_files(path).unwrap_or_default() {
        let file_path = path.join(&file);
        let added = std::fs::read_to_string(&file_path)
            .map(|c| c.lines().count())
            .unwrap_or(0);
        summaries.push(FileDiffSummary {
            path: file.clone(),
            added,
            removed: 0,
            is_new: true,
        });
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
    fn validate_git_ref_accepts_normal_refs() {
        assert!(validate_git_ref("main").is_ok());
        assert!(validate_git_ref("feature/foo").is_ok());
        assert!(validate_git_ref("abc123").is_ok());
        assert!(validate_git_ref("HEAD").is_ok());
        assert!(validate_git_ref("v1.0.0").is_ok());
    }

    #[test]
    fn validate_git_ref_rejects_flag_like_refs() {
        assert!(matches!(validate_git_ref("--upload-pack=evil"), Err(GitError::InvalidRef(_))));
        assert!(matches!(validate_git_ref("-b"), Err(GitError::InvalidRef(_))));
        assert!(matches!(validate_git_ref("--exec=malicious"), Err(GitError::InvalidRef(_))));
        assert!(matches!(validate_git_ref("-"), Err(GitError::InvalidRef(_))));
    }

    #[test]
    fn format_relative_time_weeks() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        assert_eq!(format_relative_time(now - 604800), "1w ago");
        assert_eq!(format_relative_time(now - 1209600), "2w ago");
    }
}
