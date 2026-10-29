---
id: 0008
title: Let a project be moved to another window
status: accepted
date: 2026-10-29
---

# 0008 — Let a project be moved to another window

## Context

ADR-0002 made a window a filtered viewport and ruled out cross-window
operations ("No 'Move to Window N'"), so the only per-project control was
hide/show *in this window*. Rearranging stages then took two steps in two
windows: hide the project here, switch windows, find it in the sidebar and
show it there. Users who split work across windows asked for one step.

## Decision

We will offer **Move to window** in the project context menu (opened from the
project column header and the sidebar). It lists every other open window,
named positionally ("Main window", "Window N") with the projects it shows, plus
**New window**.

A move is a pure visibility change on the desktop's `WindowState`s: the project
leaves the source window's hidden set and joins the target's — nothing else.
Other windows, the project, its terminals and its layout are untouched, so the
viewport model of ADR-0002 still holds; a move is shorthand for "hide here, show
there". It lives in `Workspace::move_project_to_window`
(`crates/okena-workspace/src/actions/project.rs`) and persists through
`window-layout.json` like any hide/show.

This supersedes only ADR-0002's "No cross-window operations" bullet and its
"Do not add cross-window commands to the project context menu" rule. The rest
of ADR-0002 stands.

## Consequences

**Easier.** Moving a project is one menu pick; running processes keep running
because no terminal is touched.

**Harder.** The project menu now depends on the set of open windows. A target
closed while the menu is open makes the move a no-op rather than hiding the
project everywhere.

**Not covered.** The CLI cannot move a project yet. Desktops declare their
windows to the daemon (`WsInbound::SetWindows`, #269), so `--window` can name an
extra window; what is missing is a move action and a desktop push that applies
it, as `okena worktree add --window` does.

## Alternatives considered

- **Keep hide/show only (ADR-0002 as is).** Rejected: the two-window dance is
  the friction users reported.
- **Move hides the project in every other window.** Rejected: a project shown
  in several windows would vanish from windows the user did not touch.
- **Drag a column header onto another window.** Not built: GPUI drags do not
  cross OS windows, so it would need custom cross-window drop tracking.
