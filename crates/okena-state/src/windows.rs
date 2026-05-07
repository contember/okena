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
}
