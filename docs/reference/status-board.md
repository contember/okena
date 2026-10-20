# Status board

A **status** is a label you put on a project or a worktree to sort it: *To
do*, *Review*, *Done*, … It is yours to set and clear; nothing sets it for
you. The typical use is a parent project with several worktrees: each
worktree gets a status, and the main area can stand the family's columns in
**lanes** by status, so you see where each worktree is and what runs in it
at the same time.

The whole feature is behind one switch, Settings > General > **Status
Board**, off by default. Off, nothing below exists and Okena looks exactly
as it did before.

## What you see (with the board enabled)

- **Status pill** in a project column's header, after the name: the
  column's name in its colour. A project without a status shows a muted
  *Status* affordance while the header is hovered. Click opens the picker:
  the columns, *No status*, and *Edit columns…* (opens settings).
- **Project menu → Status** (right-click a project or worktree row in the
  sidebar): the same choices as a flyout; the current one is ticked.
- **Lanes** (right-click a project or one of its worktrees → *Status
  lanes*, or `Cmd+Shift+K` / `Ctrl+Shift+K` for the focused project;
  remembered per window and project): while that family is focused, its
  columns stand in lanes, one per board column that has a member, *No
  status* last, each under a thin caption (dot in the column's colour,
  name, hairline). Columns stay live terminals and resize as before; the
  divider between two columns only exists inside a lane. With no status
  among the visible columns, or a single focused column, the grid is flat,
  lanes or not.
- **Sidebar**: once any worktree of a parent has a status, the parent's
  worktree rows split under small captions (a dot and the name in the
  column's colour), one per non-empty column in the columns' order, then
  *No status*. Settings > General > *Sidebar Family Guide* (off by
  default, independent of the board) draws a thin guide in the parent's
  colour down the expanded family, so a worktree still reads as its
  parent's while the captions say where it stands. With the board on, the
  ring before each worktree can show the project colour (the default) or
  the status colour (*Sidebar Worktree Marker*). Keyboard navigation walks
  the rows in the same order.

## Columns

The columns live in Settings > General > **Status Columns**
(`status_board.columns` in `settings.json`, see
[configuration.md](configuration.md)), shown once the board is enabled.
Each column has a stable `id`, a display `name` and an optional colour. A
fresh install starts with `todo`, `in-progress`, `review`, `done`.

- A colour (the same palette as projects) marks the status, never the
  project: it tints the pill, the lane caption, the sidebar caption and,
  with "Color Tinted Background" on, the terminal background of a project
  that has the status. The project's own dot and ring keep the project
  colour, so a worktree still reads as its parent's.
- Renaming a column keeps its id, so every project keeps its status.
- Removing a column leaves its projects' status ids in place; they render as
  *No status* until the id comes back or you re-sort them. Nothing is
  deleted on your behalf.
- Adding a column derives its id from the name (`Needs QA` → `needs-qa`,
  made unique with `-2`, `-3`, … if taken).

## Relation to agent status

[Agent status](agent-status.md) is what the agent in a pane reports about
itself (`working`, `blocked`, `done`, `idle`) and changes on its own. A
project status is a manual label and never changes on its own. Both show at
once: a column in the *Review* lane still carries its tabs' agent
indicators.

## Where it is stored

The daemon owns the status: `status` on the project in `workspace.json` (a
column id), omitted when unset. Clients set it through the
`set_project_status` action and read it from `status` on each project in
`GET /v1/state` ([remote.md](remote.md)); web and mobile receive the field
but have no UI for it yet. The lanes view is per window
(`status_lanes` on the window in `workspace.json`), like the sort mode.
