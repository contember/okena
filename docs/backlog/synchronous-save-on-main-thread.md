# Blocking save_settings / save_workspace on the main thread

- **Severity:** Low (perf)
- **Type:** perf
- **Area:** `okena-workspace`, `src/`
- **Location:** `crates/okena-workspace/src/settings.rs:381`; `src/views/root/handlers.rs:402`; `src/main.rs:78,733`

## Problem

Auto-save paths correctly use `smol::unblock` (settings.rs:351, app/mod.rs:206), but
several callers invoke the blocking fsync+rename save directly on the main thread.
Quit-handlers (main.rs) are acceptable; the in-UI ones can hiccup. settings.rs:381
also discards the `Result` (`let _ = save_settings(...)`).

## Suggested fix

Offload the non-shutdown save callers via `smol::unblock`, and stop discarding the
`Result` at settings.rs:381.
