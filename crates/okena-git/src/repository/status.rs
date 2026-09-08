//! Working-tree status, diff stats, HEAD/branch reads, and ahead/behind counts.

use std::path::Path;
use std::time::SystemTime;

use okena_core::process::{command, safe_output};

use super::diff_memo;
use crate::GitStatus;

/// Cheap identity of the commit currently checked out in a worktree.
///
/// Unlike [`GitStatus`], reading this does not inspect the index or walk the
/// working tree, so callers can poll it frequently and use changes as a signal
/// to refresh the full status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadSnapshot {
    reference: Option<String>,
    commit_id: Option<gix::ObjectId>,
}

impl HeadSnapshot {
    /// Whether HEAD switched between symbolic refs or attached/detached state.
    pub fn reference_changed(&self, previous: &Self) -> bool {
        self.reference != previous.reference
    }
}

/// Three-state result of a fresh git status fetch.
///
/// Distinguishing "not a repo" from "transient failure" lets the polling
/// watcher preserve the last known +/- counts instead of clobbering them
/// with `(0, 0)` whenever the gix status walk briefly fails (lock contention
/// with a concurrent `git add`, partial `.git/index` rewrite, etc).
pub enum StatusFetch {
    /// Got a fresh reading. Boxed because `GitStatus` dwarfs the other
    /// (data-less) variants — see `clippy::large_enum_variant`.
    Status(Box<GitStatus>),
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
    let unpushed = count_unpushed_commits(path);
    let review_base = super::branch::resolve_review_base(path);
    // Only needed to decide whether the base label is redundant (base == default);
    // skip the lookup when there's no base chip to render.
    let default_branch = review_base
        .as_ref()
        .and_then(|_| super::branch::get_default_branch(path));
    // Ahead/behind are measured against the review base (`origin/<default>`),
    // not the branch's configured upstream — so the count is consistent
    // regardless of how `@{u}` is set and matches the "review changes" diff.
    // `None` when there's no base (HEAD is on the default branch).
    let (ahead, behind) = match review_base.as_deref() {
        Some(base) => match count_ahead_behind_vs(path, base) {
            Some((a, b)) => (Some(a), Some(b)),
            None => (None, None),
        },
        None => (None, None),
    };

    StatusFetch::Status(Box::new(GitStatus {
        branch,
        lines_added,
        lines_removed,
        pr_info: None,
        ci_checks: None,
        ahead,
        behind,
        unpushed,
        review_base,
        default_branch,
    }))
}

/// Three-state result of a fresh dirty check — see [`uncommitted_changes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirtyCheck {
    /// Definitive reading of the working tree.
    Known(bool),
    /// There is no repository at the path or above it.
    NotRepo,
    /// A repository is there, but neither gix nor git could read its status.
    Unknown,
}

/// Whether a worktree/repo has uncommitted changes (staged, unstaged, or
/// untracked), keeping "could not tell" apart from "clean".
/// Always performs a fresh check (no caching).
///
/// gix answers the common case in-process. It gives up on shapes git handles
/// fine — a sparse index, a directory it cannot read — so on a user-action path
/// (never the poll loop) we ask git itself rather than report a status we do
/// not have.
pub fn uncommitted_changes(path: &Path) -> DirtyCheck {
    if let Some(dirty) = gix_uncommitted_changes(path) {
        return DirtyCheck::Known(dirty);
    }
    if !git_dir_exists_upwards(path) {
        return DirtyCheck::NotRepo;
    }
    cli_uncommitted_changes(path).map_or(DirtyCheck::Unknown, DirtyCheck::Known)
}

/// `None` when the gix walk could not run, or an entry errored part-way.
fn gix_uncommitted_changes(path: &Path) -> Option<bool> {
    let repo = crate::gix_helpers::open(path)?;
    let platform = repo.status(gix::progress::Discard).ok()?;
    let mut iter = crate::gix_helpers::single_threaded(platform)
        .untracked_files(gix::status::UntrackedFiles::Files)
        .into_iter(None)
        .ok()?;
    match iter.next() {
        Some(Ok(_)) => Some(true),
        Some(Err(_)) => None,
        None => Some(false),
    }
}

/// `None` when git itself refuses to report status.
fn cli_uncommitted_changes(path: &Path) -> Option<bool> {
    let p = path.to_str()?;
    let output = safe_output(command("git").args(["-C", p, "status", "--porcelain"])).ok()?;
    output
        .status
        .success()
        .then(|| !output.stdout.iter().all(u8::is_ascii_whitespace))
}

/// Whether a `.git` entry sits at `path` or any ancestor.
///
/// "No repository here" is a filesystem fact. Asking git would let a `.git`
/// we merely failed to read — an unparsable config, no permission — pass
/// itself off as an absent one, and absent means "nothing to protect".
fn git_dir_exists_upwards(path: &Path) -> bool {
    path.ancestors().any(|dir| dir.join(".git").exists())
}

/// Safety-gate view of [`uncommitted_changes`]: a status that could not be
/// established counts as dirty, so destructive flows prompt or refuse instead
/// of proceeding as if the checkout were clean.
///
/// Callers that can *escalate* on a dirty answer (deriving a force flag, say)
/// must match on [`uncommitted_changes`] instead — for them `Unknown` has to
/// refuse, and this collapses it into the same `true` a user-accepted dirty
/// close produces.
pub fn has_uncommitted_changes(path: &Path) -> bool {
    matches!(
        uncommitted_changes(path),
        DirtyCheck::Known(true) | DirtyCheck::Unknown
    )
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

/// Read the symbolic HEAD target and resolved commit without walking the worktree.
pub fn get_head_snapshot(path: &Path) -> Option<HeadSnapshot> {
    let repo = crate::gix_helpers::open(path)?;
    let head = repo.head().ok()?;
    Some(HeadSnapshot {
        reference: head.referent_name().map(ToString::to_string),
        commit_id: head.id().map(|id| id.detach()),
    })
}

/// Get the full 40-character SHA of HEAD, or `None` if not a git repo or HEAD
/// has no commits yet.
pub fn get_head_sha(path: &Path) -> Option<String> {
    let repo = crate::gix_helpers::open(path)?;
    let id = repo.head_id().ok()?;
    Some(id.to_hex().to_string())
}

/// Full SHA of the current branch's upstream tracking commit — the last commit
/// known (from the latest fetch) to be on the remote. `None` if HEAD is
/// detached or the branch has no upstream (never pushed).
///
/// Branch-level CI lookups (`/commits/{sha}/check-runs` and `/status`) must use
/// this rather than the local HEAD: GitHub runs CI against *pushed* commits, so
/// querying an unpushed local HEAD just returns nothing.
pub fn get_pushed_sha(path: &Path) -> Option<String> {
    let repo = crate::gix_helpers::open(path)?;
    let branch = super::head_branch_short(&repo)?;
    let head_ref = repo
        .find_reference(&format!("refs/heads/{}", branch))
        .ok()?;
    let head_full: gix::refs::FullName = head_ref.name().into();
    let upstream_name = repo
        .branch_remote_tracking_ref_name(head_full.as_ref(), gix::remote::Direction::Fetch)?
        .ok()?;
    let id = repo
        .rev_parse_single(upstream_name.as_bstr())
        .ok()?
        .detach();
    Some(id.to_hex().to_string())
}

/// Tracked per-file diff counts and the untracked-file list, produced by a
/// single working-tree walk. See [`worktree_diff`].
#[derive(Default)]
pub(crate) struct WorktreeDiff {
    /// Per-file added/removed line counts for every tracked path that differs
    /// from HEAD, as `(repo-relative path, added, removed)` — the structured
    /// equivalent of `git diff --numstat --no-renames HEAD`. Binary files
    /// appear with `(.., 0, 0)`, matching numstat's `-`/`-`.
    pub tracked: Vec<(String, usize, usize)>,
    /// Untracked file paths, scoped to the queried path's subtree but relative
    /// to the worktree root — the same base as `tracked`.
    pub untracked: Vec<String>,
    /// Worktree root both path lists are relative to.
    pub root: std::path::PathBuf,
}

/// Tracked diff counts **and** the untracked-file list from one HEAD → index →
/// worktree walk. gix's status walk already surfaces untracked entries, so
/// collecting them in the same pass avoids a second full worktree traversal —
/// previously each project paid for two walks per poll (this plus a separate
/// untracked listing), and the 5s poller fans this out across ~every project.
///
/// Computed entirely in-process via gix — no subprocess spawn. This replaced
/// the `git diff --numstat HEAD` spawn that was the last one in the 5s
/// status-poll hot path; under many projects that fan-out tripped the macOS
/// `RLIMIT_NOFILE` default and blanked status badges (#125).
///
/// Returns `None` on a transient failure (couldn't open the repo, init the
/// status walk, or an iteration step errored) so the polling watcher can keep
/// the last known counts — see `StatusFetch::Transient`.
///
/// Per-file counts are memoized across walks of the same `path` (see
/// [`diff_memo`]): a changed file whose HEAD blob id and worktree stat match
/// the previous walk reuses its counts instead of re-reading and re-diffing.
pub(crate) fn worktree_diff(path: &Path) -> Option<WorktreeDiff> {
    let repo = crate::gix_helpers::open(path)?;
    let workdir = repo.workdir()?.to_path_buf();

    // HEAD tree to diff the worktree against. An unborn HEAD (no commits yet)
    // leaves this `None`, so every tracked blob diffs against an empty source.
    let head_tree = repo.head_tree().ok();

    // Prefix from workdir down to the queried path, scoping untracked entries
    // to the (possibly monorepo-subdir) project. Both lists stay repo-relative.
    let canonical_query = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let canonical_workdir = workdir.canonicalize().unwrap_or_else(|_| workdir.clone());
    let untracked_prefix: String = canonical_query
        .strip_prefix(&canonical_workdir)
        .ok()
        .map(|p| {
            let s = p.to_string_lossy().to_string();
            if s.is_empty() {
                String::new()
            } else {
                format!("{}/", s)
            }
        })
        .unwrap_or_default();

    // One parallel HEAD → index → worktree walk. Rename tracking is disabled to
    // match `--no-renames`: a rename surfaces as a delete of the old path plus
    // an add of the new one. Capped to one thread (see `single_threaded`).
    let iter = crate::gix_helpers::single_threaded(repo.status(gix::progress::Discard).ok()?)
        .tree_index_track_renames(gix::status::tree_index::TrackRenames::Disabled)
        .untracked_files(gix::status::UntrackedFiles::Files)
        .into_iter(None)
        .ok()?;

    // Unique set of tracked paths that differ from HEAD. A path can surface in
    // both the tree→index and index→worktree phases (staged *and* further
    // edited); dedup so we count it once. We recompute each path's counts from
    // HEAD-blob vs worktree-file directly, so the staging split doesn't matter.
    // Untracked files surface as `DirectoryContents` entries — collect those in
    // the same pass instead of walking the worktree a second time.
    let mut changed: std::collections::HashSet<gix::bstr::BString> =
        std::collections::HashSet::new();
    let mut untracked: Vec<String> = Vec::new();
    for item in iter {
        let item = item.ok()?;
        if let gix::status::Item::IndexWorktree(
            gix::status::index_worktree::Item::DirectoryContents { entry, .. },
        ) = &item
        {
            if matches!(entry.status, gix::dir::entry::Status::Untracked) {
                let rela = entry.rela_path.to_string();
                if untracked_prefix.is_empty() || rela.starts_with(&untracked_prefix) {
                    untracked.push(rela);
                }
            }
            continue;
        }
        changed.insert(item.location().to_owned());
    }

    // The walk is complete, so nothing below can fail: the memo is only touched
    // by walks that finished, and a transient failure above leaves it as it was.
    let now = SystemTime::now();
    let mut memo = diff_memo::take(path);
    memo.retain_walk(&changed, &untracked);

    let mut tracked = Vec::with_capacity(changed.len());
    for rela in &changed {
        let rela_bstr = gix::bstr::BStr::new(rela);
        let rela_path = gix::path::from_bstr(rela_bstr);
        let name = String::from_utf8_lossy(rela_bstr).into_owned();
        let full_path = workdir.join(&rela_path);
        let head_entry = head_blob_entry(head_tree.as_ref(), rela_bstr);
        let observed = diff_memo::observe(&full_path, now);
        let inputs = observed.map(|observed| diff_memo::TrackedInputs {
            head: head_entry.as_ref().map(|entry| entry.object_id()),
            worktree: observed.input,
        });
        if let Some(inputs) = &inputs
            && let Some((added, removed)) = memo.tracked_counts(rela_bstr, inputs)
        {
            tracked.push((name, added, removed));
            continue;
        }

        let head_blob = head_blob_bytes(head_entry.as_ref());
        let worktree_blob = std::fs::read(&full_path);
        let (added, removed) = tracked_line_counts(
            head_blob.as_deref().unwrap_or_default(),
            worktree_blob.as_deref().unwrap_or_default(),
        );
        memo.note_computed_tracked();
        let settled = observed.is_some_and(|observed| {
            observed.trusted && diff_memo::read_settled(&observed.input, &worktree_blob)
        });
        if let Some(inputs) = inputs
            && settled
            && head_blob.is_some()
        {
            memo.remember_tracked(rela.clone(), inputs, (added, removed));
        }
        tracked.push((name, added, removed));
    }
    diff_memo::store(path, memo);

    Some(WorktreeDiff {
        tracked,
        untracked,
        root: workdir,
    })
}

/// Line count of an untracked file listed in [`WorktreeDiff::untracked`]
/// (relative to `root`), each line counting as an addition. Unreadable or
/// non-UTF-8 files count as zero. Memoized like tracked files, keyed by the
/// queried `path` that produced the listing.
pub(crate) fn untracked_line_count(path: &Path, root: &Path, file: &str) -> usize {
    let full_path = root.join(file);
    let observed = diff_memo::observe(&full_path, SystemTime::now());
    if let Some(observed) = &observed
        && let Some(lines) =
            diff_memo::with(path, |memo| memo.untracked_lines(file, &observed.input)).flatten()
    {
        return lines;
    }

    let read = std::fs::read_to_string(&full_path);
    let lines = read
        .as_ref()
        .map(|content| content.lines().count())
        .unwrap_or(0);
    diff_memo::with(path, |memo| {
        memo.note_computed_untracked();
        if let Some(observed) = observed
            && observed.trusted
            && diff_memo::read_settled(&observed.input, &read)
        {
            memo.remember_untracked(file.to_owned(), observed.input, lines);
        }
    });
    lines
}

/// Get diff statistics (total lines added, lines removed) for the working
/// directory: tracked changes vs HEAD plus untracked-file line counts.
///
/// Returns `None` on a transient failure so the polling watcher keeps the last
/// known +/- instead of blanking the badge — see `StatusFetch::Transient`.
fn get_diff_stats(path: &Path) -> Option<(usize, usize)> {
    // Single walk yields both tracked counts and untracked files. A None means
    // the gix status walk failed transiently — propagate so we don't undercount.
    let diff = worktree_diff(path)?;

    let (mut added, mut removed) = (0usize, 0usize);
    for (_path, a, r) in &diff.tracked {
        added += a;
        removed += r;
    }

    // Untracked files: count each line as an addition.
    for file in &diff.untracked {
        added += untracked_line_count(path, &diff.root, file);
    }

    Some((added, removed))
}

/// The regular-blob entry at `rela_path` in the HEAD tree. `None` when HEAD
/// is unborn, the path isn't in HEAD (freshly added), or it isn't a regular
/// blob (submodule/gitlink) — all of which diff as "no prior content".
fn head_blob_entry<'repo>(
    head_tree: Option<&gix::Tree<'repo>>,
    rela_path: &gix::bstr::BStr,
) -> Option<gix::object::tree::Entry<'repo>> {
    let path = gix::path::from_bstr(rela_path);
    match head_tree?.lookup_entry_by_path(path.as_ref()) {
        Ok(Some(entry)) if entry.mode().is_blob() => Some(entry),
        _ => None,
    }
}

/// Bytes of a HEAD blob entry, or empty when there is none. `None` only when
/// the object store failed to serve an existing entry, so the result must not
/// be remembered — the next walk may succeed.
fn head_blob_bytes(entry: Option<&gix::object::tree::Entry<'_>>) -> Option<Vec<u8>> {
    match entry {
        None => Some(Vec::new()),
        Some(entry) => entry.object().ok().map(|o| o.data.clone()),
    }
}

/// Numstat-style counts for one tracked path. Binary files report `-`/`-`
/// (i.e. 0/0) in numstat; record them with zero counts rather than diffing.
fn tracked_line_counts(head: &[u8], worktree: &[u8]) -> (usize, usize) {
    if is_binary(head) || is_binary(worktree) {
        return (0, 0);
    }
    diff_line_counts(head, worktree)
}

/// Count added/removed lines between two blob versions using imara-diff (pulled
/// in via gix's `blame` feature), with Git's slider heuristics so hunk
/// placement — and therefore the counts — match `git diff --numstat`.
fn diff_line_counts(before: &[u8], after: &[u8]) -> (usize, usize) {
    use gix::diff::blob::{
        Algorithm, InternedInput, diff_with_slider_heuristics, sources::byte_lines,
    };

    let input = InternedInput::new(byte_lines(before), byte_lines(after));
    let diff = diff_with_slider_heuristics(Algorithm::Histogram, &input);
    (
        diff.count_additions() as usize,
        diff.count_removals() as usize,
    )
}

/// Git treats a blob as binary if a NUL byte appears near the start; such files
/// report `-` in numstat and contribute nothing to the +/- totals.
fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|&b| b == 0)
}

/// Count commits the local branch is ahead of / behind its upstream.
/// Returns `None` if HEAD is detached or no upstream is configured.
///
/// Fully in-process via gix — no `git rev-list` subprocess. Mirrors
/// `git rev-list --left-right --count <upstream>...HEAD` by counting each side
/// of the symmetric difference with two hidden-tip rev-walks (see also
/// [`count_unpushed_commits`]).
pub fn count_ahead_behind(path: &Path) -> Option<(usize, usize)> {
    let repo = crate::gix_helpers::open(path)?;
    let branch = super::head_branch_short(&repo)?;

    // Resolve the upstream tracking ref via gix; `None` (skip) for branches
    // without one — the common local-only branch case.
    let head_ref = repo
        .find_reference(&format!("refs/heads/{}", branch))
        .ok()?;
    let head_full: gix::refs::FullName = head_ref.name().into();
    let upstream_name = repo
        .branch_remote_tracking_ref_name(head_full.as_ref(), gix::remote::Direction::Fetch)?
        .ok()?;

    // Resolve both tips to commit ids.
    let upstream_id = repo
        .rev_parse_single(upstream_name.as_bstr())
        .ok()?
        .detach();
    let head_id = repo.head_id().ok()?.detach();

    // ahead = commits reachable from HEAD but not upstream; behind = the reverse.
    let ahead = repo
        .rev_walk([head_id])
        .with_hidden([upstream_id])
        .all()
        .ok()?
        .filter_map(Result::ok)
        .count();
    let behind = repo
        .rev_walk([upstream_id])
        .with_hidden([head_id])
        .all()
        .ok()?
        .filter_map(Result::ok)
        .count();

    Some((ahead, behind))
}

/// Count commits HEAD is ahead of / behind an arbitrary base ref (e.g.
/// `origin/main`). Returns `(ahead, behind)` where `ahead` is commits on HEAD
/// not on `base` and `behind` is the reverse — the two sides of the three-dot
/// `base...HEAD` symmetric difference. `None` if either tip can't be resolved.
///
/// Unlike [`count_ahead_behind`] (which uses the branch's configured upstream
/// `@{u}`), this measures against a fixed base, so the numbers are stable
/// regardless of how tracking is set up and match the "review changes" diff.
pub fn count_ahead_behind_vs(path: &Path, base_ref: &str) -> Option<(usize, usize)> {
    let repo = crate::gix_helpers::open(path)?;

    // Count one side of a range spec (e.g. `base..HEAD`) by walking `to` with
    // `from` hidden — mirrors [`count_unpushed_commits`].
    let count_range = |range: &str| -> Option<usize> {
        let spec = repo.rev_parse(range).ok()?;
        let gix::revision::plumbing::Spec::Range { from, to } = spec.detach() else {
            return None;
        };
        let walk = repo.rev_walk([to]).with_hidden([from]).all().ok()?;
        Some(walk.filter_map(Result::ok).count())
    };

    let ahead = count_range(&format!("{base_ref}..HEAD"))?;
    let behind = count_range(&format!("HEAD..{base_ref}"))?;
    Some((ahead, behind))
}

/// Re-point a status's ahead/behind (and `review_base`) at the branch's open-PR
/// base when it differs from the repo default. Without this, the header always
/// measures against the default branch (e.g. `main`) even when the PR actually
/// targets `develop` or a stacked branch — so the counts, the base label, and
/// the click-to-review diff would all be wrong.
///
/// `pr_base` is the PR's base branch *name* (from `gh`, e.g. `develop`); it is
/// resolved to a concrete ref via [`resolve_base_ref`] (preferring
/// `upstream`/`origin`). `default_branch` is left untouched so the label chip
/// surfaces the non-default base instead of staying hidden. No-op when the PR
/// targets the ref we already compare against, or when nothing resolves.
pub fn apply_pr_base(status: &mut GitStatus, path: &Path, pr_base: &str) {
    let Some(base_ref) = super::branch::resolve_base_ref(path, pr_base) else {
        return;
    };
    if status.review_base.as_deref() == Some(base_ref.as_str()) {
        return;
    }
    if let Some((ahead, behind)) = count_ahead_behind_vs(path, &base_ref) {
        status.ahead = Some(ahead);
        status.behind = Some(behind);
        status.review_base = Some(base_ref);
    }
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
            other => panic!(
                "expected NotRepo for non-git path, got {:?}",
                match other {
                    StatusFetch::Status(_) => "Status",
                    StatusFetch::NotRepo => "NotRepo",
                    StatusFetch::Transient => "Transient",
                }
            ),
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
            other => panic!(
                "expected Status with 3 untracked lines, got {}",
                match other {
                    StatusFetch::Status(_) => "Status",
                    StatusFetch::NotRepo => "NotRepo",
                    StatusFetch::Transient => "Transient",
                }
            ),
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
        assert_eq!(uncommitted_changes(&repo), DirtyCheck::Known(false));
    }

    #[test]
    fn an_unreadable_status_is_unknown_rather_than_clean() {
        let (_tmp, repo) = init_temp_repo();
        std::fs::write(repo.join("file.txt"), "dirty").unwrap();
        // A directory where the index file belongs: the walk cannot start, so
        // the answer is "we do not know", not "clean".
        std::fs::remove_file(repo.join(".git").join("index")).unwrap();
        std::fs::create_dir(repo.join(".git").join("index")).unwrap();

        assert_eq!(uncommitted_changes(&repo), DirtyCheck::Unknown);
        assert!(
            has_uncommitted_changes(&repo),
            "a status we cannot read must gate destructive flows, not wave them through"
        );
    }

    #[test]
    fn a_path_outside_any_repository_is_not_dirty() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert_eq!(uncommitted_changes(&path), DirtyCheck::NotRepo);
    }

    #[test]
    fn a_repository_we_cannot_open_is_unknown_not_absent() {
        let (_tmp, repo) = init_temp_repo();
        std::fs::write(repo.join("file.txt"), "dirty").unwrap();
        // A `.git` that is plainly there but unusable. Reporting "no
        // repository" would read as "nothing to protect" and wave a
        // destructive removal through.
        std::fs::remove_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir(repo.join(".git")).unwrap();

        assert_eq!(gix_uncommitted_changes(&repo), None);
        assert_eq!(uncommitted_changes(&repo), DirtyCheck::Unknown);
        assert!(has_uncommitted_changes(&repo));
    }

    #[test]
    fn a_sparse_index_checkout_still_reports_its_dirty_state() {
        let (_tmp, repo) = init_temp_repo();
        git_in(
            &repo,
            &["sparse-checkout", "init", "--cone", "--sparse-index"],
        );

        // gix cannot walk a sparse index at all, so every answer here comes
        // from the git fallback.
        assert_eq!(gix_uncommitted_changes(&repo), None);
        assert_eq!(uncommitted_changes(&repo), DirtyCheck::Known(false));
        std::fs::write(repo.join("file.txt"), "dirty").unwrap();
        assert_eq!(uncommitted_changes(&repo), DirtyCheck::Known(true));
    }

    #[test]
    fn untracked_listing_from_a_subdirectory_uses_the_worktree_root_base() {
        let (_tmp, repo) = init_temp_repo();
        let project = repo.join("packages").join("app");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("fresh.txt"), "x").unwrap();
        std::fs::write(repo.join("outside.txt"), "y").unwrap();

        let untracked = crate::gix_helpers::list_untracked_files(&project)
            .expect("gix status should succeed on a clean test repo");
        assert_eq!(untracked, vec!["packages/app/fresh.txt".to_string()]);
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
        git_in(
            &repo,
            &["remote", "add", "origin", remote_path.to_str().unwrap()],
        );
        git_in(&repo, &["push", "-u", "origin", "main"]);

        // No unpushed commits yet.
        assert_eq!(count_unpushed_commits(&repo), Some(0));

        // Add two new commits locally.
        for i in 0..2 {
            std::fs::write(repo.join(format!("new{}.txt", i)), "x").unwrap();
            git_in(&repo, &["add", "."]);
            git_in(
                &repo,
                &[
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-m",
                    &format!("c{}", i),
                ],
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
    fn count_ahead_behind_vs_counts_against_given_base() {
        let (_tmp, repo) = init_temp_repo();
        // Branch off main and add two commits.
        git_in(&repo, &["checkout", "-b", "feature"]);
        for i in 0..2 {
            std::fs::write(repo.join(format!("f{}.txt", i)), "x").unwrap();
            git_in(&repo, &["add", "."]);
            git_in(
                &repo,
                &[
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-m",
                    &format!("f{}", i),
                ],
            );
        }
        // Two commits ahead of main, none behind — independent of any upstream.
        assert_eq!(count_ahead_behind_vs(&repo, "main"), Some((2, 0)));
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
        assert!(
            branch.chars().all(|c| c.is_ascii_hexdigit()),
            "expected hex hash, got {:?}",
            branch
        );
    }

    #[test]
    fn head_snapshot_changes_only_when_head_moves() {
        let (_tmp, repo) = init_temp_repo();
        let initial = get_head_snapshot(&repo).expect("read initial HEAD");

        std::fs::write(repo.join("file.txt"), "modified").unwrap();
        assert_eq!(get_head_snapshot(&repo).as_ref(), Some(&initial));

        git_in(&repo, &["add", "file.txt"]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "change"],
        );
        let committed = get_head_snapshot(&repo).expect("read committed HEAD");
        assert_ne!(committed, initial);
        assert!(!committed.reference_changed(&initial));

        git_in(&repo, &["checkout", "-b", "feature"]);
        let switched = get_head_snapshot(&repo).expect("read switched HEAD");
        assert_ne!(switched, committed);
        assert!(switched.reference_changed(&committed));
    }

    #[test]
    fn head_snapshot_is_specific_to_linked_worktree() {
        let (_tmp, repo) = init_temp_repo();
        let linked_parent = tempfile::tempdir().expect("create linked worktree parent");
        let linked = linked_parent.path().join("feature");
        git_in(
            &repo,
            &["worktree", "add", "-b", "feature", linked.to_str().unwrap()],
        );

        let main = get_head_snapshot(&repo).expect("read main worktree HEAD");
        let feature = get_head_snapshot(&linked).expect("read linked worktree HEAD");
        assert_ne!(feature, main);

        std::fs::write(linked.join("file.txt"), "linked change").unwrap();
        git_in(&linked, &["add", "file.txt"]);
        git_in(
            &linked,
            &["-c", "commit.gpgsign=false", "commit", "-m", "linked"],
        );
        assert_ne!(get_head_snapshot(&linked).as_ref(), Some(&feature));
        assert_eq!(get_head_snapshot(&repo).as_ref(), Some(&main));
    }

    #[test]
    fn diff_stats_clean_repo_is_zero() {
        let (_tmp, repo) = init_temp_repo();
        assert_eq!(get_diff_stats(&repo), Some((0, 0)));
    }

    #[test]
    fn diff_stats_counts_modified_tracked_file() {
        let (_tmp, repo) = init_temp_repo();
        // init_temp_repo commits file.txt == "x" (one line, no newline).
        std::fs::write(repo.join("file.txt"), "a\nb\nc\n").unwrap();
        // Full replacement of the single old line by three new ones.
        assert_eq!(get_diff_stats(&repo), Some((3, 1)));
    }

    #[test]
    fn diff_stats_dedups_staged_and_reworked_file() {
        let (_tmp, repo) = init_temp_repo();
        // Stage one version, then edit the worktree again. The path shows up in
        // both the tree→index and index→worktree phases; it must be counted
        // once, computed from HEAD ("x") vs the live worktree.
        std::fs::write(repo.join("file.txt"), "a\nb\n").unwrap();
        git_in(&repo, &["add", "file.txt"]);
        std::fs::write(repo.join("file.txt"), "a\nb\nc\n").unwrap();
        assert_eq!(get_diff_stats(&repo), Some((3, 1)));
    }

    #[test]
    fn diff_stats_counts_deleted_file_as_removals() {
        let (_tmp, repo) = init_temp_repo();
        std::fs::remove_file(repo.join("file.txt")).unwrap();
        // The one committed line is gone, nothing added.
        assert_eq!(get_diff_stats(&repo), Some((0, 1)));
    }

    #[test]
    fn diff_stats_counts_staged_new_file_once() {
        let (_tmp, repo) = init_temp_repo();
        // A staged-but-uncommitted new file is tracked (in the index), so it
        // must be counted via the tree→index walk, not double-counted as
        // untracked.
        std::fs::write(repo.join("added.txt"), "l1\nl2\n").unwrap();
        git_in(&repo, &["add", "added.txt"]);
        assert_eq!(get_diff_stats(&repo), Some((2, 0)));
    }

    #[test]
    fn diff_stats_treats_rename_as_delete_plus_add() {
        let (_tmp, repo) = init_temp_repo();
        std::fs::write(repo.join("orig.txt"), "l1\nl2\nl3\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "add orig"],
        );
        git_in(&repo, &["mv", "orig.txt", "renamed.txt"]);
        // --no-renames semantics: 3 lines removed from orig + 3 added to renamed.
        assert_eq!(get_diff_stats(&repo), Some((3, 3)));
    }

    #[test]
    fn diff_stats_matches_git_cli_numstat() {
        let (_tmp, repo) = init_temp_repo();
        // A mixed bag: modify a tracked file, add+commit then edit another,
        // delete one, stage a new file, leave one untracked.
        std::fs::write(repo.join("file.txt"), "alpha\nbeta\ngamma\n").unwrap();
        std::fs::write(repo.join("keep.txt"), "1\n2\n3\n4\n5\n").unwrap();
        std::fs::write(repo.join("doomed.txt"), "x\ny\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "seed"],
        );
        std::fs::write(repo.join("keep.txt"), "1\n2\nTWO-AND-A-HALF\n3\n4\n5\n6\n").unwrap();
        std::fs::remove_file(repo.join("doomed.txt")).unwrap();
        std::fs::write(repo.join("staged-new.txt"), "p\nq\n").unwrap();
        git_in(&repo, &["add", "staged-new.txt"]);
        std::fs::write(repo.join("untracked.txt"), "u1\nu2\nu3\n").unwrap();

        // CLI baseline: tracked numstat (HEAD) + untracked line counts.
        let out = std::process::Command::new("git")
            .args([
                "-C",
                repo.to_str().unwrap(),
                "diff",
                "--numstat",
                "--no-renames",
                "--no-color",
                "--no-ext-diff",
                "HEAD",
            ])
            .output()
            .unwrap();
        let (mut cli_add, mut cli_rem) = (0usize, 0usize);
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut p = line.split('\t');
            if let (Some(a), Some(r)) = (p.next(), p.next()) {
                cli_add += a.parse::<usize>().unwrap_or(0);
                cli_rem += r.parse::<usize>().unwrap_or(0);
            }
        }
        let untracked = std::process::Command::new("git")
            .args([
                "-C",
                repo.to_str().unwrap(),
                "ls-files",
                "--others",
                "--exclude-standard",
            ])
            .output()
            .unwrap();
        for f in String::from_utf8_lossy(&untracked.stdout).lines() {
            cli_add += std::fs::read_to_string(repo.join(f))
                .unwrap()
                .lines()
                .count();
        }

        assert_eq!(get_diff_stats(&repo), Some((cli_add, cli_rem)));
    }

    #[test]
    fn diff_stats_skips_binary_files() {
        let (_tmp, repo) = init_temp_repo();
        // A NUL byte marks the worktree blob binary — numstat reports `-`, so it
        // contributes nothing to the +/- totals.
        std::fs::write(repo.join("file.txt"), [0u8, 1, 2, 0, 5]).unwrap();
        assert_eq!(get_diff_stats(&repo), Some((0, 0)));
    }

    /// Push a file's mtime `secs` into the past so the memo may trust it.
    fn age_file(path: &Path, secs: u64) {
        let past = SystemTime::now() - std::time::Duration::from_secs(secs);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(past)
            .unwrap();
    }

    #[test]
    fn diff_memo_reuses_counts_when_nothing_changed() {
        let (_tmp, repo) = init_temp_repo();
        std::fs::write(repo.join("file.txt"), "a\nb\nc\n").unwrap();
        std::fs::write(repo.join("new.txt"), "u1\nu2\nu3\n").unwrap();
        age_file(&repo.join("file.txt"), 10);
        age_file(&repo.join("new.txt"), 10);

        assert_eq!(get_diff_stats(&repo), Some((6, 1)));
        let after_first = diff_memo::computed(&repo);
        assert_eq!((after_first.tracked, after_first.untracked), (1, 1));

        assert_eq!(get_diff_stats(&repo), Some((6, 1)));
        assert_eq!(diff_memo::computed(&repo), after_first);
    }

    #[test]
    fn diff_memo_invalidates_only_the_edited_file() {
        let (_tmp, repo) = init_temp_repo();
        std::fs::write(repo.join("other.txt"), "1\n2\n").unwrap();
        git_in(&repo, &["add", "other.txt"]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "other"],
        );
        std::fs::write(repo.join("file.txt"), "a\nb\nc\n").unwrap();
        std::fs::write(repo.join("other.txt"), "1\n2\n3\n").unwrap();
        age_file(&repo.join("file.txt"), 10);
        age_file(&repo.join("other.txt"), 10);

        assert_eq!(get_diff_stats(&repo), Some((4, 1)));
        assert_eq!(diff_memo::computed(&repo).tracked, 2);

        std::fs::write(repo.join("other.txt"), "1\n2\n3\n4\n5\n").unwrap();
        age_file(&repo.join("other.txt"), 8);
        assert_eq!(get_diff_stats(&repo), Some((6, 1)));
        assert_eq!(diff_memo::computed(&repo).tracked, 3);
    }

    #[test]
    fn diff_memo_invalidates_when_head_blob_changes() {
        let (_tmp, repo) = init_temp_repo();
        // Stage one version, then leave a different one in the worktree.
        std::fs::write(repo.join("file.txt"), "a\nb\n").unwrap();
        git_in(&repo, &["add", "file.txt"]);
        std::fs::write(repo.join("file.txt"), "a\nb\nc\n").unwrap();
        age_file(&repo.join("file.txt"), 10);

        assert_eq!(get_diff_stats(&repo), Some((3, 1)));
        assert_eq!(diff_memo::computed(&repo).tracked, 1);

        // Committing the staged version moves HEAD's blob without touching the
        // worktree file: same stat, different HEAD side.
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "staged"],
        );
        assert_eq!(get_diff_stats(&repo), Some((1, 0)));
        assert_eq!(diff_memo::computed(&repo).tracked, 2);
    }

    #[test]
    fn diff_memo_recounts_untracked_file_rewritten_with_same_size() {
        let (_tmp, repo) = init_temp_repo();
        std::fs::write(repo.join("new.txt"), "aaa\n").unwrap();
        age_file(&repo.join("new.txt"), 10);
        assert_eq!(get_diff_stats(&repo), Some((1, 0)));
        assert_eq!(diff_memo::computed(&repo).untracked, 1);

        std::fs::write(repo.join("new.txt"), "a\na\n").unwrap();
        age_file(&repo.join("new.txt"), 8);
        assert_eq!(get_diff_stats(&repo), Some((2, 0)));
        assert_eq!(diff_memo::computed(&repo).untracked, 2);
    }

    #[test]
    fn diff_memo_recomputes_files_inside_the_racy_window() {
        let (_tmp, repo) = init_temp_repo();
        std::fs::write(repo.join("file.txt"), "a\nb\nc\n").unwrap();
        std::fs::write(repo.join("new.txt"), "u1\n").unwrap();

        assert_eq!(get_diff_stats(&repo), Some((4, 1)));
        assert_eq!(get_diff_stats(&repo), Some((4, 1)));
        let computed = diff_memo::computed(&repo);
        assert_eq!((computed.tracked, computed.untracked), (2, 2));
    }

    #[test]
    fn apply_pr_base_repoints_ahead_behind_at_pr_target() {
        let (_tmp, repo) = init_temp_repo();
        let commit = |name: &str| {
            std::fs::write(repo.join(name), "x").unwrap();
            git_in(&repo, &["add", "."]);
            git_in(&repo, &["-c", "commit.gpgsign=false", "commit", "-m", name]);
        };
        // main -> develop (+1 commit) -> feature (+2 commits), HEAD on feature.
        git_in(&repo, &["checkout", "-b", "develop"]);
        commit("d0.txt");
        git_in(&repo, &["checkout", "-b", "feature"]);
        commit("f0.txt");
        commit("f1.txt");

        let StatusFetch::Status(mut s) = get_status(&repo) else {
            panic!("expected a fresh status");
        };
        // Default base is main: feature carries develop's commit plus its own two.
        assert_eq!(s.review_base.as_deref(), Some("main"));
        assert_eq!((s.ahead, s.behind), (Some(3), Some(0)));

        // The PR actually targets develop: only the two feature commits are ahead.
        apply_pr_base(&mut s, &repo, "develop");
        assert_eq!(s.review_base.as_deref(), Some("develop"));
        assert_eq!((s.ahead, s.behind), (Some(2), Some(0)));

        // No-op when the PR targets the base we already compare against.
        apply_pr_base(&mut s, &repo, "develop");
        assert_eq!((s.ahead, s.behind), (Some(2), Some(0)));
    }
}
