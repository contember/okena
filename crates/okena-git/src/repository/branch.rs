//! Branch operations: list / classify / checkout / create / delete / push,
//! plus default-branch resolution, rebase, merge, stash, and per-file
//! stage/unstage/discard.

use std::path::Path;

use okena_core::process::command;

use super::{head_branch_short, path_str, require_success};
use crate::error::{GitError, GitResult};

/// List all branches in a repository (local + remotes), deduplicating
/// `origin/<name>` against local `<name>` and skipping `*/HEAD` symrefs.
pub fn list_branches(path: &Path) -> Vec<String> {
    let list = list_branches_classified(path);
    list.local.into_iter().chain(list.remote).collect()
}

/// Get branches that don't have a worktree yet
pub fn get_available_branches_for_worktree(path: &Path) -> Vec<String> {
    let all_branches = list_branches(path);
    let used_branches: std::collections::HashSet<_> = super::get_worktree_branches(path).into_iter().collect();

    all_branches
        .into_iter()
        .filter(|b| !used_branches.contains(b))
        .collect()
}

/// Get the default branch of a repository (e.g. "main" or "master").
/// Checks the `origin/HEAD` symref first, then falls back to checking for
/// local `main` / `master` branches.
pub fn get_default_branch(repo_path: &Path) -> Option<String> {
    let repo = crate::gix_helpers::open(repo_path)?;

    // Read refs/remotes/origin/HEAD; it is a symbolic ref whose target points
    // at e.g. refs/remotes/origin/main.
    if let Ok(head_ref) = repo.find_reference("refs/remotes/origin/HEAD")
        && let Some(target_name) = head_ref.target().try_name() {
            let target = target_name.as_bstr().to_string();
            if let Some(branch) = target.strip_prefix("refs/remotes/origin/")
                && !branch.is_empty() {
                    return Some(branch.to_string());
                }
        }

    // Fallback: check if main or master branch exists locally.
    for candidate in ["main", "master"] {
        if repo.find_reference(&format!("refs/heads/{}", candidate)).is_ok() {
            return Some(candidate.to_string());
        }
    }

    None
}

/// Rebase the current branch onto a target branch.
/// Automatically aborts on failure.
pub fn rebase_onto(worktree_path: &Path, target_branch: &str) -> GitResult<()> {
    crate::validate_git_ref(target_branch)?;
    let wt_str = path_str(worktree_path)?;

    let output = command("git")
        .args(["-C", wt_str, "rebase", target_branch])
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

        // Abort the failed rebase
        let _ = command("git")
            .args(["-C", wt_str, "rebase", "--abort"])
            .output();

        Err(GitError::GitExitError {
            status: output.status.code().unwrap_or(-1),
            stderr,
        })
    }
}

/// Stash uncommitted changes.
pub fn stash_changes(path: &Path) -> GitResult<()> {
    let p = path_str(path)?;
    let output = command("git")
        .args(["-C", p, "stash"])
        .output()?;
    require_success(output)
}

/// Pop the most recent stash entry.
/// Used for recovery when rebase/merge fails after stash.
pub fn stash_pop(path: &Path) -> GitResult<()> {
    let p = path_str(path)?;
    let output = command("git")
        .args(["-C", p, "stash", "pop"])
        .output()?;
    require_success(output)
}

/// Stage a file (git add -- <file>).
pub fn stage_file(repo_path: &Path, file_path: &str) -> GitResult<()> {
    let p = path_str(repo_path)?;
    let output = command("git")
        .args(["-C", p, "add", "--", file_path])
        .output()?;
    require_success(output)
}

/// Unstage a file from the index (git restore --staged -- <file>).
/// Works for both modified and newly-added files.
pub fn unstage_file(repo_path: &Path, file_path: &str) -> GitResult<()> {
    let p = path_str(repo_path)?;
    let output = command("git")
        .args(["-C", p, "restore", "--staged", "--", file_path])
        .output()?;
    require_success(output)
}

/// Discard working-tree changes for a file (git checkout HEAD -- <file>).
/// Restores the file to its HEAD state.
pub fn discard_file_changes(repo_path: &Path, file_path: &str) -> GitResult<()> {
    let p = path_str(repo_path)?;
    let output = command("git")
        .args(["-C", p, "checkout", "HEAD", "--", file_path])
        .output()?;
    require_success(output)
}

/// Fetch from all remotes.
pub fn fetch_all(path: &Path) -> GitResult<()> {
    let p = path_str(path)?;
    let output = command("git")
        .args(["-C", p, "fetch", "--all"])
        .output()?;
    require_success(output)
}

/// Merge a branch into the current branch.
/// If `no_ff` is true, uses `--no-ff` to create a merge commit even if fast-forward is possible.
pub fn merge_branch(repo_path: &Path, branch: &str, no_ff: bool) -> GitResult<()> {
    crate::validate_git_ref(branch)?;
    let p = path_str(repo_path)?;

    let mut args = vec!["-C", p, "merge"];
    if no_ff {
        args.push("--no-ff");
    }
    args.push(branch);

    let output = command("git")
        .args(&args)
        .output()?;
    require_success(output)
}

/// Delete a local branch (uses `-d`, fails if branch has unmerged changes).
pub fn delete_local_branch(repo_path: &Path, branch: &str) -> GitResult<()> {
    crate::validate_git_ref(branch)?;
    let p = path_str(repo_path)?;
    let output = command("git")
        .args(["-C", p, "branch", "-d", "--", branch])
        .output()?;
    require_success(output)
}

/// Delete a remote branch.
pub fn delete_remote_branch(repo_path: &Path, branch: &str) -> GitResult<()> {
    crate::validate_git_ref(branch)?;
    let p = path_str(repo_path)?;
    let output = command("git")
        .args(["-C", p, "push", "origin", "--delete", "--", branch])
        .output()?;
    require_success(output)
}

/// Push a branch to origin.
pub fn push_branch(repo_path: &Path, branch: &str) -> GitResult<()> {
    crate::validate_git_ref(branch)?;
    let p = path_str(repo_path)?;
    let output = command("git")
        .args(["-C", p, "push", "origin", "--", branch])
        .output()?;
    require_success(output)
}

/// Branch list classified into local and remote, with the current branch name
/// (if HEAD points at a branch).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchList {
    /// Local branch names.
    pub local: Vec<String>,
    /// Remote branch names that don't have a matching local branch (e.g.
    /// `origin/release` when there's no local `release`). Always includes
    /// the remote prefix.
    pub remote: Vec<String>,
    /// Current HEAD branch name (`None` if detached).
    pub current: Option<String>,
}

/// List branches classified into local vs. remote.
///
/// Like [`list_branches`] but keeps the two sets separate so a UI can show
/// "LOCAL" and "REMOTE" sections. Remote branches that have a matching local
/// branch are dropped (the local one wins). `*/HEAD` symrefs are skipped.
pub fn list_branches_classified(path: &Path) -> BranchList {
    let Some(repo) = crate::gix_helpers::open(path) else {
        return BranchList::default();
    };

    let Ok(refs) = repo.references() else {
        return BranchList::default();
    };

    let mut local: Vec<String> = Vec::new();
    let mut remote: Vec<String> = Vec::new();
    let mut local_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    if let Ok(iter) = refs.local_branches() {
        for r in iter.flatten() {
            let name = r.name().shorten().to_string();
            if !name.is_empty() {
                local_names.insert(name.clone());
                local.push(name);
            }
        }
    }

    if let Ok(iter) = refs.remote_branches() {
        for r in iter.flatten() {
            let name = r.name().shorten().to_string();
            if name.is_empty() || name.ends_with("/HEAD") {
                continue;
            }
            // Skip remote refs that have a corresponding local branch
            if let Some(stripped) = name.strip_prefix("origin/")
                && local_names.contains(stripped) {
                    continue;
                }
            remote.push(name);
        }
    }

    BranchList {
        current: head_branch_short(&repo),
        local,
        remote,
    }
}

/// Checkout an existing local branch (`git checkout <branch>`).
///
/// Branch name is validated to reject flag-like values, so we can safely
/// pass it as a positional argument (git treats it as a ref, not a
/// pathspec, when it matches a branch).
pub fn checkout_local_branch(repo_path: &Path, branch: &str) -> GitResult<()> {
    crate::validate_git_ref(branch)?;
    let p = path_str(repo_path)?;
    let output = command("git")
        .args(["-C", p, "checkout", branch])
        .output()?;
    require_success(output)
}

/// Checkout a remote branch, creating a local tracking branch. The new local
/// branch name is the remote ref with its `<remote>/` prefix stripped, so
/// `origin/feature` becomes local `feature`.
pub fn checkout_remote_branch(repo_path: &Path, remote_branch: &str) -> GitResult<()> {
    crate::validate_git_ref(remote_branch)?;
    let p = path_str(repo_path)?;

    // Strip the first path segment to derive the local branch name.
    let local_name = remote_branch
        .split_once('/')
        .map(|(_, rest)| rest)
        .unwrap_or(remote_branch);
    crate::validate_git_ref(local_name)?;

    // `git checkout --track <remote>/<branch>` creates a local branch and
    // sets the upstream to the remote ref in one shot. If a local branch
    // with that name already exists, fall back to plain checkout.
    let output = command("git")
        .args(["-C", p, "checkout", "--track", remote_branch])
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    checkout_local_branch(repo_path, local_name)
}

/// Create a new branch from the given start point (or HEAD if `None`) and
/// check it out. Returns an error if the branch name already exists.
pub fn create_and_checkout_branch(
    repo_path: &Path,
    new_name: &str,
    start_point: Option<&str>,
) -> GitResult<()> {
    crate::validate_git_ref(new_name)?;
    if let Some(sp) = start_point {
        crate::validate_git_ref(sp)?;
    }
    let p = path_str(repo_path)?;

    let mut args: Vec<&str> = vec!["-C", p, "checkout", "-b", new_name];
    if let Some(sp) = start_point {
        args.push(sp);
    }

    let output = command("git").args(&args).output()?;
    require_success(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::status::get_current_branch;
    use crate::repository::test_support::{git_in, init_temp_repo};
    use std::path::PathBuf;

    #[test]
    fn rebase_onto_returns_err_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(rebase_onto(&path, "main").is_err());
    }

    #[test]
    fn merge_branch_returns_err_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(merge_branch(&path, "feature", true).is_err());
    }

    #[test]
    fn stash_changes_returns_err_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(stash_changes(&path).is_err());
    }

    #[test]
    fn stash_pop_returns_err_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(stash_pop(&path).is_err());
    }

    #[test]
    fn fetch_all_returns_err_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(fetch_all(&path).is_err());
    }

    #[test]
    fn delete_local_branch_returns_err_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(delete_local_branch(&path, "feature").is_err());
    }

    #[test]
    fn delete_remote_branch_returns_err_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(delete_remote_branch(&path, "feature").is_err());
    }

    #[test]
    fn push_branch_returns_err_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(push_branch(&path, "feature").is_err());
    }

    #[test]
    fn get_default_branch_returns_none_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(get_default_branch(&path).is_none());
    }

    #[test]
    fn list_branches_returns_local_branches() {
        let (_tmp, repo) = init_temp_repo();
        git_in(&repo, &["branch", "feature/foo"]);
        git_in(&repo, &["branch", "feature/bar"]);
        let mut branches = list_branches(&repo);
        branches.sort();
        assert_eq!(branches, vec!["feature/bar", "feature/foo", "main"]);
    }

    #[test]
    fn list_branches_classified_separates_local_and_records_current() {
        let (_tmp, repo) = init_temp_repo();
        git_in(&repo, &["branch", "feature/foo"]);
        git_in(&repo, &["branch", "feature/bar"]);

        let mut list = list_branches_classified(&repo);
        list.local.sort();
        assert_eq!(list.local, vec!["feature/bar", "feature/foo", "main"]);
        assert!(list.remote.is_empty());
        assert_eq!(list.current.as_deref(), Some("main"));
    }

    #[test]
    fn create_and_checkout_branch_switches_head() {
        let (_tmp, repo) = init_temp_repo();
        create_and_checkout_branch(&repo, "feat/header-redesign", None)
            .expect("create branch");
        assert_eq!(
            get_current_branch(&repo).as_deref(),
            Some("feat/header-redesign")
        );
    }

    #[test]
    fn checkout_local_branch_switches_back_to_main() {
        let (_tmp, repo) = init_temp_repo();
        create_and_checkout_branch(&repo, "feat/x", None).expect("create branch");
        assert_eq!(get_current_branch(&repo).as_deref(), Some("feat/x"));

        checkout_local_branch(&repo, "main").expect("checkout main");
        assert_eq!(get_current_branch(&repo).as_deref(), Some("main"));
    }

    #[test]
    fn create_and_checkout_branch_rejects_flag_like_names() {
        let (_tmp, repo) = init_temp_repo();
        let err = create_and_checkout_branch(&repo, "-rf", None);
        assert!(err.is_err(), "expected rejection of flag-like ref name");
        // No new branch should have been created.
        let branches = list_branches(&repo);
        assert!(!branches.iter().any(|b| b == "-rf"));
    }

    #[test]
    fn get_default_branch_falls_back_to_main_locally() {
        let (_tmp, repo) = init_temp_repo();
        // No origin/HEAD exists — should fall back to local "main".
        assert_eq!(get_default_branch(&repo).as_deref(), Some("main"));
    }
}
