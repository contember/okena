mod handlers;
mod pane_switcher;
mod render;
mod sidebar;
mod terminal_actions;

use crate::git::watcher::GitStatusWatcher;
use crate::remote_client::manager::RemoteConnectionManager;
use crate::terminal::backend::{TerminalBackend, LocalBackend};
use crate::terminal::pty_manager::PtyManager;
use crate::terminal::terminal::Terminal;
use crate::views::overlay_manager::OverlayManager;
use crate::views::panels::project_column::ProjectColumn;
use crate::views::sidebar_controller::SidebarController;
use crate::views::panels::sidebar::Sidebar;
use crate::views::layout::split_pane::{new_active_drag, ActiveDrag};
use crate::views::panels::status_bar::StatusBar;
use crate::views::panels::toast::ToastOverlay;
use crate::views::chrome::title_bar::TitleBar;
use crate::workspace::persistence::{load_settings, AppSettings};
use crate::workspace::request_broker::RequestBroker;
use crate::workspace::state::Workspace;
use gpui::*;
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
    request_broker: Entity<RequestBroker>,
    backend: Arc<dyn TerminalBackend>,
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
    /// Toast notification overlay
    toast_overlay: Entity<ToastOverlay>,
    /// Shared drag state for resize operations
    active_drag: ActiveDrag,
    /// Focus handle for capturing global keybindings
    focus_handle: FocusHandle,
    /// Scroll handle for horizontal scrolling of project columns
    projects_scroll_handle: ScrollHandle,
    /// Persistent container bounds for projects grid (used to compute pixel widths)
    projects_grid_bounds: Rc<RefCell<Bounds<Pixels>>>,
    /// Horizontal scrollbar drag state
    hscroll_dragging: bool,
    hscroll_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    /// Remote connection manager (set after creation)
    remote_manager: Option<Entity<RemoteConnectionManager>>,
    /// Git status watcher (set by Okena after creation)
    git_watcher: Option<Entity<GitStatusWatcher>>,
    /// Whether the pane switcher overlay is active
    pane_switch_active: bool,
    /// Pane switcher overlay entity (separate entity for proper focus handling)
    pane_switcher_entity: Option<Entity<pane_switcher::PaneSwitcher>>,
}

impl RootView {
    pub fn new(
        workspace: Entity<Workspace>,
        request_broker: Entity<RequestBroker>,
        pty_manager: Arc<PtyManager>,
        cx: &mut Context<Self>,
    ) -> Self {
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(HashMap::new()));

        // Load app settings and create sidebar controller
        let app_settings = load_settings();
        let sidebar_ctrl = SidebarController::new(&app_settings);

        // Create sidebar entity once to preserve state
        let sidebar = cx.new(|cx| Sidebar::new(workspace.clone(), request_broker.clone(), terminals.clone(), cx));

        // Create title bar entity (sync initial sidebar state)
        let sidebar_initially_open = sidebar_ctrl.is_open();
        let title_bar = cx.new(|cx| {
            let mut tb = TitleBar::new("Okena");
            tb.set_sidebar_open(sidebar_initially_open, cx);
            tb
        });

        // Create status bar entity (sync initial sidebar state)
        let workspace_for_status = workspace.clone();
        let status_bar = cx.new(|cx| {
            let mut sb = StatusBar::new(workspace_for_status, cx);
            sb.set_sidebar_open(sidebar_initially_open, cx);
            sb
        });

        // Create overlay manager
        let overlay_manager = cx.new(|_cx| OverlayManager::new(workspace.clone(), request_broker.clone()));

        // Create toast overlay
        let toast_overlay = cx.new(ToastOverlay::new);

        // Subscribe to overlay manager events
        cx.subscribe(&overlay_manager, Self::handle_overlay_manager_event).detach();

        // Observe RequestBroker to process overlay requests outside of render()
        cx.observe(&request_broker, |this, _broker, cx| {
            if this.request_broker.read(cx).has_overlay_requests() {
                this.process_pending_requests(cx);
            }
        }).detach();

        // Create focus handle for global keybindings
        let focus_handle = cx.focus_handle();

        // Wrap PtyManager in LocalBackend for the TerminalBackend trait
        let backend: Arc<dyn TerminalBackend> = Arc::new(LocalBackend::new(pty_manager));

        // Give the sidebar access to the backend for building dispatchers
        sidebar.update(cx, |s, _cx| {
            s.set_backend(backend.clone());
        });

        let mut view = Self {
            workspace,
            request_broker,
            backend,
            terminals,
            sidebar,
            sidebar_ctrl,
            app_settings,
            project_columns: HashMap::new(),
            title_bar,
            status_bar,
            overlay_manager,
            toast_overlay,
            active_drag: new_active_drag(),
            focus_handle,
            projects_scroll_handle: ScrollHandle::new(),
            projects_grid_bounds: Rc::new(RefCell::new(Bounds {
                origin: Point::default(),
                size: Size { width: px(800.0), height: px(600.0) },
            })),
            hscroll_dragging: false,
            hscroll_bounds: Rc::new(RefCell::new(None)),
            remote_manager: None,
            git_watcher: None,
            pane_switch_active: false,
            pane_switcher_entity: None,
        };

        // Initialize project columns
        view.sync_project_columns(cx);

        view
    }

    /// Get the terminals registry (for sharing with detached windows)
    pub fn terminals(&self) -> &TerminalsRegistry {
        &self.terminals
    }

    /// Set the git watcher entity (called by Okena after creation).
    pub fn set_git_watcher(&mut self, watcher: Entity<GitStatusWatcher>) {
        self.git_watcher = Some(watcher);
    }

    /// Set the remote connection manager (called after creation by Okena).
    pub fn set_remote_manager(&mut self, manager: Entity<RemoteConnectionManager>, cx: &mut Context<Self>) {
        // Observe remote manager and sync remote projects into workspace
        let workspace = self.workspace.clone();
        cx.observe(&manager, move |this, rm, cx| {
            Self::sync_remote_projects_into_workspace(&workspace, &rm, cx);
            this.sync_project_columns(cx);
            cx.notify();
        }).detach();

        // Pass to sidebar
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.set_remote_manager(manager.clone(), cx);
        });

        self.remote_manager = Some(manager);
    }

    /// Sync remote connection state into workspace as materialized ProjectData entries.
    fn sync_remote_projects_into_workspace(
        workspace: &Entity<Workspace>,
        rm: &Entity<RemoteConnectionManager>,
        cx: &mut Context<Self>,
    ) {
        use crate::workspace::state::{FolderData, ProjectData, LayoutNode};
        use crate::theme::FolderColor;
        use crate::workspace::settings::HooksConfig;
        use okena_core::client::RemoteConnectionConfig;

        // Snapshot all connection data into owned structures to release the borrow on cx
        struct ConnSnapshot {
            config: RemoteConnectionConfig,
            state: Option<okena_core::api::StateResponse>,
        }
        let snapshots: Vec<ConnSnapshot> = {
            let rm_read = rm.read(cx);
            rm_read.connections().iter().map(|(config, _status, state)| {
                ConnSnapshot {
                    config: (*config).clone(),
                    state: state.cloned(),
                }
            }).collect()
        };

        let mut expected_remote_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let active_conn_ids: std::collections::HashSet<String> = snapshots.iter()
            .map(|s| s.config.id.clone()).collect();

        for snap in &snapshots {
            let conn_id = &snap.config.id;
            let folder_id = format!("remote-folder:{}", conn_id);

            if let Some(ref state) = snap.state {
                let mut folder_project_ids: Vec<String> = Vec::new();

                for api_project in &state.projects {
                    let prefixed_id = format!("remote:{}:{}", conn_id, api_project.id);
                    expected_remote_ids.insert(prefixed_id.clone());
                    folder_project_ids.push(prefixed_id.clone());

                    let layout = api_project.layout.as_ref().map(|l| {
                        LayoutNode::from_api_prefixed(l, &format!("remote:{}", conn_id))
                    });

                    let terminal_names: std::collections::HashMap<String, String> = api_project.terminal_names.iter()
                        .map(|(k, v)| (format!("remote:{}:{}", conn_id, k), v.clone()))
                        .collect();

                    let conn_id_owned = conn_id.clone();
                    workspace.update(cx, |ws, _cx| {
                        if let Some(existing) = ws.data.projects.iter_mut().find(|p| p.id == prefixed_id) {
                            existing.name = api_project.name.clone();
                            existing.path = api_project.path.clone();
                            // Merge server layout with locally-preserved visual state
                            // (split sizes, minimized, detached, active_tab).
                            existing.layout = match (&existing.layout, &layout) {
                                (Some(local), Some(server)) => {
                                    Some(LayoutNode::merge_visual_state(server, local))
                                }
                                _ => layout,
                            };
                            existing.terminal_names = terminal_names;
                            // Don't overwrite is_visible — it's client-side state
                            // (the user may have toggled visibility locally).
                        } else {
                            ws.data.projects.push(ProjectData {
                                id: prefixed_id.clone(),
                                name: api_project.name.clone(),
                                path: api_project.path.clone(),
                                is_visible: api_project.is_visible,
                                layout,
                                terminal_names,
                                hidden_terminals: std::collections::HashMap::new(),
                                worktree_info: None,
                                folder_color: FolderColor::default(),
                                hooks: HooksConfig::default(),
                                is_remote: true,
                                connection_id: Some(conn_id_owned),
                            });
                        }
                    });
                }

                let folder_name = snap.config.name.clone();
                workspace.update(cx, |ws, _cx| {
                    if let Some(folder) = ws.data.folders.iter_mut().find(|f| f.id == folder_id) {
                        folder.name = folder_name;
                        folder.project_ids = folder_project_ids;
                    } else {
                        ws.data.folders.push(FolderData {
                            id: folder_id.clone(),
                            name: folder_name,
                            project_ids: folder_project_ids,
                            collapsed: false,
                            folder_color: FolderColor::default(),
                        });
                    }
                    if !ws.data.project_order.contains(&folder_id) {
                        ws.data.project_order.push(folder_id.clone());
                    }
                });
            } else {
                // No state (disconnected/connecting) — remove materialized projects
                let prefix = format!("remote:{}:", conn_id);
                workspace.update(cx, |ws, _cx| {
                    ws.data.projects.retain(|p| !p.id.starts_with(&prefix));
                    if let Some(folder) = ws.data.folders.iter_mut().find(|f| f.id == folder_id) {
                        folder.project_ids.clear();
                    }
                });
            }
        }

        // Remove stale remote projects/folders from connections that no longer exist
        workspace.update(cx, |ws, _cx| {
            ws.data.projects.retain(|p| {
                if p.is_remote {
                    expected_remote_ids.contains(&p.id)
                } else {
                    true
                }
            });
            ws.data.folders.retain(|f| {
                if f.id.starts_with("remote-folder:") {
                    let conn_id = f.id.strip_prefix("remote-folder:").unwrap_or("");
                    active_conn_ids.contains(conn_id)
                } else {
                    true
                }
            });
            let valid_ids: std::collections::HashSet<&str> = ws.data.projects.iter().map(|p| p.id.as_str())
                .chain(ws.data.folders.iter().map(|f| f.id.as_str()))
                .collect();
            ws.data.project_order.retain(|id| valid_ids.contains(id.as_str()));
        });

        // Notify UI without bumping data_version (remote changes shouldn't trigger auto-save)
        workspace.update(cx, |ws, cx| {
            ws.notify_ui_only(cx);
        });
    }

    /// Ensure project columns exist for all visible projects
    fn sync_project_columns(&mut self, cx: &mut Context<Self>) {
        let visible_projects: Vec<(String, bool, Option<String>)> = {
            let ws = self.workspace.read(cx);
            ws.visible_projects().iter().map(|p| {
                (p.id.clone(), p.is_remote, p.connection_id.clone())
            }).collect()
        };

        // Clean up columns for projects that no longer exist
        let visible_ids: std::collections::HashSet<&str> = visible_projects.iter()
            .map(|(id, _, _)| id.as_str())
            .collect();
        self.project_columns.retain(|id, _| {
            // Keep local project columns even when not visible (they may become visible again)
            // But remove remote project columns that are gone
            if id.starts_with("remote:") {
                visible_ids.contains(id.as_str())
            } else {
                true
            }
        });

        // Create columns for new projects
        for (project_id, is_remote, connection_id) in &visible_projects {
            if !self.project_columns.contains_key(project_id) {
                let workspace_clone = self.workspace.clone();
                let request_broker_clone = self.request_broker.clone();
                let terminals_clone = self.terminals.clone();
                let active_drag_clone = self.active_drag.clone();
                let id = project_id.clone();

                if *is_remote {
                    // Remote project: use remote backend and action dispatcher
                    if let Some(conn_id) = connection_id {
                        let backend = self.remote_manager.as_ref()
                            .and_then(|rm| rm.read(cx).backend_for(conn_id));
                        let workspace_for_dispatch = self.workspace.clone();
                        let action_dispatcher = self.remote_manager.as_ref().map(|rm| {
                            crate::action_dispatch::ActionDispatcher::Remote {
                                connection_id: conn_id.clone(),
                                manager: rm.clone(),
                                workspace: workspace_for_dispatch,
                            }
                        });

                        if let Some(backend) = backend {
                            let entity = cx.new(move |cx| {
                                let mut col = ProjectColumn::new(
                                    workspace_clone,
                                    request_broker_clone,
                                    id,
                                    backend,
                                    terminals_clone,
                                    active_drag_clone,
                                    None, // remote projects don't get git watcher
                                    cx,
                                );
                                col.set_action_dispatcher(action_dispatcher);
                                col
                            });
                            self.project_columns.insert(project_id.clone(), entity);
                        }
                    }
                } else {
                    // Local project: use local backend
                    let backend_clone = self.backend.clone();
                    let workspace_for_dispatch = self.workspace.clone();
                    let backend_for_dispatch = self.backend.clone();
                    let terminals_for_dispatch = self.terminals.clone();
                    let git_watcher = self.git_watcher.clone();
                    let entity = cx.new(move |cx| {
                        let mut col = ProjectColumn::new(
                            workspace_clone,
                            request_broker_clone,
                            id,
                            backend_clone,
                            terminals_clone,
                            active_drag_clone,
                            git_watcher,
                            cx,
                        );
                        col.set_action_dispatcher(Some(
                            crate::action_dispatch::ActionDispatcher::Local {
                                workspace: workspace_for_dispatch,
                                backend: backend_for_dispatch,
                                terminals: terminals_for_dispatch,
                            },
                        ));
                        col
                    });
                    self.project_columns.insert(project_id.clone(), entity);
                }
            }
        }
    }
}

impl_focusable!(RootView);
