//! Branch operations: list / classify / checkout / create / delete / push,
//! plus default-branch resolution, rebase, merge, stash, and per-file
//! stage/unstage/discard.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use okena_core::process::{command, safe_output};

use super::{head_branch_short, network_command, path_str, require_success};
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
    let used_branches: std::collections::HashSet<_> =
        super::get_worktree_branches(path).into_iter().collect();

    all_branches
        .into_iter()
        .filter(|b| !used_branches.contains(b))
        .collect()
}

/// Get the default branch of a repository (e.g. "main" or "master").
/// Checks the `origin/HEAD` symref first, then falls back to checking for
/// remote/local `main` / `master` branches.
pub fn get_default_branch(repo_path: &Path) -> Option<String> {
    let repo = crate::gix_helpers::open(repo_path)?;

    // Read refs/remotes/origin/HEAD; it is a symbolic ref whose target points
    // at e.g. refs/remotes/origin/main. Ignore stale/dangling targets left by
    // renamed or deleted default branches.
    if let Ok(head_ref) = repo.find_reference("refs/remotes/origin/HEAD")
        && let Some(target_name) = head_ref.target().try_name()
    {
        let target = target_name.as_bstr().to_string();
        if let Some(branch) = target.strip_prefix("refs/remotes/origin/")
            && !branch.is_empty()
            && repo.find_reference(target.as_str()).is_ok()
        {
            return Some(branch.to_string());
        }
    }

    // Fallback: check if main or master exists on origin, then locally.
    for candidate in ["main", "master"] {
        if repo
            .find_reference(&format!("refs/remotes/origin/{}", candidate))
            .is_ok()
        {
            return Some(candidate.to_string());
        }
    }
    for candidate in ["main", "master"] {
        if repo
            .find_reference(&format!("refs/heads/{}", candidate))
            .is_ok()
        {
            return Some(candidate.to_string());
        }
    }

    None
}

/// Resolve the ref to diff against for "review changes" — a three-dot
/// `base...HEAD` diff that shows everything the current branch/worktree added
/// since it diverged from the default branch (the standard "PR diff").
///
/// Prefers `origin/<default>` so the review matches what an eventual PR would
/// show; falls back to the local `<default>` branch. Returns `None` when there
/// is no sensible base — no default branch is resolvable, or HEAD is already on
/// the default branch (reviewing it against itself would be empty).
pub fn resolve_review_base(repo_path: &Path) -> Option<String> {
    let default = get_default_branch(repo_path)?;

    // Don't offer reviewing the default branch against itself.
    if let Some(current) = super::status::get_current_branch(repo_path)
        && current == default
    {
        return None;
    }

    resolve_base_ref(repo_path, &default)
}

/// Resolve the best local ref to compare against for a target branch `name`,
/// preferring the pushed copy so counts match what a PR would diff against:
/// `upstream/<name>` → `origin/<name>` → local `<name>`. Returns `None` when
/// none resolve.
///
/// `upstream` wins over `origin` for the fork workflow: when `origin` is the
/// user's fork and `upstream` is the canonical repo the PR actually targets,
/// the fork's copy lags, so comparing against it inflates "ahead" and hides
/// "behind". Ordinary single-remote repos have no `upstream` and fall through
/// to `origin/<name>` exactly as before.
pub fn resolve_base_ref(repo_path: &Path, name: &str) -> Option<String> {
    let repo = crate::gix_helpers::open(repo_path)?;
    for remote in ["upstream", "origin"] {
        if repo
            .find_reference(&format!("refs/remotes/{remote}/{name}"))
            .is_ok()
        {
            return Some(format!("{remote}/{name}"));
        }
    }
    if repo.find_reference(&format!("refs/heads/{name}")).is_ok() {
        return Some(name.to_string());
    }
    None
}

/// Rebase the current branch onto a target branch.
/// Automatically aborts on failure.
pub fn rebase_onto(worktree_path: &Path, target_branch: &str) -> GitResult<()> {
    crate::validate_git_ref(target_branch)?;
    let wt_str = path_str(worktree_path)?;

    let output = safe_output(command("git").args(["-C", wt_str, "rebase", target_branch]))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

        // Abort the failed rebase
        let _ = safe_output(command("git").args(["-C", wt_str, "rebase", "--abort"]));

        Err(GitError::GitExitError {
            status: output.status.code().unwrap_or(-1),
            stderr,
        })
    }
}

/// Stash uncommitted changes.
pub fn stash_changes(path: &Path) -> GitResult<()> {
    let p = path_str(path)?;
    let output =
        safe_output(command("git").args(["-C", p, "stash", "push", "--include-untracked"]))?;
    require_success(output)?;
    if crate::repository::status::has_uncommitted_changes(path) {
        return Err(GitError::UnsafeWorktree {
            path: path.to_path_buf(),
            reason: "checkout remains dirty after stash; refusing destructive removal".to_string(),
        });
    }
    Ok(())
}

/// Pop the most recent stash entry.
/// Used for recovery when rebase/merge fails after stash.
pub fn stash_pop(path: &Path) -> GitResult<()> {
    let p = path_str(path)?;
    let output = safe_output(command("git").args(["-C", p, "stash", "pop"]))?;
    require_success(output)
}

/// Stage a file (git add -- <file>).
pub fn stage_file(repo_path: &Path, file_path: &str) -> GitResult<()> {
    let p = path_str(repo_path)?;
    let output = safe_output(command("git").args(["-C", p, "add", "--", file_path]))?;
    require_success(output)
}

/// Unstage a file from the index (git restore --staged -- <file>).
/// Works for both modified and newly-added files.
pub fn unstage_file(repo_path: &Path, file_path: &str) -> GitResult<()> {
    let p = path_str(repo_path)?;
    let output =
        safe_output(command("git").args(["-C", p, "restore", "--staged", "--", file_path]))?;
    require_success(output)
}

/// Discard working-tree changes for a file (git checkout HEAD -- <file>).
/// Restores the file to its HEAD state.
pub fn discard_file_changes(repo_path: &Path, file_path: &str) -> GitResult<()> {
    let p = path_str(repo_path)?;
    let output = safe_output(command("git").args(["-C", p, "checkout", "HEAD", "--", file_path]))?;
    require_success(output)
}

/// Fetch from all remotes.
pub fn fetch_all(path: &Path) -> GitResult<()> {
    let p = path_str(path)?;
    let output = safe_output(network_command().args(["-C", p, "fetch", "--all"]))?;
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

    let output = safe_output(command("git").args(&args))?;
    require_success(output)
}

/// Delete a local branch (uses `-d`, fails if branch has unmerged changes).
pub fn delete_local_branch(repo_path: &Path, branch: &str) -> GitResult<()> {
    crate::validate_git_ref(branch)?;
    let p = path_str(repo_path)?;
    let output = safe_output(command("git").args(["-C", p, "branch", "-d", "--", branch]))?;
    require_success(output)
}

/// Delete a remote branch.
pub fn delete_remote_branch(repo_path: &Path, branch: &str) -> GitResult<()> {
    crate::validate_git_ref(branch)?;
    let p = path_str(repo_path)?;
    let output =
        safe_output(network_command().args(["-C", p, "push", "origin", "--delete", "--", branch]))?;
    require_success(output)
}

/// Push a branch to origin.
pub fn push_branch(repo_path: &Path, branch: &str) -> GitResult<()> {
    crate::validate_git_ref(branch)?;
    let p = path_str(repo_path)?;
    let output = safe_output(network_command().args(["-C", p, "push", "origin", "--", branch]))?;
    require_success(output)
}

/// Branch list classified into local and remote, with the current branch name
/// (if HEAD points at a branch).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchList {
    /// Local branch names.
    pub local: Vec<String>,
    /// Remote branch names that don't have a matching local branch (e.g.
    /// `origin/release` when there's no local `release`). Always includes
    /// the remote prefix.
    pub remote: Vec<String>,
    /// Current HEAD branch name (`None` if detached).
    pub current: Option<String>,
    /// Per-branch metadata, keyed by the same names used in `local`/`remote`.
    /// Empty when the metadata pass failed, or when it comes from a remote host
    /// that predates this field — consumers must treat a missing entry as
    /// "unknown" and still show the branch.
    #[serde(default)]
    pub details: HashMap<String, BranchDetail>,
}

/// What a branch picker can show beside the name: how recently the branch
/// moved, how it sits against its upstream, and whether another worktree holds
/// it. Collected for every branch in one [`collect_branch_details`] pass.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchDetail {
    /// Committer time of the branch tip, as a Unix timestamp.
    #[serde(default)]
    pub committed_at: Option<i64>,
    /// How the branch sits against its configured upstream.
    #[serde(default)]
    pub upstream: UpstreamState,
    /// Worktree holding this branch, when it is not the one we are asking
    /// from. Checking such a branch out fails, so the UI can say why up front.
    #[serde(default)]
    pub worktree: Option<String>,
}

/// A branch's relation to its configured upstream ref.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamState {
    /// No upstream configured — the branch exists only locally.
    #[default]
    Untracked,
    /// The upstream ref is configured but no longer exists on the remote.
    Gone,
    /// Tracking `name`, `ahead`/`behind` commits apart from it. Both zero
    /// means in sync.
    Tracked {
        name: String,
        ahead: usize,
        behind: usize,
    },
}

/// One line per branch: `<short name> <tip time> <upstream> <track> <worktree> <HEAD marker>`,
/// tab-separated. Tabs cannot appear in a ref name, and git emits none inside
/// these fields.
const DETAIL_FORMAT: &str = concat!(
    "%(refname:short)\t",
    "%(committerdate:unix)\t",
    "%(upstream:short)\t",
    "%(upstream:track)\t",
    "%(worktreepath)\t",
    "%(HEAD)"
);

/// Collect [`BranchDetail`] for every branch in a single `git for-each-ref`.
///
/// One subprocess beats a per-branch `gix` rev-walk by a wide margin here: git
/// answers ahead/behind for every ref off its own commit-graph in a single
/// pass (~10ms for ~70 refs). Soft-fails to an empty map — the branch list is
/// still usable without metadata, e.g. against a git too old for
/// `%(worktreepath)` (< 2.23).
fn collect_branch_details(path: &Path) -> HashMap<String, BranchDetail> {
    let Ok(p) = path_str(path) else {
        return HashMap::new();
    };
    // `LC_ALL=C` keeps `%(upstream:track)` in English; git translates it, and
    // the counts are parsed back out of that text below.
    let output = safe_output(command("git").env("LC_ALL", "C").args([
        "-C",
        p,
        "for-each-ref",
        "--format",
        DETAIL_FORMAT,
        "refs/heads",
        "refs/remotes",
    ]));
    match output {
        Ok(output) if output.status.success() => {
            parse_branch_details(&String::from_utf8_lossy(&output.stdout))
        }
        Ok(output) => {
            log::warn!(
                "git for-each-ref failed for {}: {}",
                path.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
            HashMap::new()
        }
        Err(error) => {
            log::warn!("git for-each-ref failed for {}: {error}", path.display());
            HashMap::new()
        }
    }
}

/// Parse the [`DETAIL_FORMAT`] output. Short lines are skipped rather than
/// failing the whole map — a branch without metadata still lists fine.
fn parse_branch_details(stdout: &str) -> HashMap<String, BranchDetail> {
    let mut map = HashMap::new();
    for line in stdout.lines() {
        let mut fields = line.split('\t');
        let (Some(name), Some(time), Some(upstream), Some(track), Some(worktree), Some(head)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        map.insert(
            name.to_string(),
            BranchDetail {
                committed_at: time.parse().ok(),
                upstream: parse_upstream_track(upstream, track),
                // The worktree we are asking from is where a checkout lands
                // anyway; only another one blocks it.
                worktree: (head.trim() != "*" && !worktree.is_empty())
                    .then(|| worktree.to_string()),
            },
        );
    }
    map
}

/// Turn `%(upstream:short)` + `%(upstream:track)` into an [`UpstreamState`].
///
/// `track` is empty both for a branch without an upstream and for one in sync
/// with it, which is why the upstream name is read alongside it. Otherwise it
/// reads `[gone]`, `[ahead N]`, `[behind N]` or `[ahead N, behind M]`.
fn parse_upstream_track(upstream: &str, track: &str) -> UpstreamState {
    if upstream.is_empty() {
        return UpstreamState::Untracked;
    }
    let inner = track.trim().trim_start_matches('[').trim_end_matches(']');
    if inner == "gone" {
        return UpstreamState::Gone;
    }
    let mut ahead = 0;
    let mut behind = 0;
    for part in inner.split(',') {
        let mut words = part.split_whitespace();
        match (words.next(), words.next().and_then(|n| n.parse().ok())) {
            (Some("ahead"), Some(n)) => ahead = n,
            (Some("behind"), Some(n)) => behind = n,
            _ => {}
        }
    }
    UpstreamState::Tracked {
        name: upstream.to_string(),
        ahead,
        behind,
    }
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
                && local_names.contains(stripped)
            {
                continue;
            }
            remote.push(name);
        }
    }

    BranchList {
        current: head_branch_short(&repo),
        local,
        remote,
        details: collect_branch_details(path),
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
    let output = safe_output(command("git").args(["-C", p, "checkout", branch]))?;
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
    let output = safe_output(command("git").args(["-C", p, "checkout", "--track", remote_branch]))?;
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

    let output = safe_output(command("git").args(&args))?;
    require_success(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::status::get_current_branch;

    /// Sample `for-each-ref` output in [`DETAIL_FORMAT`]: current branch,
    /// a branch held by another worktree, one whose upstream is gone, and a
    /// remote-only ref.
    const SAMPLE: &str = concat!(
        "main\t1700000000\torigin/main\t[behind 3]\t/repo\t*\n",
        "feature\t1699000000\torigin/feature\t[ahead 2, behind 1]\t/repo/../wt-feature\t \n",
        "stale\t1698000000\torigin/stale\t[gone]\t\t \n",
        "local-only\t1697000000\t\t\t\t \n",
        "origin/release\t1696000000\t\t\t\t \n",
    );

    #[test]
    fn parses_tracking_counts_and_recency() {
        let details = parse_branch_details(SAMPLE);

        assert_eq!(
            details["main"],
            BranchDetail {
                committed_at: Some(1700000000),
                upstream: UpstreamState::Tracked {
                    name: "origin/main".to_string(),
                    ahead: 0,
                    behind: 3,
                },
                // HEAD marker set: this is our own worktree, not a blocker.
                worktree: None,
            }
        );
        assert_eq!(
            details["feature"].upstream,
            UpstreamState::Tracked {
                name: "origin/feature".to_string(),
                ahead: 2,
                behind: 1,
            }
        );
        assert_eq!(
            details["feature"].worktree.as_deref(),
            Some("/repo/../wt-feature")
        );
    }

    #[test]
    fn parses_missing_and_gone_upstreams() {
        let details = parse_branch_details(SAMPLE);

        assert_eq!(details["stale"].upstream, UpstreamState::Gone);
        assert_eq!(details["local-only"].upstream, UpstreamState::Untracked);
        assert_eq!(details["origin/release"].upstream, UpstreamState::Untracked);
        assert_eq!(details["origin/release"].committed_at, Some(1696000000));
    }

    #[test]
    fn in_sync_branch_is_tracked_not_untracked() {
        // Empty `%(upstream:track)` means either "no upstream" or "in sync";
        // the upstream name is what tells them apart.
        let details = parse_branch_details("main\t1700000000\torigin/main\t\t\t*\n");
        assert_eq!(
            details["main"].upstream,
            UpstreamState::Tracked {
                name: "origin/main".to_string(),
                ahead: 0,
                behind: 0,
            }
        );
    }

    #[test]
    fn short_and_empty_lines_are_skipped() {
        let details = parse_branch_details("broken-line\t1700000000\n\nmain\t1\t\t\t\t*\n");
        assert!(!details.contains_key("broken-line"));
        assert!(details.contains_key("main"));
    }

    #[test]
    fn classified_list_carries_details_from_real_git() {
        let (_tmp, repo) = init_temp_repo();
        let wt_tmp = tempfile::tempdir().expect("create worktree tempdir");
        let wt_path = wt_tmp.path().join("wt-feat");
        git_in(
            &repo,
            &[
                "worktree",
                "add",
                wt_path.to_str().expect("utf-8 path"),
                "-b",
                "feat",
            ],
        );

        let list = list_branches_classified(&repo);

        let main = &list.details["main"];
        assert!(main.committed_at.is_some_and(|t| t > 0));
        // No remote in a temp repo, so nothing tracks anything.
        assert_eq!(main.upstream, UpstreamState::Untracked);
        assert_eq!(main.worktree, None, "our own worktree must not be flagged");
        assert!(
            list.details["feat"].worktree.is_some(),
            "a branch held by another worktree must report its path"
        );
    }

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
    fn stash_changes_preserves_untracked_file_from_linked_worktree() {
        let (_tmp, repo) = init_temp_repo();
        let worktree_tmp = tempfile::tempdir().expect("create worktree tempdir");
        let worktree = worktree_tmp.path().join("feature");
        git_in(
            &repo,
            &[
                "worktree",
                "add",
                worktree.to_str().unwrap(),
                "-b",
                "feature",
            ],
        );
        std::fs::write(worktree.join("untracked.txt"), "preserve me\n").unwrap();

        stash_changes(&worktree).expect("stash untracked file");

        assert!(!worktree.join("untracked.txt").exists());
        assert!(!crate::repository::status::has_uncommitted_changes(
            &worktree
        ));
        git_in(&repo, &["worktree", "remove", worktree.to_str().unwrap()]);
        git_in(&repo, &["stash", "apply"]);
        assert_eq!(
            std::fs::read_to_string(repo.join("untracked.txt")).unwrap(),
            "preserve me\n"
        );
    }

    #[test]
    fn stash_changes_rejects_dirty_submodule_checkout() {
        let (_tmp, repo) = init_temp_repo();
        let submodule_tmp = tempfile::tempdir().expect("create submodule tempdir");
        let submodule = submodule_tmp.path();
        git_in(submodule, &["init", "-b", "main"]);
        std::fs::write(submodule.join("sub.txt"), "base\n").unwrap();
        git_in(submodule, &["add", "sub.txt"]);
        git_in(submodule, &["commit", "-m", "base"]);
        git_in(
            &repo,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                submodule.to_str().unwrap(),
                "submodule",
            ],
        );
        git_in(&repo, &["commit", "-m", "add submodule"]);
        std::fs::write(repo.join("submodule/sub.txt"), "dirty\n").unwrap();

        let error = stash_changes(&repo).expect_err("dirty submodule must block removal");

        assert!(error.to_string().contains("remains dirty after stash"));
        assert_eq!(
            std::fs::read_to_string(repo.join("submodule/sub.txt")).unwrap(),
            "dirty\n"
        );
        assert!(crate::repository::status::has_uncommitted_changes(&repo));
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
        create_and_checkout_branch(&repo, "feat/header-redesign", None).expect("create branch");
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

    #[test]
    fn get_default_branch_ignores_stale_origin_head() {
        let (_tmp, repo) = init_temp_repo();
        git_in(&repo, &["branch", "backup/pre-cleanup"]);
        git_in(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git_in(
            &repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/backup/pre-cleanup",
            ],
        );

        assert_eq!(get_default_branch(&repo).as_deref(), Some("main"));
        assert_eq!(resolve_review_base(&repo), None);
    }

    #[test]
    fn resolve_review_base_is_none_on_default_branch() {
        let (_tmp, repo) = init_temp_repo();
        // HEAD is on "main" (the default branch) — nothing to review against.
        assert_eq!(resolve_review_base(&repo), None);
    }

    #[test]
    fn resolve_review_base_falls_back_to_local_default_on_feature_branch() {
        let (_tmp, repo) = init_temp_repo();
        create_and_checkout_branch(&repo, "feat/x", None).expect("create branch");
        // No origin ref exists, so it falls back to the local default branch.
        assert_eq!(resolve_review_base(&repo).as_deref(), Some("main"));
    }

    #[test]
    fn resolve_review_base_returns_none_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(resolve_review_base(&path).is_none());
    }

    #[test]
    fn resolve_base_ref_prefers_upstream_then_origin_then_local() {
        let (_tmp, repo) = init_temp_repo();
        // Local branch only.
        assert_eq!(resolve_base_ref(&repo, "main").as_deref(), Some("main"));

        // Add an origin remote-tracking ref: now preferred over local.
        git_in(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        assert_eq!(
            resolve_base_ref(&repo, "main").as_deref(),
            Some("origin/main")
        );

        // Add an upstream remote-tracking ref: fork workflow — upstream wins.
        git_in(&repo, &["update-ref", "refs/remotes/upstream/main", "HEAD"]);
        assert_eq!(
            resolve_base_ref(&repo, "main").as_deref(),
            Some("upstream/main")
        );
    }

    #[test]
    fn resolve_base_ref_is_none_when_branch_missing() {
        let (_tmp, repo) = init_temp_repo();
        assert_eq!(resolve_base_ref(&repo, "does-not-exist"), None);
    }

    #[test]
    fn resolve_review_base_prefers_upstream_on_feature_branch() {
        let (_tmp, repo) = init_temp_repo();
        // Fork: origin (the fork) and upstream (canonical) both have main.
        git_in(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git_in(&repo, &["update-ref", "refs/remotes/upstream/main", "HEAD"]);
        create_and_checkout_branch(&repo, "feat/x", None).expect("create branch");
        // Review base is the upstream copy the PR really targets, not the fork's.
        assert_eq!(resolve_review_base(&repo).as_deref(), Some("upstream/main"));
    }
}
