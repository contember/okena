use crate::terminal::shell_config::ShellType;
use crate::workspace::state::{DetachedTerminalState, FocusedTerminalState, LayoutNode, ProjectData, SplitDirection, Workspace};
use gpui::*;
use std::collections::HashMap;

impl Workspace {
    /// Toggle project visibility
    pub fn toggle_project_visibility(&mut self, project_id: &str, cx: &mut Context<Self>) {
        self.with_project(project_id, cx, |project| {
            project.is_visible = !project.is_visible;
            true
        });
    }

    /// Set terminal ID at a layout path and assign a default name based on directory
    pub fn set_terminal_id(
        &mut self,
        project_id: &str,
        path: &[usize],
        terminal_id: String,
        cx: &mut Context<Self>,
    ) {
        let tid = terminal_id.clone();
        self.with_project(project_id, cx, |project| {
            // Set terminal ID in layout node
            if let Some(ref mut layout) = project.layout {
                if let Some(node) = layout.get_at_path_mut(path) {
                    if let LayoutNode::Terminal { terminal_id: id, .. } = node {
                        *id = Some(terminal_id);

                        // Set default name based on directory if not already set
                        if !project.terminal_names.contains_key(&tid) {
                            let base_name = std::path::Path::new(&project.path)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("Terminal")
                                .to_string();

                            // Check if name already exists and add counter if needed
                            let existing_names: Vec<_> = project.terminal_names.values().collect();
                            let default_name = if existing_names.contains(&&base_name) {
                                // Find next available number
                                let mut counter = 2;
                                loop {
                                    let candidate = format!("{} ({})", base_name, counter);
                                    if !existing_names.contains(&&candidate) {
                                        break candidate;
                                    }
                                    counter += 1;
                                }
                            } else {
                                base_name
                            };

                            project.terminal_names.insert(tid, default_name);
                        }
                        return true;
                    }
                }
            }
            false
        });
    }

    /// Set shell type for a terminal at a layout path
    pub fn set_terminal_shell(
        &mut self,
        project_id: &str,
        path: &[usize],
        shell_type: ShellType,
        cx: &mut Context<Self>,
    ) {
        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Terminal { shell_type: st, .. } = node {
                *st = shell_type;
                return true;
            }
            false
        });
    }

    /// Get shell type for a terminal at a layout path
    pub fn get_terminal_shell(&self, project_id: &str, path: &[usize]) -> Option<ShellType> {
        let project = self.project(project_id)?;
        if let Some(LayoutNode::Terminal { shell_type, .. }) = project.layout.as_ref().and_then(|l| l.get_at_path(path)) {
            Some(shell_type.clone())
        } else {
            None
        }
    }

    /// Set shell type for a terminal by its ID
    pub fn set_terminal_shell_by_id(
        &mut self,
        project_id: &str,
        terminal_id: &str,
        shell_type: ShellType,
        cx: &mut Context<Self>,
    ) {
        if let Some(project) = self.project(project_id) {
            if let Some(path) = project.layout.as_ref().and_then(|l| l.find_terminal_path(terminal_id)) {
                self.set_terminal_shell(project_id, &path, shell_type, cx);
            }
        }
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
                    cx.notify();
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
                            cx.notify();
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
            self.focused_terminal = None;
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

    /// Toggle terminal minimized state
    pub fn toggle_terminal_minimized(
        &mut self,
        project_id: &str,
        path: &[usize],
        cx: &mut Context<Self>,
    ) {
        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Terminal { minimized, .. } = node {
                *minimized = !*minimized;
                true
            } else {
                false
            }
        });
    }

    /// Add a new project
    /// If `with_terminal` is false, creates a bookmark project without a terminal layout.
    pub fn add_project(&mut self, name: String, path: String, with_terminal: bool, cx: &mut Context<Self>) {
        let id = uuid::Uuid::new_v4().to_string();
        let project = ProjectData {
            id: id.clone(),
            name,
            path,
            is_visible: true,
            layout: if with_terminal { Some(LayoutNode::new_terminal()) } else { None },
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
        };
        self.data.projects.push(project);
        self.data.project_order.push(id);
        cx.notify();
    }

    /// Start a terminal for a project that doesn't have one (bookmark -> active project)
    pub fn start_terminal(&mut self, project_id: &str, cx: &mut Context<Self>) {
        if let Some(project) = self.project_mut(project_id) {
            if project.layout.is_none() {
                project.layout = Some(LayoutNode::new_terminal());
                cx.notify();
            }
        }
    }

    /// Add a new terminal to a project by splitting the root layout
    pub fn add_terminal(&mut self, project_id: &str, cx: &mut Context<Self>) {
        if let Some(project) = self.project_mut(project_id) {
            if let Some(ref old_layout) = project.layout {
                let old_layout = old_layout.clone();
                project.layout = Some(LayoutNode::Split {
                    direction: SplitDirection::Vertical,
                    sizes: vec![50.0, 50.0],
                    children: vec![old_layout, LayoutNode::new_terminal()],
                });
            } else {
                // Project has no layout - create one with a terminal
                project.layout = Some(LayoutNode::new_terminal());
            }
            cx.notify();
        }
    }

    /// Set focused project (focus mode)
    pub fn set_focused_project(&mut self, project_id: Option<String>, cx: &mut Context<Self>) {
        self.focused_project_id = project_id;
        // Exit fullscreen when changing focus
        self.fullscreen_terminal = None;
        cx.notify();
    }

    /// Enter fullscreen mode for a terminal
    pub fn set_fullscreen_terminal(
        &mut self,
        project_id: String,
        terminal_id: String,
        cx: &mut Context<Self>,
    ) {
        log::info!("set_fullscreen_terminal called with project_id={}, terminal_id={}", project_id, terminal_id);

        // Find the layout path for this terminal
        let layout_path = self.project(&project_id)
            .and_then(|p| p.layout.as_ref())
            .and_then(|l| l.find_terminal_path(&terminal_id))
            .unwrap_or_default();

        log::info!("layout_path for terminal: {:?}", layout_path);

        // Use FocusManager for fullscreen entry
        self.focus_manager.enter_fullscreen(project_id.clone(), layout_path.clone());

        // Update legacy state for compatibility
        self.fullscreen_terminal = Some(crate::workspace::state::FullscreenState {
            project_id: project_id.clone(),
            terminal_id: terminal_id.clone(),
        });
        log::info!("fullscreen_terminal set to Some with terminal_id={}", terminal_id);

        // Also focus the project
        self.focused_project_id = Some(project_id.clone());

        // Sync focused_terminal for visual indicator
        self.focused_terminal = Some(FocusedTerminalState {
            project_id,
            layout_path,
        });

        cx.notify();
    }

    /// Enter fullscreen mode for the first terminal in a project
    pub fn fullscreen_project(&mut self, project_id: String, cx: &mut Context<Self>) {
        if let Some(project) = self.project(&project_id) {
            let terminal_ids = project.layout.as_ref()
                .map(|l| l.collect_terminal_ids())
                .unwrap_or_default();
            if let Some(first_id) = terminal_ids.first().cloned() {
                self.set_fullscreen_terminal(project_id, first_id, cx);
            }
        }
    }

    /// Exit fullscreen mode
    ///
    /// Restores focus to the previously focused terminal if one was saved.
    pub fn exit_fullscreen(&mut self, cx: &mut Context<Self>) {
        self.fullscreen_terminal = None;

        // Use FocusManager for focus restoration
        if let Some(restored) = self.focus_manager.exit_fullscreen() {
            // Restore the focused terminal state for visual indicator
            self.focused_terminal = Some(FocusedTerminalState {
                project_id: restored.project_id,
                layout_path: restored.layout_path,
            });
        }

        cx.notify();
    }

    /// Rename a project
    pub fn rename_project(&mut self, project_id: &str, new_name: String, cx: &mut Context<Self>) {
        self.with_project(project_id, cx, |project| {
            project.name = new_name;
            true
        });
    }

    /// Rename a terminal
    pub fn rename_terminal(
        &mut self,
        project_id: &str,
        terminal_id: &str,
        new_name: String,
        cx: &mut Context<Self>,
    ) {
        let terminal_id = terminal_id.to_string();
        self.with_project(project_id, cx, |project| {
            project.terminal_names.insert(terminal_id, new_name);
            true
        });
    }

    /// Set terminal hidden state
    #[allow(dead_code)] // API for future terminal visibility control
    pub fn set_terminal_hidden(
        &mut self,
        project_id: &str,
        terminal_id: &str,
        hidden: bool,
        cx: &mut Context<Self>,
    ) {
        let terminal_id = terminal_id.to_string();
        self.with_project(project_id, cx, |project| {
            project.hidden_terminals.insert(terminal_id, hidden);
            true
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

    /// Set focused terminal (for visual indicator)
    ///
    /// This updates both the FocusManager and the legacy focused_terminal state.
    /// Focus events propagate: terminal focus -> pane focus -> project awareness
    pub fn set_focused_terminal(
        &mut self,
        project_id: String,
        layout_path: Vec<usize>,
        cx: &mut Context<Self>,
    ) {
        // Update FocusManager
        self.focus_manager.focus_terminal(project_id.clone(), layout_path.clone());

        // Update legacy state for compatibility
        self.focused_terminal = Some(FocusedTerminalState {
            project_id,
            layout_path,
        });
        cx.notify();
    }

    /// Clear focused terminal
    ///
    /// This is typically called when entering a modal context (search, rename, etc.)
    /// The current focus is saved for restoration when the modal closes.
    pub fn clear_focused_terminal(&mut self, cx: &mut Context<Self>) {
        // Use FocusManager to save focus for restoration
        self.focus_manager.enter_modal();
        // Don't clear focused_terminal - visual indicator remains during modal
        cx.notify();
    }

    /// Restore focused terminal after modal dismissal
    ///
    /// Called when exiting a modal context to restore the previous focus.
    pub fn restore_focused_terminal(&mut self, cx: &mut Context<Self>) {
        // Use FocusManager to restore focus
        if let Some(restored) = self.focus_manager.exit_modal() {
            self.focused_terminal = Some(FocusedTerminalState {
                project_id: restored.project_id,
                layout_path: restored.layout_path,
            });
        }
        cx.notify();
    }

    /// Focus a terminal by its ID (finds path automatically)
    ///
    /// This is a convenience method that looks up the layout path and calls set_focused_terminal.
    pub fn focus_terminal_by_id(
        &mut self,
        project_id: &str,
        terminal_id: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(project) = self.project(project_id) {
            if let Some(ref layout) = project.layout {
                if let Some(path) = layout.find_terminal_path(terminal_id) {
                    // Switch to the terminal's project so it becomes visible
                    self.set_focused_project(Some(project_id.to_string()), cx);
                    // Use the unified focus method for consistent propagation
                    self.set_focused_terminal(project_id.to_string(), path, cx);
                }
            }
        }
    }

    /// Restore (un-minimize) a terminal at a path
    pub fn restore_terminal(&mut self, project_id: &str, path: &[usize], cx: &mut Context<Self>) {
        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Terminal { minimized, .. } = node {
                *minimized = false;
                true
            } else {
                false
            }
        });
    }

    /// Toggle terminal minimized state by terminal ID (finds path automatically)
    pub fn toggle_terminal_minimized_by_id(
        &mut self,
        project_id: &str,
        terminal_id: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(project) = self.project_mut(project_id) {
            if let Some(ref mut layout) = project.layout {
                if let Some(path) = layout.find_terminal_path(terminal_id) {
                    if let Some(node) = layout.get_at_path_mut(&path) {
                        if let LayoutNode::Terminal { minimized, .. } = node {
                            *minimized = !*minimized;
                            cx.notify();
                        }
                    }
                }
            }
        }
    }

    /// Check if a terminal is minimized by ID
    pub fn is_terminal_minimized(&self, project_id: &str, terminal_id: &str) -> bool {
        if let Some(project) = self.project(project_id) {
            if let Some(ref layout) = project.layout {
                if let Some(path) = layout.find_terminal_path(terminal_id) {
                    if let Some(LayoutNode::Terminal { minimized, .. }) = layout.get_at_path(&path) {
                        return *minimized;
                    }
                }
            }
        }
        false
    }

    /// Update project column widths
    pub fn update_project_widths(&mut self, widths: HashMap<String, f32>, cx: &mut Context<Self>) {
        self.data.project_widths = widths;
        cx.notify();
    }

    /// Get project width or default equal distribution
    pub fn get_project_width(&self, project_id: &str, visible_count: usize) -> f32 {
        self.data.project_widths
            .get(project_id)
            .copied()
            .unwrap_or_else(|| 100.0 / visible_count as f32)
    }

    /// Close all terminals in a project
    /// Returns the list of terminal IDs that were closed (for PTY cleanup)
    /// The project becomes a bookmark (no terminals) after this operation
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
        if let Some(ref focused) = self.focused_terminal {
            if focused.project_id == project_id {
                self.focused_terminal = None;
            }
        }

        // Exit fullscreen if a terminal from this project was in fullscreen
        if let Some(ref fs) = self.fullscreen_terminal {
            if fs.project_id == project_id {
                self.fullscreen_terminal = None;
            }
        }

        // Remove any detached terminals from this project
        self.detached_terminals.retain(|d| d.project_id != project_id);

        cx.notify();
        terminal_ids
    }

    /// Delete a project
    pub fn delete_project(&mut self, project_id: &str, cx: &mut Context<Self>) {
        // Remove from projects list
        self.data.projects.retain(|p| p.id != project_id);
        // Remove from project order
        self.data.project_order.retain(|id| id != project_id);
        // Remove from widths
        self.data.project_widths.remove(project_id);
        // Clear focus if this was the focused project
        if self.focused_project_id.as_deref() == Some(project_id) {
            self.focused_project_id = None;
        }
        // Exit fullscreen if this project's terminal was in fullscreen
        if let Some(fs) = &self.fullscreen_terminal {
            if fs.project_id == project_id {
                self.fullscreen_terminal = None;
            }
        }
        cx.notify();
    }

    /// Detach a terminal to a separate window
    /// Returns the detached state for window creation
    pub fn detach_terminal(
        &mut self,
        project_id: &str,
        path: &[usize],
        cx: &mut Context<Self>,
    ) -> Option<DetachedTerminalState> {
        // Get terminal ID from the layout node
        let terminal_id = if let Some(project) = self.project(project_id) {
            if let Some(ref layout) = project.layout {
                if let Some(LayoutNode::Terminal { terminal_id: Some(id), .. }) = layout.get_at_path(path) {
                    id.clone()
                } else {
                    return None;
                }
            } else {
                return None;
            }
        } else {
            return None;
        };

        // Check if already detached
        if self.detached_terminals.iter().any(|d| d.terminal_id == terminal_id) {
            return None;
        }

        // Mark terminal as detached in layout
        self.with_layout_node(project_id, path, cx, |node| {
            if let LayoutNode::Terminal { detached, .. } = node {
                *detached = true;
                true
            } else {
                false
            }
        });

        // Create detached state
        let state = DetachedTerminalState {
            terminal_id: terminal_id.clone(),
            project_id: project_id.to_string(),
            layout_path: path.to_vec(),
        };

        self.detached_terminals.push(state.clone());
        cx.notify();

        Some(state)
    }

    /// Re-attach a detached terminal back to its original location
    pub fn attach_terminal(&mut self, terminal_id: &str, cx: &mut Context<Self>) {
        // Find and remove from detached list
        let detached = self.detached_terminals.iter()
            .position(|d| d.terminal_id == terminal_id)
            .map(|i| self.detached_terminals.remove(i));

        if let Some(state) = detached {
            // Mark terminal as not detached in layout
            self.with_layout_node(&state.project_id, &state.layout_path, cx, |node| {
                if let LayoutNode::Terminal { detached, .. } = node {
                    *detached = false;
                    true
                } else {
                    false
                }
            });
        }

        cx.notify();
    }

    /// Check if a terminal is detached
    pub fn is_terminal_detached(&self, terminal_id: &str) -> bool {
        self.detached_terminals.iter().any(|d| d.terminal_id == terminal_id)
    }

    /// Move a project to a new position in the order
    pub fn move_project(&mut self, project_id: &str, new_index: usize, cx: &mut Context<Self>) {
        // Find current index
        if let Some(current_index) = self.data.project_order.iter().position(|id| id == project_id) {
            // Remove from current position
            let id = self.data.project_order.remove(current_index);
            // Adjust target index if needed
            let target = if new_index > current_index {
                new_index.saturating_sub(1)
            } else {
                new_index
            };
            // Insert at new position
            let target = target.min(self.data.project_order.len());
            self.data.project_order.insert(target, id);
            cx.notify();
        }
    }

    /// Get detached terminal state by terminal ID
    #[allow(dead_code)] // API for detached terminal access
    pub fn get_detached_terminal(&self, terminal_id: &str) -> Option<&DetachedTerminalState> {
        self.detached_terminals.iter().find(|d| d.terminal_id == terminal_id)
    }

    /// Clear the worktree dialog request
    pub fn clear_worktree_dialog_request(&mut self, cx: &mut Context<Self>) {
        self.worktree_dialog_request = None;
        cx.notify();
    }

    /// Request showing the context menu for a project
    pub fn request_context_menu(&mut self, project_id: &str, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        self.context_menu_request = Some(crate::workspace::state::ContextMenuRequest {
            project_id: project_id.to_string(),
            position,
        });
        cx.notify();
    }

    /// Clear the context menu request
    pub fn clear_context_menu_request(&mut self, cx: &mut Context<Self>) {
        self.context_menu_request = None;
        cx.notify();
    }

    /// Request showing the shell selector for a terminal
    pub fn request_shell_selector(
        &mut self,
        project_id: &str,
        terminal_id: &str,
        current_shell: crate::terminal::shell_config::ShellType,
        cx: &mut Context<Self>,
    ) {
        self.shell_selector_request = Some(crate::workspace::state::ShellSelectorRequest {
            project_id: project_id.to_string(),
            terminal_id: terminal_id.to_string(),
            current_shell,
        });
        cx.notify();
    }

    /// Clear the shell selector request
    pub fn clear_shell_selector_request(&mut self, cx: &mut Context<Self>) {
        self.shell_selector_request = None;
        cx.notify();
    }

    /// Create a worktree project from an existing project
    /// Returns the new project ID on success
    pub fn create_worktree_project(
        &mut self,
        parent_project_id: &str,
        branch: &str,
        target_path: &str,
        create_branch: bool,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        // Get parent project info
        let parent = self.project(parent_project_id)
            .ok_or_else(|| "Parent project not found".to_string())?;

        let parent_path = parent.path.clone();
        let parent_layout = parent.layout.clone();

        // Determine the actual repo path (if parent is a worktree, use its main repo)
        let repo_path = if let Some(ref wt_info) = parent.worktree_info {
            std::path::PathBuf::from(&wt_info.main_repo_path)
        } else {
            std::path::PathBuf::from(&parent_path)
        };

        // Create the git worktree
        let target = std::path::PathBuf::from(target_path);
        crate::git::create_worktree(&repo_path, branch, &target, create_branch)?;

        // Create new project with cloned layout (or new terminal if parent has no layout)
        let id = uuid::Uuid::new_v4().to_string();
        let project_name = format!("{} ({})",
            std::path::Path::new(&parent_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Project"),
            branch
        );

        let new_layout = parent_layout
            .as_ref()
            .map(|l| l.clone_structure());

        let project = ProjectData {
            id: id.clone(),
            name: project_name,
            path: target_path.to_string(),
            is_visible: true,
            layout: new_layout,
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: Some(crate::workspace::state::WorktreeMetadata {
                parent_project_id: parent_project_id.to_string(),
                main_repo_path: repo_path.to_string_lossy().to_string(),
            }),
        };

        // Insert after parent project in order
        let parent_index = self.data.project_order
            .iter()
            .position(|pid| pid == parent_project_id)
            .unwrap_or(self.data.project_order.len());

        self.data.projects.push(project);
        self.data.project_order.insert(parent_index + 1, id.clone());

        cx.notify();
        Ok(id)
    }

    /// Remove a worktree project and its git worktree
    pub fn remove_worktree_project(&mut self, project_id: &str, force: bool, cx: &mut Context<Self>) -> Result<(), String> {
        let project = self.project(project_id)
            .ok_or_else(|| "Project not found".to_string())?;

        // Ensure it's a worktree project
        if project.worktree_info.is_none() {
            return Err("Not a worktree project".to_string());
        }

        let worktree_path = std::path::PathBuf::from(&project.path);

        // Remove the git worktree
        crate::git::remove_worktree(&worktree_path, force)?;

        // Delete the project from workspace
        self.delete_project(project_id, cx);

        Ok(())
    }
}
