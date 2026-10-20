# Status board

A **status** is a label you put on a project or a worktree to sort it: *To
do*, *Review*, *Done*, … It is yours to set and clear; nothing sets it for
you. The typical use is a parent project with several worktrees: each
worktree gets a status, the sidebar groups them under it, and a kanban
**board** over the parent shows where every worktree stands. The board can
also cover every project.

Nothing changes until you set the first status: a project without statuses
renders exactly as it always did, and the board is only ever opened on
demand.

## What you see

- **Project menu → Status** (right-click a project or worktree row in the
  sidebar): a flyout with the columns; the current one is ticked, *No
  status* is last and ticked when the project has none. Picking a column
  sets the status, *No status* clears it.
- **Sidebar**: once any worktree of a parent has a status, the parent's
  worktree rows split under small captions, one per non-empty column in the
  columns' order, then *No status*. Worktrees keep their order inside a
  group. Keyboard navigation walks the rows in the same order.
- **Status board** (the kanban button in a project's header, `Cmd+Shift+K` /
  `Ctrl+Shift+K` for the focused project, project context menu → *Status
  Board…*, or the command palette: *Status Board* / *Status Board (All
  Projects)*): one column per board column plus *No status*, one card per
  project. Opened from a worktree, the board covers its parent's family —
  the parent and all its worktrees. A card shows the project's name and
  colour, how many terminals it holds, in the all-projects scope its parent,
  and the most urgent agent status its terminals report (blocked beats done
  beats working). Click a card to focus that project. Drag a card onto a
  column to set its status; a board opened for a family can switch to *All
  projects* and back.

## Columns

The columns live in Settings > General > **Status Columns**
(`status_board.columns` in `settings.json`, see
[configuration.md](configuration.md)). Each column has a stable `id`, a
display `name` and an optional colour. A fresh install starts with `todo`,
`in-progress`, `review`, `done`.

- A colour (the same palette as projects) tints the sidebar caption and the
  board column header.
- Renaming a column keeps its id, so every project keeps its status.
- Removing a column leaves its projects' status ids in place; they render as
  *No status* until the id comes back or you re-sort them. Nothing is
  deleted on your behalf.
- Adding a column derives its id from the name (`Needs QA` → `needs-qa`,
  made unique with `-2`, `-3`, … if taken).

## Relation to agent status

[Agent status](agent-status.md) is what the agent in a pane reports about
itself (`working`, `blocked`, `done`, `idle`) and changes on its own. A
project status is a manual label and never changes on its own. The two are
independent and both are shown: a card in *Review* still carries its
terminals' most urgent agent dot and text.

## Where it is stored

The daemon owns it: `status` on the project in `workspace.json` (a column
id), omitted when unset. Clients set it through the `set_project_status`
action and read it from `status` on each project in `GET /v1/state`
([remote.md](remote.md)); web and mobile receive the field but have no UI for
it yet.
