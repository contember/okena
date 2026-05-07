//! Pane and tab moves (same-project and cross-project).

use crate::focus::FocusManager;
use crate::state::{DropZone, LayoutNode, SplitDirection, Workspace};
use gpui::*;

impl Workspace {
    /// Move a terminal pane to a new position relative to a target terminal.
    ///
    /// Extracts the source terminal from its current position and inserts it
    /// next to the target based on the drop zone (Top/Bottom/Left/Right/Center).
    /// Supports both same-project and cross-project moves.
    pub fn move_pane(
        &mut self,
        focus_manager: &mut FocusManager,
        source_project_id: &str,
        source_terminal_id: &str,
        target_project_id: &str,
        target_terminal_id: &str,
        zone: DropZone,
        cx: &mut Context<Self>,
    ) {
        // Self-drop check
        if source_terminal_id == target_terminal_id {
            return;
        }

        if source_project_id == target_project_id {
            self.move_pane_same_project(focus_manager, source_project_id, source_terminal_id, target_terminal_id, zone, cx);
        } else {
            self.move_pane_cross_project(focus_manager, source_project_id, source_terminal_id, target_project_id, target_terminal_id, zone, cx);
        }
    }

    /// Same-project pane move (original logic).
    fn move_pane_same_project(
        &mut self,
        focus_manager: &mut FocusManager,
        project_id: &str,
        source_terminal_id: &str,
        target_terminal_id: &str,
        zone: DropZone,
        cx: &mut Context<Self>,
    ) {
        let project = match self.project(project_id) {
            Some(p) => p,
            None => return,
        };
        let layout = match project.layout.as_ref() {
            Some(l) => l,
            None => return,
        };

        // Only-terminal check: don't move if it's the only terminal
        if layout.collect_terminal_ids().len() <= 1 {
            return;
        }

        // Find source path
        let source_path = match layout.find_terminal_path(source_terminal_id) {
            Some(p) => p,
            None => return,
        };

        // Clone source node before removal
        let source_node = match layout.get_at_path(&source_path) {
            Some(node) => node.clone(),
            None => return,
        };

        if source_path.is_empty() {
            // Source is root — can't remove root
            return;
        }

        // Perform the mutation in a block to limit mutable borrow scope
        let new_focus_path = {
            let project = match self.project_mut(project_id) {
                Some(p) => p,
                None => return,
            };
            let layout = match project.layout.as_mut() {
                Some(l) => l,
                None => return,
            };

            if layout.remove_at_path(&source_path).is_none() {
                return;
            }

            // Re-find target path after removal (indices may have shifted)
            let target_path = match layout.find_terminal_path(target_terminal_id) {
                Some(p) => p,
                None => return,
            };

            // Get target node and replace it with wrapper
            let target_node = match layout.get_at_path(&target_path) {
                Some(node) => node.clone(),
                None => return,
            };

            let wrapper = Self::build_drop_zone_wrapper(source_node, target_node, zone);

            // Replace target node with wrapper
            if let Some(node) = layout.get_at_path_mut(&target_path) {
                *node = wrapper;
            }

            // Normalize to flatten nested same-direction splits
            layout.normalize();

            // Find the new path for focus before releasing borrow
            layout.find_terminal_path(source_terminal_id)
        };

        self.notify_data(cx);

        // Update focus to moved terminal's new path
        if let Some(new_path) = new_focus_path {
            self.set_focused_terminal(focus_manager, project_id.to_string(), new_path, cx);
        }
    }

    /// Cross-project pane move: extract terminal from source project, insert into target project.
    fn move_pane_cross_project(
        &mut self,
        focus_manager: &mut FocusManager,
        source_project_id: &str,
        source_terminal_id: &str,
        target_project_id: &str,
        target_terminal_id: &str,
        zone: DropZone,
        cx: &mut Context<Self>,
    ) {
        // Find project indices (needed for split borrows)
        let src_idx = match self.data.projects.iter().position(|p| p.id == source_project_id) {
            Some(i) => i,
            None => return,
        };
        let tgt_idx = match self.data.projects.iter().position(|p| p.id == target_project_id) {
            Some(i) => i,
            None => return,
        };

        // Validate source has layout and terminal exists
        let src_layout = match self.data.projects[src_idx].layout.as_ref() {
            Some(l) => l,
            None => return,
        };
        let source_path = match src_layout.find_terminal_path(source_terminal_id) {
            Some(p) => p,
            None => return,
        };
        let source_node = match src_layout.get_at_path(&source_path) {
            Some(node) => node.clone(),
            None => return,
        };

        // Block if terminal is a service terminal
        if self.data.projects[src_idx].service_terminals.values().any(|id| id == source_terminal_id) {
            return;
        }

        // Validate target has layout and target terminal exists
        let tgt_layout = match self.data.projects[tgt_idx].layout.as_ref() {
            Some(l) => l,
            None => return,
        };
        if tgt_layout.find_terminal_path(target_terminal_id).is_none() {
            return;
        }

        // --- Extract from source ---
        let src_project = &mut self.data.projects[src_idx];
        if source_path.is_empty() {
            // Source is root — remove entire layout
            src_project.layout = None;
        } else if let Some(src_layout) = src_project.layout.as_mut() {
            if src_layout.remove_at_path(&source_path).is_none() {
                return;
            }
            src_layout.normalize();
        } else {
            // Layout was validated Some above; disappearing is a bug, not a crash.
            return;
        }

        // Migrate metadata from source to target
        let terminal_name = src_project.terminal_names.remove(source_terminal_id);
        let hidden_state = src_project.hidden_terminals.remove(source_terminal_id);

        // Cleanup orphaned source metadata
        let src_layout_ids: std::collections::HashSet<String> = src_project.layout.as_ref()
            .map(|l| l.collect_terminal_ids().into_iter().collect())
            .unwrap_or_default();
        src_project.terminal_names.retain(|id, _| src_layout_ids.contains(id));
        src_project.hidden_terminals.retain(|id, _| src_layout_ids.contains(id));

        // --- Insert into target ---
        let tgt_project = &mut self.data.projects[tgt_idx];

        if let Some(name) = terminal_name {
            tgt_project.terminal_names.insert(source_terminal_id.to_string(), name);
        }
        if let Some(hidden) = hidden_state {
            tgt_project.hidden_terminals.insert(source_terminal_id.to_string(), hidden);
        }

        let new_focus_path = if let Some(ref mut tgt_layout) = tgt_project.layout {
            // Re-find target path in target layout
            let target_path = match tgt_layout.find_terminal_path(target_terminal_id) {
                Some(p) => p,
                None => return,
            };
            let target_node = match tgt_layout.get_at_path(&target_path) {
                Some(node) => node.clone(),
                None => return,
            };

            let wrapper = Self::build_drop_zone_wrapper(source_node, target_node, zone);

            if let Some(node) = tgt_layout.get_at_path_mut(&target_path) {
                *node = wrapper;
            }
            tgt_layout.normalize();
            tgt_layout.find_terminal_path(source_terminal_id)
        } else {
            // Target has no layout — set source node as root
            let root = source_node;
            let path = root.find_terminal_path(source_terminal_id);
            tgt_project.layout = Some(root);
            path
        };

        self.notify_data(cx);

        // Focus the moved terminal in the target project
        if let Some(new_path) = new_focus_path {
            self.set_focused_terminal(focus_manager, target_project_id.to_string(), new_path, cx);
        }
    }

    /// Build wrapper node for drop zone placement.
    fn build_drop_zone_wrapper(source_node: LayoutNode, target_node: LayoutNode, zone: DropZone) -> LayoutNode {
        match zone {
            DropZone::Top => LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                sizes: vec![50.0, 50.0],
                children: vec![source_node, target_node],
            },
            DropZone::Bottom => LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                sizes: vec![50.0, 50.0],
                children: vec![target_node, source_node],
            },
            DropZone::Left => LayoutNode::Split {
                direction: SplitDirection::Vertical,
                sizes: vec![50.0, 50.0],
                children: vec![source_node, target_node],
            },
            DropZone::Right => LayoutNode::Split {
                direction: SplitDirection::Vertical,
                sizes: vec![50.0, 50.0],
                children: vec![target_node, source_node],
            },
            DropZone::Center => LayoutNode::Tabs {
                children: vec![target_node, source_node],
                active_tab: 1,
            },
        }
    }

    /// Move a terminal into an existing tab group.
    ///
    /// Extracts the source terminal from its current position and inserts it
    /// into the Tabs container at `tabs_path` at the given `insert_index`
    /// (or appends if `None`). This avoids the nested-Tabs problem that
    /// `move_pane(Center)` would create when the target is already inside
    /// a tab group.
    ///
    /// After removal the layout may collapse (e.g. a 2-child split dissolves),
    /// so we locate the target tab group by finding a reference terminal that
    /// was already in it, rather than relying on the original `tabs_path`.
    ///
    /// Supports cross-project moves when `target_project_id` differs from
    /// `source_project_id`.
    pub fn move_terminal_to_tab_group(
        &mut self,
        focus_manager: &mut FocusManager,
        source_project_id: &str,
        terminal_id: &str,
        target_project_id: &str,
        tabs_path: &[usize],
        insert_index: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        if source_project_id == target_project_id {
            self.move_terminal_to_tab_group_same_project(focus_manager, source_project_id, terminal_id, tabs_path, insert_index, cx);
        } else {
            self.move_terminal_to_tab_group_cross_project(focus_manager, source_project_id, terminal_id, target_project_id, tabs_path, insert_index, cx);
        }
    }

    /// Same-project tab group move (original logic).
    fn move_terminal_to_tab_group_same_project(
        &mut self,
        focus_manager: &mut FocusManager,
        project_id: &str,
        terminal_id: &str,
        tabs_path: &[usize],
        insert_index: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let project = match self.project(project_id) {
            Some(p) => p,
            None => return,
        };
        let layout = match project.layout.as_ref() {
            Some(l) => l,
            None => return,
        };

        // Find source path
        let source_path = match layout.find_terminal_path(terminal_id) {
            Some(p) => p,
            None => return,
        };

        // Don't move if source is already in the target tab group
        if !source_path.is_empty() {
            let source_parent = &source_path[..source_path.len() - 1];
            if source_parent == tabs_path {
                // Already in this tab group — treat as reorder or noop
                if let Some(idx) = insert_index {
                    let from = source_path[source_path.len() - 1];
                    if from != idx {
                        self.move_tab(project_id, tabs_path, from, idx, cx);
                    }
                }
                return;
            }
        }

        // Clone source node
        let source_node = match layout.get_at_path(&source_path) {
            Some(node) => node.clone(),
            None => return,
        };

        if source_path.is_empty() {
            return; // Can't remove root
        }

        // Find a reference terminal already in the target tab group so we can
        // re-locate the group after removal may have shifted paths.
        let reference_tid = match layout.get_at_path(tabs_path) {
            Some(node) => {
                let ids = node.collect_terminal_ids();
                // Pick a terminal that isn't the one we're moving
                ids.into_iter().find(|id| id != terminal_id)
            }
            None => return,
        };
        let reference_tid = match reference_tid {
            Some(id) => id,
            None => return, // Tab group has no other terminals
        };

        // Perform mutation
        let new_focus_path = {
            let project = match self.project_mut(project_id) {
                Some(p) => p,
                None => return,
            };
            let layout = match project.layout.as_mut() {
                Some(l) => l,
                None => return,
            };

            if layout.remove_at_path(&source_path).is_none() {
                return;
            }

            // Re-find the tabs container via the reference terminal
            let ref_path = match layout.find_terminal_path(&reference_tid) {
                Some(p) => p,
                None => return,
            };
            // The Tabs node is the parent of the reference terminal
            let new_tabs_path = if ref_path.is_empty() {
                // Reference terminal is at root — layout collapsed unexpectedly
                return;
            } else {
                &ref_path[..ref_path.len() - 1]
            };

            let tabs_node = match layout.get_at_path_mut(new_tabs_path) {
                Some(node) => node,
                None => return,
            };

            if let LayoutNode::Tabs { children, active_tab } = tabs_node {
                let idx = insert_index.unwrap_or(children.len());
                let clamped = idx.min(children.len());
                children.insert(clamped, source_node);
                *active_tab = clamped;
            } else {
                // Target is not a Tabs container (layout shifted) — abort
                return;
            }

            layout.normalize();
            layout.find_terminal_path(terminal_id)
        };

        self.notify_data(cx);

        if let Some(new_path) = new_focus_path {
            self.set_focused_terminal(focus_manager, project_id.to_string(), new_path, cx);
        }
    }

    /// Cross-project tab group move: extract terminal from source project, insert into target tab group.
    fn move_terminal_to_tab_group_cross_project(
        &mut self,
        focus_manager: &mut FocusManager,
        source_project_id: &str,
        terminal_id: &str,
        target_project_id: &str,
        tabs_path: &[usize],
        insert_index: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let src_idx = match self.data.projects.iter().position(|p| p.id == source_project_id) {
            Some(i) => i,
            None => return,
        };
        let tgt_idx = match self.data.projects.iter().position(|p| p.id == target_project_id) {
            Some(i) => i,
            None => return,
        };

        // Validate source
        let src_layout = match self.data.projects[src_idx].layout.as_ref() {
            Some(l) => l,
            None => return,
        };
        let source_path = match src_layout.find_terminal_path(terminal_id) {
            Some(p) => p,
            None => return,
        };
        let source_node = match src_layout.get_at_path(&source_path) {
            Some(node) => node.clone(),
            None => return,
        };

        // Block service terminals
        if self.data.projects[src_idx].service_terminals.values().any(|id| id == terminal_id) {
            return;
        }

        // Validate target has the tab group
        let tgt_layout = match self.data.projects[tgt_idx].layout.as_ref() {
            Some(l) => l,
            None => return,
        };

        // Find a reference terminal in the target tab group
        let reference_tid = match tgt_layout.get_at_path(tabs_path) {
            Some(node) => {
                let ids = node.collect_terminal_ids();
                ids.into_iter().find(|id| id != terminal_id)
            }
            None => return,
        };
        let reference_tid = match reference_tid {
            Some(id) => id,
            None => return,
        };

        // --- Extract from source ---
        let src_project = &mut self.data.projects[src_idx];
        if source_path.is_empty() {
            src_project.layout = None;
        } else if let Some(src_layout) = src_project.layout.as_mut() {
            if src_layout.remove_at_path(&source_path).is_none() {
                return;
            }
            src_layout.normalize();
        } else {
            // Layout was validated Some above; disappearing is a bug, not a crash.
            return;
        }

        // Migrate metadata
        let terminal_name = src_project.terminal_names.remove(terminal_id);
        let hidden_state = src_project.hidden_terminals.remove(terminal_id);

        // Cleanup orphaned source metadata
        let src_layout_ids: std::collections::HashSet<String> = src_project.layout.as_ref()
            .map(|l| l.collect_terminal_ids().into_iter().collect())
            .unwrap_or_default();
        src_project.terminal_names.retain(|id, _| src_layout_ids.contains(id));
        src_project.hidden_terminals.retain(|id, _| src_layout_ids.contains(id));

        // --- Insert into target ---
        let tgt_project = &mut self.data.projects[tgt_idx];

        if let Some(name) = terminal_name {
            tgt_project.terminal_names.insert(terminal_id.to_string(), name);
        }
        if let Some(hidden) = hidden_state {
            tgt_project.hidden_terminals.insert(terminal_id.to_string(), hidden);
        }

        let new_focus_path = if let Some(ref mut tgt_layout) = tgt_project.layout {
            // Re-find the tabs container via the reference terminal
            let ref_path = match tgt_layout.find_terminal_path(&reference_tid) {
                Some(p) => p,
                None => return,
            };
            let new_tabs_path = if ref_path.is_empty() {
                return;
            } else {
                ref_path[..ref_path.len() - 1].to_vec()
            };

            let tabs_node = match tgt_layout.get_at_path_mut(&new_tabs_path) {
                Some(node) => node,
                None => return,
            };

            if let LayoutNode::Tabs { children, active_tab } = tabs_node {
                let idx = insert_index.unwrap_or(children.len());
                let clamped = idx.min(children.len());
                children.insert(clamped, source_node);
                *active_tab = clamped;
            } else {
                return;
            }

            tgt_layout.normalize();
            tgt_layout.find_terminal_path(terminal_id)
        } else {
            // Target has no layout — set source node as root
            let root = source_node;
            let path = root.find_terminal_path(terminal_id);
            tgt_project.layout = Some(root);
            path
        };

        self.notify_data(cx);

        if let Some(new_path) = new_focus_path {
            self.set_focused_terminal(focus_manager, target_project_id.to_string(), new_path, cx);
        }
    }
}
