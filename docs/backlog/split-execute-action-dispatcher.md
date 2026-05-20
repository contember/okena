# Split execute_action (900-line match, 40+ arms)

- **Severity:** Medium (maintainability)
- **Type:** refactor
- **Area:** `src/` (desktop app)
- **Location:** `src/workspace/actions/execute.rs:48-` (~900 lines)

## Problem

`execute_action` is one ~900-line match with 40+ arms mixing terminal ops, tab ops,
and a large self-contained block of Git operations (lines 259-396+).

## Suggested fix

Split into `execute_terminal_action` / `execute_tab_action` / `execute_git_action`
sub-dispatchers; the Git arms in particular are a cohesive group that lifts out
cleanly.
