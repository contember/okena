# git/ — Git Integration

Git status, diff parsing, and worktree operations for project directories.

## Files

| File | Purpose |
|------|---------|
| `mod.rs` | `GitStatus` — cached git status with 5-second TTL. Tracks branch, dirty state, ahead/behind counts. |
| `repository.rs` | Worktree operations — create, remove, list branches. Used by the worktree dialog overlay. |
| `diff.rs` | Diff parsing — `DiffLine`, `DiffHunk`, `DiffResult`, `DiffMode` (unified/side-by-side). Parses `git diff` output into structured data. |

## Key Patterns

- **Cached status**: Git status is cached with a 5s TTL to avoid expensive `git status` calls on every render.
- **Worktree workflow**: Worktrees are managed as lightweight branch checkouts alongside the main repo.
