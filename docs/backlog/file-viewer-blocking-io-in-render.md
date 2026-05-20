# File viewer: blocking filesystem I/O on the render thread

- **Severity:** High (perf / UI stall)
- **Type:** perf
- **Area:** `okena-files`
- **Location:** `crates/okena-files/src/file_viewer/render.rs:733`, `loading.rs:97-118`, `mod.rs:556-566`

## Problem

`render()` calls `check_active_tab_freshness()`, which (once/sec) does synchronous
`std::fs::metadata` and, on a detected mtime change, `std::fs::read_to_string` + a
full re-highlight of the whole file — all on the UI thread inside the render pass.
On a slow disk / network mount this stalls the frame.

## Suggested fix

Move freshness checks to a background task (`cx.spawn` + `background_executor`,
like `spawn_tab_load` already does) and only swap in results via `entity.update`.
