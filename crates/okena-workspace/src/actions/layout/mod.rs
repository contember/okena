//! Layout manipulation workspace actions
//!
//! Actions for splitting, tabs, and closing terminals within layouts.
//! Split by concern:
//! - [`split`]    — `split_terminal`, split-size updates, equalize
//! - [`tabs`]     — `add_tab`, `set_active_tab`, `move_tab`
//! - [`close`]    — `close_terminal`, `close_tab`, `close_other_tabs`, `close_tabs_to_right`
//! - [`move_ops`] — `move_pane`, `move_terminal_to_tab_group` (same + cross project)

mod close;
mod move_ops;
mod split;
mod tabs;

use crate::state::Workspace;

impl Workspace {
    /// Remove terminal_names/hidden_terminals entries that are no longer in the layout.
    /// Returns the orphaned terminal IDs (for PTY cleanup by callers).
    pub(super) fn cleanup_orphaned_metadata(&mut self, project_id: &str) -> Vec<String> {
        let Some(project) = self.project_mut(project_id) else {
            return vec![];
        };

        let layout_ids: std::collections::HashSet<String> = project.layout.as_ref()
            .map(|l| l.collect_terminal_ids().into_iter().collect())
            .unwrap_or_default();

        let orphaned: Vec<String> = project.terminal_names.keys()
            .filter(|id| !layout_ids.contains(id.as_str()))
            .cloned()
            .collect();

        for id in &orphaned {
            project.terminal_names.remove(id);
            project.hidden_terminals.remove(id);
        }

        orphaned
    }
}

#[cfg(test)]
mod tests_simulate;

#[cfg(test)]
mod tests_gpui;
