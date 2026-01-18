use crate::terminal::pty_manager::{PtyEvent, PtyManager};
use crate::terminal::terminal::Terminal;
use crate::theme::theme;
use crate::views::command_palette::{CommandPalette, CommandPaletteEvent};
use crate::views::fullscreen_terminal::FullscreenTerminal;
use crate::views::keybindings_help::{KeybindingsHelp, KeybindingsHelpEvent};
use crate::views::navigation::clear_pane_map;
use crate::views::project_column::ProjectColumn;
use crate::views::session_manager::{SessionManager, SessionManagerEvent};
use crate::views::sidebar::Sidebar;
use crate::views::split_pane::{get_active_drag, compute_resize, render_project_divider};
use crate::keybindings::{ShowKeybindings, ShowSessionManager, ShowThemeSelector, ShowCommandPalette, ShowSettings, OpenSettingsFile, ToggleSidebar, ToggleSidebarAutoHide, CreateWorktree};
use crate::settings::open_settings_file;
use crate::views::settings_panel::{SettingsPanel, SettingsPanelEvent};
use crate::views::status_bar::StatusBar;
use crate::views::theme_selector::{ThemeSelector, ThemeSelectorEvent};
use crate::views::title_bar::TitleBar;
use crate::views::worktree_dialog::{WorktreeDialog, WorktreeDialogEvent};
use crate::workspace::persistence::{load_settings, save_settings, AppSettings};
use crate::workspace::state::{ContextMenuRequest, Workspace};
use crate::git;
use async_channel::Receiver;
use gpui::*;
use gpui::prelude::*;
use parking_lot::Mutex;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// Shared terminals registry for PTY event routing
pub type TerminalsRegistry = Arc<Mutex<HashMap<String, Arc<Terminal>>>>;

/// Sidebar width constant
const SIDEBAR_WIDTH: f32 = 250.0;

/// Root view of the application
pub struct RootView {
    workspace: Entity<Workspace>,
    pty_manager: Arc<PtyManager>,
    terminals: TerminalsRegistry,
    sidebar: Entity<Sidebar>,
    sidebar_open: bool,
    /// Animation progress for sidebar (0.0 = collapsed, 1.0 = fully open)
    sidebar_animation: f32,
    /// Whether auto-hide mode is enabled
    sidebar_auto_hide: bool,
    /// Whether sidebar is temporarily shown in auto-hide mode
    sidebar_hover_shown: bool,
    /// App settings for persistence
    app_settings: AppSettings,
    /// Stored project column entities (created once, not during render)
    project_columns: HashMap<String, Entity<ProjectColumn>>,
    /// Title bar entity
    title_bar: Entity<TitleBar>,
    /// Status bar entity
    status_bar: Entity<StatusBar>,
    /// Keybindings help overlay
    keybindings_help: Option<Entity<KeybindingsHelp>>,
    /// Session manager overlay
    session_manager: Option<Entity<SessionManager>>,
    /// Theme selector overlay
    theme_selector: Option<Entity<ThemeSelector>>,
    /// Command palette overlay
    command_palette: Option<Entity<CommandPalette>>,
    /// Settings panel overlay
    settings_panel: Option<Entity<SettingsPanel>>,
    /// Worktree dialog overlay
    worktree_dialog: Option<Entity<WorktreeDialog>>,
    /// Context menu state: (project_id, position)
    context_menu: Option<ContextMenuRequest>,
    /// Fullscreen terminal overlay (stored to preserve animation state)
    fullscreen_terminal: Option<Entity<FullscreenTerminal>>,
    /// Currently displayed fullscreen state (to detect changes)
    fullscreen_state: Option<(String, String)>,
}

impl RootView {
    pub fn new(
        workspace: Entity<Workspace>,
        pty_manager: Arc<PtyManager>,
        pty_events: Receiver<PtyEvent>,
        cx: &mut Context<Self>,
    ) -> Self {
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(HashMap::new()));

        // Load app settings
        let app_settings = load_settings();
        let sidebar_open = app_settings.sidebar.is_open;
        let sidebar_auto_hide = app_settings.sidebar.auto_hide;

        // Create sidebar entity once to preserve state
        let sidebar = cx.new(|_cx| Sidebar::new(workspace.clone(), SIDEBAR_WIDTH, terminals.clone()));

        // Create title bar entity
        let workspace_for_title = workspace.clone();
        let title_bar = cx.new(|_cx| TitleBar::new("Term Manager", workspace_for_title));

        // Create status bar entity
        let status_bar = cx.new(|cx| StatusBar::new(cx));

        let mut view = Self {
            workspace,
            pty_manager,
            terminals,
            sidebar,
            sidebar_open,
            sidebar_animation: if sidebar_open { 1.0 } else { 0.0 },
            sidebar_auto_hide,
            sidebar_hover_shown: false,
            app_settings,
            project_columns: HashMap::new(),
            title_bar,
            status_bar,
            keybindings_help: None,
            session_manager: None,
            theme_selector: None,
            command_palette: None,
            settings_panel: None,
            worktree_dialog: None,
            context_menu: None,
            fullscreen_terminal: None,
            fullscreen_state: None,
        };

        // Initialize project columns
        view.sync_project_columns(cx);

        // Start PTY event loop
        view.start_pty_event_loop(pty_events, cx);

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

    fn start_pty_event_loop(
        &mut self,
        pty_events: Receiver<PtyEvent>,
        cx: &mut Context<Self>,
    ) {
        let terminals = self.terminals.clone();

        // PTY event loop - processes all events and notifies once per batch
        cx.spawn(async move |this: WeakEntity<RootView>, cx| {
            loop {
                // Wait for an event
                let event = match pty_events.recv().await {
                    Ok(event) => event,
                    Err(_) => break, // Channel closed
                };

                // Process first event
                match &event {
                    PtyEvent::Data { terminal_id, data } => {
                        let terminals_guard = terminals.lock();
                        if let Some(terminal) = terminals_guard.get(terminal_id) {
                            terminal.process_output(data);
                        }
                    }
                    PtyEvent::Exit { terminal_id, .. } => {
                        terminals.lock().remove(terminal_id);
                    }
                }

                // Drain any additional pending events (batch processing)
                while let Ok(event) = pty_events.try_recv() {
                    match &event {
                        PtyEvent::Data { terminal_id, data } => {
                            let terminals_guard = terminals.lock();
                            if let Some(terminal) = terminals_guard.get(terminal_id) {
                                terminal.process_output(data);
                            }
                        }
                        PtyEvent::Exit { terminal_id, .. } => {
                            terminals.lock().remove(terminal_id);
                        }
                    }
                }

                // Notify once after processing the batch
                let _ = this.update(cx, |_this, cx| {
                    cx.notify();
                });
            }
        })
        .detach();
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

    fn show_keybindings_help(&mut self, cx: &mut Context<Self>) {
        if self.keybindings_help.is_some() {
            // Toggle off if already showing
            self.keybindings_help = None;
        } else {
            let help = cx.new(|cx| KeybindingsHelp::new(cx));
            cx.subscribe(&help, |this, _, event: &KeybindingsHelpEvent, cx| {
                match event {
                    KeybindingsHelpEvent::Close => {
                        this.keybindings_help = None;
                        cx.notify();
                    }
                }
            })
            .detach();
            self.keybindings_help = Some(help);
        }
        cx.notify();
    }

    fn show_session_manager(&mut self, cx: &mut Context<Self>) {
        if self.session_manager.is_some() {
            // Toggle off if already showing
            self.session_manager = None;
        } else {
            let workspace = self.workspace.clone();
            let manager = cx.new(|cx| SessionManager::new(workspace, cx));
            cx.subscribe(&manager, |this, _, event: &SessionManagerEvent, cx| {
                match event {
                    SessionManagerEvent::Close => {
                        this.session_manager = None;
                        cx.notify();
                    }
                    SessionManagerEvent::SwitchWorkspace(data) => {
                        // Close the session manager
                        this.session_manager = None;

                        // Kill all existing terminals
                        {
                            let terminals = this.terminals.lock();
                            for terminal in terminals.values() {
                                this.pty_manager.kill(&terminal.terminal_id);
                            }
                        }
                        this.terminals.lock().clear();

                        // Clear project columns (will be recreated)
                        this.project_columns.clear();

                        // Clear fullscreen state
                        this.fullscreen_terminal = None;
                        this.fullscreen_state = None;

                        // Update workspace with new data
                        this.workspace.update(cx, |ws, cx| {
                            ws.data = data.clone();
                            ws.focused_project_id = None;
                            ws.fullscreen_terminal = None;
                            ws.focused_terminal = None;
                            ws.detached_terminals.clear();
                            cx.notify();
                        });

                        // Sync project columns for new data
                        this.sync_project_columns(cx);

                        cx.notify();
                    }
                }
            })
            .detach();
            self.session_manager = Some(manager);
        }
        cx.notify();
    }

    fn show_theme_selector(&mut self, cx: &mut Context<Self>) {
        if self.theme_selector.is_some() {
            // Toggle off if already showing
            self.theme_selector = None;
        } else {
            let selector = cx.new(|cx| ThemeSelector::new(cx));
            cx.subscribe(&selector, |this, _, event: &ThemeSelectorEvent, cx| {
                match event {
                    ThemeSelectorEvent::Close => {
                        this.theme_selector = None;
                        cx.notify();
                    }
                }
            })
            .detach();
            self.theme_selector = Some(selector);
        }
        cx.notify();
    }

    fn show_command_palette(&mut self, cx: &mut Context<Self>) {
        if self.command_palette.is_some() {
            // Toggle off if already showing
            self.command_palette = None;
        } else {
            let palette = cx.new(|cx| CommandPalette::new(cx));
            cx.subscribe(&palette, |this, _, event: &CommandPaletteEvent, cx| {
                match event {
                    CommandPaletteEvent::Close => {
                        this.command_palette = None;
                        cx.notify();
                    }
                }
            })
            .detach();
            self.command_palette = Some(palette);
        }
        cx.notify();
    }

    fn show_settings_panel(&mut self, cx: &mut Context<Self>) {
        if self.settings_panel.is_some() {
            // Toggle off if already showing
            self.settings_panel = None;
        } else {
            let panel = cx.new(|cx| SettingsPanel::new(cx));
            cx.subscribe(&panel, |this, _, event: &SettingsPanelEvent, cx| {
                match event {
                    SettingsPanelEvent::Close => {
                        this.settings_panel = None;
                        cx.notify();
                    }
                }
            })
            .detach();
            self.settings_panel = Some(panel);
        }
        cx.notify();
    }

    /// Show worktree dialog for a project
    pub fn show_worktree_dialog(&mut self, project_id: String, project_path: String, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let dialog = cx.new(|cx| {
            WorktreeDialog::new(workspace, project_id, project_path, cx)
        });
        cx.subscribe(&dialog, |this, _, event: &WorktreeDialogEvent, cx| {
            match event {
                WorktreeDialogEvent::Close => {
                    this.close_worktree_dialog(cx);
                }
                WorktreeDialogEvent::Created(new_project_id) => {
                    // Spawn terminals for the new worktree project
                    this.spawn_terminals_for_project(new_project_id.clone(), cx);
                    this.close_worktree_dialog(cx);
                }
            }
        })
        .detach();
        self.worktree_dialog = Some(dialog);
        // Clear focused terminal during modal
        self.workspace.update(cx, |ws, cx| {
            ws.clear_focused_terminal(cx);
        });
        cx.notify();
    }

    /// Close worktree dialog
    fn close_worktree_dialog(&mut self, cx: &mut Context<Self>) {
        self.worktree_dialog = None;
        // Restore focus after modal
        self.workspace.update(cx, |ws, cx| {
            ws.restore_focused_terminal(cx);
        });
        cx.notify();
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
            Some(info) => info,
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

    /// Show context menu for a project
    fn show_context_menu(&mut self, request: ContextMenuRequest, cx: &mut Context<Self>) {
        self.context_menu = Some(request);
        cx.notify();
    }

    /// Hide context menu
    fn hide_context_menu(&mut self, cx: &mut Context<Self>) {
        self.context_menu = None;
        self.workspace.update(cx, |ws, cx| {
            ws.clear_context_menu_request(cx);
        });
        cx.notify();
    }

    /// Render context menu overlay
    fn render_context_menu(&self, request: &ContextMenuRequest, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.clone();
        let project_id = request.project_id.clone();
        let position = request.position;

        // Get project info
        let ws = self.workspace.read(cx);
        let project = ws.project(&project_id);
        let project_name = project.map(|p| p.name.clone()).unwrap_or_default();
        let project_path = project.map(|p| p.path.clone()).unwrap_or_default();
        let is_worktree = project.map(|p| p.worktree_info.is_some()).unwrap_or(false);
        let is_git_repo = git::get_git_status(std::path::Path::new(&project_path)).is_some();

        let project_id_for_add = project_id.clone();
        let project_id_for_worktree = project_id.clone();
        let project_id_for_rename = project_id.clone();
        let project_id_for_close_wt = project_id.clone();
        let project_id_for_delete = project_id.clone();
        let project_name_for_rename = project_name.clone();
        let project_path_for_worktree = project_path.clone();

        div()
            .absolute()
            .inset_0()
            .id("context-menu-backdrop")
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.hide_context_menu(cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| {
                this.hide_context_menu(cx);
            }))
            .child(
                div()
                    .absolute()
                    .left(position.x)
                    .top(position.y)
                    .bg(rgb(t.bg_primary))
                    .border_1()
                    .border_color(rgb(t.border))
                    .rounded(px(4.0))
                    .shadow_xl()
                    .min_w(px(160.0))
                    .py(px(4.0))
                    .id("project-context-menu")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        // Add terminal option
                        div()
                            .id("context-menu-add-terminal")
                            .px(px(12.0))
                            .py(px(6.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .cursor_pointer()
                            .text_size(px(12.0))
                            .text_color(rgb(t.text_primary))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .child(
                                svg()
                                    .path("icons/plus.svg")
                                    .size(px(14.0))
                                    .text_color(rgb(t.text_secondary))
                            )
                            .child("Add Terminal")
                            .on_click({
                                let workspace = workspace.clone();
                                cx.listener(move |this, _, _window, cx| {
                                    this.hide_context_menu(cx);
                                    workspace.update(cx, |ws, cx| {
                                        ws.add_terminal(&project_id_for_add, cx);
                                    });
                                })
                            }),
                    )
                    // Create Worktree option (only for git repos that are not already worktrees)
                    .when(is_git_repo && !is_worktree, |d| {
                        d.child(
                            div()
                                .id("context-menu-create-worktree")
                                .px(px(12.0))
                                .py(px(6.0))
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .cursor_pointer()
                                .text_size(px(12.0))
                                .text_color(rgb(t.text_primary))
                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                .child(
                                    svg()
                                        .path("icons/git-branch.svg")
                                        .size(px(14.0))
                                        .text_color(rgb(t.text_secondary))
                                )
                                .child("Create Worktree...")
                                .on_click(cx.listener({
                                    let project_id = project_id_for_worktree.clone();
                                    let project_path = project_path_for_worktree.clone();
                                    move |this, _, _window, cx| {
                                        this.hide_context_menu(cx);
                                        this.show_worktree_dialog(project_id.clone(), project_path.clone(), cx);
                                    }
                                }))
                        )
                    })
                    // Separator
                    .child(
                        div()
                            .h(px(1.0))
                            .mx(px(8.0))
                            .my(px(4.0))
                            .bg(rgb(t.border)),
                    )
                    .child(
                        // Rename option - note: rename is complex because it's in sidebar, just delete for now
                        div()
                            .id("context-menu-rename")
                            .px(px(12.0))
                            .py(px(6.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .cursor_pointer()
                            .text_size(px(12.0))
                            .text_color(rgb(t.text_primary))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .child(
                                svg()
                                    .path("icons/edit.svg")
                                    .size(px(14.0))
                                    .text_color(rgb(t.text_secondary))
                            )
                            .child("Rename Project")
                            .on_click({
                                cx.listener(move |this, _, _window, cx| {
                                    this.hide_context_menu(cx);
                                    // For now, just log - rename requires sidebar state
                                    log::info!("Rename project: {} ({})", project_name_for_rename, project_id_for_rename);
                                })
                            }),
                    )
                    // Close Worktree option (only for worktree projects)
                    .when(is_worktree, |d| {
                        d.child(
                            div()
                                .id("context-menu-close-worktree")
                                .px(px(12.0))
                                .py(px(6.0))
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .cursor_pointer()
                                .text_size(px(12.0))
                                .text_color(rgb(t.warning))
                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                .child(
                                    svg()
                                        .path("icons/git-branch.svg")
                                        .size(px(14.0))
                                        .text_color(rgb(t.warning))
                                )
                                .child("Close Worktree")
                                .on_click({
                                    let workspace = workspace.clone();
                                    let project_id = project_id_for_close_wt.clone();
                                    cx.listener(move |this, _, _window, cx| {
                                        this.hide_context_menu(cx);
                                        let result = workspace.update(cx, |ws, cx| {
                                            ws.remove_worktree_project(&project_id, false, cx)
                                        });
                                        if let Err(e) = result {
                                            log::error!("Failed to close worktree: {}", e);
                                        }
                                    })
                                })
                        )
                    })
                    .child(
                        // Delete option
                        div()
                            .id("context-menu-delete")
                            .px(px(12.0))
                            .py(px(6.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .cursor_pointer()
                            .text_size(px(12.0))
                            .text_color(rgb(t.error))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .child(
                                svg()
                                    .path("icons/trash.svg")
                                    .size(px(14.0))
                                    .text_color(rgb(t.error))
                            )
                            .child("Delete Project")
                            .on_click({
                                let workspace = workspace.clone();
                                cx.listener(move |this, _, _window, cx| {
                                    this.hide_context_menu(cx);
                                    workspace.update(cx, |ws, cx| {
                                        ws.delete_project(&project_id_for_delete, cx);
                                    });
                                })
                            }),
                    ),
            )
    }

    /// Create worktree from the focused project
    fn create_worktree_from_focus(&mut self, cx: &mut Context<Self>) {
        // Get the focused project ID and info
        let project_info = {
            let ws = self.workspace.read(cx);
            let project_id = ws.focused_terminal
                .as_ref()
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
                self.show_worktree_dialog(project_id, project_path, cx);
            } else {
                log::info!("Cannot create worktree: project is not a git repo or is already a worktree");
            }
        }
    }

    /// Toggle sidebar visibility with animation
    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        self.sidebar_hover_shown = false;

        // Start animation
        let target = if self.sidebar_open { 1.0 } else { 0.0 };
        self.animate_sidebar(target, cx);

        // Persist state
        self.app_settings.sidebar.is_open = self.sidebar_open;
        let _ = save_settings(&self.app_settings);
    }

    /// Toggle auto-hide mode
    fn toggle_sidebar_auto_hide(&mut self, cx: &mut Context<Self>) {
        self.sidebar_auto_hide = !self.sidebar_auto_hide;

        // If auto-hide was just enabled, close the sidebar
        if self.sidebar_auto_hide && self.sidebar_open {
            self.sidebar_open = false;
            self.animate_sidebar(0.0, cx);
        }

        // Persist state
        self.app_settings.sidebar.auto_hide = self.sidebar_auto_hide;
        self.app_settings.sidebar.is_open = self.sidebar_open;
        let _ = save_settings(&self.app_settings);

        cx.notify();
    }

    /// Show sidebar temporarily in auto-hide mode
    fn show_sidebar_on_hover(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_auto_hide && !self.sidebar_open && !self.sidebar_hover_shown {
            self.sidebar_hover_shown = true;
            self.animate_sidebar(1.0, cx);
        }
    }

    /// Hide sidebar when mouse leaves in auto-hide mode
    fn hide_sidebar_on_leave(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_auto_hide && self.sidebar_hover_shown {
            self.sidebar_hover_shown = false;
            self.animate_sidebar(0.0, cx);
        }
    }

    /// Animate sidebar to target value (0.0 = collapsed, 1.0 = expanded)
    /// Uses batched updates with fewer re-renders for smoother animation
    fn animate_sidebar(&mut self, target: f32, cx: &mut Context<Self>) {
        let current = self.sidebar_animation;

        // Skip animation if already at target
        if (current - target).abs() < 0.01 {
            self.sidebar_animation = target;
            cx.notify();
            return;
        }

        // Use eased animation with fewer steps but visual smoothness from easing
        cx.spawn(async move |this: WeakEntity<RootView>, cx| {
            let duration_ms = 150;
            let frame_time_ms = 16; // ~60fps
            let steps = duration_ms / frame_time_ms;
            let step_duration = std::time::Duration::from_millis(frame_time_ms as u64);

            for i in 1..=steps {
                smol::Timer::after(step_duration).await;

                // Use ease-out cubic for smoother deceleration
                let t = i as f32 / steps as f32;
                let eased = 1.0 - (1.0 - t).powi(3); // ease-out cubic
                let progress = current + (target - current) * eased;

                let result = this.update(cx, |this, cx| {
                    this.sidebar_animation = progress.clamp(0.0, 1.0);
                    cx.notify();
                });
                if result.is_err() {
                    break;
                }
            }

            // Ensure we reach the target exactly
            let _ = this.update(cx, |this, cx| {
                this.sidebar_animation = target;
                cx.notify();
            });
        }).detach();
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Sync fullscreen entity with workspace state (creates entity only when state changes)
        self.sync_fullscreen(cx);

        // Check for worktree dialog request from workspace
        if let Some(request) = self.workspace.read(cx).worktree_dialog_request.clone() {
            if self.worktree_dialog.is_none() {
                self.show_worktree_dialog(request.project_id, request.project_path, cx);
                self.workspace.update(cx, |ws, cx| {
                    ws.clear_worktree_dialog_request(cx);
                });
            }
        }

        // Check for context menu request from workspace
        if let Some(request) = self.workspace.read(cx).context_menu_request.clone() {
            if self.context_menu.is_none() {
                self.show_context_menu(request.clone(), cx);
                self.workspace.update(cx, |ws, cx| {
                    ws.clear_context_menu_request(cx);
                });
            }
        }

        let has_fullscreen = self.fullscreen_terminal.is_some();
        if has_fullscreen {
            log::info!("RootView render: has_fullscreen=true, fullscreen_terminal={:?}",
                self.workspace.read(cx).fullscreen_terminal);
        }
        let has_keybindings_help = self.keybindings_help.is_some();
        let has_session_manager = self.session_manager.is_some();
        let has_theme_selector = self.theme_selector.is_some();
        let has_command_palette = self.command_palette.is_some();
        let has_settings_panel = self.settings_panel.is_some();
        let has_worktree_dialog = self.worktree_dialog.is_some();
        let has_context_menu = self.context_menu.is_some();

        // Clear the pane map at the start of each render cycle
        // Each terminal pane will re-register itself during prepaint
        clear_pane_map();

        // Get active drag for global mouse handling
        let active_drag = get_active_drag(cx);
        let workspace = self.workspace.clone();

        // Capture state for mouse move handler
        let sidebar_auto_hide = self.sidebar_auto_hide;
        let sidebar_hover_shown = self.sidebar_hover_shown;
        let current_sidebar_width = self.sidebar_animation * SIDEBAR_WIDTH;

        div()
            .id("root")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(t.bg_primary))
            // Global mouse move handler for resize and auto-hide
            .on_mouse_move(cx.listener({
                let active_drag = active_drag.clone();
                let workspace = workspace.clone();
                move |this, event: &MouseMoveEvent, _window, cx| {
                    // Handle resize drag
                    if let Some(ref state) = *active_drag.borrow() {
                        compute_resize(event.position, state, &workspace, cx);
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
            .on_action(cx.listener(|this, _: &ShowKeybindings, _window, cx| {
                this.show_keybindings_help(cx);
            }))
            // Handle show session manager action
            .on_action(cx.listener(|this, _: &ShowSessionManager, _window, cx| {
                this.show_session_manager(cx);
            }))
            // Handle show theme selector action
            .on_action(cx.listener(|this, _: &ShowThemeSelector, _window, cx| {
                this.show_theme_selector(cx);
            }))
            // Handle show command palette action
            .on_action(cx.listener(|this, _: &ShowCommandPalette, _window, cx| {
                this.show_command_palette(cx);
            }))
            // Handle show settings panel action
            .on_action(cx.listener(|this, _: &ShowSettings, _window, cx| {
                this.show_settings_panel(cx);
            }))
            // Handle open settings file action
            .on_action(cx.listener(|_this, _: &OpenSettingsFile, _window, _cx| {
                open_settings_file();
            }))
            // Handle create worktree action
            .on_action(cx.listener(|this, _: &CreateWorktree, _window, cx| {
                this.create_worktree_from_focus(cx);
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
                    .when(self.sidebar_auto_hide && !self.sidebar_open && !self.sidebar_hover_shown, |d| {
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
                            let sidebar_width = self.sidebar_animation * SIDEBAR_WIDTH;
                            let show_sidebar = self.sidebar_animation > 0.01;

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
                                            .w(px(SIDEBAR_WIDTH))
                                            .h_full()
                                            .child(self.sidebar.clone())
                                    )
                                })
                        }
                    )
                    .child(
                        // Main area
                        div()
                            .id("main-area")
                            .flex_1()
                            .flex()
                            .flex_col()
                            .min_h_0()
                            .child(
                                // Projects grid or fullscreen
                                div()
                                    .id("projects-container")
                                    .flex_1()
                                    .min_h_0()
                                    .relative()
                                    .child(self.render_projects_grid(cx))
                                    .when(has_fullscreen, |d| {
                                        if let Some(fullscreen) = &self.fullscreen_terminal {
                                            d.child(fullscreen.clone())
                                        } else {
                                            d
                                        }
                                    }),
                            ),
                    ),
            )
            // Status bar at the bottom
            .child(self.status_bar.clone())
            // Keybindings help overlay (renders on top of everything)
            .when(has_keybindings_help, |d| {
                if let Some(help) = &self.keybindings_help {
                    d.child(help.clone())
                } else {
                    d
                }
            })
            // Session manager overlay (renders on top of everything)
            .when(has_session_manager, |d| {
                if let Some(manager) = &self.session_manager {
                    d.child(manager.clone())
                } else {
                    d
                }
            })
            // Theme selector overlay (renders on top of everything)
            .when(has_theme_selector, |d| {
                if let Some(selector) = &self.theme_selector {
                    d.child(selector.clone())
                } else {
                    d
                }
            })
            // Command palette overlay (renders on top of everything)
            .when(has_command_palette, |d| {
                if let Some(palette) = &self.command_palette {
                    d.child(palette.clone())
                } else {
                    d
                }
            })
            // Settings panel overlay (renders on top of everything)
            .when(has_settings_panel, |d| {
                if let Some(panel) = &self.settings_panel {
                    d.child(panel.clone())
                } else {
                    d
                }
            })
            // Worktree dialog overlay (renders on top of everything)
            .when(has_worktree_dialog, |d| {
                if let Some(dialog) = &self.worktree_dialog {
                    d.child(dialog.clone())
                } else {
                    d
                }
            })
            // Context menu overlay (renders on top of everything)
            .when(has_context_menu, |d| {
                if let Some(request) = &self.context_menu {
                    d.child(self.render_context_menu(request, cx))
                } else {
                    d
                }
            })
    }
}
