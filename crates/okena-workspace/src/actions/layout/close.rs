//! Terminal and tab close operations.

use crate::focus::FocusManager;
use crate::state::{LayoutNode, Workspace};
use gpui::*;

impl Workspace {
    /// Close a terminal at a path.
    /// Returns the terminal IDs that were removed from the layout.
    pub fn close_terminal(&mut self, project_id: &str, path: &[usize], cx: &mut Context<Self>) -> Vec<String> {
        if let Some(project) = self.project_mut(project_id)
            && let Some(ref mut layout) = project.layout {
                if path.is_empty() {
                    // Closing root - remove layout entirely (project becomes bookmark)
                    project.layout = None;
                    self.notify_data(cx);
                    return self.cleanup_orphaned_metadata(project_id);
                }

                // Focus-preservation: if the parent is a Tabs container and we're
                // closing a tab *before* the active one, the active tab shifts left
                // by one (its content moves to index active_tab - 1). Capture the
                // original active_tab here so we can set the new index absolutely
                // after removal — we must NOT decrement the post-removal value,
                // because remove_at_path already clamps active_tab when it was the
                // last tab, and an additional decrement would overshoot by one.
                let (parent_path, child_index) = path.split_at(path.len() - 1);
                let child_index = child_index[0];
                let prev_active_tab = match layout.get_at_path(parent_path) {
                    Some(LayoutNode::Tabs { active_tab, .. }) if child_index < *active_tab => {
                        Some(*active_tab)
                    }
                    _ => None,
                };

                // Delegate the tree mutation (remove child, collapse a parent left
                // with a single child, clamp active_tab) to the shared, unit-tested
                // LayoutNode::remove_at_path. It returns the removed node, or None
                // for an invalid path (out-of-range index / Terminal parent).
                if layout.remove_at_path(path).is_some() {
                    // After removal the parent may have collapsed (single child left)
                    // or no longer be a Tabs — skip those; only adjust a surviving Tabs.
                    if let Some(prev_active_tab) = prev_active_tab
                        && let Some(LayoutNode::Tabs { children, active_tab }) =
                            layout.get_at_path_mut(parent_path)
                    {
                        *active_tab = (prev_active_tab - 1).min(children.len().saturating_sub(1));
                    }
                    self.notify_data(cx);
                    return self.cleanup_orphaned_metadata(project_id);
                }
            }
        vec![]
    }

    /// Close a terminal and focus its sibling (reverse of splitting).
    /// Returns the terminal IDs that were removed from the layout.
    pub fn close_terminal_and_focus_sibling(&mut self, focus_manager: &mut FocusManager, project_id: &str, path: &[usize], cx: &mut Context<Self>) -> Vec<String> {
        if path.is_empty() {
            // Closing root - remove layout (project becomes bookmark)
            let removed = self.close_terminal(project_id, path, cx);
            // Clear focused terminal since there's nothing to focus
            focus_manager.clear_focus();
            return removed;
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
        let removed = self.close_terminal(project_id, path, cx);

        // Focus the sibling
        if let Some(focus_path) = focus_path {
            self.set_focused_terminal(focus_manager, project_id.to_string(), focus_path, cx);
        }

        removed
    }

    /// Close a tab at a specific index within a tabs container.
    /// Returns the terminal IDs that were removed.
    #[allow(dead_code)]
    pub fn close_tab(
        &mut self,
        project_id: &str,
        path: &[usize],
        tab_index: usize,
        cx: &mut Context<Self>,
    ) -> Vec<String> {
        let applied = self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Tabs { children, active_tab } = node {
                if tab_index >= children.len() || children.len() <= 1 {
                    return false;
                }

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

        if applied { self.cleanup_orphaned_metadata(project_id) } else { vec![] }
    }

    /// Close all tabs except the one at the specified index.
    /// Returns the terminal IDs that were removed.
    #[allow(dead_code)]
    pub fn close_other_tabs(
        &mut self,
        project_id: &str,
        path: &[usize],
        keep_index: usize,
        cx: &mut Context<Self>,
    ) -> Vec<String> {
        let applied = self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Tabs { children, .. } = node {
                if keep_index >= children.len() {
                    return false;
                }

                let kept_tab = children[keep_index].clone();
                *node = kept_tab;
                true
            } else {
                false
            }
        });

        if applied { self.cleanup_orphaned_metadata(project_id) } else { vec![] }
    }

    /// Close all tabs to the right of the specified index.
    /// Returns the terminal IDs that were removed.
    #[allow(dead_code)]
    pub fn close_tabs_to_right(
        &mut self,
        project_id: &str,
        path: &[usize],
        from_index: usize,
        cx: &mut Context<Self>,
    ) -> Vec<String> {
        let applied = self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Tabs { children, active_tab } = node {
                if from_index >= children.len() {
                    return false;
                }

                children.truncate(from_index + 1);

                if children.len() == 1 {
                    *node = children.remove(0);
                    return true;
                }

                if *active_tab >= children.len() {
                    *active_tab = children.len().saturating_sub(1);
                }

                true
            } else {
                false
            }
        });

        if applied { self.cleanup_orphaned_metadata(project_id) } else { vec![] }
    }
}
