# RootView god object + remote-sync logic in the view layer

- **Severity:** Medium (architecture)
- **Type:** refactor
- **Area:** `src/` (desktop app), `okena-workspace`
- **Location:** `src/views/root/mod.rs:47-94`, `367-625`, `render.rs:101`

## Problem

`RootView` is a 22-field god object holding workspace, broker, backend, terminals,
sidebar+controller, columns map, title/status bars, overlay manager, toast, drag
state, scroll/bounds refs, three optional managers, and bookkeeping fields. It mixes
layout/scroll, remote sync, git-provider building, and column lifecycle.

Two specific leaks:
- `sync_remote_projects_into_workspace` (367-625, ~260 lines) is heavy
  remote-state reconciliation (prefixing, folder/order rebuild, merge_visual_state,
  stale pruning, pending-focus diffing) sitting in the *view* layer — it's
  workspace/state-domain logic.
- `render_projects_grid` (render.rs:101) mutates state during render
  (`pending_center_scroll.take()`, `sync_project_columns` creates entities,
  `prune_pane_map`, re-queues `cx.notify()`).

## Suggested fix

- Move remote reconciliation into `okena-workspace`
  (e.g. `Workspace::apply_remote_snapshot`) so it's unit-testable without GPUI.
- Extract a `ProjectColumnManager` (columns map + sync/create, lines 670-809) and a
  `ProjectsScroll` helper.
- Drive `sync_project_columns` from the existing workspace observer, not from `render`.
