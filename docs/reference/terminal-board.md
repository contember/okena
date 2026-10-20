# Terminal status board

A **status** is a label you put on a terminal to sort it: *To do*, *Review*,
*Done*, … It is yours to set and clear; nothing sets it for you. The
statuses of a project show as groups inside the sidebar and as columns on a
kanban **board** that covers one project or every project.

Nothing changes until you set the first status: a project without statuses
renders exactly as it always did, and the board is only ever opened on
demand.

## What you see

- **Terminal menu → Status** (right-click a tab, a pane header's terminal
  button, or a terminal row in the sidebar): a
  flyout with the columns; the current one is ticked, *No status* is last and
  ticked when the terminal has none. Picking a column sets the status, *No
  status* clears it.
- **Sidebar**: once any terminal of a project has a status, the project's
  Terminals list splits under small captions, one per non-empty column in
  the columns' order, then *No status*. Terminals keep their layout order
  inside a group. Keyboard navigation walks the rows in the same order.
- **Status board** (the kanban button in a project's header, `Cmd+Shift+K` /
  `Ctrl+Shift+K` for the focused project, project context menu → *Status
  Board…*, or the command palette: *Status Board* / *Status Board (All
  Projects)*): one column per board column plus *No status*,
  one card per terminal. A card shows the terminal's name, in the
  all-projects scope its project and colour, and the agent status the
  terminal reports (dot + text) when there is one. Click a card to jump to
  that pane. Drag a card onto a column to set its status; the board opened
  for a project can switch to *All projects* and back.

## Columns

The columns live in Settings > General > **Terminal Status Columns**
(`terminal_board.columns` in `settings.json`, see
[configuration.md](configuration.md)). Each column has a stable `id` and a
display `name`. A fresh install starts with `todo`, `in-progress`, `review`,
`done`.

- Each column can take a colour (the same palette as projects); it tints
  the sidebar caption and the board column header.
- Renaming a column keeps its id, so every terminal keeps its status.
- Removing a column leaves its terminals' status ids in place; they render
  as *No status* until the id comes back or you re-sort them. Nothing is
  deleted on your behalf.
- Adding a column derives its id from the name (`Needs QA` → `needs-qa`,
  made unique with `-2`, `-3`, … if taken).

## Relation to agent status

[Agent status](agent-status.md) is what the agent in a pane reports about
itself (`working`, `blocked`, `done`, `idle`) and changes on its own. A
terminal status is a manual label and never changes on its own. The two are
independent and both are shown: a card in *Review* still carries its agent's
dot and text.

## Where it is stored

The daemon owns it, like terminal names: `terminal_statuses` on the project
in `workspace.json` (terminal id → column id), omitted when empty. It follows
the terminal when the terminal moves to another project and is dropped when
the terminal closes. Clients set it through the `set_terminal_status` action
and read it from `terminal_statuses` on each project in `GET /v1/state`
([remote.md](remote.md)); web and mobile receive the field but have no UI for
it yet.
