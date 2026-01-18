use std::path::Path;
use std::process::Command;

use super::GitStatus;

/// Get branches that are already checked out in worktrees
fn get_worktree_branches(path: &Path) -> Vec<String> {
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return vec![],
    };

    let output = Command::new("git")
        .args(["-C", path_str, "worktree", "list", "--porcelain"])
        .output()
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

    let output = Command::new("git")
        .args(&args)
        .output()
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

    let output = Command::new("git")
        .args(&args)
        .output()
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

    let output = Command::new("git")
        .args(["-C", path_str, "branch", "-a", "--format=%(refname:short)"])
        .output()
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
    let output = Command::new("git")
        .args(["-C", path.to_str()?, "rev-parse", "--is-inside-work-tree"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let branch = get_branch_name(path);
    let (lines_added, lines_removed) = get_diff_stats(path);

    Some(GitStatus {
        branch,
        lines_added,
        lines_removed,
    })
}

/// Get the current branch name or short commit hash for detached HEAD
fn get_branch_name(path: &Path) -> Option<String> {
    let path_str = path.to_str()?;

    // Try to get branch name
    let output = Command::new("git")
        .args(["-C", path_str, "symbolic-ref", "--short", "HEAD"])
        .output()
        .ok()?;

    if output.status.success() {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !branch.is_empty() {
            return Some(branch);
        }
    }

    // Detached HEAD - get short commit hash
    let output = Command::new("git")
        .args(["-C", path_str, "rev-parse", "--short", "HEAD"])
        .output()
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
    let output = Command::new("git")
        .args(["-C", path_str, "diff", "--numstat", "HEAD"])
        .output()
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
    let output = Command::new("git")
        .args(["-C", path_str, "ls-files", "--others", "--exclude-standard"])
        .output()
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
