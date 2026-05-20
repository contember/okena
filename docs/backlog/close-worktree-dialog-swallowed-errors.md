# close_worktree_dialog: swallowed errors on recovery paths

- **Severity:** Low (observability / data safety)
- **Type:** error-handling
- **Area:** `okena-views-git`
- **Location:** `crates/okena-views-git/src/close_worktree_dialog/execute.rs` (20+ `let _ = ...`, esp. `stash_pop` at 98/149/205/235)

## Problem

The worktree-close flow has 20+ `let _ = cx.update(...)` / `let _ = git::stash_pop(...)`
calls swallowing errors. The `stash_pop` failures are recovery paths where a
silently-failed pop leaves the user's changes stashed with no surfaced toast — the
user thinks the operation completed but their work is hidden in a stash.

## Suggested fix

At minimum `log::warn!` on the recovery-path errors; ideally surface a toast when
`stash_pop` fails so the user knows their changes are stashed.
