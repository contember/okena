//! Repository operations, split into cohesive submodules.
//!
//! All public items are re-exported here, so external code keeps using the
//! flat `okena_git::repository::*` (and `okena_git::*`) paths unchanged.
//!
//! | Submodule | Responsibility |
//! |-----------|----------------|
//! | [`worktree`] | create / remove / list worktrees, clean stale dirs |
//! | [`branch`]   | list / checkout / create / delete / push branches, rebase, merge, stash, per-file stage |
//! | [`status`]   | working-tree status, diff stats, HEAD/branch reads, ahead/behind |
//! | [`ci`]       | GitHub PR info + CI check parsing |
//! | [`paths`]    | repo-root resolution and worktree/project path computation |

use std::path::Path;

use crate::error::{GitError, GitResult};

pub mod branch;
pub mod ci;
pub mod paths;
pub mod status;
pub mod worktree;

pub use branch::{
    checkout_local_branch, checkout_remote_branch, create_and_checkout_branch,
    delete_local_branch, delete_remote_branch, discard_file_changes, fetch_all,
    get_available_branches_for_worktree, get_default_branch, list_branches,
    list_branches_classified, merge_branch, push_branch, rebase_onto, stage_file,
    stash_changes, stash_pop, unstage_file, BranchList,
};
pub use ci::{get_ci_checks, get_pr_info};
pub use paths::{
    compute_target_paths, get_repo_root, normalize_path, project_path_in_worktree,
    resolve_git_root_and_subdir,
};
pub use status::{
    count_ahead_behind, count_unpushed_commits, get_current_branch, get_head_sha, get_status,
    has_uncommitted_changes, StatusFetch,
};
pub use worktree::{
    create_worktree, create_worktree_with_start_point, list_git_worktrees, remove_worktree,
    remove_worktree_fast,
};

/// Run a git command and return `Ok(())` if it exits successfully,
/// or `Err(GitExitError)` with the stderr message.
pub(crate) fn require_success(output: std::process::Output) -> GitResult<()> {
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(GitError::GitExitError {
            status: output.status.code().unwrap_or(-1),
            stderr,
        })
    }
}

/// Convert a `Path` to a UTF-8 `&str`, returning `GitError::InvalidPath` on failure.
pub(crate) fn path_str(path: &Path) -> GitResult<&str> {
    path.to_str().ok_or_else(|| GitError::InvalidPath(path.to_path_buf()))
}

/// Get branches that are already checked out in worktrees (main + linked).
/// Detached worktrees are skipped.
pub(crate) fn get_worktree_branches(path: &Path) -> Vec<String> {
    worktree::list_git_worktrees(path).into_iter().map(|(_, b)| b).collect()
}

/// Read the short branch name from a repo's HEAD, or `None` if detached.
pub(crate) fn head_branch_short(repo: &gix::Repository) -> Option<String> {
    repo.head_name().ok().flatten().map(|n| n.shorten().to_string())
}

/// Shared test helpers used by submodule unit tests.
#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};

    /// Helper: initialise a throwaway git repo with one commit so worktrees can
    /// be created from it.
    pub(crate) fn init_temp_repo() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().expect("create temp dir");
        let repo = tmp.path().to_path_buf();
        let r = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "test@test")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "test@test")
                .output()
                .expect("git command failed")
        };
        r(&["init", "-b", "main"]);
        std::fs::write(repo.join("file.txt"), "x").unwrap();
        r(&["add", "."]);
        r(&["-c", "commit.gpgsign=false", "commit", "-m", "init"]);
        (tmp, repo)
    }

    /// Run a git command in `repo`, asserting success.
    pub(crate) fn git_in(repo: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test")
            .output()
            .expect("git command failed");
        assert!(status.status.success(), "git {:?} failed: {}", args, String::from_utf8_lossy(&status.stderr));
    }
}
