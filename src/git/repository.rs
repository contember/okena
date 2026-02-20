use std::path::Path;

use super::GitStatus;
use crate::process::{command, safe_output};

/// Get branches that are already checked out in worktrees
fn get_worktree_branches(path: &Path) -> Vec<String> {
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return vec![],
    };

    let output = safe_output(
        command("git").args(["-C", path_str, "worktree", "list", "--porcelain"]),
    )
    .ok();

    let mut branches = Vec::new();

    if let Some(output) = output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if line.starts_with("branch ") {
                    let branch = line.strip_prefix("branch refs/heads/").unwrap_or(
                        line.strip_prefix("branch ").unwrap_or("")
                    );
                    if !branch.is_empty() {
                        branches.push(branch.to_string());
                    }
                }
            }
        }
    }

    branches
}

/// Create a new worktree
/// Returns Ok(()) on success, Err(error_message) on failure
pub fn create_worktree(repo_path: &Path, branch: &str, target_path: &Path, create_branch: bool) -> Result<(), String> {
    let repo_str = repo_path.to_str().ok_or("Invalid repo path")?;
    let target_str = target_path.to_str().ok_or("Invalid target path")?;

    let mut args = vec!["-C", repo_str, "worktree", "add"];

    if create_branch {
        args.push("-b");
        args.push(branch);
        args.push(target_str);
    } else {
        args.push(target_str);
        args.push(branch);
    }

    let output = safe_output(command("git").args(&args))
        .map_err(|e| format!("Failed to execute git: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(stderr.trim().to_string())
    }
}

/// Remove a worktree
/// Returns Ok(()) on success, Err(error_message) on failure
pub fn remove_worktree(worktree_path: &Path, force: bool) -> Result<(), String> {
    // First, find the main repo by getting the common git dir
    let path_str = worktree_path.to_str().ok_or("Invalid worktree path")?;

    let mut args = vec!["-C", path_str, "worktree", "remove"];

    if force {
        args.push("--force");
    }

    args.push(path_str);

    let output = safe_output(command("git").args(&args))
        .map_err(|e| format!("Failed to execute git: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(stderr.trim().to_string())
    }
}

/// List all branches in a repository
fn list_branches(path: &Path) -> Vec<String> {
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return vec![],
    };

    let output = safe_output(
        command("git").args(["-C", path_str, "branch", "-a", "--format=%(refname:short)"]),
    )
    .ok();

    let mut branches = Vec::new();

    if let Some(output) = output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let branch = line.trim();
                if !branch.is_empty() {
                    // Skip remote tracking branches that duplicate local ones
                    if branch.starts_with("origin/") {
                        let local_name = branch.strip_prefix("origin/").unwrap_or(branch);
                        if !branches.contains(&local_name.to_string()) {
                            branches.push(branch.to_string());
                        }
                    } else {
                        branches.push(branch.to_string());
                    }
                }
            }
        }
    }

    branches
}

/// Get branches that don't have a worktree yet
pub fn get_available_branches_for_worktree(path: &Path) -> Vec<String> {
    let all_branches = list_branches(path);
    let used_branches: std::collections::HashSet<_> = get_worktree_branches(path).into_iter().collect();

    all_branches
        .into_iter()
        .filter(|b| !used_branches.contains(b))
        .collect()
}

/// Get git status for a directory path.
/// Returns None if not a git repository.
pub fn get_status(path: &Path) -> Option<GitStatus> {
    // Check if we're in a git repo
    let output = safe_output(
        command("git").args(["-C", path.to_str()?, "rev-parse", "--is-inside-work-tree"]),
    )
    .ok()?;

    if !output.status.success() {
        return None;
    }

    let branch = get_current_branch(path);
    let (lines_added, lines_removed) = get_diff_stats(path);

    Some(GitStatus {
        branch,
        lines_added,
        lines_removed,
    })
}

/// Check if a worktree/repo has uncommitted changes (staged, unstaged, or untracked).
/// Always performs a fresh check (no caching).
pub fn has_uncommitted_changes(path: &Path) -> bool {
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return false,
    };

    let output = command("git")
        .args(["-C", path_str, "status", "--porcelain"])
        .output()
        .ok();

    match output {
        Some(output) if output.status.success() => {
            !String::from_utf8_lossy(&output.stdout).trim().is_empty()
        }
        _ => false,
    }
}

/// Get the current branch name or short commit hash for detached HEAD.
pub fn get_current_branch(path: &Path) -> Option<String> {
    let path_str = path.to_str()?;

    // Try to get branch name
    let output = safe_output(
        command("git").args(["-C", path_str, "symbolic-ref", "--short", "HEAD"]),
    )
    .ok()?;

    if output.status.success() {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !branch.is_empty() {
            return Some(branch);
        }
    }

    // Detached HEAD - get short commit hash
    let output = safe_output(
        command("git").args(["-C", path_str, "rev-parse", "--short", "HEAD"]),
    )
    .ok()?;

    if output.status.success() {
        let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !hash.is_empty() {
            return Some(hash);
        }
    }

    None
}

/// Get diff statistics (lines added, lines removed) for working directory
fn get_diff_stats(path: &Path) -> (usize, usize) {
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return (0, 0),
    };

    // Get diff stats for staged + unstaged changes
    let output = safe_output(
        command("git").args(["-C", path_str, "diff", "--numstat", "--no-color", "--no-ext-diff", "HEAD"]),
    )
    .ok();

    let (mut added, mut removed) = (0usize, 0usize);

    if let Some(output) = output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split('\t').collect();
                if parts.len() >= 2 {
                    // Binary files show "-" instead of numbers
                    if let Ok(a) = parts[0].parse::<usize>() {
                        added += a;
                    }
                    if let Ok(r) = parts[1].parse::<usize>() {
                        removed += r;
                    }
                }
            }
        }
    }

    // Also include untracked files (count lines)
    let output = safe_output(
        command("git").args(["-C", path_str, "ls-files", "--others", "--exclude-standard"]),
    )
    .ok();

    if let Some(output) = output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for file in stdout.lines() {
                if !file.is_empty() {
                    // Count lines in untracked file
                    let file_path = path.join(file);
                    if let Ok(content) = std::fs::read_to_string(&file_path) {
                        added += content.lines().count();
                    }
                }
            }
        }
    }

    (added, removed)
}

/// Get the default branch of a repository (e.g. "main" or "master").
/// Checks `git symbolic-ref refs/remotes/origin/HEAD` first, then falls back
/// to checking for `main` / `master` branches.
pub fn get_default_branch(repo_path: &Path) -> Option<String> {
    let path_str = repo_path.to_str()?;

    // Try symbolic-ref first
    let output = command("git")
        .args(["-C", path_str, "symbolic-ref", "refs/remotes/origin/HEAD"])
        .output()
        .ok()?;

    if output.status.success() {
        let refname = String::from_utf8_lossy(&output.stdout).trim().to_string();
        // refs/remotes/origin/main -> main
        if let Some(branch) = refname.strip_prefix("refs/remotes/origin/") {
            if !branch.is_empty() {
                return Some(branch.to_string());
            }
        }
    }

    // Fallback: check if main or master branch exists
    for candidate in &["main", "master"] {
        let output = command("git")
            .args(["-C", path_str, "rev-parse", "--verify", candidate])
            .output()
            .ok();
        if let Some(output) = output {
            if output.status.success() {
                return Some(candidate.to_string());
            }
        }
    }

    None
}

/// Rebase the current branch onto a target branch.
/// Automatically aborts on failure.
pub fn rebase_onto(worktree_path: &Path, target_branch: &str) -> Result<(), String> {
    let path_str = worktree_path.to_str().ok_or("Invalid worktree path")?;

    let output = command("git")
        .args(["-C", path_str, "rebase", target_branch])
        .output()
        .map_err(|e| format!("Failed to execute git rebase: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

        // Abort the failed rebase
        let _ = command("git")
            .args(["-C", path_str, "rebase", "--abort"])
            .output();

        Err(stderr)
    }
}

/// Stash uncommitted changes.
pub fn stash_changes(path: &Path) -> Result<(), String> {
    let path_str = path.to_str().ok_or("Invalid path")?;
    let output = command("git")
        .args(["-C", path_str, "stash"])
        .output()
        .map_err(|e| format!("Failed to execute git: {}", e))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(stderr.trim().to_string())
    }
}

/// Pop the most recent stash entry.
/// Used for recovery when rebase/merge fails after stash.
pub fn stash_pop(path: &Path) -> Result<(), String> {
    let path_str = path.to_str().ok_or("Invalid path")?;
    let output = command("git")
        .args(["-C", path_str, "stash", "pop"])
        .output()
        .map_err(|e| format!("Failed to execute git: {}", e))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(stderr.trim().to_string())
    }
}

/// Fetch from all remotes.
pub fn fetch_all(path: &Path) -> Result<(), String> {
    let path_str = path.to_str().ok_or("Invalid path")?;
    let output = command("git")
        .args(["-C", path_str, "fetch", "--all"])
        .output()
        .map_err(|e| format!("Failed to execute git: {}", e))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(stderr.trim().to_string())
    }
}

/// Merge a branch into the current branch.
/// If `no_ff` is true, uses `--no-ff` to create a merge commit even if fast-forward is possible.
pub fn merge_branch(repo_path: &Path, branch: &str, no_ff: bool) -> Result<(), String> {
    let path_str = repo_path.to_str().ok_or("Invalid repo path")?;

    let mut args = vec!["-C", path_str, "merge"];
    if no_ff {
        args.push("--no-ff");
    }
    args.push(branch);

    let output = command("git")
        .args(&args)
        .output()
        .map_err(|e| format!("Failed to execute git merge: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(stderr.trim().to_string())
    }
}

/// Delete a local branch (uses `-d`, fails if branch has unmerged changes).
pub fn delete_local_branch(repo_path: &Path, branch: &str) -> Result<(), String> {
    let path_str = repo_path.to_str().ok_or("Invalid path")?;
    let output = command("git")
        .args(["-C", path_str, "branch", "-d", branch])
        .output()
        .map_err(|e| format!("Failed to execute git: {}", e))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(stderr.trim().to_string())
    }
}

/// Delete a remote branch.
pub fn delete_remote_branch(repo_path: &Path, branch: &str) -> Result<(), String> {
    let path_str = repo_path.to_str().ok_or("Invalid path")?;
    let output = command("git")
        .args(["-C", path_str, "push", "origin", "--delete", branch])
        .output()
        .map_err(|e| format!("Failed to execute git: {}", e))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(stderr.trim().to_string())
    }
}

/// Push a branch to origin.
pub fn push_branch(repo_path: &Path, branch: &str) -> Result<(), String> {
    let path_str = repo_path.to_str().ok_or("Invalid path")?;
    let output = command("git")
        .args(["-C", path_str, "push", "origin", branch])
        .output()
        .map_err(|e| format!("Failed to execute git: {}", e))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(stderr.trim().to_string())
    }
}

/// Count commits that haven't been pushed to the upstream branch.
/// Returns 0 on any error (no upstream, not a git repo, etc.).
pub fn count_unpushed_commits(path: &Path) -> usize {
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return 0,
    };
    let output = command("git")
        .args(["-C", path_str, "rev-list", "@{u}..HEAD", "--count"])
        .output()
        .ok();
    match output {
        Some(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse::<usize>()
                .unwrap_or(0)
        }
        _ => 0,
    }
}

/// List all worktrees in a repository.
/// Returns vec of (path, branch_name) pairs.
#[allow(dead_code)]
pub fn list_git_worktrees(repo_path: &Path) -> Vec<(String, String)> {
    let path_str = match repo_path.to_str() {
        Some(s) => s,
        None => return vec![],
    };
    let output = command("git")
        .args(["-C", path_str, "worktree", "list", "--porcelain"])
        .output()
        .ok();
    let mut result = Vec::new();
    if let Some(output) = output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut current_path = String::new();
            for line in stdout.lines() {
                if let Some(wt_path) = line.strip_prefix("worktree ") {
                    current_path = wt_path.to_string();
                } else if let Some(branch_ref) = line.strip_prefix("branch refs/heads/") {
                    if !current_path.is_empty() {
                        result.push((current_path.clone(), branch_ref.to_string()));
                    }
                }
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn has_uncommitted_changes_returns_false_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(!has_uncommitted_changes(&path));
    }

    #[test]
    fn get_default_branch_returns_none_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(get_default_branch(&path).is_none());
    }

    #[test]
    fn get_current_branch_returns_none_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(get_current_branch(&path).is_none());
    }

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
    fn count_unpushed_commits_returns_zero_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert_eq!(count_unpushed_commits(&path), 0);
    }

    #[test]
    fn list_git_worktrees_returns_empty_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(list_git_worktrees(&path).is_empty());
    }
}
