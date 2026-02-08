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
        self.with_layout_node_normalized(project_id, path, cx, |node| {
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
                LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
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
            LayoutNode::Split { children, .. } => {
                assert_eq!(children.len(), 2);
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
}
