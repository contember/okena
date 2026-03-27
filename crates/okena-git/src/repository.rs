use std::path::{Component, Path, PathBuf};

use crate::GitStatus;
use okena_core::process::{command, safe_output};

/// Get the root directory of the git repository containing the given path.
/// Returns None if the path is not inside a git repository.
pub fn get_repo_root(path: &Path) -> Option<PathBuf> {
    let path_str = path.to_str()?;
    let output = safe_output(
        command("git").args(["-C", path_str, "rev-parse", "--show-toplevel"]),
    )
    .ok()?;

    if output.status.success() {
        let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !root.is_empty() {
            return Some(PathBuf::from(root));
        }
    }

    None
}

/// Get branches that are already checked out in worktrees
pub(crate) fn get_worktree_branches(path: &Path) -> Vec<String> {
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

/// If `target_path` exists but is NOT a currently registered worktree, remove
/// the stale directory and prune worktree metadata so a fresh `worktree add`
/// can succeed.  Returns an error only when the path is still an active worktree.
fn clean_stale_worktree_dir(repo_path: &Path, target_path: &Path) -> Result<(), String> {
    if !target_path.exists() {
        return Ok(());
    }

    // Ask git which paths are active worktrees
    let repo_str = repo_path.to_str().ok_or("Invalid repo path")?;
    let output = safe_output(
        command("git").args(["-C", repo_str, "worktree", "list", "--porcelain"]),
    )
    .map_err(|e| format!("Failed to list worktrees: {}", e))?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let target_normalized = normalize_path(target_path);
        for line in stdout.lines() {
            if let Some(wt_path) = line.strip_prefix("worktree ") {
                if normalize_path(Path::new(wt_path)) == target_normalized {
                    return Err(format!(
                        "Directory '{}' is already an active worktree",
                        target_path.display()
                    ));
                }
            }
        }
    }

    // Not an active worktree — remove the stale directory and prune metadata
    log::info!(
        "Removing stale worktree directory: {}",
        target_path.display()
    );
    std::fs::remove_dir_all(target_path)
        .map_err(|e| format!("Failed to remove stale directory '{}': {}", target_path.display(), e))?;

    let _ = safe_output(command("git").args(["-C", repo_str, "worktree", "prune"]));

    Ok(())
}

/// Create a new worktree
/// Returns Ok(()) on success, Err(error_message) on failure
pub fn create_worktree(repo_path: &Path, branch: &str, target_path: &Path, create_branch: bool) -> Result<(), String> {
    crate::validate_git_ref(branch)?;
    clean_stale_worktree_dir(repo_path, target_path)?;

    let repo_str = repo_path.to_str().ok_or("Invalid repo path")?;
    let target_str = target_path.to_str().ok_or("Invalid target path")?;

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

    let output = safe_output(command("git").args(&args))
        .map_err(|e| format!("Failed to execute git: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(stderr.trim().to_string())
    }
}

/// Create a new worktree with an optional pre-fetched start point.
/// If `start_branch` is Some, creates `-b <branch> <target> origin/<start_branch>`
/// without re-fetching (caller is expected to have fetched already).
pub fn create_worktree_with_start_point(
    repo_path: &Path,
    branch: &str,
    target_path: &Path,
    start_branch: Option<&str>,
) -> Result<(), String> {
    crate::validate_git_ref(branch)?;
    if let Some(sb) = start_branch {
        crate::validate_git_ref(sb)?;
    }
    clean_stale_worktree_dir(repo_path, target_path)?;

    let repo_str = repo_path.to_str().ok_or("Invalid repo path")?;
    let target_str = target_path.to_str().ok_or("Invalid target path")?;

    let mut args = vec!["-C", repo_str, "worktree", "add", "-b", branch, target_str];

    let start_point;
    if let Some(sb) = start_branch {
        start_point = format!("origin/{}", sb);
        args.push(&start_point);
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

/// Fast worktree removal: delete the directory and prune stale worktree metadata.
/// Much faster than `git worktree remove` which does expensive status checks.
/// Only safe when the caller has already handled dirty state (stash/discard).
///
/// Note: `git worktree prune` removes ALL stale entries (not just the one we deleted).
/// This is safe because prune only acts on entries whose directories no longer exist,
/// and we only delete the single target directory before pruning.
pub fn remove_worktree_fast(worktree_path: &Path, main_repo_path: &Path) -> Result<(), String> {
    // Remove the worktree directory (treat NotFound as success — already gone)
    match std::fs::remove_dir_all(worktree_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("Failed to remove worktree directory: {}", e)),
    }

    // Prune stale worktree entries from the main repo
    let main_str = main_repo_path.to_str().ok_or("Invalid main repo path")?;
    let output = safe_output(command("git").args(["-C", main_str, "worktree", "prune"]))
        .map_err(|e| format!("Failed to prune worktrees: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::warn!("git worktree prune warning: {}", stderr.trim());
    }

    Ok(())
}

/// List all branches in a repository
pub fn list_branches(path: &Path) -> Vec<String> {
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
        pr_info: None,
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
    crate::validate_git_ref(target_branch)?;
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
    crate::validate_git_ref(branch)?;
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
    crate::validate_git_ref(branch)?;
    let path_str = repo_path.to_str().ok_or("Invalid path")?;
    let output = command("git")
        .args(["-C", path_str, "branch", "-d", "--", branch])
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
    crate::validate_git_ref(branch)?;
    let path_str = repo_path.to_str().ok_or("Invalid path")?;
    let output = command("git")
        .args(["-C", path_str, "push", "origin", "--delete", "--", branch])
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
    crate::validate_git_ref(branch)?;
    let path_str = repo_path.to_str().ok_or("Invalid path")?;
    let output = command("git")
        .args(["-C", path_str, "push", "origin", "--", branch])
        .output()
        .map_err(|e| format!("Failed to execute git: {}", e))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(stderr.trim().to_string())
    }
}

/// Count commits that haven't been pushed to the branch's own remote.
/// Compares against `origin/<branch>` rather than `@{u}` because worktree
/// branches created from `origin/main` auto-track main, which would
/// incorrectly report all feature commits as unpushed.
/// Returns 0 if the branch has never been pushed (no `origin/<branch>` ref).
pub fn count_unpushed_commits(path: &Path) -> usize {
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return 0,
    };
    let branch = match get_current_branch(path) {
        Some(b) => b,
        None => return 0,
    };
    let remote_ref = format!("origin/{}..HEAD", branch);
    let output = command("git")
        .args(["-C", path_str, "rev-list", &remote_ref, "--count"])
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

/// Get PR info for the current branch (if any PR exists).
/// Uses `gh pr view` which requires the GitHub CLI to be installed and authenticated.
pub fn get_pr_info(path: &Path) -> Option<super::PrInfo> {
    let path_str = path.to_str()?;

    let output = safe_output(
        command("gh")
            .args(["pr", "view", "--json", "url,state,isDraft,number", "--jq", "[.url, .state, .isDraft, .number] | @tsv"])
            .current_dir(path_str),
    )
    .ok()?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout.trim();
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 4 && parts[0].starts_with("http") {
            let url = parts[0].to_string();
            let is_draft = parts[2] == "true";
            let number = parts[3].parse::<u32>().unwrap_or(0);
            let state = if is_draft {
                super::PrState::Draft
            } else {
                match parts[1] {
                    "OPEN" => super::PrState::Open,
                    "MERGED" => super::PrState::Merged,
                    "CLOSED" => super::PrState::Closed,
                    other => {
                        log::warn!("Unknown PR state '{}', defaulting to Open", other);
                        super::PrState::Open
                    }
                }
            };
            return Some(super::PrInfo { url, state, number, ci_checks: None });
        }
    }

    None
}

/// Parse CI check buckets from a JSON array string (extracted for testability).
pub(crate) fn parse_ci_checks(json_str: &str) -> Option<super::CiCheckSummary> {
    let checks: Vec<serde_json::Value> = serde_json::from_str(json_str).ok()?;

    if checks.is_empty() {
        return None;
    }

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut pending = 0usize;

    for check in &checks {
        match check.get("bucket").and_then(|v| v.as_str()) {
            Some("pass") => passed += 1,
            Some("fail") | Some("cancel") => failed += 1,
            Some("pending") => pending += 1,
            _ => {} // "skipping" and unknown — don't count toward total
        }
    }

    let total = passed + failed + pending;
    if total == 0 {
        return None;
    }

    let status = if failed > 0 {
        super::CiStatus::Failure
    } else if pending > 0 {
        super::CiStatus::Pending
    } else {
        super::CiStatus::Success
    };

    Some(super::CiCheckSummary { status, passed, failed, pending, total })
}

/// Get CI check status for the current branch's PR.
/// Uses `gh pr checks --json bucket` which returns a flat JSON array.
pub fn get_ci_checks(path: &Path) -> Option<super::CiCheckSummary> {
    let path_str = path.to_str()?;

    let output = safe_output(
        command("gh")
            .args(["pr", "checks", "--json", "bucket"])
            .current_dir(path_str),
    )
    .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_ci_checks(stdout.trim())
}

/// List worktrees found in the template container directory.
/// Normalize a path by resolving `.` and `..` components without filesystem access.
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => { result.pop(); }
            Component::CurDir => {}
            other => result.push(other),
        }
    }
    result
}

/// Given a worktree checkout path and a subdir, return the project path.
/// If subdir is empty, returns the worktree path as-is.
pub fn project_path_in_worktree(worktree_path: &str, subdir: &Path) -> String {
    if subdir.as_os_str().is_empty() {
        worktree_path.to_string()
    } else {
        PathBuf::from(worktree_path)
            .join(subdir)
            .to_string_lossy()
            .to_string()
    }
}

/// Compute worktree and project paths from template, git root, and subdir.
/// Returns (worktree_path, project_path).
pub fn compute_target_paths(
    git_root: &Path,
    subdir: &Path,
    template: &str,
    branch: &str,
) -> (String, String) {
    let repo_name = git_root.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");
    let safe_branch = branch.replace('/', "-");

    let expanded = template
        .replace("{repo}", repo_name)
        .replace("{branch}", &safe_branch);

    let worktree_path = {
        let path = PathBuf::from(&expanded);
        if path.is_relative() {
            normalize_path(&git_root.join(&expanded))
                .to_string_lossy()
                .to_string()
        } else {
            expanded
        }
    };

    let project_path = project_path_in_worktree(&worktree_path, subdir);

    (worktree_path, project_path)
}


/// Get commit graph with topology (railways) for a repository.
///
/// Uses `git log --graph` to get lane positions, producing both commit rows
/// and connector rows (branch/merge lines between commits).
/// If `branch` is Some, shows the log for that branch instead of HEAD.
pub fn get_commit_graph(path: &Path, limit: usize, branch: Option<&str>) -> Vec<super::GraphRow> {
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return vec![],
    };

    let mut args = vec![
        "-C".to_string(), path_str.to_string(), "log".to_string(), "--graph".to_string(),
        format!("--format=%x00%h%x01%s%x01%an%x01%at%x01%P%x01%D"),
        format!("-n{}", limit),
        "--no-color".to_string(),
    ];
    if let Some(b) = branch {
        args.push(b.to_string());
    }

    let output = safe_output(
        command("git").args(args.iter().map(|s| s.as_str()).collect::<Vec<_>>()),
    )
    .ok();

    match output {
        Some(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            parse_commit_graph_output(&stdout)
        }
        _ => vec![],
    }
}

/// Parse `git log --graph --format="%x00%h%x01%s%x01%an%x01%at%x01%P"` output.
///
/// Lines containing `\x00` are commit lines — everything before is the graph prefix.
/// Lines without `\x00` are graph connector lines (branch/merge topology).
pub(crate) fn parse_commit_graph_output(stdout: &str) -> Vec<super::GraphRow> {
    let mut rows = Vec::new();

    for line in stdout.lines() {
        if let Some(null_pos) = line.find('\x00') {
            // Commit line: graph prefix + commit data
            let graph = line[..null_pos].to_string();
            let data = &line[null_pos + 1..];

            // Fields: hash \x01 message \x01 author \x01 timestamp \x01 parents \x01 decorations
            let parts: Vec<&str> = data.split('\x01').collect();
            if parts.len() < 4 {
                continue;
            }

            let hash = parts[0].to_string();
            let message = parts[1].to_string();
            let author = parts[2].to_string();
            let timestamp = parts[3].parse::<i64>().unwrap_or(0);
            let is_merge = parts.get(4).map_or(false, |p| p.contains(' '));
            let refs: Vec<String> = parts.get(5)
                .filter(|s| !s.is_empty())
                .map(|s| s.split(", ").map(|r| r.to_string()).collect())
                .unwrap_or_default();

            rows.push(super::GraphRow::Commit(super::CommitLogEntry {
                hash,
                message,
                author,
                timestamp,
                is_merge,
                graph,
                refs,
            }));
        } else {
            // Connector line: just graph characters
            let trimmed = line.trim_end();
            if !trimmed.is_empty() {
                rows.push(super::GraphRow::Connector(trimmed.to_string()));
            }
        }
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn get_repo_root_returns_none_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(get_repo_root(&path).is_none());
    }

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

    /// Compare computed paths as `Path` objects for cross-platform correctness
    fn assert_paths_eq(actual: &str, expected: &Path) {
        assert_eq!(Path::new(actual), expected);
    }

    #[test]
    fn target_path_simple_repo() {
        let git_root = PathBuf::from("/projects/myrepo");
        let subdir = Path::new("");
        let (wt, proj) = compute_target_paths(&git_root, subdir, "../{repo}-wt/{branch}", "feature");
        let expected = PathBuf::from("/projects").join("myrepo-wt").join("feature");
        assert_paths_eq(&wt, &expected);
        assert_paths_eq(&proj, &expected);
    }

    #[test]
    fn target_path_monorepo() {
        let git_root = PathBuf::from("/projects/monorepo");
        let subdir = Path::new("app-in-monorepo");
        let (wt, proj) = compute_target_paths(&git_root, subdir, "../{repo}-wt/{branch}", "feature");
        let expected_wt = PathBuf::from("/projects").join("monorepo-wt").join("feature");
        assert_paths_eq(&wt, &expected_wt);
        assert_paths_eq(&proj, &expected_wt.join("app-in-monorepo"));
    }

    #[test]
    fn target_path_nested_monorepo_subdir() {
        let git_root = PathBuf::from("/projects/monorepo");
        let subdir = Path::new("packages/app");
        let (wt, proj) = compute_target_paths(&git_root, subdir, "../{repo}-wt/{branch}", "fix-bug");
        let expected_wt = PathBuf::from("/projects").join("monorepo-wt").join("fix-bug");
        assert_paths_eq(&wt, &expected_wt);
        assert_paths_eq(&proj, &expected_wt.join("packages").join("app"));
    }

    #[test]
    fn target_path_absolute_template() {
        let git_root = PathBuf::from("/projects/monorepo");
        let subdir = Path::new("app");
        let (wt, proj) = compute_target_paths(&git_root, subdir, "/tmp/worktrees/{repo}/{branch}", "main");
        let expected_wt = PathBuf::from("/tmp").join("worktrees").join("monorepo").join("main");
        assert_paths_eq(&wt, &expected_wt);
        assert_paths_eq(&proj, &expected_wt.join("app"));
    }

    #[test]
    fn target_path_branch_with_slashes() {
        let git_root = PathBuf::from("/projects/repo");
        let subdir = Path::new("");
        let (wt, proj) = compute_target_paths(&git_root, subdir, "../{repo}-wt/{branch}", "feature/my-branch");
        let expected = PathBuf::from("/projects").join("repo-wt").join("feature-my-branch");
        assert_paths_eq(&wt, &expected);
        assert_paths_eq(&proj, &expected);
    }

    // ─── get_repo_root worktree / monorepo tests ───────────────────────

    /// Helper: initialise a throwaway git repo with one commit so worktrees can
    /// be created from it.
    fn init_temp_repo() -> (tempfile::TempDir, PathBuf) {
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

    #[test]
    fn get_repo_root_returns_toplevel_for_subdirectory() {
        let (_tmp, repo) = init_temp_repo();
        let sub = repo.join("packages").join("app");
        std::fs::create_dir_all(&sub).unwrap();

        let root = get_repo_root(&sub).expect("should resolve repo root");
        assert_eq!(root, repo.canonicalize().unwrap());
    }

    #[test]
    fn get_repo_root_resolves_worktree_root_not_subdir() {
        let (_tmp, repo) = init_temp_repo();
        // Create a worktree on a new branch
        let wt_path = repo.parent().unwrap().join("my-worktree");
        let status = std::process::Command::new("git")
            .args([
                "-C",
                repo.to_str().unwrap(),
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "-b",
                "wt-branch",
            ])
            .output()
            .expect("git worktree add");
        assert!(status.status.success(), "worktree add failed");

        // Create a nested subdirectory inside the worktree (monorepo subproject)
        let nested = wt_path.join("packages").join("app");
        std::fs::create_dir_all(&nested).unwrap();

        // get_repo_root from the nested subdir should return the worktree root,
        // NOT the main repo — this is the path `git worktree remove` needs.
        let root = get_repo_root(&nested).expect("should resolve worktree root");
        assert_eq!(root, wt_path.canonicalize().unwrap());
    }

    // ─── CI check parsing tests ────────────────────────────────────────

    #[test]
    fn parse_ci_all_pass() {
        let json = r#"[{"bucket":"pass"},{"bucket":"pass"},{"bucket":"pass"}]"#;
        let result = super::parse_ci_checks(json).unwrap();
        assert_eq!(result.status, super::super::CiStatus::Success);
        assert_eq!(result.passed, 3);
        assert_eq!(result.failed, 0);
        assert_eq!(result.pending, 0);
        assert_eq!(result.total, 3);
    }

    #[test]
    fn parse_ci_with_failure() {
        let json = r#"[{"bucket":"pass"},{"bucket":"fail"},{"bucket":"pass"}]"#;
        let result = super::parse_ci_checks(json).unwrap();
        assert_eq!(result.status, super::super::CiStatus::Failure);
        assert_eq!(result.passed, 2);
        assert_eq!(result.failed, 1);
        assert_eq!(result.total, 3);
    }

    #[test]
    fn parse_ci_with_pending() {
        let json = r#"[{"bucket":"pass"},{"bucket":"pending"},{"bucket":"pending"}]"#;
        let result = super::parse_ci_checks(json).unwrap();
        assert_eq!(result.status, super::super::CiStatus::Pending);
        assert_eq!(result.passed, 1);
        assert_eq!(result.pending, 2);
        assert_eq!(result.total, 3);
    }

    #[test]
    fn parse_ci_skipping_excluded_from_total() {
        let json = r#"[{"bucket":"pass"},{"bucket":"skipping"},{"bucket":"pass"}]"#;
        let result = super::parse_ci_checks(json).unwrap();
        assert_eq!(result.status, super::super::CiStatus::Success);
        assert_eq!(result.passed, 2);
        assert_eq!(result.total, 2);
    }

    #[test]
    fn parse_ci_cancel_counts_as_failure() {
        let json = r#"[{"bucket":"pass"},{"bucket":"cancel"}]"#;
        let result = super::parse_ci_checks(json).unwrap();
        assert_eq!(result.status, super::super::CiStatus::Failure);
        assert_eq!(result.failed, 1);
    }

    #[test]
    fn parse_ci_empty_array() {
        assert!(super::parse_ci_checks("[]").is_none());
    }

    #[test]
    fn parse_ci_invalid_json() {
        assert!(super::parse_ci_checks("not json").is_none());
    }

    #[test]
    fn parse_ci_only_skipping() {
        let json = r#"[{"bucket":"skipping"},{"bucket":"skipping"}]"#;
        assert!(super::parse_ci_checks(json).is_none());
    }

    // ─── commit graph parsing tests ────────────────────────────────────

    #[test]
    fn parse_graph_linear_commits() {
        let output = "* \x00abc1234\x01Fix bug\x01alice\x011700000000\x01aabbccdd\x01HEAD -> main, origin/main\n\
                       * \x00def5678\x01Add test\x01bob\x011699999000\x01abc1234\x01\n";
        let rows = super::parse_commit_graph_output(output);
        assert_eq!(rows.len(), 2);
        match &rows[0] {
            super::super::GraphRow::Commit(e) => {
                assert_eq!(e.hash, "abc1234");
                assert_eq!(e.graph, "* ");
                assert!(!e.is_merge);
                assert_eq!(e.refs, vec!["HEAD -> main", "origin/main"]);
            }
            _ => panic!("expected commit row"),
        }
        match &rows[1] {
            super::super::GraphRow::Commit(e) => {
                assert!(e.refs.is_empty());
            }
            _ => panic!("expected commit row"),
        }
    }

    #[test]
    fn parse_graph_with_connectors() {
        let output = "*   \x00aaa1111\x01Merge PR\x01carol\x011700000000\x01bbb2222 ccc3333\x01\n\
                       |\\  \n\
                       | * \x00ccc3333\x01Feature\x01dave\x011699999000\x01ddd4444\x01\n\
                       |/  \n\
                       * \x00ddd4444\x01Base\x01eve\x011699998000\x01eee5555\x01\n";
        let rows = super::parse_commit_graph_output(output);
        assert_eq!(rows.len(), 5);
        // Row 0: merge commit
        assert!(matches!(&rows[0], super::super::GraphRow::Commit(e) if e.is_merge));
        // Row 1: connector "|\  "
        assert!(matches!(&rows[1], super::super::GraphRow::Connector(g) if g.contains('\\')));
        // Row 2: branch commit
        assert!(matches!(&rows[2], super::super::GraphRow::Commit(e) if e.hash == "ccc3333"));
        // Row 3: connector "|/  "
        assert!(matches!(&rows[3], super::super::GraphRow::Connector(g) if g.contains('/')));
        // Row 4: base commit
        assert!(matches!(&rows[4], super::super::GraphRow::Commit(_)));
    }

    #[test]
    fn parse_graph_empty() {
        assert!(super::parse_commit_graph_output("").is_empty());
        assert!(super::parse_commit_graph_output("\n").is_empty());
    }

    #[test]
    fn parse_graph_preserves_graph_prefix() {
        let output = "| | * \x00fff6666\x01Deep branch\x01frank\x011700000000\x01ggg7777\x01\n";
        let rows = super::parse_commit_graph_output(output);
        assert_eq!(rows.len(), 1);
        match &rows[0] {
            super::super::GraphRow::Commit(e) => {
                assert_eq!(e.graph, "| | * ");
            }
            _ => panic!("expected commit row"),
        }
    }

    #[test]
    fn parse_graph_refs() {
        // Single ref
        let output = "* \x00aaa1111\x01Msg\x01alice\x011700000000\x01bbb2222\x01tag: v1.0\n";
        let rows = super::parse_commit_graph_output(output);
        match &rows[0] {
            super::super::GraphRow::Commit(e) => {
                assert_eq!(e.refs, vec!["tag: v1.0"]);
            }
            _ => panic!("expected commit row"),
        }

        // Multiple refs
        let output = "* \x00aaa1111\x01Msg\x01alice\x011700000000\x01bbb2222\x01HEAD -> main, origin/main, tag: v2.0\n";
        let rows = super::parse_commit_graph_output(output);
        match &rows[0] {
            super::super::GraphRow::Commit(e) => {
                assert_eq!(e.refs, vec!["HEAD -> main", "origin/main", "tag: v2.0"]);
            }
            _ => panic!("expected commit row"),
        }

        // No refs (empty decoration field)
        let output = "* \x00aaa1111\x01Msg\x01alice\x011700000000\x01bbb2222\x01\n";
        let rows = super::parse_commit_graph_output(output);
        match &rows[0] {
            super::super::GraphRow::Commit(e) => {
                assert!(e.refs.is_empty());
            }
            _ => panic!("expected commit row"),
        }
    }
}
