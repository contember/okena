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

use okena_core::process::command;
use std::path::Path;

use crate::error::{GitError, GitResult};

pub mod branch;
pub mod ci;
pub mod clone;
pub mod paths;
pub mod status;
pub mod worktree;

pub use branch::{
    BranchList, checkout_local_branch, checkout_remote_branch, create_and_checkout_branch,
    delete_local_branch, delete_remote_branch, discard_file_changes, fetch_all,
    get_available_branches_for_worktree, get_default_branch, list_branches,
    list_branches_classified, merge_branch, push_branch, rebase_onto, resolve_base_ref,
    resolve_review_base, stage_file, stash_changes, stash_pop, unstage_file,
};
pub use ci::{
    CiFetch, PrFetch, fetch_ci_checks, fetch_pr_info, has_github_remote, list_pull_requests,
};
pub use clone::{
    CloneProgress, clone_dir_name, clone_repository, finish_clone_repository, is_complete_checkout,
    parse_clone_progress, start_clone_repository, validate_clone_url,
};
pub use paths::{
    compute_target_paths, get_repo_common_dir, get_repo_root, normalize_path,
    project_path_in_worktree, resolve_git_root_and_subdir,
};
pub(crate) use status::worktree_diff;
pub use status::{
    HeadSnapshot, StatusFetch, apply_pr_base, count_ahead_behind, count_ahead_behind_vs,
    count_unpushed_commits, get_current_branch, get_head_sha, get_head_snapshot, get_status,
    has_uncommitted_changes,
};
pub use worktree::{
    OrphanedWorktree, VerifiedWorktree, create_worktree, create_worktree_with_start_point,
    fetch_and_fast_forward, list_git_worktrees, list_linked_worktree_paths, move_worktree,
    remove_orphaned_worktree, remove_worktree, remove_worktree_fast, verify_linked_worktree_fresh,
    verify_orphaned_worktree,
};

/// Build a `git` command for an operation that talks to a remote.
///
/// Git asks for credentials on `/dev/tty`, not stdin, so redirecting stdin is
/// no defence: in a background process group that read raises SIGTTIN and the
/// child stops forever, leaving the caller blocked in `wait()` with nothing in
/// the log. Refuse every interactive prompt so a missing credential fails fast.
///
/// The second half of the defence is in the command bus, which gives every
/// child its own session and so no controlling terminal to prompt on.
pub(crate) fn network_command() -> std::process::Command {
    let mut cmd = command("git");
    // Empty `GIT_ASKPASS` is deliberate: git reads it as "set but unusable" and
    // skips both `core.askpass` and `SSH_ASKPASS` instead of falling through.
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        .env("GCM_INTERACTIVE", "never");
    // Only when the user has not chosen their own ssh command, so a custom
    // `GIT_SSH_COMMAND` keeps working. BatchMode still authenticates via an
    // agent; it only turns passphrase and host-key prompts into failures.
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    cmd
}

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
    path.to_str()
        .ok_or_else(|| GitError::InvalidPath(path.to_path_buf()))
}

/// Get branches that are already checked out in worktrees (main + linked).
/// Detached worktrees are skipped.
pub(crate) fn get_worktree_branches(path: &Path) -> Vec<String> {
    worktree::list_git_worktrees(path)
        .into_iter()
        .map(|(_, b)| b)
        .collect()
}

/// Read the short branch name from a repo's HEAD, or `None` if detached.
pub(crate) fn head_branch_short(repo: &gix::Repository) -> Option<String> {
    repo.head_name()
        .ok()
        .flatten()
        .map(|n| n.shorten().to_string())
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
        assert!(
            status.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&status.stderr)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A remote git command must refuse every interactive prompt. Without this
    /// a clone of a private repo blocks on a credential prompt it can never
    /// show, and the caller waits forever.
    #[test]
    fn network_commands_refuse_interactive_prompts() {
        let cmd = network_command();
        let env: std::collections::HashMap<_, _> = cmd
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
            .collect();

        assert_eq!(
            env.get("GIT_TERMINAL_PROMPT").map(String::as_str),
            Some("0")
        );
        // Set-but-empty, so git skips askpass instead of falling back to
        // `core.askpass` / `SSH_ASKPASS`.
        assert_eq!(env.get("GIT_ASKPASS").map(String::as_str), Some(""));
        assert_eq!(env.get("SSH_ASKPASS").map(String::as_str), Some(""));
    }

    /// A user who configured their own ssh command keeps it; we only fill in a
    /// non-interactive default when the slot is free.
    #[test]
    fn a_user_ssh_command_is_left_alone() {
        let ours = network_command()
            .get_envs()
            .find(|(k, _)| *k == "GIT_SSH_COMMAND")
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().into_owned());

        match std::env::var_os("GIT_SSH_COMMAND") {
            // Nothing configured: we supply a batch-mode default.
            None => assert_eq!(ours.as_deref(), Some("ssh -o BatchMode=yes")),
            // Configured: we must not override it.
            Some(_) => assert_eq!(ours, None),
        }
    }
}
