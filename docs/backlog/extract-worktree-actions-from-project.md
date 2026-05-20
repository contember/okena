# Extract worktree lifecycle out of actions/project.rs

- **Severity:** Medium (maintainability)
- **Type:** refactor
- **Area:** `okena-workspace`
- **Location:** `crates/okena-workspace/src/actions/project.rs` (1220 lines)

## Problem

The `impl` exposes ~25 public mutators (~770 non-test lines): project CRUD,
ordering, widths/heights, *and* the entire worktree lifecycle
(`create_worktree_project`, `register_worktree_project_inner`,
`fire_worktree_hooks`, `add_discovered_worktree`, `remove_worktree_project`,
`reorder_worktree`, ...) all hang off `Workspace`.

## Suggested fix

Split the worktree concern into its own `actions/worktree.rs` module — the layout
actions are already split this way, so apply the same treatment.

Related cleanup: inline `self.data.projects.iter_mut().find(|p| p.id == ...)` is
repeated at project.rs:377,542,587,678 and state.rs:169,184,287 despite an existing
`project_mut(id)` helper (state.rs:387) — route them through it.
