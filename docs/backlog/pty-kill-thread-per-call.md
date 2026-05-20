# PTY kill() spawns an unbounded detached thread per call

- **Severity:** Medium (resource scaling)
- **Type:** perf / concurrency
- **Area:** `okena-terminal`
- **Location:** `crates/okena-terminal/src/pty_manager.rs:548-586`, `933`; callers `src/app/mod.rs:519,612-620`

## Problem

Each `kill()` (and `cleanup_exited()`) spawns a fresh OS thread to join the
reader/writer + run subprocess teardown. The app exit path loops over all
`exit_events` calling `kill()`, and the service manager kills terminals in loops
too. On bulk shutdown / many simultaneous exits this spawns N teardown threads at
once, each potentially blocking on `lsof` / `tmux kill-session` / `waitpid`.

Related: `kill()` and `cleanup_exited()` can both fire for the same terminal (EOF
Exit event then explicit kill), racing two teardown threads against the same dtach
socket. Safe-by-luck today via the `HashMap` removal, but under-documented.

## Suggested fix

Use a small shared teardown worker / bounded queue instead of one detached thread
per terminal. Add an explicit "handle gone → only session kill" path to make the
double-fire intent legible.
