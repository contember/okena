# OverlayManager: collapse event-passthrough boilerplate

- **Severity:** Medium (maintainability)
- **Type:** refactor
- **Area:** `src/` (desktop app)
- **Location:** `src/views/overlay_manager.rs` (1548 lines), `OverlayManagerEvent` (32 variants, lines 77-172)

## Problem

Nearly every overlay follows the identical
`is_modal/close_modal/cx.new/cx.subscribe/open_modal` pattern, and ~25 of the 32
`OverlayManagerEvent` variants just re-emit a context-menu event 1:1 up to RootView
(e.g. `ContextMenuEvent::AddTerminal` → `OverlayManagerEvent::AddTerminal` →
`RootView` dispatch). Three layers of match arms per command.

Related smells in the same file:
- `hide_modal` (247-256) and `close_modal` (237-245) are byte-for-byte identical → delete `hide_modal`.
- Every toggle ends with a redundant `cx.notify()` though `open_modal`/`close_modal` already notify.

## Suggested fix

Generalize the `toggle_overlay!` macro to cover the parametric cases, and let
context menus emit a single `OverlayManagerEvent::Action(...)` carrying the
dispatcher payload instead of one variant per command.
