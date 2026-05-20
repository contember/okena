# okena-git — Git Integration

Git status, diff parsing, and worktree operations for project directories.

## Files

| File | Purpose |
|------|---------|
| `lib.rs` | `GitStatus` — cached git status. Tracks branch, dirty state, ahead/behind counts. PR/CI types (`PrInfo`, `PrState`, `CiCheck`, `CiStatus`, `CiCheckSummary`). `validate_git_ref`. Re-exports the `repository` API. |
| `diff.rs` | Diff parsing — `DiffLine`, `DiffHunk`, `DiffResult`, `DiffMode` (unified/side-by-side). Parses `git diff` output into structured data. |
| `repository/` | Repository operations, split into submodules. `mod.rs` declares them, re-exports the public API (so `okena_git::repository::*` paths are unchanged), and holds shared private helpers (`require_success`, `path_str`, `head_branch_short`, `get_worktree_branches`) plus `#[cfg(test)] test_support` (shared `init_temp_repo` / `git_in`). |
| `repository/worktree.rs` | Worktree ops — `create_worktree`, `create_worktree_with_start_point`, `remove_worktree`, `remove_worktree_fast`, `list_git_worktrees`, stale-dir cleanup. |
| `repository/branch.rs` | Branch ops — list/classify (`BranchList`), checkout/create/delete/push, `get_default_branch`, rebase, merge, stash, per-file stage/unstage/discard. |
| `repository/status.rs` | Working-tree status & diff stats — `StatusFetch`, `get_status`, `has_uncommitted_changes`, `get_current_branch`, `get_head_sha`, diff-stats, ahead/behind & unpushed counts. |
| `repository/ci.rs` | CI/PR integration — `get_pr_info`, `get_ci_checks`, and the pure, unit-tested parsers `parse_ci_checks` / `parse_branch_ci`. |
| `repository/paths.rs` | Path utilities — `get_repo_root`, `normalize_path`, `resolve_git_root_and_subdir`, `project_path_in_worktree`, `compute_target_paths`. |
| `branch_names.rs` | Branch name utilities and validation. |

## Key Patterns

- **Cached status**: Git status is cached in-memory and populated by background polling. `get_git_status` is non-blocking (returns cached data or None).
- **Worktree workflow**: Worktrees are managed as lightweight branch checkouts alongside the main repo.
- **Diff views**: UI for diffs lives in `crates/okena-views-git/src/diff_viewer/`.
