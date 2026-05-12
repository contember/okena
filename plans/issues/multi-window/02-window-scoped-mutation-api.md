---
title: Window-scoped mutation API and visibility refactor
status: done
type: AFK
blocked-by: [01-schema-foundation-and-migration]
user-stories: [2, 3, 4, 5, 6, 19]
---

## What to build

A new `okena-workspace::windows` module exposing pure operations on `WorkspaceData`, each addressing a specific window by `WindowId`. This is the deep module that future UI code calls into; it can be unit-tested without GPUI.

Operations:

- `set_folder_filter(&mut WorkspaceData, WindowId, Option<FolderId>)`
- `toggle_hidden(&mut WorkspaceData, WindowId, &ProjectId)` (insert if absent, remove if present)
- `set_project_width(&mut WorkspaceData, WindowId, &ProjectId, f32)`
- `set_folder_collapsed(&mut WorkspaceData, WindowId, &FolderId, bool)`
- `set_os_bounds(&mut WorkspaceData, WindowId, Bounds)`
- `delete_project_scrub_all_windows(&mut WorkspaceData, &ProjectId)` — removes the project ID from every window's hidden set, widths map, and any other per-window per-project storage. Called from the existing project-delete path.

A `WindowId` type that distinguishes "main" from an extra (e.g. an enum or a struct wrapping an `Option<Uuid>`). Lookup helpers: `WindowState`'s by-id getter / mutable getter on `WorkspaceData`.

The `Workspace` GPUI entity gains thin wrappers around the above so call sites can mutate per-window state without grabbing `WorkspaceData` directly. The transient `active_folder_filter` field is removed from `Workspace`; reads and writes are redirected to `main_window.folder_filter` (still single-window in this slice — only main exists).

`compute_visible_projects` already takes `&WindowState` from slice 01; this slice adds no signature change but does ensure all sidebar / project-column rendering paths source their filter state from the correct `WindowState` (currently always main).

After this slice, the app still runs single-window with no user-visible change. Internally, every read/write of folder filter, hidden state, widths, and folder collapse goes through the new window-scoped surface.

## Acceptance criteria

- [x] `okena-workspace::windows` module exists with the operations listed above. Each is a pure function on `&mut WorkspaceData`.
- [x] `WindowId` type distinguishes main from extras and is `Copy`-friendly enough for ergonomic use in callers.
- [x] `Workspace` entity exposes window-scoped mutation methods (e.g. `set_folder_filter(&mut self, WindowId, …, &mut Context<Self>)`) that delegate to the pure module and bump `data_version` correctly to trigger persistence.
- [x] `Workspace::active_folder_filter` field is removed; the entity's old `active_folder_filter()` getter and `set_folder_filter()` setter now read/write `main_window.folder_filter`.
- [x] All existing call sites that toggled visibility, collapsed folders, set widths, or set the folder filter are updated to route through the new window-scoped API targeting `WindowId::main`.
- [x] Project delete invokes `delete_project_scrub_all_windows` so no orphan entries remain.
- [x] Unit tests for each pure operation: applies to the targeted window only, leaves other windows untouched (test with both `main_window` and a manually-constructed extra in `extra_windows`).
- [x] Test: `delete_project_scrub_all_windows` removes the project ID from every window's hidden set and widths map, including extras.
- [x] `cargo build` and `cargo test` both green. App launches and existing single-window UX is byte-for-byte identical.

## Notes

- Pure-function tests on `WorkspaceData` are the primary test surface here — much faster and clearer than GPUI tests. Mirror the style of the visibility tests in `crates/okena-workspace/src/visibility.rs`.
- Folder filter today is transient (not persisted). After this slice it lives in `WindowState` and persists. That's a behavior change for slice 01's main-window: folder filter survives across launches. This is intentional and matches the PRD ("each window owns its own folder filter").
- Do NOT add any spawn / extra-window UI in this slice. Extras may exist in the data structures (added by hand in tests) but no runtime path creates one yet. That's slice 05.
- Save bumps must continue to debounce as today; window-scoped mutations should produce identical save-cadence behavior.
