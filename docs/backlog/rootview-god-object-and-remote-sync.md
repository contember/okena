# RootView god object — remaining column/scroll extraction

- **Severity:** Medium (architecture)
- **Type:** refactor
- **Area:** `src/` (desktop app)
- **Location:** `src/views/root/mod.rs` (`sync_project_columns` ~669-809, `project_columns` map), `render.rs:100` (`render_projects_grid`)

## Done

The headline leak — `sync_remote_projects_into_workspace` (~260 lines of remote-state
reconciliation in the view layer) — has been moved into `okena-workspace`:
`okena_workspace::remote_apply::apply_remote_snapshot` is a pure, GPUI-free core
(unit-tested), with a thin `Workspace::apply_remote_snapshot` wrapper for the
focus/notify side-effects. The view function shrank from ~258 lines to ~26 (just
snapshots the connection entity and delegates).

## Remaining (deferred — behavior-sensitive)

`RootView` is still a large struct, and two coupled concerns remain in the view:

- **`ProjectColumnManager` extraction**: the `project_columns` map plus
  `sync_project_columns` (and `create_remote_column` / `create_local_column`).
  Not a purely mechanical move — the create methods need `cx.new()` and read
  `remote_manager` / `git_watcher` / `service_manager` / `backend` and call
  `build_git_provider`, so a clean extraction must thread those dependencies.
- **Drive column sync from the workspace observer, not `render`**:
  `render_projects_grid` (render.rs:100) mutates state during render
  (`pending_center_scroll.take()`, entity creation, `prune_pane_map`, re-queued
  `cx.notify()`). The center-scroll logic deliberately runs *during* a render pass
  after layout has reported overflow (`max_offset > 0`), deferring across frames —
  moving it to an observer would break that frame-deferral mechanism. Needs a
  different scroll-anchoring approach before it can leave the render path.

## Suggested fix

Extract `ProjectColumnManager` once the dependency threading is designed, and rework
the center-scroll so column sync no longer needs to run inside `render`.
