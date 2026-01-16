use git2::{DiffOptions, Repository};
use std::path::Path;

use super::GitStatus;

/// Get git status for a directory path.
/// Returns None if not a git repository.
pub fn get_status(path: &Path) -> Option<GitStatus> {
    let repo = Repository::discover(path).ok()?;

    let branch = get_branch_name(&repo);
    let (lines_added, lines_removed) = get_diff_stats(&repo);

    Some(GitStatus {
        branch,
        lines_added,
        lines_removed,
    })
}

/// Get the current branch name or short commit hash for detached HEAD
fn get_branch_name(repo: &Repository) -> Option<String> {
    let head = repo.head().ok()?;

    if head.is_branch() {
        // Regular branch
        head.shorthand().map(String::from)
    } else {
        // Detached HEAD - show short commit hash
        head.target().map(|oid| format!("{:.7}", oid))
    }
}

/// Get diff statistics (lines added, lines removed) for working directory
fn get_diff_stats(repo: &Repository) -> (usize, usize) {
    // Get HEAD tree for comparison
    let head_tree = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_tree().ok());

    // Configure diff options
    let mut opts = DiffOptions::new();
    opts.include_untracked(true);

    // Get diff between HEAD and working directory (includes staged + unstaged)
    let diff = repo
        .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))
        .ok();

    match diff {
        Some(diff) => {
            match diff.stats() {
                Ok(stats) => (stats.insertions(), stats.deletions()),
                Err(_) => (0, 0),
            }
        }
        None => (0, 0),
    }
}
