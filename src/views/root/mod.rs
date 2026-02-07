mod handlers;
mod render;
mod sidebar;
mod terminal_actions;

use crate::terminal::pty_manager::PtyManager;
use crate::terminal::terminal::Terminal;
use crate::views::overlay_manager::OverlayManager;
use crate::views::project_column::ProjectColumn;
use crate::views::sidebar_controller::SidebarController;
use crate::views::sidebar::Sidebar;
use crate::views::split_pane::{new_active_drag, ActiveDrag};
use crate::views::status_bar::StatusBar;
use crate::views::title_bar::TitleBar;
use crate::workspace::persistence::{load_settings, AppSettings};
use crate::workspace::state::Workspace;
use gpui::*;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// Shared terminals registry for PTY event routing
pub type TerminalsRegistry = Arc<Mutex<HashMap<String, Arc<Terminal>>>>;

/// Root view of the application
pub struct RootView {
    workspace: Entity<Workspace>,
    pty_manager: Arc<PtyManager>,
    terminals: TerminalsRegistry,
    sidebar: Entity<Sidebar>,
    /// Sidebar state controller
    sidebar_ctrl: SidebarController,
    /// App settings for persistence
    app_settings: AppSettings,
    /// Stored project column entities (created once, not during render)
    project_columns: HashMap<String, Entity<ProjectColumn>>,
    /// Title bar entity
    title_bar: Entity<TitleBar>,
    /// Status bar entity
    status_bar: Entity<StatusBar>,
    /// Centralized overlay manager
    overlay_manager: Entity<OverlayManager>,
    /// Shared drag state for resize operations
    active_drag: ActiveDrag,
    /// Focus handle for capturing global keybindings
    focus_handle: FocusHandle,
}

impl RootView {
    pub fn new(
        workspace: Entity<Workspace>,
        pty_manager: Arc<PtyManager>,
        cx: &mut Context<Self>,
    ) -> Self {
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(HashMap::new()));

        // Load app settings and create sidebar controller
        let app_settings = load_settings();
        let sidebar_ctrl = SidebarController::new(&app_settings);

        // Create sidebar entity once to preserve state
        let sidebar = cx.new(|cx| Sidebar::new(workspace.clone(), terminals.clone(), cx));

        // Create title bar entity
        let workspace_for_title = workspace.clone();
        let title_bar = cx.new(|_cx| TitleBar::new("Okena", workspace_for_title));

        // Create status bar entity
        let status_bar = cx.new(StatusBar::new);

        // Create overlay manager
        let overlay_manager = cx.new(|_cx| OverlayManager::new(workspace.clone()));

        // Subscribe to overlay manager events
        cx.subscribe(&overlay_manager, Self::handle_overlay_manager_event).detach();

        // Observe Workspace to process overlay requests outside of render()
        cx.observe(&workspace, |this, _workspace, cx| {
            this.process_pending_requests(cx);
        }).detach();

        // Create focus handle for global keybindings
        let focus_handle = cx.focus_handle();

        let mut view = Self {
            workspace,
            pty_manager,
            terminals,
            sidebar,
            sidebar_ctrl,
            app_settings,
            project_columns: HashMap::new(),
            title_bar,
            status_bar,
            overlay_manager,
            active_drag: new_active_drag(),
            focus_handle,
        };

        // Initialize project columns
        view.sync_project_columns(cx);

        view
    }

    /// Get the terminals registry (for sharing with detached windows)
    pub fn terminals(&self) -> &TerminalsRegistry {
        &self.terminals
    }

    /// Ensure project columns exist for all visible projects
    fn sync_project_columns(&mut self, cx: &mut Context<Self>) {
        let visible_project_ids: Vec<String> = {
            let ws = self.workspace.read(cx);
            ws.visible_projects().iter().map(|p| p.id.clone()).collect()
        };

        // Create columns for new projects
        for project_id in &visible_project_ids {
            if !self.project_columns.contains_key(project_id) {
                let workspace_clone = self.workspace.clone();
                let pty_manager_clone = self.pty_manager.clone();
                let terminals_clone = self.terminals.clone();
                let active_drag_clone = self.active_drag.clone();
                let id = project_id.clone();
                let entity = cx.new(move |cx| {
                    ProjectColumn::new(
                        workspace_clone,
                        id,
                        pty_manager_clone,
                        terminals_clone,
                        active_drag_clone,
                        cx,
                    )
                });
                self.project_columns.insert(project_id.clone(), entity);
            }
        }
    }
}

impl_focusable!(RootView);
