//! Layout manipulation workspace actions
//!
//! Actions for splitting, tabs, and closing terminals within layouts.

use crate::views::layout::pane_drag::DropZone;
use crate::workspace::state::{LayoutNode, SplitDirection, Workspace};
use gpui::*;

impl Workspace {
    /// Remove terminal_names/hidden_terminals entries that are no longer in the layout.
    /// Returns the orphaned terminal IDs (for PTY cleanup by callers).
    fn cleanup_orphaned_metadata(&mut self, project_id: &str) -> Vec<String> {
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
    /// Split a terminal at a path
    pub fn split_terminal(
        &mut self,
        project_id: &str,
        path: &[usize],
        direction: SplitDirection,
        cx: &mut Context<Self>,
    ) {
        log::info!("Workspace::split_terminal called for project {} at path {:?}", project_id, path);

        // Perform the split and find the new terminal's path after normalization.
        let new_path = if let Some(project) = self.project_mut(project_id) {
            if let Some(ref mut layout) = project.layout {
                if let Some(node) = layout.get_at_path_mut(path) {
                    log::info!("Found node at path, splitting...");
                    let old_node = node.clone();
                    *node = LayoutNode::Split {
                        direction,
                        sizes: vec![50.0, 50.0],
                        children: vec![old_node, LayoutNode::new_terminal()],
                    };
                    layout.normalize();
                    log::info!("Split complete");
                    // The newly created terminal has terminal_id: None — find its path
                    layout.find_uninitialized_terminal_path()
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        self.notify_data(cx);

        if let Some(new_path) = new_path {
            self.set_focused_terminal(project_id.to_string(), new_path, cx);
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

    /// Close a terminal at a path.
    /// Returns the terminal IDs that were removed from the layout.
    pub fn close_terminal(&mut self, project_id: &str, path: &[usize], cx: &mut Context<Self>) -> Vec<String> {
        if let Some(project) = self.project_mut(project_id) {
            if let Some(ref mut layout) = project.layout {
                if path.is_empty() {
                    // Closing root - remove layout entirely (project becomes bookmark)
                    project.layout = None;
                    self.notify_data(cx);
                    return self.cleanup_orphaned_metadata(project_id);
                }

                let parent_path = &path[..path.len() - 1];
                let child_index = path[path.len() - 1];

                if let Some(parent) = layout.get_at_path_mut(parent_path) {
                    match parent {
                        LayoutNode::Split { children, sizes, .. } => {
                            if children.len() <= 2 {
                                let remaining_index = if child_index == 0 { 1 } else { 0 };
                                if let Some(remaining) = children.get(remaining_index).cloned() {
                                    *parent = remaining;
                                }
                            } else {
                                children.remove(child_index);
                                if child_index < sizes.len() {
                                    sizes.remove(child_index);
                                }
                            }
                            self.notify_data(cx);
                            return self.cleanup_orphaned_metadata(project_id);
                        }
                        LayoutNode::Tabs { children, .. } => {
                            if children.len() <= 2 {
                                let remaining_index = if child_index == 0 { 1 } else { 0 };
                                if let Some(remaining) = children.get(remaining_index).cloned() {
                                    *parent = remaining;
                                }
                            } else {
                                children.remove(child_index);
                            }
                            self.notify_data(cx);
                            return self.cleanup_orphaned_metadata(project_id);
                        }
                        LayoutNode::Grid { children, rows, cols, .. } => {
                            if child_index < children.len() {
                                // Replace with empty terminal (grid keeps dimensions)
                                children[child_index] = LayoutNode::new_terminal();
                                // Collapse if 1×1
                                if *rows == 1 && *cols == 1 {
                                    let remaining = children.remove(0);
                                    *parent = remaining;
                                }
                            }
                            self.notify_data(cx);
                            return self.cleanup_orphaned_metadata(project_id);
                        }
                        _ => {}
                    }
                }
            }
        }
        vec![]
    }

    /// Close a terminal and focus its sibling (reverse of splitting).
    /// Returns the terminal IDs that were removed from the layout.
    pub fn close_terminal_and_focus_sibling(&mut self, project_id: &str, path: &[usize], cx: &mut Context<Self>) -> Vec<String> {
        if path.is_empty() {
            // Closing root - remove layout (project becomes bookmark)
            let removed = self.close_terminal(project_id, path, cx);
            // Clear focused terminal since there's nothing to focus
            self.focus_manager.clear_focus();
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
                        LayoutNode::Grid { children, cols, .. } => {
                            // Grid: cell gets replaced with empty terminal, focus neighbor
                            // Prefer same-row neighbor, else adjacent row
                            let c = *cols;
                            let row = child_index / c;
                            let col = child_index % c;
                            // Try right neighbor in same row, then left, then next row, then prev row
                            let candidates = [
                                if col + 1 < c { Some(row * c + col + 1) } else { None },
                                if col > 0 { Some(row * c + col - 1) } else { None },
                                if child_index + c < children.len() { Some(child_index + c) } else { None },
                                if child_index >= c { Some(child_index - c) } else { None },
                            ];
                            let sibling_idx = candidates.iter().flatten().next().copied()
                                .unwrap_or(0);
                            if let Some(sibling) = children.get(sibling_idx) {
                                let relative_path = sibling.find_first_terminal_path();
                                let mut full_path = parent_path.to_vec();
                                full_path.push(sibling_idx);
                                full_path.extend(relative_path);
                                Some(full_path)
                            } else {
                                None
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
            self.set_focused_terminal(project_id.to_string(), focus_path, cx);
        }

        removed
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

    /// Close a tab at a specific index within a tabs container.
    /// Returns the terminal IDs that were removed.
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
    /// Move a terminal pane to a new position relative to a target terminal.
    ///
    /// Extracts the source terminal from its current position and inserts it
    /// next to the target based on the drop zone (Top/Bottom/Left/Right/Center).
    pub fn move_pane(
        &mut self,
        source_project_id: &str,
        source_terminal_id: &str,
        target_project_id: &str,
        target_terminal_id: &str,
        zone: DropZone,
        cx: &mut Context<Self>,
    ) {
        // v1: same-project moves only
        if source_project_id != target_project_id {
            return;
        }
        // Self-drop check
        if source_terminal_id == target_terminal_id {
            return;
        }

        let project_id = source_project_id;

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

            let wrapper = match zone {
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
            };

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
            self.set_focused_terminal(project_id.to_string(), new_path, cx);
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
    pub fn move_terminal_to_tab_group(
        &mut self,
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
            self.set_focused_terminal(project_id.to_string(), new_path, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::workspace::state::{LayoutNode, SplitDirection};
    use crate::terminal::shell_config::ShellType;

    fn terminal_node(id: &str) -> LayoutNode {
        LayoutNode::Terminal {
            terminal_id: Some(id.to_string()),
            minimized: false,
            detached: false,
            shell_type: ShellType::Default,
            zoom_level: 1.0,
        }
    }

    /// Simulate split_terminal: replace a node with a Split containing it + new terminal
    fn simulate_split(node: &mut LayoutNode, direction: SplitDirection) {
        let old_node = node.clone();
        *node = LayoutNode::Split {
            direction,
            sizes: vec![50.0, 50.0],
            children: vec![old_node, LayoutNode::new_terminal()],
        };
        node.normalize();
    }

    /// Simulate add_tab: replace a node with a Tabs containing it + new terminal
    fn simulate_add_tab(node: &mut LayoutNode) {
        let old_node = node.clone();
        *node = LayoutNode::Tabs {
            children: vec![old_node, LayoutNode::new_terminal()],
            active_tab: 1,
        };
    }

    /// Simulate close_terminal: remove child at index, replacing parent with sibling if 2 children
    fn simulate_close(layout: &mut LayoutNode, path: &[usize]) -> bool {
        if path.is_empty() {
            return false; // would set layout to None in real code
        }
        let parent_path = &path[..path.len() - 1];
        let child_index = path[path.len() - 1];

        if let Some(parent) = layout.get_at_path_mut(parent_path) {
            match parent {
                LayoutNode::Split { children, sizes, .. } => {
                    if children.len() <= 2 {
                        let remaining_index = if child_index == 0 { 1 } else { 0 };
                        if let Some(remaining) = children.get(remaining_index).cloned() {
                            *parent = remaining;
                            return true;
                        }
                    } else {
                        children.remove(child_index);
                        if child_index < sizes.len() {
                            sizes.remove(child_index);
                        }
                        return true;
                    }
                }
                LayoutNode::Tabs { children, .. } => {
                    if children.len() <= 2 {
                        let remaining_index = if child_index == 0 { 1 } else { 0 };
                        if let Some(remaining) = children.get(remaining_index).cloned() {
                            *parent = remaining;
                            return true;
                        }
                    } else {
                        children.remove(child_index);
                        return true;
                    }
                }
                LayoutNode::Grid { children, rows, cols, .. } => {
                    if child_index < children.len() {
                        children[child_index] = LayoutNode::new_terminal();
                        if *rows == 1 && *cols == 1 {
                            let remaining = children.remove(0);
                            *parent = remaining;
                        }
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }

    #[test]
    fn test_split_terminal_creates_split() {
        let mut layout = terminal_node("t1");
        simulate_split(&mut layout, SplitDirection::Vertical);

        match &layout {
            LayoutNode::Split { direction, children, sizes } => {
                assert_eq!(*direction, SplitDirection::Vertical);
                assert_eq!(children.len(), 2);
                assert_eq!(sizes.len(), 2);
                assert!(matches!(&children[0], LayoutNode::Terminal { terminal_id: Some(id), .. } if id == "t1"));
                assert!(matches!(&children[1], LayoutNode::Terminal { terminal_id: None, .. }));
            }
            _ => panic!("Expected split"),
        }
    }

    #[test]
    fn test_nested_split_normalizes() {
        // Split a terminal that's already inside a split of the same direction
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![50.0, 50.0],
            children: vec![terminal_node("t1"), terminal_node("t2")],
        };
        // Split t1 horizontally — should flatten
        if let Some(node) = layout.get_at_path_mut(&[0]) {
            simulate_split(node, SplitDirection::Horizontal);
        }
        layout.normalize();

        match &layout {
            LayoutNode::Split { direction, children, .. } => {
                assert_eq!(*direction, SplitDirection::Horizontal);
                // Should be flattened to 3 children
                assert_eq!(children.len(), 3);
            }
            _ => panic!("Expected flattened split"),
        }
    }

    #[test]
    fn test_add_tab_creates_tab_group() {
        let mut layout = terminal_node("t1");
        simulate_add_tab(&mut layout);

        match &layout {
            LayoutNode::Tabs { children, active_tab } => {
                assert_eq!(children.len(), 2);
                assert_eq!(*active_tab, 1);
            }
            _ => panic!("Expected tabs"),
        }
    }

    #[test]
    fn test_add_tab_to_existing_tabs() {
        let mut layout = LayoutNode::Tabs {
            children: vec![terminal_node("t1"), terminal_node("t2")],
            active_tab: 0,
        };
        if let LayoutNode::Tabs { children, active_tab } = &mut layout {
            children.push(LayoutNode::new_terminal());
            *active_tab = children.len() - 1;
        }
        match &layout {
            LayoutNode::Tabs { children, active_tab } => {
                assert_eq!(children.len(), 3);
                assert_eq!(*active_tab, 2);
            }
            _ => panic!("Expected tabs"),
        }
    }

    #[test]
    fn test_close_terminal_sibling_replaces_parent() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![50.0, 50.0],
            children: vec![terminal_node("t1"), terminal_node("t2")],
        };
        simulate_close(&mut layout, &[0]);
        assert!(matches!(&layout, LayoutNode::Terminal { terminal_id: Some(id), .. } if id == "t2"));
    }

    #[test]
    fn test_close_terminal_from_3_child_split() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![33.0, 33.0, 34.0],
            children: vec![terminal_node("t1"), terminal_node("t2"), terminal_node("t3")],
        };
        simulate_close(&mut layout, &[1]);
        match &layout {
            LayoutNode::Split { children, sizes, .. } => {
                assert_eq!(children.len(), 2);
                assert_eq!(sizes.len(), 2);
                // t1 and t3 remain
                let ids: Vec<_> = children.iter().map(|c| match c {
                    LayoutNode::Terminal { terminal_id: Some(id), .. } => id.as_str(),
                    _ => "",
                }).collect();
                assert_eq!(ids, vec!["t1", "t3"]);
            }
            _ => panic!("Expected split with 2 children"),
        }
    }

    #[test]
    fn test_close_terminal_from_3_child_sizes_consistent() {
        // Verify that closing a child from a 3-child split keeps sizes in sync
        // and that the remaining sizes sum correctly
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![25.0, 50.0, 25.0],
            children: vec![terminal_node("t1"), terminal_node("t2"), terminal_node("t3")],
        };

        // Close the middle terminal (index 1, size 50.0)
        simulate_close(&mut layout, &[1]);
        match &layout {
            LayoutNode::Split { children, sizes, .. } => {
                assert_eq!(children.len(), 2);
                assert_eq!(sizes.len(), 2);
                // Sizes should be [25.0, 25.0] — the middle entry was removed
                assert_eq!(sizes, &vec![25.0, 25.0]);
            }
            _ => panic!("Expected split with 2 children"),
        }

        // Close the first terminal (index 0) — should collapse to single terminal
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            sizes: vec![30.0, 40.0, 30.0],
            children: vec![terminal_node("t1"), terminal_node("t2"), terminal_node("t3")],
        };
        simulate_close(&mut layout, &[0]);
        match &layout {
            LayoutNode::Split { children, sizes, .. } => {
                assert_eq!(children.len(), 2);
                assert_eq!(sizes.len(), 2);
                assert_eq!(sizes, &vec![40.0, 30.0]);
            }
            _ => panic!("Expected split with 2 children"),
        }
    }

    #[test]
    fn test_move_tab() {
        let mut layout = LayoutNode::Tabs {
            children: vec![terminal_node("t1"), terminal_node("t2"), terminal_node("t3")],
            active_tab: 0,
        };
        // Move tab at index 0 to index 2
        if let LayoutNode::Tabs { children, active_tab } = &mut layout {
            let tab = children.remove(0);
            children.insert(2.min(children.len()), tab);
            // active_tab was 0, which was the moved tab, so update
            *active_tab = 2.min(children.len() - 1);
        }
        match &layout {
            LayoutNode::Tabs { children, active_tab } => {
                let ids: Vec<_> = children.iter().map(|c| match c {
                    LayoutNode::Terminal { terminal_id: Some(id), .. } => id.as_str(),
                    _ => "",
                }).collect();
                assert_eq!(ids, vec!["t2", "t3", "t1"]);
                assert_eq!(*active_tab, 2);
            }
            _ => panic!("Expected tabs"),
        }
    }
}

#[cfg(test)]
mod gpui_tests {
    use gpui::AppContext as _;
    use crate::views::layout::pane_drag::DropZone;
    use crate::workspace::state::{LayoutNode, ProjectData, SplitDirection, Workspace, WorkspaceData};
    use crate::workspace::settings::HooksConfig;
    use crate::terminal::shell_config::ShellType;
    use crate::theme::FolderColor;
    use std::collections::HashMap;

    fn make_project(id: &str) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: format!("Project {}", id),
            path: "/tmp/test".to_string(),
            is_visible: true,
            layout: Some(LayoutNode::Terminal {
                terminal_id: Some(format!("term_{}", id)),
                minimized: false,
                detached: false,
                shell_type: ShellType::Default,
                zoom_level: 1.0,
            }),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
        }
    }

    fn make_workspace_data(projects: Vec<ProjectData>, order: Vec<&str>) -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects,
            project_order: order.into_iter().map(String::from).collect(),
            project_widths: HashMap::new(),
            folders: vec![],
        }
    }

    #[gpui::test]
    fn test_split_terminal_gpui(cx: &mut gpui::TestAppContext) {
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let v0 = workspace.read_with(cx, |ws: &Workspace, _cx| ws.data_version());

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.split_terminal("p1", &[], SplitDirection::Vertical, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(ws.data_version() > v0);
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            match layout {
                LayoutNode::Split { direction, children, .. } => {
                    assert_eq!(*direction, SplitDirection::Vertical);
                    assert_eq!(children.len(), 2);
                    // First child should be the original terminal
                    assert!(matches!(&children[0], LayoutNode::Terminal { terminal_id: Some(id), .. } if id == "term_p1"));
                    // Second child should be a new terminal
                    assert!(matches!(&children[1], LayoutNode::Terminal { terminal_id: None, .. }));
                }
                _ => panic!("Expected split after split_terminal"),
            }
        });
    }

    #[gpui::test]
    fn test_add_tab_gpui(cx: &mut gpui::TestAppContext) {
        let data = make_workspace_data(vec![make_project("p1")], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.add_tab("p1", &[], cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            match layout {
                LayoutNode::Tabs { children, active_tab } => {
                    assert_eq!(children.len(), 2);
                    assert_eq!(*active_tab, 1);
                }
                _ => panic!("Expected tabs after add_tab"),
            }
        });
    }

    #[gpui::test]
    fn test_close_terminal_gpui(cx: &mut gpui::TestAppContext) {
        // Create a project with a 2-child split
        let mut project = make_project("p1");
        project.layout = Some(LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![50.0, 50.0],
            children: vec![
                LayoutNode::Terminal {
                    terminal_id: Some("t1".to_string()),
                    minimized: false,
                    detached: false,
                    shell_type: ShellType::Default,
                    zoom_level: 1.0,
                },
                LayoutNode::Terminal {
                    terminal_id: Some("t2".to_string()),
                    minimized: false,
                    detached: false,
                    shell_type: ShellType::Default,
                    zoom_level: 1.0,
                },
            ],
        });
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.close_terminal("p1", &[0], cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            // After closing child 0, sibling (t2) should replace the split
            assert!(matches!(layout, LayoutNode::Terminal { terminal_id: Some(id), .. } if id == "t2"));
        });
    }

    #[gpui::test]
    fn test_close_tab_gpui(cx: &mut gpui::TestAppContext) {
        let mut project = make_project("p1");
        project.layout = Some(LayoutNode::Tabs {
            children: vec![
                LayoutNode::Terminal {
                    terminal_id: Some("t1".to_string()),
                    minimized: false,
                    detached: false,
                    shell_type: ShellType::Default,
                    zoom_level: 1.0,
                },
                LayoutNode::Terminal {
                    terminal_id: Some("t2".to_string()),
                    minimized: false,
                    detached: false,
                    shell_type: ShellType::Default,
                    zoom_level: 1.0,
                },
                LayoutNode::Terminal {
                    terminal_id: Some("t3".to_string()),
                    minimized: false,
                    detached: false,
                    shell_type: ShellType::Default,
                    zoom_level: 1.0,
                },
            ],
            active_tab: 2,
        });
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        // Close tab 0
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.close_tab("p1", &[], 0, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            match layout {
                LayoutNode::Tabs { children, active_tab } => {
                    assert_eq!(children.len(), 2);
                    // active_tab was 2, after removing index 0 it should be 1
                    assert_eq!(*active_tab, 1);
                    // Remaining are t2 and t3
                    let ids: Vec<_> = children.iter().filter_map(|c| match c {
                        LayoutNode::Terminal { terminal_id: Some(id), .. } => Some(id.as_str()),
                        _ => None,
                    }).collect();
                    assert_eq!(ids, vec!["t2", "t3"]);
                }
                _ => panic!("Expected tabs"),
            }
        });
    }

    #[gpui::test]
    fn test_move_tab_gpui(cx: &mut gpui::TestAppContext) {
        let mut project = make_project("p1");
        project.layout = Some(LayoutNode::Tabs {
            children: vec![
                LayoutNode::Terminal {
                    terminal_id: Some("t1".to_string()),
                    minimized: false,
                    detached: false,
                    shell_type: ShellType::Default,
                    zoom_level: 1.0,
                },
                LayoutNode::Terminal {
                    terminal_id: Some("t2".to_string()),
                    minimized: false,
                    detached: false,
                    shell_type: ShellType::Default,
                    zoom_level: 1.0,
                },
                LayoutNode::Terminal {
                    terminal_id: Some("t3".to_string()),
                    minimized: false,
                    detached: false,
                    shell_type: ShellType::Default,
                    zoom_level: 1.0,
                },
            ],
            active_tab: 0,
        });
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        // Move tab from index 0 to index 2
        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.move_tab("p1", &[], 0, 2, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            match layout {
                LayoutNode::Tabs { children, active_tab } => {
                    let ids: Vec<_> = children.iter().filter_map(|c| match c {
                        LayoutNode::Terminal { terminal_id: Some(id), .. } => Some(id.as_str()),
                        _ => None,
                    }).collect();
                    assert_eq!(ids, vec!["t2", "t3", "t1"]);
                    assert_eq!(*active_tab, 2); // active_tab was 0 (the moved tab), should follow
                }
                _ => panic!("Expected tabs"),
            }
        });
    }

    // === move_pane tests ===

    fn terminal_node_t(id: &str) -> LayoutNode {
        LayoutNode::Terminal {
            terminal_id: Some(id.to_string()),
            minimized: false,
            detached: false,
            shell_type: ShellType::Default,
            zoom_level: 1.0,
        }
    }

    fn make_project_with_layout(id: &str, layout: LayoutNode) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: format!("Project {}", id),
            path: "/tmp/test".to_string(),
            is_visible: true,
            layout: Some(layout),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
        }
    }

    #[gpui::test]
    fn test_move_pane_left_creates_vertical_split(cx: &mut gpui::TestAppContext) {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            sizes: vec![50.0, 50.0],
            children: vec![terminal_node_t("t1"), terminal_node_t("t2")],
        };
        let project = make_project_with_layout("p1", layout);
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.move_pane("p1", "t1", "p1", "t2", DropZone::Left, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            // t1 dropped on left of t2 -> V[t1, t2] which is same direction as parent,
            // so normalize flattens it back. Result is still V[t1, t2].
            let ids = layout.collect_terminal_ids();
            assert_eq!(ids, vec!["t1", "t2"]);
            match layout {
                LayoutNode::Split { direction, .. } => {
                    assert_eq!(*direction, SplitDirection::Vertical);
                }
                _ => panic!("Expected vertical split"),
            }
        });
    }

    #[gpui::test]
    fn test_move_pane_top_creates_horizontal_split(cx: &mut gpui::TestAppContext) {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            sizes: vec![50.0, 50.0],
            children: vec![terminal_node_t("t1"), terminal_node_t("t2")],
        };
        let project = make_project_with_layout("p1", layout);
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.move_pane("p1", "t1", "p1", "t2", DropZone::Top, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            // t1 removed -> t2 becomes root. t1 dropped on top of t2 -> H[t1, t2]
            match layout {
                LayoutNode::Split { direction, children, .. } => {
                    assert_eq!(*direction, SplitDirection::Horizontal);
                    assert_eq!(children.len(), 2);
                    let ids = layout.collect_terminal_ids();
                    assert_eq!(ids, vec!["t1", "t2"]);
                }
                _ => panic!("Expected horizontal split"),
            }
        });
    }

    #[gpui::test]
    fn test_move_pane_bottom_creates_horizontal_split(cx: &mut gpui::TestAppContext) {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            sizes: vec![50.0, 50.0],
            children: vec![terminal_node_t("t1"), terminal_node_t("t2")],
        };
        let project = make_project_with_layout("p1", layout);
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.move_pane("p1", "t1", "p1", "t2", DropZone::Bottom, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            match layout {
                LayoutNode::Split { direction, children, .. } => {
                    assert_eq!(*direction, SplitDirection::Horizontal);
                    assert_eq!(children.len(), 2);
                    let ids = layout.collect_terminal_ids();
                    // Bottom: target first, then source
                    assert_eq!(ids, vec!["t2", "t1"]);
                }
                _ => panic!("Expected horizontal split"),
            }
        });
    }

    #[gpui::test]
    fn test_move_pane_center_creates_tab_group(cx: &mut gpui::TestAppContext) {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            sizes: vec![50.0, 50.0],
            children: vec![terminal_node_t("t1"), terminal_node_t("t2")],
        };
        let project = make_project_with_layout("p1", layout);
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.move_pane("p1", "t1", "p1", "t2", DropZone::Center, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            match layout {
                LayoutNode::Tabs { children, active_tab } => {
                    assert_eq!(children.len(), 2);
                    assert_eq!(*active_tab, 1);
                    let ids = layout.collect_terminal_ids();
                    assert_eq!(ids, vec!["t2", "t1"]);
                }
                _ => panic!("Expected tabs, got {:?}", layout),
            }
        });
    }

    #[gpui::test]
    fn test_move_pane_self_drop_is_noop(cx: &mut gpui::TestAppContext) {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            sizes: vec![50.0, 50.0],
            children: vec![terminal_node_t("t1"), terminal_node_t("t2")],
        };
        let project = make_project_with_layout("p1", layout);
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let v0 = workspace.read_with(cx, |ws: &Workspace, _cx| ws.data_version());

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.move_pane("p1", "t1", "p1", "t1", DropZone::Top, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            // Version should not have changed
            assert_eq!(ws.data_version(), v0);
        });
    }

    #[gpui::test]
    fn test_move_pane_only_terminal_is_noop(cx: &mut gpui::TestAppContext) {
        // Single terminal - can't move it
        let project = make_project("p1");
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let v0 = workspace.read_with(cx, |ws: &Workspace, _cx| ws.data_version());

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.move_pane("p1", "term_p1", "p1", "term_p1", DropZone::Left, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert_eq!(ws.data_version(), v0);
        });
    }

    // === move_terminal_to_tab_group tests ===

    #[gpui::test]
    fn test_move_terminal_to_tab_group_inserts_at_position(cx: &mut gpui::TestAppContext) {
        // V[Tabs[t1, t2], t3] → move t3 into tabs at index 1 → Tabs[t1, t3, t2]
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            sizes: vec![50.0, 50.0],
            children: vec![
                LayoutNode::Tabs {
                    children: vec![terminal_node_t("t1"), terminal_node_t("t2")],
                    active_tab: 0,
                },
                terminal_node_t("t3"),
            ],
        };
        let project = make_project_with_layout("p1", layout);
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.move_terminal_to_tab_group("p1", "t3", &[0], Some(1), cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            match layout {
                LayoutNode::Tabs { children, active_tab } => {
                    assert_eq!(children.len(), 3);
                    assert_eq!(*active_tab, 1);
                    let ids: Vec<_> = children.iter().filter_map(|c| match c {
                        LayoutNode::Terminal { terminal_id: Some(id), .. } => Some(id.as_str()),
                        _ => None,
                    }).collect();
                    assert_eq!(ids, vec!["t1", "t3", "t2"]);
                }
                _ => panic!("Expected tabs, got {:?}", layout),
            }
        });
    }

    #[gpui::test]
    fn test_move_terminal_to_tab_group_appends(cx: &mut gpui::TestAppContext) {
        // V[Tabs[t1, t2], t3] → move t3 into tabs at end → Tabs[t1, t2, t3]
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            sizes: vec![50.0, 50.0],
            children: vec![
                LayoutNode::Tabs {
                    children: vec![terminal_node_t("t1"), terminal_node_t("t2")],
                    active_tab: 0,
                },
                terminal_node_t("t3"),
            ],
        };
        let project = make_project_with_layout("p1", layout);
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.move_terminal_to_tab_group("p1", "t3", &[0], None, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            match layout {
                LayoutNode::Tabs { children, active_tab } => {
                    assert_eq!(children.len(), 3);
                    assert_eq!(*active_tab, 2);
                    let ids: Vec<_> = children.iter().filter_map(|c| match c {
                        LayoutNode::Terminal { terminal_id: Some(id), .. } => Some(id.as_str()),
                        _ => None,
                    }).collect();
                    assert_eq!(ids, vec!["t1", "t2", "t3"]);
                }
                _ => panic!("Expected tabs, got {:?}", layout),
            }
        });
    }

    #[gpui::test]
    fn test_move_terminal_to_tab_group_same_group_reorders(cx: &mut gpui::TestAppContext) {
        // Tabs[t1, t2, t3] → move t1 (already in group) to index 2 → reorder
        let layout = LayoutNode::Tabs {
            children: vec![terminal_node_t("t1"), terminal_node_t("t2"), terminal_node_t("t3")],
            active_tab: 0,
        };
        let project = make_project_with_layout("p1", layout);
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.move_terminal_to_tab_group("p1", "t1", &[], Some(2), cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            match layout {
                LayoutNode::Tabs { children, .. } => {
                    let ids: Vec<_> = children.iter().filter_map(|c| match c {
                        LayoutNode::Terminal { terminal_id: Some(id), .. } => Some(id.as_str()),
                        _ => None,
                    }).collect();
                    assert_eq!(ids, vec!["t2", "t3", "t1"]);
                }
                _ => panic!("Expected tabs, got {:?}", layout),
            }
        });
    }

    #[gpui::test]
    fn test_move_pane_3_children_with_flatten(cx: &mut gpui::TestAppContext) {
        // V[t1, t2, t3] -> drag t1 to top of t3 -> V[t2, H[t1, t3]]
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            sizes: vec![33.0, 33.0, 34.0],
            children: vec![terminal_node_t("t1"), terminal_node_t("t2"), terminal_node_t("t3")],
        };
        let project = make_project_with_layout("p1", layout);
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.move_pane("p1", "t1", "p1", "t3", DropZone::Top, cx);
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let layout = ws.project("p1").unwrap().layout.as_ref().unwrap();
            match layout {
                LayoutNode::Split { direction, children, .. } => {
                    assert_eq!(*direction, SplitDirection::Vertical);
                    assert_eq!(children.len(), 2);
                    // First child is t2
                    assert!(matches!(&children[0], LayoutNode::Terminal { terminal_id: Some(id), .. } if id == "t2"));
                    // Second child is H[t1, t3]
                    match &children[1] {
                        LayoutNode::Split { direction: inner_dir, children: inner_children, .. } => {
                            assert_eq!(*inner_dir, SplitDirection::Horizontal);
                            assert_eq!(inner_children.len(), 2);
                            let inner_ids: Vec<_> = inner_children.iter().filter_map(|c| match c {
                                LayoutNode::Terminal { terminal_id: Some(id), .. } => Some(id.as_str()),
                                _ => None,
                            }).collect();
                            assert_eq!(inner_ids, vec!["t1", "t3"]);
                        }
                        _ => panic!("Expected inner horizontal split"),
                    }
                }
                _ => panic!("Expected vertical split"),
            }
        });
    }

    // === metadata cleanup tests ===

    fn make_project_with_names(id: &str, layout: LayoutNode, names: Vec<(&str, &str)>) -> ProjectData {
        let mut p = make_project_with_layout(id, layout);
        for (tid, name) in names {
            p.terminal_names.insert(tid.to_string(), name.to_string());
        }
        p
    }

    #[gpui::test]
    fn test_close_terminal_cleans_metadata(cx: &mut gpui::TestAppContext) {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![50.0, 50.0],
            children: vec![terminal_node_t("t1"), terminal_node_t("t2")],
        };
        let project = make_project_with_names("p1", layout, vec![("t1", "Term 1"), ("t2", "Term 2")]);
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let removed = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.close_terminal("p1", &[0], cx)
        });

        assert_eq!(removed, vec!["t1"]);
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let p = ws.project("p1").unwrap();
            assert!(!p.terminal_names.contains_key("t1"));
            assert!(p.terminal_names.contains_key("t2"));
        });
    }

    #[gpui::test]
    fn test_close_tab_cleans_metadata(cx: &mut gpui::TestAppContext) {
        let layout = LayoutNode::Tabs {
            children: vec![terminal_node_t("t1"), terminal_node_t("t2"), terminal_node_t("t3")],
            active_tab: 0,
        };
        let project = make_project_with_names("p1", layout, vec![
            ("t1", "Term 1"), ("t2", "Term 2"), ("t3", "Term 3"),
        ]);
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let removed = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.close_tab("p1", &[], 1, cx)
        });

        assert_eq!(removed, vec!["t2"]);
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let p = ws.project("p1").unwrap();
            assert!(p.terminal_names.contains_key("t1"));
            assert!(!p.terminal_names.contains_key("t2"));
            assert!(p.terminal_names.contains_key("t3"));
        });
    }

    #[gpui::test]
    fn test_close_other_tabs_cleans_metadata(cx: &mut gpui::TestAppContext) {
        let layout = LayoutNode::Tabs {
            children: vec![terminal_node_t("t1"), terminal_node_t("t2"), terminal_node_t("t3")],
            active_tab: 0,
        };
        let project = make_project_with_names("p1", layout, vec![
            ("t1", "Term 1"), ("t2", "Term 2"), ("t3", "Term 3"),
        ]);
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let removed = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.close_other_tabs("p1", &[], 1, cx)
        });

        assert_eq!(removed.len(), 2);
        assert!(removed.contains(&"t1".to_string()));
        assert!(removed.contains(&"t3".to_string()));
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let p = ws.project("p1").unwrap();
            assert!(!p.terminal_names.contains_key("t1"));
            assert!(p.terminal_names.contains_key("t2"));
            assert!(!p.terminal_names.contains_key("t3"));
        });
    }

    #[gpui::test]
    fn test_close_tabs_to_right_cleans_metadata(cx: &mut gpui::TestAppContext) {
        let layout = LayoutNode::Tabs {
            children: vec![terminal_node_t("t1"), terminal_node_t("t2"), terminal_node_t("t3")],
            active_tab: 0,
        };
        let project = make_project_with_names("p1", layout, vec![
            ("t1", "Term 1"), ("t2", "Term 2"), ("t3", "Term 3"),
        ]);
        let data = make_workspace_data(vec![project], vec!["p1"]);
        let workspace = cx.new(|_cx| Workspace::new(data));

        let removed = workspace.update(cx, |ws: &mut Workspace, cx| {
            ws.close_tabs_to_right("p1", &[], 0, cx)
        });

        assert_eq!(removed.len(), 2);
        assert!(removed.contains(&"t2".to_string()));
        assert!(removed.contains(&"t3".to_string()));
        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let p = ws.project("p1").unwrap();
            assert!(p.terminal_names.contains_key("t1"));
            assert!(!p.terminal_names.contains_key("t2"));
            assert!(!p.terminal_names.contains_key("t3"));
        });
    }
}
