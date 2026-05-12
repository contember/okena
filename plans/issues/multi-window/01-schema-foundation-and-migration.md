---
title: Schema foundation and migration to main_window slot
status: done
type: AFK
blocked-by: []
user-stories: [23, 29, 30]
---

## What to build

Introduce the multi-window data model in the persisted Workspace shape, even though the app still runs single-window after this slice. The pure-data crate gains a `WindowState` struct. `WorkspaceData` gains `main_window: WindowState` (always present, single slot — the compile-time invariant that main exists) and `extra_windows: Vec<WindowState>` (empty for now).

Three legacy fields are removed and migrated into `main_window`:

- `ProjectData.show_in_overview` (false → project ID added to `main_window.hidden_project_ids`)
- `FolderData.collapsed` (true → `(folder_id, true)` in `main_window.folder_collapsed`)
- `WorkspaceData.project_widths` (the whole map → `main_window.project_widths`)

`compute_visible_projects` is rewritten to take `&WindowState` for hidden-set and folder-filter inputs (instead of separate args / per-project flags), and existing call sites pass the workspace's `main_window`. The visibility test suite is updated to express the same scenarios via per-window state — no behavior change for end users.

Persistence:

- Bump `WorkspaceData.version`.
- Migration code runs on load: detect old version, transform shape, save back in the new shape on next mutation.
- Bootstrap fallback: missing/corrupt windows section produces a default `main_window` and empty `extra_windows`. The schema invariant — main is always present — must hold across every load path.

After this slice, the app runs exactly like today. Existing users' `workspace.json` upgrades transparently. No new UI, no spawn action, no per-window divergence yet.

## Acceptance criteria

- [x] `WindowState` exists in `okena-state` with serde derive and the five fields from the PRD (`hidden_project_ids`, `folder_filter`, `project_widths`, `folder_collapsed`, `os_bounds`).
- [x] `WorkspaceData` exposes `main_window: WindowState` and `extra_windows: Vec<WindowState>`. Old top-level `project_widths` is gone.
- [x] `ProjectData.show_in_overview` and `FolderData.collapsed` are removed.
- [x] `compute_visible_projects` signature takes `&WindowState`. All existing tests in `visibility.rs` pass after rewrite.
- [x] Loading an old-shape `workspace.json` (with `show_in_overview=false` projects, `collapsed=true` folders, top-level `project_widths`) yields a `WorkspaceData` whose `main_window` carries those values exactly. Round-trip (load → save → load) is stable.
- [x] A fresh install / corrupt file loads with a default `main_window` and empty `extras`.
- [x] `cargo build` and `cargo test` both green.
- [ ] Manual: launch app on a populated `workspace.json` from before this slice; observe sidebar, hidden projects, collapsed folders, column widths all unchanged.

## Notes

- Prior art for the migration round-trip pattern: `ProjectData` legacy hooks tests in `crates/okena-state/src/workspace_data.rs` (`project_data_with_legacy_hooks_migrates_on_load`, `project_data_legacy_hooks_save_roundtrip_uses_grouped_format`). Mirror their style for `show_in_overview`, `collapsed`, `project_widths`.
- The `Workspace` entity field `active_folder_filter` is intentionally NOT moved in this slice — that happens in slice 02. Keep it as a transient field for now and route `compute_visible_projects` calls through `main_window.folder_filter` only after the API is in place.
- Do NOT introduce any window-scoped setters or new mutation API in this slice. That belongs to slice 02. Goal here: get the data shape right and migrate clean.
