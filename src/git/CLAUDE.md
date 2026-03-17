# git/ — Git Integration

Git status, diff parsing, and worktree operations for project directories.

## Files

| File | Purpose |
|------|---------|
| `mod.rs` | `GitStatus` — cached git status populated by background watcher. Tracks branch, dirty state, ahead/behind counts. |
| `repository.rs` | Worktree operations — create, remove, list branches. Used by the worktree dialog overlay. |
| `diff.rs` | Diff parsing — `DiffLine`, `DiffHunk`, `DiffResult`, `DiffMode` (unified/side-by-side). Parses `git diff` output into structured data. |

## Key Patterns

- **Cached status**: Git status is cached in-memory and populated by the background watcher. `get_git_status` is non-blocking (returns cached data or None).
- **Worktree workflow**: Worktrees are managed as lightweight branch checkouts alongside the main repo.
