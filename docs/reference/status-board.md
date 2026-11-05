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
  column's name in its colour, always shown while a status is set (in a
  lane too). A project without a status shows a muted *Status*
  affordance while the header is hovered. Click opens the picker:
  the columns, *No status*, and *Edit columns…* (opens settings).
- **Project menu → Status** (right-click a project or worktree row in the
  sidebar): the same choices as a flyout; the current one is ticked.
- **Lanes** (right-click a project or one of its worktrees → *Status
  lanes*, or `Cmd+Shift+K` / `Ctrl+Shift+K` for the focused project;
  remembered per window and project; with the board off the shortcut
  passes through to the terminal): while that family is focused, its
  columns stand in lanes, one per board column that has a member, with
  *No status* where the settings put it (last by default). A narrow spine
  runs down the left of each lane: an edge and a light tint in the
  column's colour (*No status* uses its own colour, see below; grey when
  uncoloured), the number of columns in the lane, and the name spelled
  letter by letter downwards (GPUI cannot rotate text). In a columns grid
  the spine is the whole gap between two lanes; in a rows grid a hairline
  separates the lanes. Project colours stay where they always are (header
  line and dot). Columns stay live terminals and
  resize as before; the divider between two columns only exists inside a
  lane. With no status
  among the visible columns, or a single focused column, the grid is flat,
  lanes or not.
- **Overview lanes** (Overview row's view menu → *Status lanes*, or
  `Cmd+Shift+K` / `Ctrl+Shift+K` with no project focused; remembered per
  window): the overview stands every visible column in lanes by its own
  status, whatever family it belongs to, a folder filter included. This is
  a separate switch from a family's lanes: turning one on leaves the other
  as it was. The tile grid layout stays flat.
- **Sidebar**: once any member of a family (the main checkout or one of
  its worktrees) has a status, the family's rows, the main checkout among
  them, split under small captions (a dot and the name in the column's
  colour), one per non-empty group in the board order (the columns, with
  *No status* where the settings put it, last by default). A family
  without any status stays as before, main checkout first. Drag a row
  onto a row or caption of another group to move it there: it takes that
  group's status (*No status* clears it), and a worktree dropped on a
  worktree also takes that row's place. Within a group, dragging a worktree
  reorders as before. The main checkout drags too, while the board is on;
  it only changes status. Settings > General > *Sidebar Family Guide* (off
  by default, independent of the board) draws a thin guide in the parent's
  colour down the expanded family, under the family's expand arrow, so a
  worktree still reads as its parent's while the captions say where it
  stands. With the board on, the
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
  project: it tints the pill, the lane spine, the sidebar caption and,
  with "Color Tinted Background" on, the terminal background of a project
  that has the status. The project's own dot and ring keep the project
  colour, so a worktree still reads as its parent's.
- Renaming a column keeps its id, so every project keeps its status.
- Removing a column leaves its projects' status ids in place; they render as
  *No status* until you re-sort them (or restore the column's id in
  `settings.json`). Nothing is deleted on your behalf.
- *No status* is not a column (it cannot be renamed or removed), but it
  has a colour and a place of its own. Its row stands among the columns in
  the board order: its swatches set the colour
  (`status_board.no_status_color`), grey by default, which tints the
  *No status* lane spine and sidebar caption, never a project; its arrows
  move it (`status_board.no_status_position`, last by default), and the
  lanes, the sidebar groups and `okena status ls` follow. Moving a column
  past it moves *No status* instead; removing a column keeps it between
  the same neighbours.
- Adding a column gives it a fresh random id (a UUID), never derived from
  the name, so a new column never picks up the statuses of a removed one.

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
but have no UI for it yet. A status set from another client repaints the
cached terminal panes like a rename or recolour does (it is part of
`Workspace::project_presentation`). The lanes view is client presentation,
per window: `status_lanes_projects` (the parent ids with lanes on) and
`status_lanes_overview` (omitted when off) on each window in
`window-layout.json`, like the sort mode. An API client switches a family's
lanes with `set_status_lanes` and the overview's with
`set_overview_status_lanes`; the server pushes both on to attached desktops.

## From the command line

`okena status` covers everything above (see `okena status --help`):

| Command | Does |
|---|---|
| `okena status ls [project] [--json]` | Every project, or one family (the parent and its worktrees), in board order: `name`, column name, column id. `--json` adds the columns, whether the board is on and the *No status* colour and position. |
| `okena status get [project]` | One project's column name and id (nothing when unset). |
| `okena status set <column> [project]` | Set a status. The column is an id or a case-insensitive name. |
| `okena status clear [project]` | Clear it. |
| `okena status lanes <on\|off> [project] [--window w]` | Lanes for the project's family in the `--window` target, else each attached desktop's active window. |
| `okena status lanes <on\|off> --overview [--window w]` | The overview's lanes, in the same target window. |
| `okena status enable` / `disable` | The master switch. |
| `okena status column ls \| add <name> [--color c] \| rename \| color \| move <column> <position> \| rm` | Edit the columns as the settings editor does: ids stay, a new column gets a random id, removing one keeps its projects' ids. `add` prints the new id. `move` counts columns only; *No status* keeps its place. |
| `okena status column no-status [color] [--position n]` | Print the *No status* colour and position (`color<TAB>position`), or set either. The position is 1-based in the board order (columns + 1 = last). |

A project is an id, a case-insensitive name or a path. Omitted, it is the
project of the terminal the command runs in (`$OKENA_TERMINAL_ID`), so an
agent working in a worktree can run `okena status set review`; the focused
project is never assumed. The CLI refuses a blank column name or one another
column already has, so names stay usable as addresses. `set` and `lanes`
warn on stderr while the board is off. `okena ls` shows each project's status
too.

## Remote projects

- **Columns are per daemon.** `status_board` lives in each daemon's
  `settings.json`, and a client resolves every status, a remote project's
  included, against its own daemon's columns. The default ids (`todo`,
  `in-progress`, `review`, `done`) match on every install; a column added
  on another machine has its own random id and shows as *No status* here.
  Setting a status on a remote project sends this client's column id to
  that project's daemon.
- **Older daemons.** A daemon from before the status board does not know
  `set_project_status` and rejects it; the status stays as it was. Clients
  do not learn a remote daemon's version, so the pill and the menu entry
  still show for its projects.
