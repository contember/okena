//! Focus and fullscreen workspace actions
//!
//! Actions for managing terminal and project focus, including fullscreen mode.

use crate::state::Workspace;
use gpui::*;

impl Workspace {
    /// Set focused project (focus mode)
    ///
    /// This zooms the main view to show only this project.
    /// Also focuses the first terminal in the project if one exists.
    /// If the project has no layout, drills into the first worktree child.
    pub fn set_focused_project(&mut self, project_id: Option<String>, cx: &mut Context<Self>) {
        // Clear fullscreen without restoring old project_id (we're overriding it)
        self.focus_manager.clear_fullscreen_without_restore();

        // Set the focused project via FocusManager (controls main view zoom)
        self.focus_manager.set_focused_project_id(project_id.clone());

        // Focus the first terminal in the project
        if let Some(ref pid) = project_id {
            self.focus_first_terminal_in(pid);
        }

        cx.notify();
    }

    /// Set focused project in individual mode (show only this project, not its worktree children).
    /// Used when clicking a "main worktree" entry in the sidebar.
    pub fn set_focused_project_individual(&mut self, project_id: Option<String>, cx: &mut Context<Self>) {
        self.focus_manager.clear_fullscreen_without_restore();
        self.focus_manager.set_focused_project_id_individual(project_id.clone());

        if let Some(ref pid) = project_id {
            self.focus_first_terminal_in(pid);
        }

        cx.notify();
    }

    /// Toggle folder selection: sets folder filter and focuses the first terminal inside.
    /// If the folder is already selected, deselects it.
    pub fn toggle_folder_focus(&mut self, folder_id: &str, cx: &mut Context<Self>) {
        let selecting = self.active_folder_filter().map(|s| s.as_str()) != Some(folder_id);
        if selecting {
            self.set_folder_filter(Some(folder_id.to_string()), cx);
            // Clear project focus so all visible folder projects show
            self.focus_manager.set_focused_project_id(None);
            // Focus the first project's terminal
            if let Some(first_pid) = self.folder(folder_id).and_then(|f| f.project_ids.first()).cloned() {
                self.focus_first_terminal_in(&first_pid);
            }
        } else {
            self.set_folder_filter(None, cx);
        }
        cx.notify();
    }

    /// Resolve a focusable project and focus its first terminal.
    ///
    /// If the project has no layout (e.g. only worktree children), drills into
    /// the first worktree child that has a terminal.
    fn focus_first_terminal_in(&mut self, project_id: &str) {
        // Try the project itself first, then its worktree children
        let candidates = std::iter::once(project_id.to_string())
            .chain(self.worktree_child_ids(project_id));
        for id in candidates {
            if let Some(project) = self.project(&id) {
                if let Some(layout) = project.layout.as_ref() {
                    // Focus the currently visible terminal (follows active tabs)
                    let path = layout.find_visible_terminal_path();
                    self.focus_manager.focus_terminal(id, path);
                    return;
                }
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
                    // Activate any tabs along the path so the terminal becomes visible
                    if let Some(project_mut) = self.project_mut(project_id) {
                        if let Some(ref mut layout) = project_mut.layout {
                            layout.activate_tabs_along_path(&path);
                        }
                    }
                    self.notify_data(cx);
                    // Focus the terminal without changing which projects are shown
                    self.set_focused_terminal(project_id.to_string(), path, cx);
                }
            }
        }
    }
}
