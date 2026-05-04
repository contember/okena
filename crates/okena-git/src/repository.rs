use std::path::{Component, Path, PathBuf};

use crate::error::{GitError, GitResult};
use crate::GitStatus;
use okena_core::process::{command, safe_output};

/// Run a git command and return `Ok(())` if it exits successfully,
/// or `Err(GitExitError)` with the stderr message.
fn require_success(output: std::process::Output) -> GitResult<()> {
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
fn path_str(path: &Path) -> GitResult<&str> {
    path.to_str().ok_or_else(|| GitError::InvalidPath(path.to_path_buf()))
}

/// Get the root directory of the git repository containing the given path.
/// Returns None if the path is not inside a git repository.
pub fn get_repo_root(path: &Path) -> Option<PathBuf> {
    let repo = crate::gix_helpers::open(path)?;
    repo.workdir().map(|p| p.to_path_buf())
}

/// Get branches that are already checked out in worktrees (main + linked).
/// Detached worktrees are skipped.
pub(crate) fn get_worktree_branches(path: &Path) -> Vec<String> {
    list_git_worktrees(path).into_iter().map(|(_, b)| b).collect()
}

/// Read the short branch name from a repo's HEAD, or `None` if detached.
fn head_branch_short(repo: &gix::Repository) -> Option<String> {
    repo.head_name().ok().flatten().map(|n| n.shorten().to_string())
}

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
            if let Some(wt_path) = line.strip_prefix("worktree ") {
                if normalize_path(Path::new(wt_path)) == target_normalized {
                    return Err(GitError::WorktreeExists {
                        path: target_path.to_path_buf(),
                    });
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

/// List all branches in a repository (local + remotes), deduplicating
/// `origin/<name>` against local `<name>` and skipping `*/HEAD` symrefs.
pub fn list_branches(path: &Path) -> Vec<String> {
    let Some(repo) = crate::gix_helpers::open(path) else {
        return vec![];
    };

    let Ok(refs) = repo.references() else {
        return vec![];
    };

    let mut branches: Vec<String> = Vec::new();
    let mut local_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    if let Ok(iter) = refs.local_branches() {
        for r in iter.flatten() {
            let name = r.name().shorten().to_string();
            if !name.is_empty() {
                local_names.insert(name.clone());
                branches.push(name);
            }
        }
    }

    if let Ok(iter) = refs.remote_branches() {
        for r in iter.flatten() {
            let name = r.name().shorten().to_string();
            if name.is_empty() || name.ends_with("/HEAD") {
                continue;
            }
            if let Some(local) = name.strip_prefix("origin/") {
                if local_names.contains(local) {
                    continue;
                }
            }
            branches.push(name);
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
    let Some(repo) = crate::gix_helpers::open(path) else {
        return false;
    };

    let Ok(platform) = repo.status(gix::progress::Discard) else {
        return false;
    };

    let Ok(iter) = platform
        .untracked_files(gix::status::UntrackedFiles::Files)
        .into_iter(None)
    else {
        return false;
    };

    iter.filter_map(Result::ok).next().is_some()
}

/// Get the current branch name or short commit hash for detached HEAD.
pub fn get_current_branch(path: &Path) -> Option<String> {
    let repo = crate::gix_helpers::open(path)?;
    let head = repo.head().ok()?;

    if let Some(name) = head.referent_name() {
        // Use the file-name component for the short branch name (matches
        // `git symbolic-ref --short HEAD`, which strips `refs/heads/`).
        return Some(name.shorten().to_string());
    }

    // Detached HEAD — return short hash of HEAD's commit.
    let id = head.id()?;
    Some(id.shorten().ok()?.to_string())
}

/// Get diff statistics (lines added, lines removed) for working directory.
///
/// Still shells out to `git diff --numstat HEAD`: the gix equivalent would
/// require a 3-way walk (HEAD tree → index → worktree) plus per-blob line
/// diffing via imara-diff. This is the last remaining spawn in the polling
/// hot path; everything else is now gix-native.
fn get_diff_stats(path: &Path) -> (usize, usize) {
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return (0, 0),
    };

    // Get diff stats for staged + unstaged changes
    let (mut added, mut removed) = (0usize, 0usize);

    match safe_output(
        command("git").args(["-C", path_str, "diff", "--numstat", "--no-color", "--no-ext-diff", "HEAD"]),
    ) {
        Ok(output) if output.status.success() => {
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
        Ok(_) => {}
        Err(e) => log::warn!("git diff --numstat failed: {e}"),
    }

    // Also include untracked files (count lines)
    for file in crate::gix_helpers::list_untracked_files(path) {
        let file_path = path.join(&file);
        if let Ok(content) = std::fs::read_to_string(&file_path) {
            added += content.lines().count();
        }
    }

    (added, removed)
}

/// Get the default branch of a repository (e.g. "main" or "master").
/// Checks the `origin/HEAD` symref first, then falls back to checking for
/// local `main` / `master` branches.
pub fn get_default_branch(repo_path: &Path) -> Option<String> {
    let repo = crate::gix_helpers::open(repo_path)?;

    // Read refs/remotes/origin/HEAD; it is a symbolic ref whose target points
    // at e.g. refs/remotes/origin/main.
    if let Ok(head_ref) = repo.find_reference("refs/remotes/origin/HEAD") {
        if let Some(target_name) = head_ref.target().try_name() {
            let target = target_name.as_bstr().to_string();
            if let Some(branch) = target.strip_prefix("refs/remotes/origin/") {
                if !branch.is_empty() {
                    return Some(branch.to_string());
                }
            }
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

/// Count commits that haven't been pushed to the branch's own remote.
/// Compares against `origin/<branch>` rather than `@{u}` because worktree
/// branches created from `origin/main` auto-track main, which would
/// incorrectly report all feature commits as unpushed.
/// Returns 0 if the branch has never been pushed (no `origin/<branch>` ref).
pub fn count_unpushed_commits(path: &Path) -> usize {
    let Some(repo) = crate::gix_helpers::open(path) else {
        return 0;
    };
    let Some(branch) = get_current_branch(path) else {
        return 0;
    };

    let revspec = format!("origin/{}..HEAD", branch);
    let Ok(spec) = repo.rev_parse(revspec.as_str()) else {
        return 0;
    };

    let gix::revision::plumbing::Spec::Range { from, to } = spec.detach() else {
        return 0;
    };

    let Ok(walk) = repo.rev_walk([to]).with_hidden([from]).all() else {
        return 0;
    };

    walk.filter_map(Result::ok).count()
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
    if let Ok(main_repo) = gix::open(repo.common_dir()) {
        if let (Some(workdir), Some(branch)) = (main_repo.workdir(), head_branch_short(&main_repo)) {
            result.push((workdir.to_string_lossy().into_owned(), branch));
        }
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

/// Resolve the git repository root and the project subdirectory within it.
///
/// For a monorepo project at `/repo/packages/app`, returns
/// `(/repo, packages/app)`. For a root-level project, subdir is empty.
/// Both paths are normalized before `strip_prefix` to handle symlinks,
/// trailing slashes, and `..` components.
pub fn resolve_git_root_and_subdir(project_path: &Path) -> (PathBuf, PathBuf) {
    let git_root = get_repo_root(project_path)
        .unwrap_or_else(|| project_path.to_path_buf());
    let norm_project = normalize_path(project_path);
    let norm_root = normalize_path(&git_root);
    let subdir = norm_project.strip_prefix(&norm_root)
        .unwrap_or(Path::new(""))
        .to_path_buf();
    (git_root, subdir)
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

    match safe_output(
        command("git").args(args.iter().map(|s| s.as_str()).collect::<Vec<_>>()),
    ) {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            parse_commit_graph_output(&stdout)
        }
        Ok(_) => vec![],
        Err(e) => {
            log::warn!("git log --graph failed: {e}");
            vec![]
        }
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

    /// Run a git command in `repo`, asserting success.
    fn git_in(repo: &Path, args: &[&str]) {
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

    #[test]
    fn has_uncommitted_detects_untracked() {
        let (_tmp, repo) = init_temp_repo();
        std::fs::write(repo.join("untracked.txt"), "hello").unwrap();
        assert!(has_uncommitted_changes(&repo));
    }

    #[test]
    fn has_uncommitted_detects_modified_tracked() {
        let (_tmp, repo) = init_temp_repo();
        std::fs::write(repo.join("file.txt"), "modified").unwrap();
        assert!(has_uncommitted_changes(&repo));
    }

    #[test]
    fn has_uncommitted_detects_staged_only() {
        let (_tmp, repo) = init_temp_repo();
        std::fs::write(repo.join("file.txt"), "staged change").unwrap();
        git_in(&repo, &["add", "file.txt"]);
        assert!(has_uncommitted_changes(&repo));
    }

    #[test]
    fn has_uncommitted_returns_false_for_clean_repo() {
        let (_tmp, repo) = init_temp_repo();
        assert!(!has_uncommitted_changes(&repo));
    }

    #[test]
    fn untracked_listing_honors_gitignore() {
        let (_tmp, repo) = init_temp_repo();
        std::fs::write(repo.join(".gitignore"), "ignored.txt\n").unwrap();
        git_in(&repo, &["add", ".gitignore"]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "ignore"],
        );

        std::fs::write(repo.join("ignored.txt"), "x").unwrap();
        std::fs::write(repo.join("seen.txt"), "y").unwrap();

        let untracked = crate::gix_helpers::list_untracked_files(&repo);
        assert!(untracked.contains(&"seen.txt".to_string()));
        assert!(!untracked.contains(&"ignored.txt".to_string()));
    }

    #[test]
    fn count_unpushed_returns_zero_when_no_remote() {
        let (_tmp, repo) = init_temp_repo();
        // No origin/main exists — should return 0, not error.
        assert_eq!(count_unpushed_commits(&repo), 0);
    }

    #[test]
    fn count_unpushed_returns_correct_count() {
        let (_tmp, repo) = init_temp_repo();
        let remote_tmp = tempfile::tempdir().expect("create remote tempdir");
        let remote_path = remote_tmp.path().join("origin.git");
        git_in(&repo, &["init", "--bare", remote_path.to_str().unwrap()]);
        git_in(&repo, &["remote", "add", "origin", remote_path.to_str().unwrap()]);
        git_in(&repo, &["push", "-u", "origin", "main"]);

        // No unpushed commits yet.
        assert_eq!(count_unpushed_commits(&repo), 0);

        // Add two new commits locally.
        for i in 0..2 {
            std::fs::write(repo.join(format!("new{}.txt", i)), "x").unwrap();
            git_in(&repo, &["add", "."]);
            git_in(
                &repo,
                &["-c", "commit.gpgsign=false", "commit", "-m", &format!("c{}", i)],
            );
        }

        assert_eq!(count_unpushed_commits(&repo), 2);
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

        let mut branches = get_worktree_branches(&repo);
        branches.sort();
        assert_eq!(branches, vec!["feat", "main"]);
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
    fn get_default_branch_falls_back_to_main_locally() {
        let (_tmp, repo) = init_temp_repo();
        // No origin/HEAD exists — should fall back to local "main".
        assert_eq!(get_default_branch(&repo).as_deref(), Some("main"));
    }

    #[test]
    fn get_current_branch_returns_main_after_init() {
        let (_tmp, repo) = init_temp_repo();
        assert_eq!(get_current_branch(&repo).as_deref(), Some("main"));
    }

    #[test]
    fn get_current_branch_returns_short_hash_when_detached() {
        let (_tmp, repo) = init_temp_repo();
        // Detach HEAD on the current commit
        git_in(&repo, &["checkout", "--detach", "HEAD"]);
        let branch = get_current_branch(&repo).expect("should return short hash");
        // Short hash from gix has at least 7 chars and is hex
        assert!(branch.len() >= 7, "expected short hash, got {:?}", branch);
        assert!(branch.chars().all(|c| c.is_ascii_hexdigit()), "expected hex hash, got {:?}", branch);
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
        // Worktree lives in its own tempdir so parallel runs don't collide on
        // a shared /tmp path that survives between runs.
        let wt_tmp = tempfile::tempdir().expect("create worktree tempdir");
        let wt_path = wt_tmp.path().join("my-worktree");
        git_in(
            &repo,
            &["worktree", "add", wt_path.to_str().unwrap(), "-b", "wt-branch"],
        );

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
