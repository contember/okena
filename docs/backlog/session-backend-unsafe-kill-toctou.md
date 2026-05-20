# session_backend kill_session: undocumented unsafe + PID TOCTOU

- **Severity:** Medium (safety)
- **Type:** concurrency / safety
- **Area:** `okena-terminal`
- **Location:** `crates/okena-terminal/src/session_backend.rs:298-328` (kill at 316)

## Problem

For dtach, `kill_session` runs `lsof` to discover PIDs holding the socket, then
`unsafe { libc::kill(pid, SIGTERM) }` against a PID from a *separate* `lsof` call —
a classic TOCTOU: by the time we signal, the PID may have been recycled, so we could
signal an unrelated process. There is no fallback if the process ignores SIGTERM.
The `unsafe` block is undocumented.

## Suggested fix

Add a `// SAFETY:` note, treat the socket-holder set as best-effort, and consider a
fallback path. Audit the other `unsafe` libc calls (`getuid`/`waitpid`) for SAFETY
comments too.
