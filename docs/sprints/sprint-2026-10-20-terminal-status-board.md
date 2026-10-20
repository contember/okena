<!--
On close, prepend an OUTCOME block here, then `git mv` this file to ../archive/:

> **OUTCOME — shipped YYYY-MM-DD.** <one-paragraph result.> Commit map: WU1 → <sha>,
> WU2 → <sha>, … Verification: <the gate command + numbers>. Backlog closed:
> <ids deleted/rescoped>. Deferred: <honest notes>.
-->

# Sprint — Terminal status board (2026-10-20)

**Goal.** Let a user sort the terminals of a project into statuses and look at
them as a kanban board, without changing anything for a user who never sets a
status.

**Theme.** Today a project's terminals are one flat list (sidebar) and one
layout tree (main area). When several agents run in one project there is no
way to say "this one is waiting for review, that one is done". A status is a
*manual* label on a terminal, persisted by the daemon like `terminal_names`.
It drives two surfaces: status headers inside the sidebar project, and a
board overlay (per project, or all projects) with one column per status and
drag between columns. Agent status (`OSC 9001`, automatic) stays a separate,
orthogonal signal shown on the cards.

Success: a project with no statuses renders byte-for-byte as before; a
project with one status shows grouped rows in the sidebar; the board opens
from the project menu / command palette, moves a card between columns and the
sidebar follows.

## Refs re-verified at HEAD (2026-10-20)

- ✔ Per-terminal metadata is a map on the project, daemon-owned, mirrored to
  clients through `ApiProject` — `terminal_names` at
  `crates/okena-state/src/workspace_data.rs:203-204`, wire field at
  `crates/okena-core/src/api.rs:413`, built at
  `crates/okena-app-core/src/remote_snapshot.rs:60`, re-keyed with the
  `remote:<conn>:` prefix at `crates/okena-workspace/src/remote_apply.rs:157-161`
  and `:206`.
- ✔ A per-terminal action goes `ActionRequest` (`api.rs:812` `RenameTerminal`)
  → `execute_action` (`crates/okena-app-core/src/workspace/actions/execute/mod.rs:191`)
  → `Workspace::rename_terminal`
  (`crates/okena-workspace/src/actions/terminal.rs:86`, `with_project` +
  `notify_data`). The daemon runs the same `execute_action`
  (`crates/okena-daemon-core/src/command_loop.rs:23`). The desktop strips the
  id prefix in `crates/okena-app/src/action_dispatch.rs:698-706`.
- ✔ `terminal_names` is cleaned up when a terminal goes away or is re-keyed —
  `crates/okena-workspace/src/state.rs:680, 874, 1995` (remove) and `:2096`
  (rename id). A new map must follow every one of those sites.
- ✔ The sidebar's expanded terminal rows come from
  `Sidebar::render_expanded_children` (`crates/okena-views-sidebar/src/sidebar/render.rs`,
  called at `:1176`) and the keyboard cursor mirrors them in
  `push_group_cursor_items` (`sidebar/cursor.rs`). Any grouping must land in
  both or cursor index `i` stops matching row `i`.
- ✔ Section-header pattern to reuse: `render_activity_tier_header`
  (`sidebar/render.rs:137-164`) and the LOCAL / REMOTE headings in
  `sidebar/section_header.rs` (PR #246).
- ✔ The terminal menu is one view used from the tab header and the sidebar row
  (`crates/okena-views-terminal/src/overlays/terminal_menu.rs`, events at `:18`,
  sections via `menu_section` at `:392/:443/:491`, submenus via
  `okena_ui::submenu::{Submenu, SubmenuState}` at `:12`).
- ✔ Overlays are created through `OverlayManager::toggle_*` / `show_*`
  (`crates/okena-app/src/views/overlay_manager.rs:670-2035`); a polling overlay
  example is `HookLog` (`crates/okena-views-sidebar/src/hook_log.rs`).
  Global actions register in `crates/okena-app/src/keybindings/mod.rs:40-50`
  and `keybindings/descriptions.rs:564-571` (command palette), and are
  handled in `crates/okena-app/src/views/window/render.rs:1131`.
- ✔ Jumping to a pane from anywhere: `focus_terminal_by_id`
  (`crates/okena-workspace/src/actions/focus.rs:305`), the same path the
  AGENTS list uses.
- ✔ Sidebar drag primitives: `ProjectDrag` / `ProjectDragView`
  (`crates/okena-views-sidebar/src/drag.rs`), dropped via `on_drop` in
  `sidebar/render.rs:962-987`.
- ✔ Settings: nested groups like `SidebarSettings`
  (`crates/okena-workspace/src/settings.rs`), setters in
  `crates/okena-app-core/src/settings.rs`, panel in
  `crates/okena-app/src/views/overlays/settings_panel/render_general.rs`.
  No list-valued setting has a panel editor yet.
- ⚠ `ApiProject` and `ProjectData` are built as struct literals in 14 and 27
  places (tests included). Adding a field means touching each; neither type
  implements `Default`.
- ✔ Web (`web/src/api/types.ts:27`) and mobile
  (`crates/okena-mobile-ffi/src/types.rs`) read `ApiProject` field by field;
  an extra field with `#[serde(default, skip_serializing_if = …)]` is invisible
  to them.

## Work units

### WU1 — Status model, action, wire (effort M)

- **Problem.** Nothing on a terminal can hold a user-set status; every client
  reads terminal metadata from `ApiProject`.
- **Verify first.** `grep -n "terminal_names" crates/okena-state crates/okena-core crates/okena-workspace crates/okena-app-core` — the sites a new map must mirror.
- **Scope.**
  1. `okena-state`: `ProjectData.terminal_statuses: HashMap<String, String>`
     (terminal id → column id), `#[serde(default)]`. Follow every
     `terminal_names` cleanup / re-key site (`state.rs:680, 874, 1995, 2096`).
  2. `okena-core`: `ApiProject.terminal_statuses` with
     `#[serde(default, skip_serializing_if = "HashMap::is_empty")]`;
     `ActionRequest::SetTerminalStatus { project_id, terminal_id, status: Option<String> }`
     (`None` clears). Prefix-stripping arm in `action_dispatch.rs` next to
     `RenameTerminal`.
  3. `okena-workspace`: `Workspace::set_terminal_status` in
     `actions/terminal.rs` (insert / remove, `with_project`);
     `remote_apply.rs` re-keys the map like `terminal_names`; the
     presentation fingerprint in `state.rs` (`project_presentation`, PR #247)
     gains the map so a status set elsewhere repaints cached views.
  4. `okena-app-core`: `execute/mod.rs` arm → `terminal::set_status`;
     `remote_snapshot.rs` copies the map into `ApiProject`.
  5. Update the 14 + 27 struct-literal sites.
- **Acceptance / witness.** Unit tests: `set_terminal_status` sets, replaces
  and clears; closing a terminal drops its entry; renaming a terminal id
  carries it; `remote_apply` prefixes keys; a `workspace.json` without the
  field loads (serde default); `ApiProject` round-trips the field and omits it
  when empty. `cargo test -p okena-state -p okena-workspace -p okena-app-core -p okena-core`.
- **Touch points.** `crates/okena-state/src/workspace_data.rs`,
  `crates/okena-core/src/api.rs`, `crates/okena-workspace/src/{actions/terminal.rs,state.rs,remote_apply.rs}`,
  `crates/okena-app-core/src/{workspace/actions/execute/{mod.rs,terminal.rs},remote_snapshot.rs}`,
  `crates/okena-app/src/action_dispatch.rs`, literal sites in
  `okena-cli`, `okena-transport`, `okena-mobile-ffi` tests.

### WU2 — Board columns setting (effort M)

- **Problem.** Statuses need a shared, editable column list; no list-valued
  setting has UI today.
- **Verify first.** `grep -n "SidebarSettings\|StatusBarSettings" crates/okena-workspace/src/settings.rs`
  for the nested-group pattern; `grep -n "settings_input_row" crates/okena-app/src/views/overlays/settings_panel/*.rs`
  for the input row helper.
- **Scope.**
  1. `AppSettings.status_board: StatusBoardSettings { columns: Vec<BoardColumn { id: String, name: String }> }`,
     default `todo / in-progress / review / done` ("To do", "In progress",
     "Review", "Done"). Ids are stable slugs; renaming keeps the id so
     existing statuses survive. A status whose column no longer exists
     renders as unassigned (never deleted).
  2. Setters: `set_status_board_columns(Vec<BoardColumn>)`.
  3. Settings > General > "Terminal Status Columns": a small list editor
     (name input per row, remove, add, move up/down). Reuses `SimpleInput`
     and the existing row/toggle components.
  4. Sidebar / board read the list through the existing `GetSettingsFn`
     bridge (`okena_views_sidebar::SidebarSettings`) and a `GlobalBoardColumns`
     fn pointer in `okena-ui` for the overlay, mirroring `GlobalStatusBarStyle`
     (`crates/okena-ui/src/metrics.rs:27-36`).
- **Acceptance / witness.** Serde test: missing `status_board` → defaults;
  a 1.x `settings.json` loads unchanged. Unit test for the pure
  `resolve_column(columns, status_id) -> Option<&BoardColumn>`.
- **Touch points.** `crates/okena-workspace/src/settings.rs`,
  `crates/okena-app-core/src/settings.rs`,
  `crates/okena-app/src/views/overlays/settings_panel/render_general.rs`,
  `crates/okena-ui/src/metrics.rs` (or a new `board.rs`),
  `crates/okena-app/src/views/window/mod.rs` (callback wiring),
  `docs/reference/configuration.md`.

### WU3 — "Status" in the terminal menu (effort S)

- **Problem.** No way to set a status from the UI.
- **Verify first.** Open the terminal menu from a tab and from a sidebar row;
  confirm both reach `TerminalMenu` with a `TerminalMenuInvocation`.
- **Scope.** A `Status` section in `terminal_menu.rs` listing the columns
  with a check on the current one plus "None". Emits
  `TerminalMenuEvent::SetStatus { project_id, terminal_id, status }`;
  `OverlayManager` turns it into `ActionRequest::SetTerminalStatus` the same
  way `RenameTerminal` is handled (`overlay_manager.rs:1497`). Submenu if the
  flat section makes the menu too tall (>6 columns).
- **Acceptance / witness.** Manual: set, change, clear from both entry
  points; the daemon's `workspace.json` shows the map. Unit test on the pure
  "which item is checked" helper.
- **Touch points.** `crates/okena-views-terminal/src/overlays/terminal_menu.rs`,
  `crates/okena-app/src/views/overlay_manager.rs`.

### WU4 — Status groups in the sidebar (effort M)

- **Problem.** The sidebar shows a project's terminals as one list regardless
  of status.
- **Verify first.** Read `render_expanded_children` and
  `push_group_cursor_items` side by side; note every row the cursor counts.
- **Scope.**
  1. A pure `group_terminals_by_status(terminal_ids, statuses, columns) -> Vec<(Option<&BoardColumn>, Vec<String>)>`
     in `okena-views-sidebar` (or `okena-workspace` so the board can share
     it): columns in settings order, each with its terminals in layout
     order, unassigned last. Empty columns are skipped. A project with no
     statuses yields a single unlabeled group — the existing rendering.
  2. `render_expanded_children` emits a small header row (non-cursor,
     `render_activity_tier_header` style) before each labeled group;
     `cursor_scroll_indices` bookkeeping extends the header offset logic from
     PR #246 to these headers.
  3. Terminal row right-click already opens the terminal menu (WU3).
- **Acceptance / witness.** Unit tests for the grouping fn (ordering, unknown
  column → unassigned, no statuses → one group). Manual: keyboard up/down
  walks rows in visual order across headers.
- **Touch points.** `crates/okena-views-sidebar/src/sidebar/{render.rs,cursor.rs}`,
  a new `sidebar/status_groups.rs`.

### WU5 — Board overlay (effort L)

- **Problem.** No kanban view.
- **Verify first.** How `HookLog` is opened, sized and closed
  (`toggle_hook_log`, `overlay_manager.rs:765`; Escape binding for the
  `HookLog` key context at `keybindings/mod.rs:193`). How `ProjectDrag`
  renders its ghost (`drag.rs`).
- **Scope.**
  1. `crates/okena-app/src/views/overlays/status_board.rs`: a modal overlay
     with a header (scope switch *This project / All projects*, close), one
     column per settings column plus *Unassigned*, cards per terminal: name
     (`terminal_display_name`), project name + colour dot when in
     all-projects scope, agent status dot + text when present, last activity
     relative time. Columns scroll vertically; the board scrolls horizontally
     past ~5 columns.
  2. Click a card → `focus_terminal_by_id` + close. Drag a card onto a column
     → `ActionRequest::SetTerminalStatus` via the project's dispatcher. The
     board observes the workspace so the daemon's snapshot moves the card;
     no optimistic local write (daemon authority, `okena-app/src/CLAUDE.md`).
  3. Entry points: `ShowStatusBoard` (focused project) and
     `ShowStatusBoardAll` global actions (command palette, no default
     keybinding); "Status board…" in the project context menu
     (`crates/okena-views-sidebar/src/context_menu.rs` near "Rename Project").
  4. Scope defaults to the project it was opened from; the all-projects mode
     remembers the last choice for the session only.
- **Acceptance / witness.** Pure tests for the card/column model builder
  (`board_model(projects, columns, scope) -> Vec<BoardColumnModel>`): scope
  filtering, column order, unassigned bucket, agent status passthrough.
  Manual: drag across columns updates `workspace.json` and the sidebar
  group; opening from a project pre-selects it; all-projects shows project
  names.
- **Touch points.** new `crates/okena-app/src/views/overlays/status_board.rs`
  (+ `overlays/mod.rs`), `overlay_manager.rs`, `keybindings/{mod.rs,descriptions.rs}`,
  `views/window/render.rs` (action handlers),
  `crates/okena-views-sidebar/src/context_menu.rs`.

### WU6 — Docs (effort S)

- **Scope.** `docs/reference/status-board.md` (what a status is, where it
  shows, how it relates to agent status, the setting), a settings row in
  `docs/reference/configuration.md`, a cross-reference from
  `docs/reference/agent-status.md`, `docs/reference/remote.md` gains
  `terminal_statuses` + `SetTerminalStatus`. `docs/INDEX.md` and
  `docs/reference/README.md` entries.
- **Acceptance / witness.** Docs reviewed against the shipped behaviour in
  the same change that closes the sprint.

## Out of scope (explicit)

- **Live terminal previews on cards.** A card is text; rendering a terminal
  grid off-layout is a renderer project of its own.
- **Automatic status from agent lifecycle** (e.g. `done` → Review). Decided
  manual-only for v1; revisit as a setting if asked.
- **Per-project column sets.** One global list; per-project overrides would
  double the settings UI and the state.
- **Web / mobile UI.** The field rides the wire so they stay in sync later;
  no board or grouping there now.
- **Keyboard shortcuts for moving a terminal between statuses.** Menu and
  drag only.
- **Status on hook / service terminals.** Only layout terminals.

## Decisions

- **Status is manual and separate from agent status.** Agent status is pushed
  by the agent and sticky; a user label must not be overwritten by it. Cards
  and sidebar rows show both.
- **Columns live in settings, keyed by stable ids.** Renaming a column keeps
  every terminal's status; deleting one leaves those terminals unassigned
  rather than deleting data.
- **Board is an overlay, not a layout mode.** Keeps the per-window layout
  tree untouched and ships without touching `LayoutContainer`.
- **Zero default impact.** No new field has a non-empty default; no surface
  changes until a status exists. Covered by the "no statuses → identical
  rendering" tests in WU4 and the serde-default tests in WU1/WU2.

## Sequencing

| Step | WUs | Notes |
|---|---|---|
| 1 | WU1 ∥ WU2 | independent; WU2 only needs the column type |
| 2 | WU3 | needs WU1 (action) + WU2 (columns) |
| 3 | WU4 ∥ WU5 | both consume WU1–3; share the grouping helper |
| 4 | WU6 | closes the sprint |

## Run log

<!-- Append as you work: discoveries, deviations, blockers. -->

- 2026-10-20 WU1: beyond the four `terminal_names` cleanup sites the plan
  listed, the status also had to ride the runtime slot
  (`ProjectRuntimeSlot`, `take_project_layout_runtime`), both move paths in
  `actions/layout/move_ops.rs`, the orphan prune in `actions/layout/mod.rs`
  (which is what `close_terminal` uses) and the failed-hook cleanup in
  `execute/session.rs`. Found by the close-cleanup test failing first.
- 2026-10-20 WU1: the presentation fingerprint (`project_presentation`) the
  plan wanted to extend is on PR #247, not on `main` yet; add
  `terminal_statuses` to it when the two land together.
- 2026-10-20 WU1: `okena-mobile-ffi` got its test literal updated but was not
  compiled here (its deps are not in the offline cache); run its tests
  before merge.
- 2026-10-20 WU4: the sidebar on `main` had no bookkeeping for non-cursor
  rows between terminal rows; added `fold_extra_rows` in
  `sidebar/status_groups.rs`, used by both the manual and the activity
  path. PR #246 adds a sibling mechanism for host section headers; merge
  them onto one when both are in.
- 2026-10-20 WU5: no per-terminal activity timestamp exists, so the card
  carries no "last activity" (the plan listed it); only the project has one.
- 2026-10-20 **Pivot after the first review:** the unit is the project /
  worktree, not the terminal. "I have several worktrees of one project and
  want to see where each of them stands." Replaced `terminal_statuses` with
  `ProjectData.status`, `SetTerminalStatus` with `SetProjectStatus`, the
  terminal-menu flyout with a Status flyout on the project context menu,
  the terminal groups in the sidebar with worktree groups under the parent,
  and the board's cards with projects (opened from a worktree it covers the
  parent's family). Everything named `terminal_board` became
  `status_board`. The per-terminal carry code (runtime slots, moves, close
  pruning) went away with the map. Plan sections above describe the
  original terminal-level design.
