//! Layout manipulation workspace actions
//!
//! Actions for splitting, tabs, and closing terminals within layouts.

use crate::workspace::state::{LayoutNode, SplitDirection, Workspace};
use gpui::*;

impl Workspace {
    /// Split a terminal at a path
    pub fn split_terminal(
        &mut self,
        project_id: &str,
        path: &[usize],
        direction: SplitDirection,
        cx: &mut Context<Self>,
    ) {
        log::info!("Workspace::split_terminal called for project {} at path {:?}", project_id, path);
        self.with_layout_node(project_id, path, cx, |node| {
            log::info!("Found node at path, splitting...");
            let old_node = node.clone();
            *node = LayoutNode::Split {
                direction,
                sizes: vec![50.0, 50.0],
                children: vec![old_node, LayoutNode::new_terminal()],
            };
            log::info!("Split complete");
            true
        });

        // Normalize to flatten if parent was already a same-direction split
        if let Some(project) = self.project_mut(project_id) {
            if let Some(ref mut layout) = project.layout {
                layout.normalize();
            }
        }
    }

    /// Add a new tab - either to existing tab group (if parent is Tabs) or create new tab group
    pub fn add_tab(
        &mut self,
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
                        self.add_tab_to_group(project_id, parent_path, cx);
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
    }

    /// Add a new tab to an existing Tabs container
    pub fn add_tab_to_group(
        &mut self,
        project_id: &str,
        tabs_path: &[usize],
        cx: &mut Context<Self>,
    ) {
        self.with_layout_node(project_id, tabs_path, cx, |node| {
            if let LayoutNode::Tabs { children, active_tab } = node {
                children.push(LayoutNode::new_terminal());
                *active_tab = children.len() - 1;
                log::info!("Added new tab to existing group, now {} tabs", children.len());
                true
            } else {
                false
            }
        });
    }

    /// Close a terminal at a path
    pub fn close_terminal(&mut self, project_id: &str, path: &[usize], cx: &mut Context<Self>) {
        if let Some(project) = self.project_mut(project_id) {
            if let Some(ref mut layout) = project.layout {
                if path.is_empty() {
                    // Closing root - remove layout entirely (project becomes bookmark)
                    project.layout = None;
                    self.notify_data(cx);
                    return;
                }

                let parent_path = &path[..path.len() - 1];
                let child_index = path[path.len() - 1];

                if let Some(parent) = layout.get_at_path_mut(parent_path) {
                    match parent {
                        LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                            if children.len() <= 2 {
                                // Replace parent with remaining child
                                let remaining_index = if child_index == 0 { 1 } else { 0 };
                                if let Some(remaining) = children.get(remaining_index).cloned() {
                                    *parent = remaining;
                                }
                            } else {
                                // Just remove the child
                                children.remove(child_index);
                            }
                            self.notify_data(cx);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Close a terminal and focus its sibling (reverse of splitting)
    /// Focuses the previous sibling, or the next one if closing the first child
    pub fn close_terminal_and_focus_sibling(&mut self, project_id: &str, path: &[usize], cx: &mut Context<Self>) {
        if path.is_empty() {
            // Closing root - remove layout (project becomes bookmark)
            self.close_terminal(project_id, path, cx);
            // Clear focused terminal since there's nothing to focus
            self.focus_manager.clear_focus();
            return;
        }

        // Calculate the sibling to focus before closing
        let focus_path = if let Some(project) = self.project(project_id) {
            if let Some(ref layout) = project.layout {
                let parent_path = &path[..path.len() - 1];
                let child_index = path[path.len() - 1];

                if let Some(parent) = layout.get_at_path(parent_path) {
                    match parent {
                        LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
                            if children.len() <= 2 {
                                // Parent will dissolve - sibling moves to parent_path
                                let sibling_index = if child_index == 0 { 1 } else { 0 };
                                if let Some(sibling) = children.get(sibling_index) {
                                    // Find first terminal within the sibling
                                    let relative_path = sibling.find_first_terminal_path();
                                    let mut full_path = parent_path.to_vec();
                                    full_path.extend(relative_path);
                                    Some(full_path)
                                } else {
                                    Some(parent_path.to_vec())
                                }
                            } else {
                                // Parent keeps multiple children
                                // Focus previous sibling, or next if closing first
                                let sibling_index = if child_index > 0 { child_index - 1 } else { 1 };
                                if let Some(sibling) = children.get(sibling_index) {
                                    let relative_path = sibling.find_first_terminal_path();
                                    let mut full_path = parent_path.to_vec();
                                    full_path.push(sibling_index);
                                    full_path.extend(relative_path);
                                    // Adjust index if sibling comes after closed terminal
                                    if sibling_index > child_index {
                                        full_path[parent_path.len()] -= 1;
                                    }
                                    Some(full_path)
                                } else {
                                    None
                                }
                            }
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        // Close the terminal
        self.close_terminal(project_id, path, cx);

        // Focus the sibling
        if let Some(focus_path) = focus_path {
            self.set_focused_terminal(project_id.to_string(), focus_path, cx);
        }
    }

    /// Update split sizes at a path
    pub fn update_split_sizes(
        &mut self,
        project_id: &str,
        path: &[usize],
        new_sizes: Vec<f32>,
        cx: &mut Context<Self>,
    ) {
        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Split { sizes, .. } = node {
                *sizes = new_sizes;
                true
            } else {
                false
            }
        });
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

    /// Close a tab at a specific index within a tabs container
    pub fn close_tab(
        &mut self,
        project_id: &str,
        path: &[usize],
        tab_index: usize,
        cx: &mut Context<Self>,
    ) {
        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Tabs { children, active_tab } = node {
                if tab_index >= children.len() {
                    return false;
                }

                if children.len() <= 1 {
                    // Can't close the last tab
                    return false;
                }

                // Remove the tab
                children.remove(tab_index);

                // If only one tab remains, dissolve the tab group
                if children.len() == 1 {
                    *node = children.remove(0);
                    return true;
                }

                // Adjust active_tab index
                if *active_tab >= children.len() {
                    *active_tab = children.len().saturating_sub(1);
                } else if tab_index < *active_tab {
                    *active_tab = active_tab.saturating_sub(1);
                }

                true
            } else {
                false
            }
        });
    }

    /// Close all tabs except the one at the specified index
    pub fn close_other_tabs(
        &mut self,
        project_id: &str,
        path: &[usize],
        keep_index: usize,
        cx: &mut Context<Self>,
    ) {
        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Tabs { children, .. } = node {
                if keep_index >= children.len() {
                    return false;
                }

                // Keep only the tab at keep_index and dissolve the tab group
                let kept_tab = children[keep_index].clone();
                *node = kept_tab;

                true
            } else {
                false
            }
        });
    }

    /// Close all tabs to the right of the specified index
    pub fn close_tabs_to_right(
        &mut self,
        project_id: &str,
        path: &[usize],
        from_index: usize,
        cx: &mut Context<Self>,
    ) {
        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Tabs { children, active_tab } = node {
                if from_index >= children.len() {
                    return false;
                }

                // Remove all tabs after from_index
                children.truncate(from_index + 1);

                // If only one tab remains, dissolve the tab group
                if children.len() == 1 {
                    *node = children.remove(0);
                    return true;
                }

                // Adjust active_tab if it was to the right
                if *active_tab >= children.len() {
                    *active_tab = children.len().saturating_sub(1);
                }

                true
            } else {
                false
            }
        });
    }

    /// Close all terminals in a project
    /// Returns the list of terminal IDs that were closed (for PTY cleanup)
    /// The project becomes a bookmark (no terminals) after this operation
    #[allow(dead_code)]
    pub fn close_all_terminals(&mut self, project_id: &str, cx: &mut Context<Self>) -> Vec<String> {
        let terminal_ids = if let Some(project) = self.project(project_id) {
            project.layout.as_ref()
                .map(|l| l.collect_terminal_ids())
                .unwrap_or_default()
        } else {
            return vec![];
        };

        // Clear the layout entirely (project becomes a bookmark)
        if let Some(project) = self.project_mut(project_id) {
            project.layout = None;
            // Clear terminal names for removed terminals
            for tid in &terminal_ids {
                project.terminal_names.remove(tid);
                project.hidden_terminals.remove(tid);
            }
        }

        // Clear focused terminal if it was in this project
        if let Some(ref focused) = self.focus_manager.focused_terminal_state() {
            if focused.project_id == project_id {
                self.focus_manager.clear_focus();
            }
        }

        // Exit fullscreen if a terminal from this project was in fullscreen
        if self.focus_manager.fullscreen_project_id() == Some(project_id) {
            self.focus_manager.exit_fullscreen();
        }

        // Remove any detached terminals from this project
        self.detached_terminals.retain(|d| d.project_id != project_id);

        self.notify_data(cx);
        terminal_ids
    }
}
