# Multi-Window Viewports

## Problem Statement

A user managing many projects in Okena currently has only one Workspace viewport: a single window with one project-columns grid. Filtering (folder filter, focus zoom, hide/show project) is global — toggling visibility for one task disturbs every other task. There is no way to keep two parallel "stages" — for example, a "client work" set of project columns side-by-side with a "personal" set — without manually toggling filters back and forth.

## Solution

Allow the user to spawn additional windows. Each window is an independent **viewport** onto the same underlying Workspace: same projects, same folders, same terminals, same settings — but its own filter state (hidden project set, folder filter, focus zoom, column widths, folder-collapsed states, OS bounds). The sidebar appears in every window but reflects that window's filter. Closing a window does not delete any projects.

The first window (the "main" window) behaves as today — its config persists across launches, and closing it quits the app. Extra windows are ephemeral: they are forgotten on close, but if open at quit they are restored on the next launch.

## User Stories

1. As a developer, I want to open a second window so that I can keep two unrelated sets of project columns visible side-by-side without re-toggling filters.
2. As a developer, I want each window to remember its own folder filter so that switching focus in one window does not change what the other shows.
3. As a developer, I want each window to remember its own hidden-project set so that I can hide a noisy project in one window while still seeing it in another.
4. As a developer, I want each window to remember its own column widths so that the same project can appear at a different size depending on which window I am in.
5. As a developer, I want each window to remember its own folder-collapsed states in the sidebar so that the sidebars do not fight each other.
6. As a developer, I want each window to remember its own focus-zoom (single-project mode) so that zooming in one window does not zoom in the other.
7. As a developer, I want the underlying Workspace (projects, folders, ordering, hooks, layouts) to be a single shared truth so that adding a project, renaming a project, or editing hooks affects every window consistently.
8. As a developer, I want all terminals and PTYs to be shared so that the same terminal can render in multiple windows simultaneously and stays in sync byte-for-byte.
9. As a developer, I want the sidebar to remain present and consistent in every window (same projects, same folders, same actions) so that I never have to think about which window can do what.
10. As a developer, I want to spawn a new window from the menu (File → New Window), the keyboard (Cmd+Shift+N), and the command palette ("Window: New") so that the action is available wherever my hands are.
11. As a developer, I do not want to be prompted for a window name on spawn so that the spawn flow is one click / one keystroke.
12. As a developer, I want a new window to start empty (no project columns visible) so that I can deliberately curate what goes in it without inheriting noise from elsewhere.
13. As a developer, I want a new window to cascade-offset from the spawning window's position so that it does not stack invisibly on top.
14. As a developer, I want to add a project from any window so that the project lands in that window only (visible there, hidden everywhere else by default), matching where I was looking when I created it.
15. As a developer, I want to delete a project from anywhere so that it disappears from every window and the underlying Workspace simultaneously.
16. As a developer, I want to click a hidden project in the sidebar so that it appears via focus-zoom for the session — matching today's focus-override behavior, just per-window.
17. As a developer, I want the sidebar context-menu item to read "Hide Project" / "Show Project" when only one window exists so that the existing single-window experience is unchanged.
18. As a developer, I want the sidebar context-menu item to read "Hide from this window" / "Show in this window" when multiple windows exist so that the per-window scope is explicit.
19. As a developer, I do not want any cross-window operations in the project context menu (no "Move to Window N", no "Show in Window N") so that each window is self-contained and there is no bookkeeping I have to think about.
20. As a developer, I want column ordering to be shared across windows (reordering in W1 reorders in W2 and in the sidebar) so that there is one canonical project order across the app.
21. As a developer, I want closing the main window to quit the app so that today's "the window is the app" intuition is preserved.
22. As a developer, I want closing an extra window to forget its config so that closed extras do not pile up in some hidden list.
23. As a developer, I want the main window's bounds and filter config to persist across quits so that next launch picks up exactly where I left off.
24. As a developer, I want extra windows that were open at quit to also be restored on next launch (with their bounds and filters) so that a multi-window session survives a restart.
25. As a developer, I want the app to bootstrap a single fresh main window on first launch (no persisted windows on disk yet) so that the experience matches today for new users.
26. As a developer, I want OS-level keyboard actions and overlays (command palette, project switcher, theme selector, settings) to be scoped to whichever window has focus so that triggering them does not act on the wrong window.
27. As a developer, I want the CLI (e.g. `okena open <path>`) to land its action in the focused window if any, falling back to main otherwise, so that scripted invocations are predictable.
28. As a developer, I want toasts to render in each window separately so that I see the relevant feedback in the window where the action occurred.
29. As a developer, I want my existing `workspace.json` to migrate cleanly: any project I had previously hidden becomes hidden in the (now-named) main window only; folders I had collapsed stay collapsed in main; column widths I had configured land on main; everything else is unchanged.
30. As a developer, I do not want my single-window workflow to feel different until I actively spawn an extra window so that there is no learning tax for users who never use the feature.
31. As a developer, I want each window's sidebar collapse state to be independent so that one window's expanded folders don't force the other's open.
32. As a developer, I want the same terminal visible in two windows to update in both simultaneously (typing in W1 reflects in W2 instantly) so that no view stalls or shows stale output.

## Implementation Decisions

### Conceptual model

- A **Window** is a filtered viewport onto the shared Workspace, not a partition. The same project can be visible in zero, one, or many windows.
- The **main window** is special: it always exists in persistence, and closing it quits the app. Its state is preserved on close.
- **Extra windows** are ephemeral: forgotten on close, restored if open at quit.
- New project visibility rule: when added from window X, visible only in X (other windows' hidden sets gain its ID).
- New extra window starts with all current projects in its hidden set (so the grid is empty at spawn).

### Schema (workspace.json, version bumped)

- `WorkspaceData` gains:
  - `main_window: WindowState` — always present, single slot (compile-time guarantee).
  - `extra_windows: Vec<WindowState>` — forgotten on close.
- `WindowState` (new struct, lives in the pure-data crate):
  - `hidden_project_ids: HashSet<ProjectId>`
  - `folder_filter: Option<FolderId>`
  - `project_widths: HashMap<ProjectId, f32>`
  - `folder_collapsed: HashMap<FolderId, bool>`
  - `os_bounds: Option<Bounds>`
- `WorkspaceData.project_widths` is removed (migrated into `main_window.project_widths`).
- `ProjectData.show_in_overview` is removed (any `false` migrates to the main window's hidden set).
- `FolderData.collapsed` is removed (any `true` migrates to the main window's folder-collapsed map).
- Schema invariant: deserialization must always produce `main_window` even from corrupt or missing input — fall back to defaults.
- Migration: `version` field bumps; load-time migration code in the persistence module rewrites old shape into the new shape.

### Module sketch

- **`okena-state` (pure data crate)**
  - Add `WindowState` struct (serde, no GPUI).
  - Adjust `WorkspaceData` shape: add `main_window`, `extra_windows`; remove the migrated fields above.
- **`okena-workspace::visibility` (deep module, expanded)**
  - `compute_visible_projects` is rewritten to take a `&WindowState` (for hidden set, folder filter) instead of separate args. Pure function. Easy to unit test.
- **`okena-workspace::windows` (new module — primary deep module)**
  - `spawn_extra_window(&mut WorkspaceData) -> WindowId` — appends a `WindowState` whose `hidden_project_ids` is the set of all current project IDs.
  - `close_extra_window(&mut WorkspaceData, WindowId)` — removes the entry.
  - On project add: helper that pushes the new project ID into every other window's hidden set (per the visibility rule).
  - On project delete: helper that scrubs the project ID from every window's hidden set, project-widths, etc.
  - Window-scoped setters: `set_folder_filter`, `toggle_hidden`, `set_folder_collapsed`, `set_project_width`, `set_os_bounds`. Each takes a `WindowId`.
  - Pure operations on `WorkspaceData`. Testable without GPUI.
- **`okena-workspace::persistence` (modified)**
  - Migration: detect old version, transform `show_in_overview`/`collapsed`/global-widths into `main_window` fields.
  - Save/load round-trip preserves new shape.
  - Bootstrap path: empty/missing windows on disk → produce default `main_window`, empty `extra_windows`.
- **`okena-workspace::focus` (no shape change, lifecycle change)**
  - `FocusManager` instances move from being a single field on `Workspace` to being one per Window entity. The struct itself is unchanged.
- **`okena-workspace::state::Workspace` (modified)**
  - `Workspace` stays a single GPUI entity, since the underlying data is shared. Its mutation API gains a `WindowId` parameter for window-scoped operations (folder filter, hidden set, widths, collapse, focus zoom).
  - `active_folder_filter` (today on the entity) moves into `WindowState`.
- **`RootView` → `WindowView` (rename + scope change)**
  - Per-window state (sidebar entity, sidebar controller, title bar, status bar, overlay manager, toast overlay, scroll handles, drag state, project columns map, focus handle, request broker) becomes per-window — one `WindowView` instance per window.
  - Each `WindowView` reads its own `WindowId` and queries the shared `Workspace` for window-scoped state.
- **`Okena` coordinator (modified)**
  - Holds `main_window: Entity<WindowView>` and `extra_windows: HashMap<WindowId, Entity<WindowView>>`.
  - Spawns OS windows per `WindowState`. Creates `WindowView` entities. Observes the shared `Workspace` to sync UI.
  - At startup: open OS windows for `main_window` and every `extra_windows` entry.
- **`content_pane_registry` (modified)**
  - Today: `HashMap<terminal_id, WeakEntity<TerminalContent>>`. Replaced by `HashMap<terminal_id, Vec<WeakEntity<TerminalContent>>>` so PTY events fan out to every window rendering that terminal. Detached-terminal popups already use this registry, so they piggyback the fix.
  - Stale weak references cleaned up lazily (drop dead handles on iteration).
- **Spawn action**
  - New keybinding action wired to Cmd+Shift+N.
  - Menu item ("File → New Window" or "Window → New Window") added to app menus.
  - Command palette entry "Window: New".
- **Sidebar context menu (`okena-views-sidebar`)**
  - When `extra_windows.is_empty()`: keep existing "Hide Project" / "Show Project" labels.
  - When ≥1 extra window exists: relabel to "Hide from this window" / "Show in this window".
  - No cross-window items (no "Show in Window N", no "Move to Window N").

### Lifecycle and runtime

- Spawn flow: trigger action in window X → mutate `WorkspaceData.extra_windows` (push new `WindowState` with all-projects-hidden snapshot) → `Okena` observer opens an OS window with cascade-offset bounds and instantiates a fresh `WindowView`.
- Close flow:
  - Main: triggers app quit (current `LastWindowClosed` mode keeps working). Save path captures main + currently-open extras.
  - Extra: removes the entry from `extra_windows`, drops the `WindowView` entity, notifies persistence.
- Restore flow at startup: open `main_window`'s OS window first; then open each extra in order.
- Action routing: keyboard actions and overlays scope to the focused window via GPUI focus handles (already the pattern). CLI commands target the focused window or fall back to main.
- Active drag, scroll position, focus zoom: per-window, transient (not persisted). Persistence saves only what's in `WindowState`.

### Migration

On load of an existing `workspace.json`:

1. If `version < new_version`, run migration.
2. For every project with `show_in_overview = false`, insert its ID into `main_window.hidden_project_ids`. Remove the field.
3. For every folder with `collapsed = true`, insert `(folder_id, true)` into `main_window.folder_collapsed`. Remove the field.
4. Move `WorkspaceData.project_widths` wholesale into `main_window.project_widths`. Remove the top-level field.
5. Initialize `main_window.folder_filter = None`, `main_window.os_bounds = None`.
6. Initialize `extra_windows = vec![]`.
7. Bump stored version. Save round-trips into the new shape.

If the file is missing/corrupt: fall back to a default `WorkspaceData` with a default `main_window`.

## Testing Decisions

A good test here verifies external behavior — what data the user sees in each window, what the sidebar shows, what survives a save/load round-trip — without coupling to internal field names or method-call sequences. Prefer pure-function tests on `WorkspaceData` mutations and on the visibility computation; reach for `#[gpui::test]` only for the entity-level wiring that must run in a GPUI context.

### Modules to be tested

- **`okena-workspace::visibility::compute_visible_projects`** — pure. Test matrix expands to cover per-window state:
  - Hidden set hides projects in the result.
  - Folder filter limits the result to the chosen folder, with focus override still letting a focused project through.
  - Empty hidden set + no folder filter = all projects visible.
  - Worktree grouping under parent unchanged.
  - All current visibility tests in this module are updated, not deleted.
- **`okena-workspace::windows` (new module)** — pure operations on `WorkspaceData`. Tests:
  - `spawn_extra_window` produces a `WindowState` whose hidden set contains every current project ID.
  - Adding a project pushes its ID into every *other* window's hidden set (3b-ii rule).
  - Deleting a project scrubs it from every window's hidden set, widths map, and folder-collapsed map (no orphan entries).
  - `close_extra_window` removes the entry; main is never removed.
  - Setters (`set_folder_filter`, `toggle_hidden`, `set_project_width`, `set_folder_collapsed`, `set_os_bounds`) mutate the targeted window only.
- **`okena-workspace::persistence` migration** — round-trip test:
  - Old-version JSON with `show_in_overview=false`, `FolderData.collapsed=true`, top-level `project_widths` loads → values land on `main_window`.
  - Save → reload yields a stable identical structure (idempotent).
  - Missing/corrupt windows section falls back to a default `main_window` and empty extras (schema invariant).
- **`okena-state::WindowState` serde** — round-trip a populated `WindowState` through JSON.
- **Per-window `FocusManager` isolation** — `#[gpui::test]`. Two `FocusManager` instances; pushing/popping in one does not affect the other.
- **Window lifecycle smoke** — `#[gpui::test]` covering spawn → entry appears in `extra_windows`; close-extra → entry removed; close-main → quit path triggers (the quit itself is hard to assert in unit; verify the save invocation captures current state).
- **Window-scoped mutations on `Workspace` entity** — `#[gpui::test]`. Calling a window-scoped setter with `WindowId::main` writes to `main_window`; with an extra's id, writes to that entry.

### Prior art

- `crates/okena-workspace/src/visibility.rs` already has a thorough pure-function test suite (`focused_project_shown_even_when_hidden`, `focus_worktree_shows_only_worktree`, etc.). Mirror this style for the new per-window cases.
- `crates/okena-state/src/workspace_data.rs` has serde round-trip tests including legacy-shape migration (`project_data_with_legacy_hooks_migrates_on_load`, `project_data_legacy_hooks_save_roundtrip_uses_grouped_format`). Mirror this for the schema bump and `WindowState`.
- `crates/okena-workspace/src/state.rs` shows the `#[gpui::test]` setup pattern (`init_test_settings` helper, explicit closure types). Reuse for entity-level tests.

### What NOT to test

- Trivial getters / setters / forwarders.
- Render paths (per repo convention).
- Cosmetic widget tweaks (label changes, menu wording) beyond confirming the conditional ("if extras exist, label changes") in a unit-testable spot.

## Out of Scope

- User-given window names and a rename UI.
- Cross-window operations in the project context menu ("Move to Window N", "Show in Window N").
- A "Show all in this window" / "Hide from all other windows" nuclear menu option.
- Per-window column ordering (ordering stays shared with the sidebar).
- A picker dialog at spawn ("which projects in the new window?"). Spawn is always empty.
- Saved/named window layouts that can be reopened from a menu after close. Closed extras are gone.
- Exposing per-window state over the remote control API. Remote sees a flat workspace; multi-window is local-only for v1.
- Mobile and web clients — they are unaffected.
- Reopening a closed main window mid-session. Closing main quits.
- Multi-monitor placement heuristics beyond cascade-offset on spawn.

## Further Notes

- The main-is-special invariant lives at the data layer (`main_window` is its own slot) and at the lifecycle layer (closing it quits). Both reinforce the same intuition; do not let one drift without the other.
- Today's transient state on the `Workspace` entity (`active_folder_filter`, `focus_manager`) becomes per-window. `active_folder_filter` moves into the persisted `WindowState`; `FocusManager` becomes a field on each `WindowView` (still transient).
- The fan-out fix on `content_pane_registry` is the single piece of plumbing that lets the same terminal render in two windows at once. Without it, only one window's pane would update on PTY data; bytes would visually stall in others.
- Migration must be exercised by a real fixture that mirrors a typical existing user's `workspace.json` — the failure mode (silent loss of a hidden project's hidden state, or losing column widths) is hard to spot post-hoc.
- The user explicitly chose: no ADRs for this work.
