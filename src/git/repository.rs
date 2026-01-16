use std::path::Path;
use std::process::Command;

use super::GitStatus;

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
