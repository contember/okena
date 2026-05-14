---
title: RootView to WindowView rename and per-window FocusManager
status: done
type: AFK
blocked-by: [02-window-scoped-mutation-api]
user-stories: [6, 9, 26, 28, 31]
---

## What to build

Make the per-window UI entity its own concept, separate from the singleton coordinator. Today `RootView` is implicitly "the one window" — sidebar, overlay manager, toast overlay, scroll handle, drag state, project columns map, focus handle, request broker all live there. Rename it to `WindowView` and re-shape so that each instance owns the per-window UI state for exactly one window, addressed by a `WindowId`.

Concretely:

- Rename the type and its module(s) `RootView` → `WindowView`. Update imports.
- Add `window_id: WindowId` to `WindowView`. Reads of folder filter, hidden set, widths, collapse, focus zoom go through `window_id` against the shared `Workspace` entity.
- `FocusManager` moves off the `Workspace` entity onto `WindowView` (one instance per window). The `FocusManager` struct itself is unchanged; only ownership shifts.
- `Okena` coordinator stops holding a single `root_view: Entity<RootView>`. It now holds `main_window: Entity<WindowView>` (single, always present) and an empty `extra_windows: HashMap<WindowId, Entity<WindowView>>` (populated by future slices). The PTY event loop, save observer, etc. all keep working — they just iterate windows where they previously addressed one.
- `RequestBroker`, `OverlayManager`, `ToastOverlay`, `SidebarController` instances become per-window (each `WindowView` constructs and owns its own).
- The single OS window opened in `main.rs` continues to open exactly as today, just hosting a `WindowView` for `WindowId::main`.

After this slice the app is still single-window from the user's perspective. Internally, the architecture is multi-window-shaped — adding extras in slice 05 is a matter of pushing more entries into `Okena::extra_windows`.

## Acceptance criteria

- [x] `RootView` is renamed to `WindowView` everywhere (type, files, comments).
- [x] `WindowView` carries a `window_id: WindowId` and uses it to address window-scoped state on the shared `Workspace`.
- [x] `FocusManager` is no longer a field on the `Workspace` entity. Each `WindowView` owns its own.
- [x] `Okena` holds `main_window: Entity<WindowView>` and `extra_windows: HashMap<WindowId, Entity<WindowView>>`; the latter is empty after this slice.
- [x] Per-window UI entities (`SidebarController`, `OverlayManager`, `ToastOverlay`, `RequestBroker`, scroll handles, drag state) are constructed inside `WindowView::new` rather than passed in from a singleton.
- [x] GPUI test: two `FocusManager` instances created independently — pushing/popping focus on one does not change the other's state.
- [ ] App launches; sidebar, overlays, toasts, command palette, project switcher, drag-resize, scroll, project zoom, and folder filter all behave exactly as today.
- [x] `cargo build` and `cargo test` both green.

## Notes

- This is a pure refactor slice. No new behavior, no schema changes, no UI additions. Goal: make the next slices possible.
- Be careful with `cx.observe(&workspace, …)` blocks in `Okena` and former `RootView`. Several of them assume there's "one" RootView; they need to fan out across `main_window` + `extra_windows.values()` — but during this slice the extras map is always empty, so the loops are trivial.
- The `pub fn terminals()` getter pattern `Okena` uses to share `TerminalsRegistry` should not change shape; the registry stays a single `Arc<Mutex<…>>` shared across all windows.
- `simple_root::SimpleRoot` (Linux) and `gpui_component::Root` continue to wrap a `WindowView` per OS window — same wrapper, just renamed inner type.
