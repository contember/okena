//! Window-scoped operations on `WorkspaceData`.
//!
//! Pure operations that look up or mutate a single targeted window's state
//! by `WindowId`. Each setter routes through the `window_mut` lookup pair so
//! an unknown extra id (e.g. caller raced a close) becomes a silent no-op
//! rather than a panic, absorbing the close-race bookkeeping at the data
//! layer instead of forcing every call site to pre-check existence.
//!
//! These live in their own module per the slice 02 acceptance criterion that
//! a `windows` module exists with the operations listed in the PRD's "Module
//! sketch -> okena-workspace::windows" section. They are inherent methods on
//! `WorkspaceData` rather than free-standing functions because every prior
//! commit on this slice settled on that style and the operations cleanly fit
//! it; the issue's free-standing `fn` signatures are descriptive of shape,
//! not prescriptive of where they live.

use crate::window_id::WindowId;
use crate::window_state::{WindowBounds, WindowState};
use crate::workspace_data::WorkspaceData;

impl WorkspaceData {
    /// Look up a window's state by id.
    ///
    /// `WindowId::Main` always returns `Some(&main_window)` (the main slot is a
    /// compile-time invariant). `WindowId::Extra(uuid)` walks `extra_windows`
    /// and returns the entry whose `state.id == uuid`, or `None` if no such
    /// extra exists. The `None` return for an unknown extra is the
    /// "targeted window was just closed" signal that window-scoped setters
    /// will treat as a silent no-op.
    pub fn window(&self, id: WindowId) -> Option<&WindowState> {
        match id {
            WindowId::Main => Some(&self.main_window),
            WindowId::Extra(uuid) => self.extra_windows.iter().find(|w| w.id == uuid),
        }
    }

    /// Mutable counterpart to `window`. Same lookup contract; returns
    /// `Some(&mut main_window)` for `WindowId::Main`, the matching extra by
    /// id for `WindowId::Extra(_)`, or `None` for an unknown extra.
    pub fn window_mut(&mut self, id: WindowId) -> Option<&mut WindowState> {
        match id {
            WindowId::Main => Some(&mut self.main_window),
            WindowId::Extra(uuid) => self.extra_windows.iter_mut().find(|w| w.id == uuid),
        }
    }

    /// Set the folder filter on the targeted window. `None` clears the filter.
    ///
    /// `WindowId::Main` always lands on `main_window`. `WindowId::Extra(_)`
    /// targets the matching extra by id; if no such extra exists (e.g. the
    /// caller raced a close), the call is a silent no-op rather than an error.
    /// This matches the `window_mut` lookup contract.
    pub fn set_folder_filter(&mut self, id: WindowId, filter: Option<String>) {
        if let Some(w) = self.window_mut(id) {
            w.folder_filter = filter;
        }
    }

    /// Set a single project's column width in the targeted window.
    ///
    /// Inserts the (project_id, width) pair into the targeted window's
    /// `project_widths` map, overwriting any prior value for the same id. The
    /// pair-shaped API matches the runtime shape of a column-resize event
    /// (one column moves at a time), in contrast to the legacy entity method
    /// `update_project_widths` that takes a wholesale `HashMap<String, f32>`.
    /// Unknown extra ids are a silent no-op, matching the `window_mut` lookup
    /// contract.
    pub fn set_project_width(&mut self, id: WindowId, project_id: &str, width: f32) {
        if let Some(w) = self.window_mut(id) {
            w.project_widths.insert(project_id.to_string(), width);
        }
    }

    /// Toggle a project's hidden state in the targeted window.
    ///
    /// If `project_id` is absent from the window's `hidden_project_ids` set, it
    /// is inserted (project becomes hidden). If present, it is removed (project
    /// becomes visible). Unknown extra ids are a silent no-op, matching the
    /// `window_mut` lookup contract.
    pub fn toggle_hidden(&mut self, id: WindowId, project_id: &str) {
        if let Some(w) = self.window_mut(id) {
            if !w.hidden_project_ids.remove(project_id) {
                w.hidden_project_ids.insert(project_id.to_string());
            }
        }
    }

    /// Set a folder's collapsed state in the targeted window's sidebar.
    ///
    /// `collapsed = true` inserts `(folder_id, true)` into the targeted window's
    /// `folder_collapsed` map. `collapsed = false` removes any existing entry --
    /// the runtime convention is "absence == expanded", so the map only stores
    /// `true` values. Mirrors the entity-level `Workspace::toggle_folder_collapsed`
    /// behavior, in contrast to a hypothetical `insert(folder_id, collapsed)`
    /// shape that would store explicit `false` entries. Unknown extra ids are
    /// a silent no-op, matching the `window_mut` lookup contract.
    pub fn set_folder_collapsed(&mut self, id: WindowId, folder_id: &str, collapsed: bool) {
        if let Some(w) = self.window_mut(id) {
            if collapsed {
                w.folder_collapsed.insert(folder_id.to_string(), true);
            } else {
                w.folder_collapsed.remove(folder_id);
            }
        }
    }

    /// Apply the multi-window new-project visibility rule.
    ///
    /// PRD `plans/multi-window.md` user story 14: "I want to add a project
    /// from any window so that the project lands in that window only
    /// (visible there, hidden everywhere else by default)." Walks
    /// `main_window` plus every entry in `extra_windows` and inserts
    /// `project_id` into each window's `hidden_project_ids` set EXCEPT the
    /// `spawning_window`'s. Called from project-creation paths after the
    /// project is pushed onto `projects` so the new project becomes visible
    /// in the spawning window only.
    ///
    /// Idempotent: a window that already has the id in its set is a no-op
    /// for that window (HashSet::insert returns bool but never panics on a
    /// duplicate). Other per-window fields (`project_widths`,
    /// `folder_collapsed`, `folder_filter`, `os_bounds`) are left untouched
    /// since the rule is scoped to the visibility set.
    ///
    /// `WindowId::Extra(uuid)` for an unknown extra (e.g. the caller raced
    /// a close, or a sentinel id is passed for the no-spawning-window case)
    /// degenerates to "hide in main + every extra" -- the spawning window
    /// doesn't exist as a viewport that would benefit from default
    /// visibility, so the rule defaults to fully hidden. Mirrors the
    /// silent-no-op shape of the window-scoped setters when targeted at an
    /// already-closed extra.
    ///
    /// Mirrors the inverse helper `delete_project_scrub_all_windows`,
    /// which removes the id from every window's per-project storage on
    /// project-delete so no orphan entries survive.
    pub fn add_project_hide_in_other_windows(&mut self, project_id: &str, spawning_window: WindowId) {
        if spawning_window != WindowId::Main {
            self.main_window.hidden_project_ids.insert(project_id.to_string());
        }
        for extra in &mut self.extra_windows {
            if spawning_window != WindowId::Extra(extra.id) {
                extra.hidden_project_ids.insert(project_id.to_string());
            }
        }
    }

    /// Hide a project in every persisted window state.
    ///
    /// Used for remote projects whose server-side `show_in_overview` flag is
    /// false. Unlike `add_project_hide_in_other_windows`, there is no spawning
    /// window that should see the project by default; every open viewport must
    /// start hidden.
    pub fn hide_project_in_all_windows(&mut self, project_id: &str) {
        self.main_window.hidden_project_ids.insert(project_id.to_string());
        for extra in &mut self.extra_windows {
            extra.hidden_project_ids.insert(project_id.to_string());
        }
    }

    /// Remove a project's id from every window's per-project storage.
    ///
    /// Walks `main_window` plus every entry in `extra_windows`, and removes
    /// `project_id` from each window's `hidden_project_ids` set and
    /// `project_widths` map. Idempotent: a project absent from a given window
    /// is a no-op for that window. Other per-window fields (`folder_filter`,
    /// `folder_collapsed`, `os_bounds`) are not per-project storage and are
    /// left untouched.
    ///
    /// Called from the project-delete path so no orphan per-project entries
    /// survive the delete on any window.
    pub fn delete_project_scrub_all_windows(&mut self, project_id: &str) {
        self.main_window.hidden_project_ids.remove(project_id);
        self.main_window.project_widths.remove(project_id);
        for extra in &mut self.extra_windows {
            extra.hidden_project_ids.remove(project_id);
            extra.project_widths.remove(project_id);
        }
    }

    /// Remove a folder id from every window's per-folder storage.
    ///
    /// Clears `folder_filter` when it points at the deleted folder and removes
    /// any collapsed-state entry for the same folder. Idempotent, mirroring the
    /// project scrub helper.
    pub fn delete_folder_scrub_all_windows(&mut self, folder_id: &str) {
        if self.main_window.folder_filter.as_deref() == Some(folder_id) {
            self.main_window.folder_filter = None;
        }
        self.main_window.folder_collapsed.remove(folder_id);
        for extra in &mut self.extra_windows {
            if extra.folder_filter.as_deref() == Some(folder_id) {
                extra.folder_filter = None;
            }
            extra.folder_collapsed.remove(folder_id);
        }
    }

    /// Set the OS window bounds on the targeted window.
    ///
    /// `Some(bounds)` records the latest OS-reported origin/size so the next
    /// launch can restore the window in the same place. `None` clears the
    /// bounds (the next launch falls back to the OS default / cascade-offset).
    /// Mirrors `set_folder_filter` shape since both fields are `Option`-typed.
    /// Unknown extra ids are a silent no-op, matching the `window_mut` lookup
    /// contract.
    pub fn set_os_bounds(&mut self, id: WindowId, bounds: Option<WindowBounds>) {
        if let Some(w) = self.window_mut(id) {
            w.os_bounds = bounds;
        }
    }

    /// Set the sidebar open/closed state on the targeted window. Unknown
    /// extra ids are a silent no-op, matching the `window_mut` contract.
    pub fn set_sidebar_open(&mut self, id: WindowId, open: bool) {
        if let Some(w) = self.window_mut(id) {
            w.sidebar_open = Some(open);
        }
    }

    /// Append a fresh extra window onto `extra_windows` and return its id.
    ///
    /// Snapshots the current set of project IDs into the new window's
    /// `hidden_project_ids` so the spawned window's grid is empty at first
    /// render -- the user sees a blank viewport they then curate via the
    /// per-window "Show in this window" sidebar action.
    ///
    /// `spawning_bounds` carries the live OS bounds of the window that
    /// triggered the spawn (read by the action handler from
    /// `gpui::Window::window_bounds()`). When `Some`, the new entry's
    /// `os_bounds` is seeded with origin shifted by `+30,+30` and size
    /// preserved (the cascade-offset rule from PRD line 27 + slice 05
    /// notes line 57). When `None` (e.g. the action handler could not
    /// read its window's live bounds), `os_bounds` stays `None` and the
    /// OS picks a default position when the observer opens the window.
    /// The data layer takes the caller-supplied bounds rather than
    /// reaching into GPUI itself so this function stays GPUI-free and
    /// unit-testable. Other per-window fields (`folder_filter`,
    /// `project_widths`, `folder_collapsed`) are left at default.
    pub fn spawn_extra_window(&mut self, spawning_bounds: Option<WindowBounds>) -> WindowId {
        let mut state = WindowState::default();
        state.hidden_project_ids = self.projects.iter().map(|p| p.id.clone()).collect();
        state.os_bounds = spawning_bounds.map(|b| WindowBounds {
            origin_x: b.origin_x + 30.0,
            origin_y: b.origin_y + 30.0,
            width: b.width,
            height: b.height,
        });
        let id = state.id;
        self.extra_windows.push(state);
        WindowId::Extra(id)
    }

    /// Drop the extra window with the given id from `extra_windows`.
    ///
    /// Lifecycle counterpart to `spawn_extra_window` — the slice 07 close-flow
    /// calls this when the user closes an extra OS window so the entry stops
    /// being persisted (PRD user story 22 + slice 07 cri 3). `WindowId::Main`
    /// is a silent no-op: main is the always-present slot that closing-main
    /// cannot remove (PRD line 53 + slice 07 cri 4 — closing main quits the
    /// app via `LastWindowClosed`, it does not delete persisted state).
    /// `WindowId::Extra(uuid)` for an unknown extra (e.g. a double-close
    /// race where two close events fire for the same window) is also a
    /// silent no-op, mirroring the close-race contract of every other
    /// window-scoped operation in this module.
    pub fn close_extra_window(&mut self, id: WindowId) {
        if let WindowId::Extra(uuid) = id {
            self.extra_windows.retain(|w| w.id != uuid);
        }
    }
}
