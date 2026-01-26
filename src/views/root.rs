use crate::terminal::pty_manager::PtyManager;
use crate::terminal::terminal::Terminal;
use crate::theme::theme;
use crate::views::fullscreen_terminal::FullscreenTerminal;
use crate::views::navigation::clear_pane_map;
use crate::views::overlay_manager::{OverlayManager, OverlayManagerEvent};
use crate::views::project_column::ProjectColumn;
use crate::views::sidebar_controller::{SidebarController, AnimationTarget, FRAME_TIME_MS};
use crate::views::sidebar::Sidebar;
use crate::views::split_pane::{get_active_drag, compute_resize, render_project_divider, render_sidebar_divider, DragState};
use crate::keybindings::{ShowKeybindings, ShowSessionManager, ShowThemeSelector, ShowCommandPalette, ShowSettings, OpenSettingsFile, ShowFileSearch, ShowProjectSwitcher, ToggleSidebar, ToggleSidebarAutoHide, CreateWorktree};
use crate::settings::open_settings_file;
use crate::views::status_bar::StatusBar;
use crate::views::title_bar::TitleBar;
use crate::workspace::persistence::{load_settings, AppSettings};
use crate::workspace::state::Workspace;
use gpui::*;
use gpui::prelude::*;
use parking_lot::Mutex;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
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
    /// Fullscreen terminal overlay (synced from workspace state)
    fullscreen_terminal: Option<Entity<FullscreenTerminal>>,
    /// Currently displayed fullscreen state (to detect changes)
    fullscreen_state: Option<(String, String)>,
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
        let sidebar = cx.new(|_cx| Sidebar::new(workspace.clone(), terminals.clone()));

        // Create title bar entity
        let workspace_for_title = workspace.clone();
        let title_bar = cx.new(|_cx| TitleBar::new("Term Manager", workspace_for_title));

        // Create status bar entity
        let status_bar = cx.new(StatusBar::new);

        // Create overlay manager
        let overlay_manager = cx.new(|_cx| OverlayManager::new(workspace.clone()));

        // Subscribe to overlay manager events
        cx.subscribe(&overlay_manager, Self::handle_overlay_manager_event).detach();

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
            fullscreen_terminal: None,
            fullscreen_state: None,
            focus_handle,
        };

        // Initialize project columns
        view.sync_project_columns(cx);

        view
    }

    /// Handle events from the OverlayManager that require RootView access.
    fn handle_overlay_manager_event(
        &mut self,
        _: Entity<OverlayManager>,
        event: &OverlayManagerEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            OverlayManagerEvent::SwitchWorkspace(data) => {
                self.handle_switch_workspace(data.clone(), cx);
            }
            OverlayManagerEvent::WorktreeCreated(new_project_id) => {
                self.spawn_terminals_for_project(new_project_id.clone(), cx);
            }
            OverlayManagerEvent::ShellSelected { shell_type, project_id, terminal_id } => {
                self.workspace.update(cx, |ws, cx| {
                    ws.set_terminal_shell_by_id(project_id, terminal_id, shell_type.clone(), cx);
                });
            }
            OverlayManagerEvent::AddTerminal { project_id } => {
                self.workspace.update(cx, |ws, cx| {
                    ws.add_terminal(project_id, cx);
                });
            }
            OverlayManagerEvent::CreateWorktree { project_id, project_path } => {
                self.overlay_manager.update(cx, |om, cx| {
                    om.show_worktree_dialog(project_id.clone(), project_path.clone(), cx);
                });
            }
            OverlayManagerEvent::RenameProject { project_id, project_name } => {
                self.workspace.update(cx, |ws, cx| {
                    ws.request_project_rename(project_id, project_name, cx);
                });
            }
            OverlayManagerEvent::CloseWorktree { project_id } => {
                let result = self.workspace.update(cx, |ws, cx| {
                    ws.remove_worktree_project(project_id, false, cx)
                });
                if let Err(e) = result {
                    log::error!("Failed to close worktree: {}", e);
                }
            }
            OverlayManagerEvent::DeleteProject { project_id } => {
                self.workspace.update(cx, |ws, cx| {
                    ws.delete_project(project_id, cx);
                });
            }
            OverlayManagerEvent::FocusProject(project_id) => {
                self.workspace.update(cx, |ws, cx| {
                    // Focus the project (like clicking on it in sidebar)
                    ws.set_focused_project(Some(project_id.clone()), cx);
                });
            }
            OverlayManagerEvent::ToggleProjectVisibility(project_id) => {
                self.workspace.update(cx, |ws, cx| {
                    ws.toggle_project_visibility(project_id, cx);
                });
            }
        }
    }

    /// Handle workspace switch from session manager.
    fn handle_switch_workspace(&mut self, data: crate::workspace::state::WorkspaceData, cx: &mut Context<Self>) {
        // Kill all existing terminals
        {
            let terminals = self.terminals.lock();
            for terminal in terminals.values() {
                self.pty_manager.kill(&terminal.terminal_id);
            }
        }
        self.terminals.lock().clear();

        // Clear project columns (will be recreated)
        self.project_columns.clear();

        // Clear fullscreen state
        self.fullscreen_terminal = None;
        self.fullscreen_state = None;

        // Update workspace with new data
        self.workspace.update(cx, |ws, cx| {
            ws.data = data;
            ws.focused_project_id = None;
            ws.fullscreen_terminal = None;
            // Clear focus state via FocusManager
            ws.focus_manager.clear_focus();
            ws.focus_manager.clear_stack();
            ws.focused_terminal = None; // Keep legacy field in sync
            ws.detached_terminals.clear();
            cx.notify();
        });

        // Sync project columns for new data
        self.sync_project_columns(cx);

        cx.notify();
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
                let id = project_id.clone();
                let entity = cx.new(move |_cx| {
                    ProjectColumn::new(
                        workspace_clone,
                        id,
                        pty_manager_clone,
                        terminals_clone,
                    )
                });
                self.project_columns.insert(project_id.clone(), entity);
            }
        }
    }

    fn render_projects_grid(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // Sync project columns to handle newly added projects
        self.sync_project_columns(cx);

        let visible_projects: Vec<_> = {
            let workspace = self.workspace.read(cx);
            workspace.visible_projects().iter().map(|p| p.id.clone()).collect()
        };

        let num_projects = visible_projects.len();

        // Get widths for each project
        // When only one project is visible (focused), always use 100%
        // Otherwise, normalize widths so they sum to 100%
        let widths: Vec<f32> = if num_projects == 1 {
            vec![100.0]
        } else if num_projects == 0 {
            vec![]
        } else {
            let workspace = self.workspace.read(cx);
            let raw_widths: Vec<f32> = visible_projects.iter()
                .map(|id| workspace.get_project_width(id, num_projects))
                .collect();

            // Normalize widths to sum to 100%
            let total: f32 = raw_widths.iter().sum();
            if total > 0.0 {
                raw_widths.iter().map(|w| w / total * 100.0).collect()
            } else {
                vec![100.0 / num_projects as f32; num_projects]
            }
        };

        // Shared bounds reference for resize calculation
        let container_bounds = Rc::new(RefCell::new(Bounds {
            origin: Point::default(),
            size: Size { width: px(800.0), height: px(600.0) },
        }));

        // Build interleaved columns and dividers
        let mut elements: Vec<AnyElement> = Vec::new();

        for (i, project_id) in visible_projects.iter().enumerate() {
            let width_percent = widths.get(i).copied().unwrap_or(100.0 / num_projects as f32);

            if let Some(col) = self.project_columns.get(project_id).cloned() {
                let col_element = div()
                    .flex_basis(relative(width_percent / 100.0))
                    .h_full()
                    .min_w(px(200.0))
                    .child(col)
                    .into_any_element();

                elements.push(col_element);

                // Add divider after each column except the last
                if i < num_projects - 1 {
                    let divider = render_project_divider(
                        i,
                        visible_projects.clone(),
                        container_bounds.clone(),
                        cx,
                    );
                    elements.push(divider.into_any_element());
                }
            }
        }

        div()
            .id("projects-grid")
            .flex_1()
            .h_full()
            .flex()
            .overflow_hidden()
            // Canvas to capture container bounds
            .child(canvas(
                {
                    let container_bounds = container_bounds.clone();
                    move |bounds, _window, _cx| {
                        *container_bounds.borrow_mut() = bounds;
                    }
                },
                |_bounds, _prepaint, _window, _cx| {},
            ).absolute().size_full())
            // Mouse handlers are on root div - no need to duplicate here
            .children(elements)
    }

    /// Sync fullscreen terminal entity with workspace state
    fn sync_fullscreen(&mut self, cx: &mut Context<Self>) {
        let current_state: Option<(String, String)> = {
            let workspace = self.workspace.read(cx);
            workspace.fullscreen_terminal.as_ref()
                .map(|fs| (fs.project_id.clone(), fs.terminal_id.clone()))
        };

        // Check if state changed
        if self.fullscreen_state != current_state {
            self.fullscreen_state = current_state.clone();

            if let Some((project_id, terminal_id)) = current_state {
                // Create new fullscreen entity
                let workspace_clone = self.workspace.clone();
                let pty_manager_clone = self.pty_manager.clone();
                let terminals_clone = self.terminals.clone();
                self.fullscreen_terminal = Some(cx.new(move |cx| {
                    FullscreenTerminal::new(
                        workspace_clone,
                        terminal_id,
                        project_id,
                        pty_manager_clone,
                        terminals_clone,
                        cx,
                    )
                }));
            } else {
                // Fullscreen was closed
                self.fullscreen_terminal = None;
            }
        }
    }

    /// Spawn terminals for all layout slots in a project that have terminal_id: None
    /// Used after creating a worktree project to immediately populate terminals
    fn spawn_terminals_for_project(&mut self, project_id: String, cx: &mut Context<Self>) {
        use crate::terminal::terminal::{Terminal, TerminalSize};
        use crate::settings::settings;

        // Get the project path and collect all terminal slots to spawn
        let project_info = {
            let ws = self.workspace.read(cx);
            ws.project(&project_id).map(|p| (p.path.clone(), p.layout.clone()))
        };

        let (project_path, layout) = match project_info {
            Some((path, Some(layout))) => (path, layout),
            Some((_, None)) => {
                log::info!("spawn_terminals_for_project: Project {} has no layout (bookmark)", project_id);
                return;
            }
            None => {
                log::error!("spawn_terminals_for_project: Project {} not found", project_id);
                return;
            }
        };

        // Get the default shell from settings
        let shell = settings(cx).default_shell;

        // Collect all paths to terminal nodes that need spawning
        let mut terminal_paths: Vec<Vec<usize>> = Vec::new();
        Self::collect_empty_terminal_paths(&layout, vec![], &mut terminal_paths);

        log::info!("spawn_terminals_for_project: Found {} empty terminal slots for project {}",
            terminal_paths.len(), project_id);

        // Spawn a terminal for each empty slot
        for path in terminal_paths {
            match self.pty_manager.create_terminal_with_shell(&project_path, Some(&shell)) {
                Ok(terminal_id) => {
                    log::info!("Spawned terminal {} for worktree at path {:?}", terminal_id, path);

                    // Store terminal ID in workspace
                    self.workspace.update(cx, |ws, cx| {
                        ws.set_terminal_id(&project_id, &path, terminal_id.clone(), cx);
                    });

                    // Create terminal wrapper and register it
                    let size = TerminalSize::default();
                    let terminal = std::sync::Arc::new(Terminal::new(
                        terminal_id.clone(),
                        size,
                        self.pty_manager.clone(),
                    ));
                    self.terminals.lock().insert(terminal_id, terminal);
                }
                Err(e) => {
                    log::error!("Failed to spawn terminal for worktree at path {:?}: {}", path, e);
                }
            }
        }

        // Sync project columns to pick up the new project
        self.sync_project_columns(cx);
    }

    /// Recursively collect paths to all Terminal nodes with terminal_id: None
    fn collect_empty_terminal_paths(
        node: &crate::workspace::state::LayoutNode,
        current_path: Vec<usize>,
        result: &mut Vec<Vec<usize>>,
    ) {
        match node {
            crate::workspace::state::LayoutNode::Terminal { terminal_id, .. } => {
                if terminal_id.is_none() {
                    result.push(current_path);
                }
            }
            crate::workspace::state::LayoutNode::Split { children, .. }
            | crate::workspace::state::LayoutNode::Tabs { children, .. } => {
                for (i, child) in children.iter().enumerate() {
                    let mut child_path = current_path.clone();
                    child_path.push(i);
                    Self::collect_empty_terminal_paths(child, child_path, result);
                }
            }
        }
    }

    /// Create worktree from the focused project
    fn create_worktree_from_focus(&mut self, cx: &mut Context<Self>) {
        // Get the focused project ID and info
        let project_info = {
            let ws = self.workspace.read(cx);
            let project_id = ws.focus_manager.focused_terminal_state()
                .map(|f| f.project_id.clone())
                .or_else(|| {
                    // Fallback: use the first visible project
                    ws.visible_projects()
                        .first()
                        .map(|p| p.id.clone())
                });

            project_id.and_then(|id| {
                ws.project(&id).map(|p| {
                    let project_path = p.path.clone();
                    let is_worktree = p.worktree_info.is_some();
                    let is_git = crate::git::get_git_status(std::path::Path::new(&project_path)).is_some();
                    (id, project_path, is_git, is_worktree)
                })
            })
        };

        if let Some((project_id, project_path, is_git, is_worktree)) = project_info {
            if is_git && !is_worktree {
                self.overlay_manager.update(cx, |om, cx| {
                    om.show_worktree_dialog(project_id, project_path, cx);
                });
            } else {
                log::info!("Cannot create worktree: project is not a git repo or is already a worktree");
            }
        }
    }

    /// Toggle sidebar visibility with animation
    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        let target = self.sidebar_ctrl.toggle(&mut self.app_settings);
        self.animate_sidebar_to(target, cx);
    }

    /// Toggle auto-hide mode
    fn toggle_sidebar_auto_hide(&mut self, cx: &mut Context<Self>) {
        let target = self.sidebar_ctrl.toggle_auto_hide(&mut self.app_settings);
        self.animate_sidebar_to(target, cx);
        cx.notify();
    }

    /// Process pending overlay requests from workspace state.
    ///
    /// This handles requests that are set in workspace state and need to trigger
    /// overlay creation in the OverlayManager. Each request is processed once and then
    /// cleared from the workspace.
    fn process_pending_requests(&mut self, cx: &mut Context<Self>) {
        // Check for worktree dialog request
        if let Some(request) = self.workspace.read(cx).worktree_dialog_request.clone() {
            if !self.overlay_manager.read(cx).has_worktree_dialog() {
                self.overlay_manager.update(cx, |om, cx| {
                    om.show_worktree_dialog(request.project_id, request.project_path, cx);
                });
                self.workspace.update(cx, |ws, cx| {
                    ws.clear_worktree_dialog_request(cx);
                });
            }
        }

        // Check for context menu request
        if let Some(request) = self.workspace.read(cx).context_menu_request.clone() {
            if !self.overlay_manager.read(cx).has_context_menu() {
                self.overlay_manager.update(cx, |om, cx| {
                    om.show_context_menu(request.clone(), cx);
                });
                self.workspace.update(cx, |ws, cx| {
                    ws.clear_context_menu_request(cx);
                });
            }
        }

        // Check for shell selector request
        if let Some(request) = self.workspace.read(cx).shell_selector_request.clone() {
            if !self.overlay_manager.read(cx).has_shell_selector() {
                self.overlay_manager.update(cx, |om, cx| {
                    om.show_shell_selector(
                        request.current_shell,
                        request.project_id,
                        request.terminal_id,
                        cx,
                    );
                });
                self.workspace.update(cx, |ws, cx| {
                    ws.clear_shell_selector_request(cx);
                });
            }
        }
    }

    /// Show sidebar temporarily in auto-hide mode
    fn show_sidebar_on_hover(&mut self, cx: &mut Context<Self>) {
        let target = self.sidebar_ctrl.show_on_hover();
        self.animate_sidebar_to(target, cx);
    }

    /// Hide sidebar when mouse leaves in auto-hide mode
    fn hide_sidebar_on_leave(&mut self, cx: &mut Context<Self>) {
        let target = self.sidebar_ctrl.hide_on_leave();
        self.animate_sidebar_to(target, cx);
    }

    /// Animate sidebar to target if needed
    fn animate_sidebar_to(&mut self, target: AnimationTarget, cx: &mut Context<Self>) {
        if let Some(target_value) = target.value() {
            self.animate_sidebar(target_value, cx);
        }
    }

    /// Animate sidebar to target value (0.0 = collapsed, 1.0 = expanded)
    fn animate_sidebar(&mut self, target: f32, cx: &mut Context<Self>) {
        let current = self.sidebar_ctrl.animation();

        // Skip animation if already at target
        if (current - target).abs() < 0.01 {
            self.sidebar_ctrl.set_animation(target);
            cx.notify();
            return;
        }

        let steps = SidebarController::animation_steps();
        let step_duration = std::time::Duration::from_millis(FRAME_TIME_MS);

        cx.spawn(async move |this: WeakEntity<RootView>, cx| {
            for i in 1..=steps {
                smol::Timer::after(step_duration).await;

                let progress = SidebarController::ease_progress(current, target, i, steps);

                let result = this.update(cx, |this, cx| {
                    this.sidebar_ctrl.set_animation(progress);
                    cx.notify();
                });
                if result.is_err() {
                    break;
                }
            }

            // Ensure we reach the target exactly
            let _ = this.update(cx, |this, cx| {
                this.sidebar_ctrl.set_animation(target);
                cx.notify();
            });
        }).detach();
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Sync fullscreen entity with workspace state (creates entity only when state changes)
        self.sync_fullscreen(cx);

        // Process any pending overlay requests from workspace
        self.process_pending_requests(cx);

        let has_fullscreen = self.fullscreen_terminal.is_some();
        if has_fullscreen {
            log::info!("RootView render: has_fullscreen=true, fullscreen_terminal={:?}",
                self.workspace.read(cx).fullscreen_terminal);
        }

        // Get overlay visibility state from overlay manager
        let om = self.overlay_manager.read(cx);
        let has_keybindings_help = om.has_keybindings_help();
        let has_session_manager = om.has_session_manager();
        let has_theme_selector = om.has_theme_selector();
        let has_command_palette = om.has_command_palette();
        let has_settings_panel = om.has_settings_panel();
        let has_project_switcher = om.has_project_switcher();
        let has_shell_selector = om.has_shell_selector();
        let has_worktree_dialog = om.has_worktree_dialog();
        let has_context_menu = om.has_context_menu();
        let has_file_search = om.has_file_search();
        let has_file_viewer = om.has_file_viewer();

        // Clear the pane map at the start of each render cycle
        // Each terminal pane will re-register itself during prepaint
        clear_pane_map();

        // Get active drag for global mouse handling
        let active_drag = get_active_drag(cx);
        let workspace = self.workspace.clone();

        // Capture sidebar state for mouse move handler
        let sidebar_auto_hide = self.sidebar_ctrl.is_auto_hide();
        let sidebar_hover_shown = self.sidebar_ctrl.is_hover_shown();
        let current_sidebar_width = self.sidebar_ctrl.current_width();

        // Clone overlay_manager for action handlers
        let overlay_manager = self.overlay_manager.clone();

        let focus_handle = self.focus_handle.clone();

        // Focus root if nothing else is focused (allows global keybindings to work)
        if window.focused(cx).is_none() {
            window.focus(&focus_handle, cx);
        }

        div()
            .id("root")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(t.bg_primary))
            .track_focus(&focus_handle)
            // Global mouse move handler for resize and auto-hide
            .on_mouse_move(cx.listener({
                let active_drag = active_drag.clone();
                let workspace = workspace.clone();
                move |this, event: &MouseMoveEvent, _window, cx| {
                    // Handle resize drag
                    if let Some(ref state) = *active_drag.borrow() {
                        match state {
                            DragState::Sidebar => {
                                // Handle sidebar resize
                                let new_width = f32::from(event.position.x);
                                this.sidebar_ctrl.set_width(new_width, &mut this.app_settings);
                                cx.notify();
                            }
                            _ => {
                                // Handle split and project column resize
                                compute_resize(event.position, state, &workspace, cx);
                            }
                        }
                    }

                    // Handle auto-hide: check if mouse left the sidebar area
                    if sidebar_auto_hide && sidebar_hover_shown {
                        // Add small margin for smoother interaction
                        let hide_threshold = current_sidebar_width + 10.0;
                        if f32::from(event.position.x) > hide_threshold {
                            this.hide_sidebar_on_leave(cx);
                        }
                    }
                }
            }))
            // Global mouse up handler to end resize
            .on_mouse_up(MouseButton::Left, {
                let active_drag = active_drag.clone();
                let terminals = self.terminals.clone();
                move |_event, _window, _cx| {
                    // Clear drag state
                    let was_dragging = active_drag.borrow().is_some();
                    *active_drag.borrow_mut() = None;

                    // Flush any pending terminal resizes when drag ends
                    // This ensures the final size is sent to the PTY
                    if was_dragging {
                        let terminals_guard = terminals.lock();
                        for terminal in terminals_guard.values() {
                            terminal.flush_pending_resize();
                        }
                    }
                }
            })
            // Handle sidebar toggle action from title bar
            .on_action(cx.listener(|this, _: &ToggleSidebar, _window, cx| {
                this.toggle_sidebar(cx);
            }))
            // Handle toggle sidebar auto-hide action
            .on_action(cx.listener(|this, _: &ToggleSidebarAutoHide, _window, cx| {
                this.toggle_sidebar_auto_hide(cx);
            }))
            // Handle show keybindings action
            .on_action(cx.listener({
                let overlay_manager = overlay_manager.clone();
                move |_this, _: &ShowKeybindings, _window, cx| {
                    overlay_manager.update(cx, |om, cx| om.toggle_keybindings_help(cx));
                }
            }))
            // Handle show session manager action
            .on_action(cx.listener({
                let overlay_manager = overlay_manager.clone();
                move |_this, _: &ShowSessionManager, _window, cx| {
                    overlay_manager.update(cx, |om, cx| om.toggle_session_manager(cx));
                }
            }))
            // Handle show theme selector action
            .on_action(cx.listener({
                let overlay_manager = overlay_manager.clone();
                move |_this, _: &ShowThemeSelector, _window, cx| {
                    overlay_manager.update(cx, |om, cx| om.toggle_theme_selector(cx));
                }
            }))
            // Handle show command palette action
            .on_action(cx.listener({
                let overlay_manager = overlay_manager.clone();
                move |_this, _: &ShowCommandPalette, _window, cx| {
                    overlay_manager.update(cx, |om, cx| om.toggle_command_palette(cx));
                }
            }))
            // Handle show settings panel action
            .on_action(cx.listener({
                let overlay_manager = overlay_manager.clone();
                move |_this, _: &ShowSettings, _window, cx| {
                    overlay_manager.update(cx, |om, cx| om.toggle_settings_panel(cx));
                }
            }))
            // Handle open settings file action
            .on_action(cx.listener(|_this, _: &OpenSettingsFile, _window, _cx| {
                open_settings_file();
            }))
            // Handle create worktree action
            .on_action(cx.listener(|this, _: &CreateWorktree, _window, cx| {
                this.create_worktree_from_focus(cx);
            }))
            // Handle show file search action
            .on_action(cx.listener({
                let overlay_manager = overlay_manager.clone();
                let workspace = workspace.clone();
                move |_this, _: &ShowFileSearch, _window, cx| {
                    // Get the focused or first visible project path
                    let project_path = workspace.read(cx).focus_manager.focused_terminal_state()
                        .map(|f| f.project_id.clone())
                        .or_else(|| {
                            workspace.read(cx).visible_projects()
                                .first()
                                .map(|p| p.id.clone())
                        })
                        .and_then(|id| {
                            workspace.read(cx).project(&id).map(|p| p.path.clone())
                        });

                    if let Some(path) = project_path {
                        overlay_manager.update(cx, |om, cx| {
                            om.toggle_file_search(std::path::PathBuf::from(path), cx);
                        });
                    }
                }
            }))
            // Handle show project switcher action
            .on_action(cx.listener({
                let overlay_manager = overlay_manager.clone();
                move |_this, _: &ShowProjectSwitcher, _window, cx| {
                    overlay_manager.update(cx, |om, cx| om.toggle_project_switcher(cx));
                }
            }))
            // Title bar at the top (with window controls)
            .child(self.title_bar.clone())
            // Main content area
            .child(
                // Content below title bar
                div()
                    .flex_1()
                    .flex()
                    .min_h_0()
                    .relative()
                    // Auto-hide hover zone (invisible strip on the left edge)
                    .when(self.sidebar_ctrl.is_auto_hide() && !self.sidebar_ctrl.is_open() && !self.sidebar_ctrl.is_hover_shown(), |d| {
                        d.child(
                            div()
                                .id("sidebar-hover-zone")
                                .absolute()
                                .left_0()
                                .top_0()
                                .h_full()
                                .w(px(8.0))
                                .hover(|s| s.cursor_pointer())
                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                                    this.show_sidebar_on_hover(cx);
                                }))
                                .on_mouse_move(cx.listener(|this, _, _window, cx| {
                                    this.show_sidebar_on_hover(cx);
                                }))
                        )
                    })
                    .child(
                        // Sidebar container - animated width
                        {
                            let sidebar_width = self.sidebar_ctrl.current_width();
                            let configured_width = self.sidebar_ctrl.width();
                            let show_sidebar = self.sidebar_ctrl.should_render();

                            div()
                                .id("sidebar-container")
                                .h_full()
                                .w(px(sidebar_width))
                                .overflow_hidden()
                                .flex_shrink_0()
                                .when(show_sidebar, |d| {
                                    d.child(
                                        // Inner wrapper to maintain sidebar at full width for clipping effect
                                        div()
                                            .w(px(configured_width))
                                            .h_full()
                                            .child(self.sidebar.clone())
                                    )
                                })
                        }
                    )
                    // Sidebar resize divider (only when sidebar is visible)
                    .when(self.sidebar_ctrl.should_render(), |d| {
                        d.child(render_sidebar_divider(cx))
                    })
                    .child(
                        // Main area
                        div()
                            .id("main-area")
                            .flex_1()
                            .flex()
                            .flex_col()
                            .min_h_0()
                            .child(
                                // Projects grid OR fullscreen (mutually exclusive)
                                div()
                                    .id("projects-container")
                                    .flex_1()
                                    .min_h_0()
                                    .size_full()
                                    .when(has_fullscreen, |d| {
                                        if let Some(fullscreen) = &self.fullscreen_terminal {
                                            d.child(fullscreen.clone())
                                        } else {
                                            d.child(self.render_projects_grid(cx))
                                        }
                                    })
                                    .when(!has_fullscreen, |d| {
                                        d.child(self.render_projects_grid(cx))
                                    }),
                            ),
                    ),
            )
            // Status bar at the bottom
            .child(self.status_bar.clone())
            // Keybindings help overlay (renders on top of everything)
            .when(has_keybindings_help, |d| {
                d.children(self.overlay_manager.read(cx).render_keybindings_help())
            })
            // Session manager overlay (renders on top of everything)
            .when(has_session_manager, |d| {
                if let Some(manager) = self.overlay_manager.read(cx).render_session_manager() {
                    d.child(manager)
                } else {
                    d
                }
            })
            // Theme selector overlay (renders on top of everything)
            .when(has_theme_selector, |d| {
                d.children(self.overlay_manager.read(cx).render_theme_selector())
            })
            // Command palette overlay (renders on top of everything)
            .when(has_command_palette, |d| {
                d.children(self.overlay_manager.read(cx).render_command_palette())
            })
            // Settings panel overlay (renders on top of everything)
            .when(has_settings_panel, |d| {
                d.children(self.overlay_manager.read(cx).render_settings_panel())
            })
            // Project switcher overlay (renders on top of everything)
            .when(has_project_switcher, |d| {
                d.children(self.overlay_manager.read(cx).render_project_switcher())
            })
            // Shell selector overlay (renders on top of everything)
            .when(has_shell_selector, |d| {
                d.children(self.overlay_manager.read(cx).render_shell_selector())
            })
            // Worktree dialog overlay (renders on top of everything)
            .when(has_worktree_dialog, |d| {
                if let Some(dialog) = self.overlay_manager.read(cx).render_worktree_dialog() {
                    d.child(dialog)
                } else {
                    d
                }
            })
            // Context menu overlay (renders on top of everything)
            .when(has_context_menu, |d| {
                if let Some(menu) = self.overlay_manager.read(cx).render_context_menu() {
                    d.child(menu)
                } else {
                    d
                }
            })
            // File search overlay (renders on top of everything)
            .when(has_file_search, |d| {
                if let Some(dialog) = self.overlay_manager.read(cx).render_file_search() {
                    d.child(dialog)
                } else {
                    d
                }
            })
            // File viewer overlay (renders on top of everything)
            .when(has_file_viewer, |d| {
                if let Some(viewer) = self.overlay_manager.read(cx).render_file_viewer() {
                    d.child(viewer)
                } else {
                    d
                }
            })
    }
}

impl_focusable!(RootView);
