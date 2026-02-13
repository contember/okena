//! Focus and fullscreen workspace actions
//!
//! Actions for managing terminal and project focus, including fullscreen mode.

use crate::workspace::state::Workspace;
use gpui::*;

impl Workspace {
    /// Set focused project (focus mode)
    ///
    /// This also focuses the first terminal in the project if one exists.
    pub fn set_focused_project(&mut self, project_id: Option<String>, cx: &mut Context<Self>) {
        // Clear fullscreen without restoring old project_id (we're overriding it)
        self.focus_manager.clear_fullscreen_without_restore();

        // Set the focused project via FocusManager
        self.focus_manager.set_focused_project_id(project_id.clone());

        // Focus the first terminal in the project
        if let Some(ref pid) = project_id {
            if let Some(project) = self.project(pid) {
                if let Some(ref layout) = project.layout {
                    // Find the first terminal's path
                    if let Some(first_path) = Self::find_first_terminal_path(layout) {
                        self.focus_manager.focus_terminal(pid.clone(), first_path);
                    }
                }
            }
        }

        cx.notify();
    }

    /// Find the path to the first terminal in a layout tree
    fn find_first_terminal_path(node: &crate::workspace::state::LayoutNode) -> Option<Vec<usize>> {
        use crate::workspace::state::LayoutNode;
        match node {
            LayoutNode::Terminal { .. } => Some(vec![]),
            LayoutNode::Split { children, .. }
            | LayoutNode::Tabs { children, .. }
            | LayoutNode::Grid { children, .. } => {
                for (i, child) in children.iter().enumerate() {
                    if let Some(mut path) = Self::find_first_terminal_path(child) {
                        path.insert(0, i);
                        return Some(path);
                    }
                }
                None
            }
        }
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

        // Use FocusManager for fullscreen entry (saves current state + sets focused_project_id)
        self.focus_manager.enter_fullscreen(project_id, layout_path, terminal_id.clone());

        log::info!("fullscreen_terminal set via FocusManager with terminal_id={}", terminal_id);

        cx.notify();
    }

    /// Exit fullscreen mode
    ///
    /// Restores focus to the previously focused terminal and project view mode.
    pub fn exit_fullscreen(&mut self, cx: &mut Context<Self>) {
        // Use FocusManager for focus + project_id restoration
        self.focus_manager.exit_fullscreen();

        cx.notify();
    }

    /// Set focused terminal (for visual indicator)
    ///
    /// Focus events propagate: terminal focus -> pane focus -> project awareness
    pub fn set_focused_terminal(
        &mut self,
        project_id: String,
        layout_path: Vec<usize>,
        cx: &mut Context<Self>,
    ) {
        // Update FocusManager
        self.focus_manager.focus_terminal(project_id.clone(), layout_path.clone());

        // Record project access time for recency sorting
        self.touch_project(&project_id);

        cx.notify();
    }

    /// Clear focused terminal
    ///
    /// This is typically called when entering a modal context (search, rename, etc.)
    /// The current focus is saved for restoration when the modal closes.
    pub fn clear_focused_terminal(&mut self, cx: &mut Context<Self>) {
        // Use FocusManager to save focus for restoration
        self.focus_manager.enter_modal();
        // Visual indicator remains during modal (FocusManager keeps current_focus)
        cx.notify();
    }

    /// Restore focused terminal after modal dismissal
    ///
    /// Called when exiting a modal context to restore the previous focus.
    pub fn restore_focused_terminal(&mut self, cx: &mut Context<Self>) {
        // Use FocusManager to restore focus
        self.focus_manager.exit_modal();
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
}
