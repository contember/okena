# okena-git — Git Integration

Git status, diff parsing, and worktree operations for project directories.

## Files

| File | Purpose |
|------|---------|
| `lib.rs` | `GitStatus` — cached git status. Tracks branch, dirty state, ahead/behind counts. PR/CI types (`PrInfo`, `PrState`, `CiCheck`, `CiStatus`, `CiCheckSummary`). `validate_git_ref`. Re-exports the `repository` API. |
| `diff.rs` | Diff parsing — `DiffLine`, `DiffHunk`, `DiffResult`, `DiffMode` (unified/side-by-side). Parses `git diff` output into structured data. |
| `repository/` | Repository operations, split into submodules. `mod.rs` declares them, re-exports the public API (so `okena_git::repository::*` paths are unchanged), and holds shared private helpers (`require_success`, `path_str`, `head_branch_short`, `get_worktree_branches`, `network_command`) plus `#[cfg(test)] test_support` (shared `init_temp_repo` / `git_in`). |
| `repository/worktree.rs` | Worktree ops — `create_worktree`, `create_worktree_with_start_point`, `remove_worktree`, `remove_worktree_fast`, `list_git_worktrees`, stale-dir cleanup. Destructive ops take a freshly verified token: `VerifiedWorktree` (`verify_linked_worktree_fresh`) for tracked checkouts, `OrphanedWorktree` (`verify_orphaned_worktree` → `remove_orphaned_worktree`) for one whose metadata entry was pruned. |
| `repository/clone.rs` | Clone ops — `clone_repository` (runs on `Lane::Long`; a clone is network-bound and unbounded), `clone_dir_name` (the directory `git clone` would create, for prefilling), `validate_clone_url`. |
| `repository/upstream.rs` | A checkout against its upstream — `current_upstream` (checked-out branch + `UpstreamState`, one `for-each-ref`), `fast_forward_to_upstream` (`merge --ff-only @{upstream}`), `origin_url`. |
| `repository/init.rs` | Starting a repository — `is_repository_at_root` (a `.git` at exactly the path; probe with this before trusting upward discovery), `has_commit_identity` (`git var`), `init_repository`, `commit_paths` (commits only the given pathspecs, unstages them again on failure). |
| `repository/branch.rs` | Branch ops — list/classify (`BranchList`), checkout/create/delete/push, `get_default_branch`, rebase, merge, stash, per-file stage/unstage/discard. |
| `repository/status.rs` | Working-tree status & diff stats — `StatusFetch`, `get_status`, `uncommitted_changes`/`DirtyCheck` + the `has_uncommitted_changes` view, `get_current_branch`, `get_head_sha`, diff-stats, ahead/behind & unpushed counts. |
| `repository/diff_memo.rs` | Private memo behind `worktree_diff` / `untracked_line_count`: per-file `(added, removed)` remembered across status walks of the same path, keyed by HEAD blob id + worktree stat, with a racy-mtime guard. |
| `repository/ci.rs` | CI/PR integration — `fetch_pr_info`, `fetch_ci_checks` (the GraphQL queries behind `gh pr list --head` / `gh pr checks`, and REST `check-runs` + `status` for branches without a PR) plus the pure, unit-tested mapping (`pr_info_from_node`, `aggregate_checks`, `summarize_checks`, `branch_ci_summary`). Both return `PrFetch`/`CiFetch` so callers can tell "no PR / no checks" from a rate-limit refusal; `fetch_ci_checks` skips the request entirely while the upstream commit still matches a settled cached result. `list_pull_requests` (user action) still shells out to `gh`. |
| `repository/github.rs` | GitHub API plumbing for the poll path — base-repo resolution with `gh`'s rules (`remote.<name>.gh-resolved`, then `upstream` > `github` > `origin`), the process-wide token cache (`GH_TOKEN`/`GITHUB_TOKEN`, else one `gh auth token` spawn per 30 min), rate-limit detection, and `GithubClient` (REST with `Link` pagination + GraphQL) over `okena_transport::http`. |
| `repository/paths.rs` | Path utilities — `get_repo_root`, `normalize_path`, `resolve_git_root_and_subdir`, `project_path_in_worktree`, `compute_target_paths`. |
| `branch_names.rs` | Branch name utilities and validation. |

## Key Patterns

- **Cached status**: Git status is cached in-memory and populated by background polling. `get_git_status` is non-blocking (returns cached data or None).
- **Remote git is non-interactive**: anything touching a remote (clone, fetch, push) is built with `network_command()`, never `command("git")`. Git prompts on `/dev/tty`, so a background child would take SIGTTIN and hang forever instead of failing.
- **Worktree workflow**: Worktrees are managed as lightweight branch checkouts alongside the main repo.
- **Diff views**: UI for diffs lives in `crates/okena-views-git/src/diff_viewer/`.
- **One path base for diffs and per-file mutations**: `FileDiff.old_path`/`new_path`
  and the `file_path` arguments of `get_file_*`, `stage_file`, `unstage_file` and
  `discard_file_changes` are **relative to the git worktree root**; `repo_path`
  may be any directory inside the repository (a monorepo subdir project included).
  The `git diff` invocations pin this with `--no-relative` — `diff.relative` in a
  user's config would otherwise move the base under a subdir project, and the
  mutations would then hit a same-named file at the root. Mutations run with
  `-C <worktree root> --literal-pathspecs`, so a filename containing `*` or `?`
  matches only itself. `get_blame` and `get_file_history` predate this contract
  and take their own bases — don't feed them diff paths without checking.
- **Dirty state has three answers, not two**: `uncommitted_changes` returns
  `DirtyCheck::{Known(bool), NotRepo, Unknown}`. gix answers in-process and the
  git CLI is the fallback for shapes it cannot walk (a sparse index, an
  unreadable directory); `NotRepo` is decided by a `.git` lookup up the tree, so
  a repository we merely failed to open cannot pass itself off as an absent one.
  `has_uncommitted_changes` folds `Unknown` into `true` for callers that only
  gate. **Any caller that escalates on `true`** — deriving a `--force`, say —
  **must match on `uncommitted_changes` and refuse on `Unknown`**, or an
  unreadable status becomes a forced destructive removal.
