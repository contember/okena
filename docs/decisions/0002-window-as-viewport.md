---
id: 0002
title: A window is a filtered viewport, not a partition of the workspace
status: accepted
date: 2026-05-12
---

# 0002 — A window is a filtered viewport, not a partition of the workspace

## Context

Okena had one viewport: a single window with one project-columns grid. Every
filter — folder filter, focus zoom, hide/show project — was global, so arranging
the grid for one task disturbed every other task. A user could not keep two
parallel stages (say "client work" beside "personal") without toggling filters
back and forth.

The obvious reading of "multiple windows" is that each window *owns* a set of
projects. That model brings bookkeeping with it: projects must be moved between
windows, a project belongs somewhere, and the user has to track which window can
do what. The alternative is that a window owns nothing and merely *filters* a
single shared workspace.

## Decision

We will treat a window as a **filtered viewport onto the shared Workspace**. The
same project may be visible in zero, one, or many windows at once.

- **Shared, one truth:** projects, folders, ordering, layouts, hooks, terminals,
  PTYs, git watchers and settings. Adding, renaming or deleting a project, or
  editing hooks, affects every window.
- **Per-window presentation only:** hidden-project set, folder filter, focus
  zoom, project sizes, folder-collapsed state, OS bounds
  (`WindowState`, `crates/okena-state/src/window_state.rs`).
- **Main is special, extras are ephemeral.** The main window always exists in
  persistence and closing it quits the app. Extra windows are forgotten on close
  but restored if open at quit.
- **No cross-window operations.** No "Move to Window N", no "Show in Window N".
  Each window is self-contained; the only per-project control is hide/show *in
  this window*.
- **A new extra window starts empty** (all current projects in its hidden set) so
  the user curates it deliberately instead of inheriting noise.
- **A project added from window X is visible only in X**, hidden elsewhere by
  default — it lands where the user was looking.
- **Windows are not named.** They are addressed positionally (main, then
  auto-numbered extras), so spawning stays one keystroke.
- **Column ordering is shared**, because there is one canonical project order
  across the app; only filtering is per-window.

Because a terminal can render in several windows at once, the content-pane
registry maps `terminal_id` to a **list** of weak handles and the PTY notify path
fans out to all of them (`crates/okena-app/src/views/window/mod.rs:36`).

## Consequences

**Easier.** Single-window users see no change until they spawn a second window —
there is no learning tax. The sidebar means the same thing everywhere. Nothing
can be "lost in another window", because closing a window never removes anything
from the Workspace.

**Harder.** Every per-window mutation needs a `WindowId` parameter, so the
workspace mutation API is wider than it would be for a global-state app. Anything
genuinely per-window must be added to `WindowState` and migrated, not stashed on
the entity. The old global fields had to be migrated into `main_window`
(`show_in_overview`, `FolderData.collapsed`, global `project_widths`).

**Now binding.** New view state belongs on `WindowState`; new domain state
belongs on `WorkspaceData`. Do not add cross-window commands to the project
context menu. `main_window.id` is opaque padding and must never be compared
across `WorkspaceData` instances — main is addressed by `WindowId::Main`.

## Alternatives considered

- **Window owns its projects (partition model).** Rejected: it forces move
  operations, a "which window does this belong to" question on every project, and
  makes deleting a window destructive. The viewport model has neither.
- **Named windows, prompted on spawn.** Rejected: it puts a dialog in front of a
  one-keystroke action for a naming that carries no behaviour.
- **Extra windows persist like main.** Rejected: closed extras would pile up in a
  hidden list the user cannot see or manage. Restoring only what was open at quit
  gives session continuity without the bookkeeping.
- **New window inherits the spawning window's visible set.** Rejected: the point
  of a second window is a different stage, so starting from the current one is
  usually the wrong default and always needs undoing.

Terminology: [`../reference/glossary.md`](../reference/glossary.md).
