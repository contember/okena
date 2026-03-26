# okena-git — Git Integration

Git status, diff parsing, and worktree operations for project directories.

## Files

| File | Purpose |
|------|---------|
| `lib.rs` | `GitStatus` — cached git status. Tracks branch, dirty state, ahead/behind counts. Background polling. |
| `diff.rs` | Diff parsing — `DiffLine`, `DiffHunk`, `DiffResult`, `DiffMode` (unified/side-by-side). Parses `git diff` output into structured data. |
| `repository.rs` | Worktree operations — create, remove, list branches. `GitWorktreeInfo`, `BranchInfo`. |
| `branch_names.rs` | Branch name utilities and validation. |

## Key Patterns

- **Cached status**: Git status is cached in-memory and populated by background polling. `get_git_status` is non-blocking (returns cached data or None).
- **Worktree workflow**: Worktrees are managed as lightweight branch checkouts alongside the main repo.
- **Diff views**: UI for diffs lives in `crates/okena-views-git/src/diff_viewer/`.
