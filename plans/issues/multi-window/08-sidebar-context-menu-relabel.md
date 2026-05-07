---
title: Sidebar context menu relabel when more than one window
status: done
type: AFK
blocked-by: [03-windowview-rename-and-per-window-focus]
user-stories: [17, 18, 19, 30]
---

## What to build

Adjust the sidebar project context menu wording based on how many windows exist:

- **One window** (no extras): keep the existing labels exactly — "Hide Project" / "Show Project". Single-window users see no UX change.
- **More than one window** (≥1 extra): relabel to "Hide from this window" / "Show in this window".

The action behavior is unchanged in both cases — it toggles the project's membership in the *current* window's `hidden_project_ids` (which is what slices 02/05 already do). Only the label changes, so the user has accurate language about scope when multiple windows are present.

No new menu items, no submenus, no cross-window operations ("Move to Window N" / "Show in Window N" are explicitly out of scope per the PRD).

The label is determined at render time by reading `WorkspaceData.extra_windows.is_empty()` from the workspace. The `is_hidden` toggle determination uses the *current window's* `WindowId` — this is the per-window state introduced by earlier slices.

## Acceptance criteria

- [x] Context menu label reads "Hide Project" / "Show Project" when only main exists.
- [x] Context menu label reads "Hide from this window" / "Show in this window" when at least one extra window exists.
- [x] Clicking the menu item toggles the project's presence in the current window's `hidden_project_ids` only — other windows' hidden sets unaffected.
- [x] When triggered from a window where the project is hidden (and not focused), clicking shows it in that window. Reverse for the unhide → hide direction.
- [x] Spawning a second window flips all open context menus' wording on the next render (no stale labels).
- [x] Closing all extras (back to one window) flips the labels back to the legacy text.
- [x] No new menu entries appear (no cross-window items).
- [x] `cargo build` and `cargo test` both green.

## Notes

- The conditional ("if extras exist, label changes") lives in the sidebar context-menu render code in `crates/okena-views-sidebar`. Touch the smallest surface possible; avoid restructuring the menu.
- "Current window" inside the sidebar context menu = the `WindowId` owned by the `WindowView` that hosts the sidebar. Pass the id through the same plumbing the rest of the sidebar uses for window-scoped reads/writes (introduced in slice 03).
- Optional polish that's cheap: reuse the same conditional for the corresponding action's command-palette label if one exists ("Hide Project" → "Hide from this window") so phrasing stays consistent. If it adds friction, skip and revisit later.

## Progress

- 2026-05-07: Slice 08 lands end-to-end (this commit). Sidebar project context menu's "Hide Project" item is now a four-way label dictated by (extras_exist, is_hidden_in_window). Pure helper `hide_project_menu_label(extras_exist, is_hidden_in_window) -> &'static str` in `crates/okena-views-sidebar/src/context_menu.rs` returns "Hide Project" / "Show Project" / "Hide from this window" / "Show in this window" per the four arms of the match. Four pure tests pin every arm. `ContextMenu` struct gains a `window_id: WindowId` field (mirrors `FolderContextMenu::window_id`); `ContextMenu::new` takes it as the first positional parameter. Render reads `extras_exist = !ws.data().extra_windows.is_empty()` plus `is_hidden_in_window = ws.data().window(self.window_id).map(|w| w.hidden_project_ids.contains(...)).unwrap_or(false)` -- the same direct-window-read pattern slice 05 cri 13 (2eb869e) introduced for `apply_set_project_show_in_overview` -- then computes the label via the helper and an icon (eye-off when visible -> action is "hide"; eye when hidden -> action is "show"). The icon flip mirrors `folder_context_menu.rs:138-139`'s eye-off/eye toggle for the "Show Only This Folder" / "Show All Projects" pair. Caller threading: `OverlayManager::show_context_menu` hoists `let window_id = self.window_id;` outside the `cx.new` capture (the same hoist pattern slice 03 established for `FolderContextMenu::new`) and passes it as the first arg. Menu structure unchanged -- no items added/removed (cri 7 satisfied structurally). The toggle behavior was already per-window via the existing `OverlayManagerEvent::ToggleProjectVisibility` -> `Workspace::toggle_project_overview_visibility(window_id, project_id, cx)` chain established by slices 02/03/05; this slice only changes the label and icon. Auto-update on extras presence change comes free from `ws.read(cx)` re-reads on every render -- the menu is dismissed on click-outside before any cross-window action could fire in practice, but the structure is correct (criterion 5/6 "next render" wording satisfied without an extra observer). Cargo green: 5 in `okena-views-sidebar --lib` (was 1, +4 helper tests), 233 in `okena-workspace --lib` (unchanged), 80 in `okena-state --lib` (unchanged), 66 in `--bin okena` (unchanged), `cargo check --workspace` clean modulo pre-existing warnings. Fulfills criteria 1, 2, 3, 4, 5, 6, 7, 8.
