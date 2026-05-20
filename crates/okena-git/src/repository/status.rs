//! Working-tree status, diff stats, HEAD/branch reads, and ahead/behind counts.

use std::path::Path;

use okena_core::process::{command, safe_output};

use super::path_str;
use crate::GitStatus;

/// Three-state result of a fresh git status fetch.
///
/// Distinguishing "not a repo" from "transient failure" lets the polling
/// watcher preserve the last known +/- counts instead of clobbering them
/// with `(0, 0)` whenever `git diff --numstat HEAD` or the gix index walk
/// briefly fails (lock contention with a concurrent `git add`, partial
/// `.git/index` rewrite, etc).
pub enum StatusFetch {
    /// Got a fresh reading.
    Status(GitStatus),
    /// Path is definitively not inside a git repository.
    NotRepo,
    /// Transient failure — caller should keep the last known cached value.
    Transient,
}

/// Get git status for a directory path.
pub fn get_status(path: &Path) -> StatusFetch {
    if crate::gix_helpers::open(path).is_none() {
        return StatusFetch::NotRepo;
    }

    let branch = get_current_branch(path);
    let Some((lines_added, lines_removed)) = get_diff_stats(path) else {
        return StatusFetch::Transient;
    };
    let (ahead, behind) = match count_ahead_behind(path) {
        Some((a, b)) => (Some(a), Some(b)),
        None => (None, None),
    };
    let unpushed = count_unpushed_commits(path);

    StatusFetch::Status(GitStatus {
        branch,
        lines_added,
        lines_removed,
        pr_info: None,
        ci_checks: None,
        ahead,
        behind,
        unpushed,
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

/// Get the full 40-character SHA of HEAD, or `None` if not a git repo or HEAD
/// has no commits yet. Used for branch-level CI lookups via the GitHub REST
/// API (`/commits/{sha}/check-runs` and `/status`).
pub fn get_head_sha(path: &Path) -> Option<String> {
    let repo = crate::gix_helpers::open(path)?;
    let id = repo.head_id().ok()?;
    Some(id.to_hex().to_string())
}

/// Get diff statistics (lines added, lines removed) for working directory.
///
/// Returns `None` on transient failure (numstat spawn failed, numstat exited
/// non-zero, or the gix-based untracked walk errored). The polling watcher
/// uses `None` to keep the last known +/- so a single bad cycle doesn't
/// blank the badge — see `StatusFetch::Transient`.
///
/// Still shells out to `git diff --numstat HEAD`: the gix equivalent would
/// require a 3-way walk (HEAD tree → index → worktree) plus per-blob line
/// diffing via imara-diff. This is the last remaining spawn in the polling
/// hot path; everything else is now gix-native.
fn get_diff_stats(path: &Path) -> Option<(usize, usize)> {
    let path_str = path.to_str()?;

    let (mut added, mut removed) = (0usize, 0usize);

    // --no-renames: report renames as a delete of the old path + add of the
    // new path rather than numstat's `old => new` arrow form. Consistent with
    // get_diff_file_summary in lib.rs.
    match safe_output(
        command("git").args(["-C", path_str, "diff", "--numstat", "--no-renames", "--no-color", "--no-ext-diff", "HEAD"]),
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
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log::warn!(
                "git diff --numstat HEAD exited {} for {}: {}",
                output.status.code().map(|c| c.to_string()).unwrap_or_else(|| "<signal>".into()),
                path_str,
                stderr.trim(),
            );
            return None;
        }
        Err(e) => {
            log::warn!("git diff --numstat HEAD spawn failed for {}: {e}", path_str);
            return None;
        }
    }

    // Also include untracked files (count lines). A None here means the gix
    // status walk failed transiently — propagate so we don't undercount.
    let untracked = crate::gix_helpers::list_untracked_files(path)?;
    for file in untracked {
        let file_path = path.join(&file);
        if let Ok(content) = std::fs::read_to_string(&file_path) {
            added += content.lines().count();
        }
    }

    Some((added, removed))
}

/// Count commits the local branch is ahead of / behind its upstream.
/// Returns `None` if HEAD is detached or no upstream is configured.
///
/// Short-circuits via gix when no upstream is configured for the current
/// branch, so the common "branch without remote tracking" case avoids the
/// `git rev-list` subprocess entirely.
pub fn count_ahead_behind(path: &Path) -> Option<(usize, usize)> {
    let repo = crate::gix_helpers::open(path)?;
    let branch = super::head_branch_short(&repo)?;

    // Cheap upstream check via gix — most branches without an upstream
    // hit this fast path and skip the spawn.
    let has_upstream = repo
        .find_reference(&format!("refs/heads/{}", branch))
        .ok()
        .and_then(|r| {
            let head_ref: gix::refs::FullName = r.name().into();
            repo.branch_remote_tracking_ref_name(head_ref.as_ref(), gix::remote::Direction::Fetch)
                .and_then(|res| res.ok())
        })
        .is_some();
    if !has_upstream {
        return None;
    }

    // `git rev-list --left-right --count <upstream>...HEAD` prints
    // "<behind>\t<ahead>".
    let revspec = format!("{0}@{{upstream}}...{0}", branch);
    let p = path_str(path).ok()?;
    let output = command("git")
        .args(["-C", p, "rev-list", "--left-right", "--count", &revspec])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut parts = stdout.split_whitespace();
    let behind: usize = parts.next()?.parse().ok()?;
    let ahead: usize = parts.next()?.parse().ok()?;
    Some((ahead, behind))
}

/// Count commits that haven't been pushed to the branch's own remote.
/// Compares against `origin/<branch>` rather than `@{u}` because worktree
/// branches created from `origin/main` auto-track main, which would
/// incorrectly report all feature commits as unpushed.
///
/// Returns `None` when there is no `origin/<branch>` ref (branch has never
/// been pushed, or remote not configured). Returns `Some(n)` otherwise —
/// `Some(0)` means everything is pushed.
pub fn count_unpushed_commits(path: &Path) -> Option<usize> {
    let repo = crate::gix_helpers::open(path)?;
    let branch = get_current_branch(path)?;

    let revspec = format!("origin/{}..HEAD", branch);
    let spec = repo.rev_parse(revspec.as_str()).ok()?;

    let gix::revision::plumbing::Spec::Range { from, to } = spec.detach() else {
        return None;
    };

    let walk = repo.rev_walk([to]).with_hidden([from]).all().ok()?;

    Some(walk.filter_map(Result::ok).count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::test_support::{git_in, init_temp_repo};
    use std::path::PathBuf;

    #[test]
    fn get_status_returns_not_repo_for_non_git_path() {
        let tmp = tempfile::tempdir().expect("create temp dir");
        match get_status(tmp.path()) {
            StatusFetch::NotRepo => {}
            other => panic!("expected NotRepo for non-git path, got {:?}", match other {
                StatusFetch::Status(_) => "Status",
                StatusFetch::NotRepo => "NotRepo",
                StatusFetch::Transient => "Transient",
            }),
        }
    }

    #[test]
    fn get_status_returns_status_for_clean_repo() {
        let (_tmp, repo) = init_temp_repo();
        match get_status(&repo) {
            StatusFetch::Status(s) => {
                assert_eq!(s.branch.as_deref(), Some("main"));
                assert_eq!(s.lines_added, 0);
                assert_eq!(s.lines_removed, 0);
            }
            StatusFetch::NotRepo => panic!("expected Status, got NotRepo"),
            StatusFetch::Transient => panic!("expected Status, got Transient"),
        }
    }

    #[test]
    fn get_status_counts_untracked_lines() {
        let (_tmp, repo) = init_temp_repo();
        std::fs::write(repo.join("new.txt"), "line1\nline2\nline3\n").unwrap();
        match get_status(&repo) {
            StatusFetch::Status(s) => assert_eq!(s.lines_added, 3),
            other => panic!("expected Status with 3 untracked lines, got {}", match other {
                StatusFetch::Status(_) => "Status",
                StatusFetch::NotRepo => "NotRepo",
                StatusFetch::Transient => "Transient",
            }),
        }
    }

    #[test]
    fn has_uncommitted_changes_returns_false_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(!has_uncommitted_changes(&path));
    }

    #[test]
    fn get_current_branch_returns_none_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(get_current_branch(&path).is_none());
    }

    #[test]
    fn count_unpushed_commits_returns_none_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert_eq!(count_unpushed_commits(&path), None);
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

        let untracked = crate::gix_helpers::list_untracked_files(&repo)
            .expect("gix status should succeed on a clean test repo");
        assert!(untracked.contains(&"seen.txt".to_string()));
        assert!(!untracked.contains(&"ignored.txt".to_string()));
    }

    #[test]
    fn count_unpushed_returns_none_when_no_remote() {
        let (_tmp, repo) = init_temp_repo();
        // No origin/main exists — should return None.
        assert_eq!(count_unpushed_commits(&repo), None);
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
        assert_eq!(count_unpushed_commits(&repo), Some(0));

        // Add two new commits locally.
        for i in 0..2 {
            std::fs::write(repo.join(format!("new{}.txt", i)), "x").unwrap();
            git_in(&repo, &["add", "."]);
            git_in(
                &repo,
                &["-c", "commit.gpgsign=false", "commit", "-m", &format!("c{}", i)],
            );
        }

        assert_eq!(count_unpushed_commits(&repo), Some(2));
    }

    #[test]
    fn count_ahead_behind_returns_none_without_upstream() {
        let (_tmp, repo) = init_temp_repo();
        // No remote, no upstream configured — must return None instead of (0,0).
        assert!(count_ahead_behind(&repo).is_none());
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
}
