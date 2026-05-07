//! Tab group operations: add, set-active, reorder.

use crate::focus::FocusManager;
use crate::state::{LayoutNode, Workspace};
use gpui::*;

impl Workspace {
    /// Add a new tab - either to existing tab group (if parent is Tabs) or create new tab group
    pub fn add_tab(
        &mut self,
        focus_manager: &mut FocusManager,
        project_id: &str,
        path: &[usize],
        cx: &mut Context<Self>,
    ) {
        log::info!("Workspace::add_tab called for project {} at path {:?}", project_id, path);

        // Check if parent is a Tabs container
        if path.len() >= 1 {
            let parent_path = &path[..path.len() - 1];
            if let Some(project) = self.project(project_id) {
                if let Some(ref layout) = project.layout {
                    if let Some(LayoutNode::Tabs { .. }) = layout.get_at_path(parent_path) {
                        // Parent is Tabs - add new tab to the group
                        self.add_tab_to_group(focus_manager, project_id, parent_path, cx);
                        return;
                    }
                }
            }
        }

        // Parent is not Tabs - create new tab group
        self.with_layout_node(project_id, path, cx, |node| {
            let old_node = node.clone();
            *node = LayoutNode::Tabs {
                children: vec![old_node, LayoutNode::new_terminal()],
                active_tab: 1,
            };
            log::info!("Created new tab group");
            true
        });

        // Focus the new tab
        let mut new_path = path.to_vec();
        new_path.push(1);
        self.set_focused_terminal(focus_manager, project_id.to_string(), new_path, cx);
    }

    /// Add a new tab to an existing Tabs container
    pub fn add_tab_to_group(
        &mut self,
        focus_manager: &mut FocusManager,
        project_id: &str,
        tabs_path: &[usize],
        cx: &mut Context<Self>,
    ) {
        let mut new_tab_index = 0;
        self.with_layout_node(project_id, tabs_path, cx, |node| {
            if let LayoutNode::Tabs { children, active_tab } = node {
                children.push(LayoutNode::new_terminal());
                *active_tab = children.len() - 1;
                new_tab_index = *active_tab;
                log::info!("Added new tab to existing group, now {} tabs", children.len());
                true
            } else {
                false
            }
        });

        // Focus the new tab
        let mut new_path = tabs_path.to_vec();
        new_path.push(new_tab_index);
        self.set_focused_terminal(focus_manager, project_id.to_string(), new_path, cx);
    }

    /// Set active tab in a tabs container
    pub fn set_active_tab(
        &mut self,
        project_id: &str,
        path: &[usize],
        tab_index: usize,
        cx: &mut Context<Self>,
    ) {
        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Tabs { active_tab, .. } = node {
                *active_tab = tab_index;
                true
            } else {
                false
            }
        });
    }

    /// Move a tab from one position to another within a tabs container
    pub fn move_tab(
        &mut self,
        project_id: &str,
        path: &[usize],
        from_index: usize,
        to_index: usize,
        cx: &mut Context<Self>,
    ) {
        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Tabs { children, active_tab } = node {
                if from_index >= children.len() || to_index >= children.len() {
                    return false;
                }
                if from_index == to_index {
                    return false;
                }

                // Remove the tab from its current position
                let tab = children.remove(from_index);

                // Clamp target index to valid range after removal
                let target = to_index.min(children.len());

                // Insert at new position
                children.insert(target, tab);

                // Update active_tab index to follow the moved tab if it was active
                if *active_tab == from_index {
                    *active_tab = target;
                } else if from_index < *active_tab && target >= *active_tab {
                    // Active tab shifted left
                    *active_tab = active_tab.saturating_sub(1);
                } else if from_index > *active_tab && target <= *active_tab {
                    // Active tab shifted right
                    *active_tab = (*active_tab + 1).min(children.len().saturating_sub(1));
                }

                true
            } else {
                false
            }
        });
    }
}
