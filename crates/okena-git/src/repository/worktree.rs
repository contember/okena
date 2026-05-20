//! Worktree operations: create / remove / list / clean stale dirs.

use std::path::Path;

use okena_core::process::{command, safe_output};

use super::branch::get_default_branch;
use super::paths::normalize_path;
use super::{head_branch_short, path_str, require_success};
use crate::error::{GitError, GitResult};

/// If `target_path` exists but is NOT a currently registered worktree, remove
/// the stale directory and prune worktree metadata so a fresh `worktree add`
/// can succeed.  Returns an error only when the path is still an active worktree.
fn clean_stale_worktree_dir(repo_path: &Path, target_path: &Path) -> GitResult<()> {
    if !target_path.exists() {
        return Ok(());
    }

    // Ask git which paths are active worktrees
    let repo_str = path_str(repo_path)?;
    let output = safe_output(
        command("git").args(["-C", repo_str, "worktree", "list", "--porcelain"]),
    )?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let target_normalized = normalize_path(target_path);
        for line in stdout.lines() {
            if let Some(wt_path) = line.strip_prefix("worktree ")
                && normalize_path(Path::new(wt_path)) == target_normalized {
                    return Err(GitError::WorktreeExists {
                        path: target_path.to_path_buf(),
                    });
                }
        }
    }

    // Not an active worktree — remove the stale directory and prune metadata
    log::info!(
        "Removing stale worktree directory: {}",
        target_path.display()
    );
    std::fs::remove_dir_all(target_path)
        .map_err(|e| GitError::RemoveFailed {
            path: target_path.to_path_buf(),
            source: e,
        })?;

    let _ = safe_output(command("git").args(["-C", repo_str, "worktree", "prune"]));

    Ok(())
}

/// Create a new worktree.
pub fn create_worktree(repo_path: &Path, branch: &str, target_path: &Path, create_branch: bool) -> GitResult<()> {
    crate::validate_git_ref(branch)?;
    clean_stale_worktree_dir(repo_path, target_path)?;

    let repo_str = path_str(repo_path)?;
    let target_str = path_str(target_path)?;

    let mut args = vec!["-C", repo_str, "worktree", "add"];

    // When creating a new branch, fetch the remote default branch first,
    // then base the worktree on origin/{default} so it starts from the
    // latest remote state instead of a potentially stale local ref.
    let start_point;
    if create_branch {
        args.push("-b");
        args.push(branch);
        args.push(target_str);
        if let Some(default_branch) = get_default_branch(repo_path) {
            let _ = safe_output(command("git").args(["-C", repo_str, "fetch", "origin", &default_branch]));
            start_point = format!("origin/{}", default_branch);
            args.push(&start_point);
        }
    } else {
        args.push(target_str);
        args.push(branch);
    }

    let output = safe_output(command("git").args(&args))?;
    require_success(output)
}

/// Create a new worktree with an optional pre-fetched start point.
/// If `start_branch` is Some, creates `-b <branch> <target> origin/<start_branch>`
/// without re-fetching (caller is expected to have fetched already).
pub fn create_worktree_with_start_point(
    repo_path: &Path,
    branch: &str,
    target_path: &Path,
    start_branch: Option<&str>,
) -> GitResult<()> {
    crate::validate_git_ref(branch)?;
    if let Some(sb) = start_branch {
        crate::validate_git_ref(sb)?;
    }
    clean_stale_worktree_dir(repo_path, target_path)?;

    let repo_str = path_str(repo_path)?;
    let target_str = path_str(target_path)?;

    let mut args = vec!["-C", repo_str, "worktree", "add", "-b", branch, target_str];

    let start_point;
    if let Some(sb) = start_branch {
        start_point = format!("origin/{}", sb);
        args.push(&start_point);
    }

    let output = safe_output(command("git").args(&args))?;
    require_success(output)
}

/// Remove a worktree.
pub fn remove_worktree(worktree_path: &Path, force: bool) -> GitResult<()> {
    let wt_str = path_str(worktree_path)?;

    let mut args = vec!["-C", wt_str, "worktree", "remove"];

    if force {
        args.push("--force");
    }

    args.push(wt_str);

    let output = safe_output(command("git").args(&args))?;
    require_success(output)
}

/// Fast worktree removal: delete the directory and prune stale worktree metadata.
/// Much faster than `git worktree remove` which does expensive status checks.
/// Only safe when the caller has already handled dirty state (stash/discard).
///
/// Note: `git worktree prune` removes ALL stale entries (not just the one we deleted).
/// This is safe because prune only acts on entries whose directories no longer exist,
/// and we only delete the single target directory before pruning.
pub fn remove_worktree_fast(worktree_path: &Path, main_repo_path: &Path) -> GitResult<()> {
    // Remove the worktree directory (treat NotFound as success — already gone)
    match std::fs::remove_dir_all(worktree_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(GitError::RemoveFailed {
            path: worktree_path.to_path_buf(),
            source: e,
        }),
    }

    // Prune stale worktree entries from the main repo
    let main_str = path_str(main_repo_path)?;
    let output = safe_output(command("git").args(["-C", main_str, "worktree", "prune"]))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::warn!("git worktree prune warning: {}", stderr.trim());
    }

    Ok(())
}

/// List all worktrees in a repository (main + linked). Returns vec of
/// (path, branch_name) pairs; detached worktrees are omitted.
pub fn list_git_worktrees(repo_path: &Path) -> Vec<(String, String)> {
    let Some(repo) = crate::gix_helpers::open(repo_path) else {
        return vec![];
    };

    let mut result = Vec::new();

    // Main worktree: open via common_dir, which always resolves to the main
    // repository even when `repo_path` lives in a linked worktree.
    if let Ok(main_repo) = gix::open(repo.common_dir())
        && let (Some(workdir), Some(branch)) = (main_repo.workdir(), head_branch_short(&main_repo)) {
            result.push((workdir.to_string_lossy().into_owned(), branch));
        }

    // Linked worktrees from .git/worktrees/*.
    if let Ok(worktrees) = repo.worktrees() {
        for proxy in worktrees {
            let Some(workdir) = proxy.base().ok() else { continue };
            let Ok(wt_repo) = proxy.into_repo_with_possibly_inaccessible_worktree() else { continue };
            if let Some(branch) = head_branch_short(&wt_repo) {
                result.push((workdir.to_string_lossy().into_owned(), branch));
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::test_support::{git_in, init_temp_repo};
    use std::path::PathBuf;

    #[test]
    fn list_git_worktrees_returns_empty_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(list_git_worktrees(&path).is_empty());
    }

    #[test]
    fn list_git_worktrees_returns_main_plus_linked() {
        let (_tmp, repo) = init_temp_repo();
        let wt_tmp = tempfile::tempdir().expect("create worktree tempdir");
        let wt_path = wt_tmp.path().join("wt-feat");
        git_in(&repo, &["worktree", "add", wt_path.to_str().unwrap(), "-b", "feat"]);

        let mut entries = list_git_worktrees(&repo);
        entries.sort_by(|a, b| a.1.cmp(&b.1));
        let branches: Vec<&str> = entries.iter().map(|(_, b)| b.as_str()).collect();
        assert_eq!(branches, vec!["feat", "main"]);
    }

    #[test]
    fn get_worktree_branches_returns_branch_names() {
        let (_tmp, repo) = init_temp_repo();
        let wt_tmp = tempfile::tempdir().expect("create worktree tempdir");
        let wt_path = wt_tmp.path().join("wt-feat");
        git_in(&repo, &["worktree", "add", wt_path.to_str().unwrap(), "-b", "feat"]);

        let mut branches = crate::repository::get_worktree_branches(&repo);
        branches.sort();
        assert_eq!(branches, vec!["feat", "main"]);
    }
}
