# Updater orchestration embedded inside render()

- **Severity:** High (architecture)
- **Type:** refactor
- **Area:** `src/` (desktop app), `okena-ext-updater`
- **Location:** `src/views/root/render.rs:662-783`

## Problem

Two large inline `cx.spawn` async blocks (update-check at 662-752, install at
754-783, ~120 lines) are embedded directly in `render()`. This is business logic
(download polling, status transitions, error handling) living in an action handler
inside `render()`.

## Suggested fix

Move the updater orchestration into `okena-ext-updater`
(e.g. `UpdateInfo::run_check(cx)` / `run_install(cx)`); the view should just
dispatch and observe `GlobalUpdateInfo`.
