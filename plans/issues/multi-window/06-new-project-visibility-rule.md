---
title: New-project add — visible only in spawning window
status: open
type: AFK
blocked-by: [02-window-scoped-mutation-api]
user-stories: [7, 14]
---

## What to build

Implement the PRD's 3b-ii rule: when a project is added to the workspace from window X, it becomes visible in X only — every other window's `hidden_project_ids` gains the new project's ID.

Apply the rule consistently across every project-creation path:

- "Add Project" dialog confirmation
- "Open in Okena" / drag-drop folder ingestion
- Worktree creation (worktree children inherit the rule for the window the worktree was created from)
- Any other code path that calls `Workspace::add_project` (or its equivalent)

Mechanically, the project-add helper accepts the `WindowId` of the spawning window and, after appending the new `ProjectData`, iterates `WorkspaceData.extra_windows` plus `main_window` and inserts the new project's ID into every window's `hidden_project_ids` *except* the spawning window's.

`add_project` becomes a wrapper around the existing project insertion logic plus the new visibility-rule helper. The helper itself is pure on `WorkspaceData` and lives in the `okena-workspace::windows` module from slice 02.

After this slice: spawn W2 (slice 05). Add a project from W1. Verify it appears in W1, is hidden in W2. Add another project from W2. Verify it appears in W2, is hidden in W1.

## Acceptance criteria

- [x] Project-add helper takes a `WindowId` (the spawning window) and updates every other window's hidden set.
- [x] All existing project-creation code paths route through the helper, passing the focused window's id.
- [ ] Manual: add project from W1 with W2 already open → project visible in W1's grid, hidden in W2's grid (sidebar still lists it in both, dimmed in W2).
- [ ] Manual: add project from W2 with W1 open → project visible in W2's grid, hidden in W1's grid.
- [ ] Manual: worktree creation from W1 places the worktree visible in W1, hidden in W2.
- [x] Pure-function test: `add_project_from_window(&mut data, project, WindowId::main)` with two extras present → project ID is in both extras' `hidden_project_ids`, not in `main_window.hidden_project_ids`.
- [x] Pure-function test: same call with `WindowId::Extra(uuid)` → main's hidden set gets it, the targeted extra's does not.
- [x] `cargo build` and `cargo test` both green.

## Notes

- `delete_project_scrub_all_windows` from slice 02 already handles the inverse: deleting a project removes its ID from every window's hidden set, no orphan entries.
- The rule applies only to the moment of creation. Post-creation, hide/unhide is a separate per-window action (the existing context menu, slice 08 for relabeling).
- If only main exists (zero extras), the helper degenerates to a no-op for the hide-elsewhere step. Single-window users see no behavior change.

## Progress

- 2026-05-07: Slice 06 lands code-complete pending manual verification (this commit). Data-layer pure helper `WorkspaceData::add_project_hide_in_other_windows(project_id, spawning_window)` in `crates/okena-state/src/windows.rs`: walks `main_window` plus every entry in `extra_windows` and inserts `project_id` into each window's `hidden_project_ids` set EXCEPT the `spawning_window`'s. Unknown extras (caller raced a close, or sentinel id signaling "no spawning window") fall through both skip-conditions and hide everywhere -- defensive contract pinned by `add_project_hide_in_other_windows_unknown_extra_hides_everywhere`. Idempotent on duplicate calls (HashSet::insert never panics) and scoped to `hidden_project_ids` only (per-window widths/filters/collapsed/bounds untouched). Six pure-function tests in `okena-state::workspace_data::tests` pin: main-spawn-inserts-in-extras-only (cri 6), extra-spawn-inserts-in-main-and-other-extras (cri 7), no-extras-main-spawn-is-noop (single-window degenerate path), unknown-extra-hides-everywhere (defensive), idempotent-on-duplicate-call, does-not-touch-widths-or-filter (scope discipline). Entity-layer threading: `Workspace::add_project`, `create_worktree_project`, `register_worktree_project`, `register_worktree_project_deferred_hooks`, `register_worktree_project_inner`, and `add_discovered_worktree` all gain a `window_id: WindowId` parameter and call `data.add_project_hide_in_other_windows(&id, window_id)` after pushing the project. The `add_discovered_worktree` body's prior unconditional `main_window.hidden_project_ids.insert` is replaced with the helper call -- legacy "hidden in main only" was a single-window approximation that left discovered worktrees visible in extras, broken by per-window curation. Two new GPUI tests in `okena-workspace::actions::project::gpui_tests`: `add_project_main_spawn_with_extra_hides_in_extra_only` (entity-level main-spawn pin), `add_project_extra_spawn_hides_in_main_and_other_extras` (entity-level extra-spawn pin). Caller threading: `AddProjectDialog` gains `window_id` field threaded from `OverlayManager::toggle_add_project_dialog` (which already had `self.window_id` from slice 03). `WorktreeDialog` gains `window_id` field threaded from `OverlayManager::show_worktree_dialog`. `WorktreeListPopover` gains `window_id` field threaded from `OverlayManager::show_worktree_list`; the on_click handler that adds a discovered worktree passes it. `Sidebar::spawn_quick_create_worktree` captures `self.window_id` and passes it to `register_worktree_project_deferred_hooks`. The two `execute_action` arms (`AddProject`, `CreateWorktree`) pass the `window_id: WindowId` parameter that's already threaded through `execute_action` from slice 05 cri 13. Fulfills criteria 1, 2, 6, 7, 8 (helper + threading + pure tests + cargo green: 77 passed in `okena-state --lib`, 231 in `okena-workspace --lib`, 62 in `--bin okena`). Manual criteria 3-5 stay unflipped because they need a GUI launch session (add project from W1/W2, worktree from W1) which the autonomous loop cannot perform.
