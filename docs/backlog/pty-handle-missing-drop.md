# PtyHandle has no Drop impl (teardown depends on callers)

- **Severity:** Medium (resource safety)
- **Type:** concurrency / correctness
- **Area:** `okena-terminal`
- **Location:** `crates/okena-terminal/src/pty_manager.rs`

## Problem

`PtyHandle` has no `Drop` impl; correct teardown depends entirely on callers
invoking `kill` / `cleanup_exited` / `detach_all`. If a `PtyHandle` is ever dropped
off the happy path (a future code path removing it from the map without calling
`shutdown_handle`), the child process is NOT killed and the reader/writer threads
leak. `PtyManager::drop` calls `detach_all` (which runs `shutdown_handle`) so it's
covered today — but the safety net belongs on `PtyHandle` itself.

## Suggested fix

Add `Drop for PtyHandle` that signals `shutdown` + drops `input_tx`/`master`
(without blocking-join) as a backstop.
