#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

pub mod blame;
pub mod branch_names;
pub mod commit_graph;
pub mod diff;
pub mod error;
pub(crate) mod gix_helpers;
pub mod repository;

pub use blame::{get_blame, BlameCommit, BlameError, BlameKind, BlameLine};
pub use error::{GitError, GitResult};
pub use commit_graph::fetch_commit_log;
pub use diff::{DiffResult, DiffMode, FileDiff, DiffLineType, get_diff_with_options, is_git_repo, get_file_contents_for_diff};
pub use repository::{
    create_worktree,
    remove_worktree,
    remove_worktree_fast,
    get_available_branches_for_worktree,
    get_repo_root,
    resolve_git_root_and_subdir,
    resolve_worktree_root_fs,
    compute_target_paths,
    project_path_in_worktree,
    has_uncommitted_changes,
    get_current_branch,
    get_default_branch,
    resolve_review_base,
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

// PR/CI types live in `okena-core` so they can ride the remote wire protocol
// (`ApiGitStatus`). Re-exported here so existing `okena_git::{PrInfo, ..}`
// paths keep working and `GitStatus` below can embed them.
pub use okena_core::api::{CiCheck, CiCheckSummary, CiStatus, PrInfo, PrState};

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
    /// CI / pipeline status for the current branch's HEAD commit.
    /// Populated from the PR's checks when a PR exists, otherwise from
    /// branch-level check-runs and statuses on the commit itself.
    #[serde(default)]
    pub ci_checks: Option<CiCheckSummary>,
    /// Number of commits the branch is ahead of its review base
    /// (`origin/<default>`, three-dot) — i.e. what it adds vs the base branch.
    /// `None` when there's no base (HEAD is on the default branch / detached).
    #[serde(default)]
    pub ahead: Option<usize>,
    /// Number of commits the branch is behind its review base — i.e. how stale
    /// it is vs the base branch. `None` when there's no base.
    #[serde(default)]
    pub behind: Option<usize>,
    /// Number of commits not yet pushed to `origin/<branch>`.
    /// Distinct from `ahead` because a branch's upstream may be `origin/main`
    /// (worktree branches) — in that case `ahead` counts feature commits vs
    /// main, while `unpushed` counts only commits missing from the branch's
    /// own remote ref. `None` when `origin/<branch>` doesn't exist (branch
    /// was never pushed or remote not configured).
    #[serde(default)]
    pub unpushed: Option<usize>,
    /// Ref to diff against for a "review changes" (three-dot `base...HEAD`)
    /// diff — e.g. `origin/main`. `None` when there is no sensible base (HEAD
    /// is on the default branch, or no default branch is resolvable).
    #[serde(default)]
    pub review_base: Option<String>,
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

/// A single commit entry for the commit log popover. The DAG topology is
/// reconstructed on the consumer side from `parents`; no graph art is stored.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CommitLogEntry {
    /// Short hash (7 chars). Used as the entry's identity for lane layout.
    pub hash: String,
    /// Short hashes of parent commits (first = first parent).
    pub parents: Vec<String>,
    /// Commit subject (first line)
    pub message: String,
    /// Author name
    pub author: String,
    /// Unix timestamp of the commit
    pub timestamp: i64,
    /// Ref decorations (e.g. "HEAD -> main", "origin/main", "tag: v1.0")
    pub refs: Vec<String>,
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
            let s = *s;
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
            ci_checks: None,
            ahead: None,
            behind: None,
            unpushed: None,
            review_base: None,
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
    let mut summaries = Vec::new();

    // Single gix walk yields both tracked changes vs HEAD (the structured
    // equivalent of `git diff --numstat --no-renames HEAD`) and untracked
    // files. Best-effort: a transient walk failure yields no entries rather
    // than erroring.
    let diff = repository::worktree_diff(path).unwrap_or_default();

    for (file, added, removed) in diff.tracked {
        summaries.push(FileDiffSummary {
            path: file,
            added,
            removed,
            is_new: false,
        });
    }

    // Untracked files count each line as an addition.
    for file in diff.untracked {
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
    fn diff_file_summary_matches_git_cli() {
        use crate::repository::test_support::{git_in, init_temp_repo};

        let (_tmp, repo) = init_temp_repo();
        // Modify a tracked file, delete one, stage a new one, leave one untracked.
        std::fs::write(repo.join("file.txt"), "a\nb\nc\n").unwrap();
        std::fs::write(repo.join("doomed.txt"), "x\ny\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["-c", "commit.gpgsign=false", "commit", "-m", "seed"]);
        std::fs::write(repo.join("file.txt"), "a\nB\nc\nd\n").unwrap();
        std::fs::remove_file(repo.join("doomed.txt")).unwrap();
        std::fs::write(repo.join("staged.txt"), "p\nq\n").unwrap();
        git_in(&repo, &["add", "staged.txt"]);
        std::fs::write(repo.join("untracked.txt"), "u1\nu2\n").unwrap();

        // CLI baseline keyed by path.
        let mut want: std::collections::HashMap<String, (usize, usize, bool)> =
            std::collections::HashMap::new();
        let out = std::process::Command::new("git")
            .args(["-C", repo.to_str().unwrap(), "diff", "--numstat", "--no-renames", "--no-color", "--no-ext-diff", "HEAD"])
            .output()
            .unwrap();
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let p: Vec<&str> = line.split('\t').collect();
            if p.len() >= 3 {
                want.insert(p[2].to_string(), (p[0].parse().unwrap_or(0), p[1].parse().unwrap_or(0), false));
            }
        }
        want.insert("untracked.txt".to_string(), (2, 0, true));

        let got: std::collections::HashMap<String, (usize, usize, bool)> = get_diff_file_summary(&repo)
            .into_iter()
            .map(|s| (s.path, (s.added, s.removed, s.is_new)))
            .collect();

        assert_eq!(got, want);
    }

    #[test]
    fn ci_tooltip_all_passed() {
        let summary = CiCheckSummary { status: CiStatus::Success, passed: 4, failed: 0, pending: 0, total: 4, checks: Vec::new() };
        assert_eq!(summary.tooltip_text(), "4/4 checks passed");
    }

    #[test]
    fn ci_tooltip_failure() {
        let summary = CiCheckSummary { status: CiStatus::Failure, passed: 3, failed: 1, pending: 0, total: 4, checks: Vec::new() };
        assert_eq!(summary.tooltip_text(), "1 failed, 3 passed of 4 checks");
    }

    #[test]
    fn ci_tooltip_pending() {
        let summary = CiCheckSummary { status: CiStatus::Pending, passed: 1, failed: 0, pending: 2, total: 3, checks: Vec::new() };
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
