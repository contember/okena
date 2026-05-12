mod handlers;
mod pane_switcher;
mod render;
mod sidebar;
mod terminal_actions;

use crate::git::watcher::GitStatusWatcher;
use crate::remote_client::manager::RemoteConnectionManager;
use crate::services::manager::ServiceManager;
use crate::terminal::backend::{TerminalBackend, LocalBackend};
use crate::terminal::pty_manager::PtyManager;
use crate::views::overlay_manager::OverlayManager;
use crate::views::panels::project_column::ProjectColumn;
use crate::views::sidebar_controller::SidebarController;
use crate::views::panels::sidebar::Sidebar;
use crate::views::layout::split_pane::{new_active_drag, ActiveDrag};
use crate::views::panels::status_bar::StatusBar;
use crate::views::panels::toast::ToastOverlay;
use crate::views::chrome::title_bar::TitleBar;
use crate::settings::settings;
use crate::workspace::focus::FocusManager;
use crate::workspace::request_broker::RequestBroker;
use crate::workspace::state::{WindowBounds as PersistedWindowBounds, WindowId, Workspace};
use gpui::*;
use parking_lot::Mutex;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// Shared terminals registry for PTY event routing (re-exported from okena-terminal)
pub use okena_terminal::TerminalsRegistry;

/// Registry mapping `terminal_id` to every `TerminalContent` weak handle that
/// renders that terminal. With multiple windows, the same terminal can render
/// in N project-column instances simultaneously (one per window whose visible
/// set includes the host project), so the PTY notify path must fan out to
/// every live entry. Dead weaks are pruned lazily on iteration.
pub type ContentPaneRegistry = Arc<Mutex<HashMap<String, Vec<WeakEntity<super::layout::terminal_pane::TerminalContent>>>>>;

/// Global content pane registry instance.
static CONTENT_PANE_REGISTRY: std::sync::OnceLock<ContentPaneRegistry> = std::sync::OnceLock::new();

/// Get or init the global content pane registry.
pub fn content_pane_registry() -> &'static ContentPaneRegistry {
    CONTENT_PANE_REGISTRY.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

/// Notify every weak entity in `weaks` via `cx.notify()`; drop dead weaks in
/// place. Returns `true` if at least one weak was alive (so callers can tell
/// whether any UI update was actually triggered). Generic over the target
/// type so the same helper services the multi-window terminal fan-out and is
/// testable without standing up a `TerminalContent`.
pub fn notify_pane_weaks<T: 'static>(
    weaks: &mut Vec<WeakEntity<T>>,
    cx: &mut App,
) -> bool {
    let mut any_alive = false;
    weaks.retain(|w| match w.update(cx, |_, cx| cx.notify()) {
        Ok(_) => {
            any_alive = true;
            true
        }
        Err(_) => false,
    });
    any_alive
}

/// Per-window view of the application: one instance per OS window.
///
/// Owns the per-window UI state (sidebar, overlays, toasts, scroll handles,
/// drag state, project columns) and addresses window-scoped state on the
/// shared `Workspace` via its own `window_id`. The single OS window opened
/// today hosts a `WindowView` for `WindowId::Main`; slice 05 spawns extras
/// that mint distinct `WindowId::Extra(uuid)`s.
pub struct WindowView {
    /// Identifies which window-scoped slot on the shared `Workspace` this
    /// view addresses (folder filter, hidden set, widths, collapse, focus
    /// zoom). Always `WindowId::Main` in single-window runtime; slice 05
    /// spawns extras that mint distinct `WindowId::Extra(uuid)`s and thread
    /// them in here so each `WindowView` sees only its own per-window state.
    window_id: WindowId,
    /// Per-window focus state: terminal focus stack, project zoom,
    /// fullscreen, modal context. Slice 03 of the multi-window plan moves
    /// this off the shared `Workspace` entity onto each `WindowView` so
    /// every window can zoom and modal-stack independently. Wrapped in
    /// `Entity<FocusManager>` so child views (sidebar, project column,
    /// terminal pane, layout container) can hold a handle to the same
    /// instance and update it through `Entity::update` without needing
    /// to route through `WindowView` first. Workspace action methods that
    /// touched focus state (`set_focused_terminal`, `set_focused_project`,
    /// etc.) now take `focus_manager: &mut FocusManager` as a parameter
    /// so the focus mutation stays scoped to the window driving the action.
    focus_manager: Entity<FocusManager>,
    workspace: Entity<Workspace>,
    request_broker: Entity<RequestBroker>,
    backend: Arc<dyn TerminalBackend>,
    terminals: TerminalsRegistry,
    sidebar: Entity<Sidebar>,
    /// Sidebar state controller
    sidebar_ctrl: SidebarController,
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
    /// Service manager (set by Okena after creation)
    service_manager: Option<Entity<ServiceManager>>,
    /// Last focused project ID (for scroll-to-focused detection)
    last_scroll_project: Option<String>,
    /// Whether a project was zoomed/focused in the last observation (for detecting unfocus)
    was_project_focused: bool,
    /// Project ID to center-scroll to after the next layout pass
    pending_center_scroll: Option<String>,
    /// Last-known on-disk paths per local project, used to detect renames
    /// so we can refresh cached git providers / service paths.
    last_project_paths: HashMap<String, String>,
    /// Last observed wholesale workspace data replacement epoch.
    last_data_replacement_epoch: u64,
}

impl WindowView {
    pub fn new(
        window_id: WindowId,
        workspace: Entity<Workspace>,
        pty_manager: Arc<PtyManager>,
        terminals: TerminalsRegistry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {

        // Per-window UI request broker. Each window (slice 05 onward) owns its
        // own queue so overlay/sidebar requests stay scoped to the window that
        // produced them; closes slice 03 acceptance criterion that per-window
        // UI entities are constructed inside `WindowView::new` rather than
        // passed in from the `Okena` singleton.
        let request_broker = cx.new(|_| RequestBroker::new());

        // Per-window focus state: terminal focus stack, project zoom,
        // fullscreen, modal context. Wrapped in Entity<FocusManager> so
        // child views (sidebar, project column, terminal pane, layout
        // container) can hold handles and update through Entity::update.
        let focus_manager = cx.new(|_| FocusManager::new());

        // Sidebar open/closed state is per-window (persisted on WindowState).
        // Seed SidebarController from the calling window's persisted value;
        // fall back to the global setting for the very first launch where
        // no per-window value exists yet.
        let app_settings = settings(cx);
        let mut sidebar_ctrl = SidebarController::new(&app_settings);
        if let Some(window_state) = workspace.read(cx).data().window(window_id) {
            // Override open-state with per-window persisted value. If the
            // controller's open flag doesn't match, toggle to flip it AND
            // snap `animation` to the matching endpoint — toggle() returns an
            // animation target the caller is expected to drive, but at init
            // we want no animation, just the right starting visual.
            if let Some(sidebar_open) = window_state.sidebar_open
                && sidebar_ctrl.is_open() != sidebar_open
            {
                sidebar_ctrl.toggle();
                sidebar_ctrl.set_animation(if sidebar_open { 1.0 } else { 0.0 });
            }
        }

        // Create sidebar entity once to preserve state
        let sidebar = cx.new(|cx| Sidebar::new(window_id, workspace.clone(), focus_manager.clone(), request_broker.clone(), terminals.clone(), cx));

        // Create title bar entity (sync initial sidebar state)
        let sidebar_initially_open = sidebar_ctrl.is_open();
        let title_bar = cx.new(|cx| {
            let mut tb = TitleBar::new("Okena");
            tb.set_sidebar_open(sidebar_initially_open, cx);
            tb
        });

        // Create status bar entity (sync initial sidebar state)
        let workspace_for_status = workspace.clone();
        let focus_manager_for_status = focus_manager.clone();
        let status_bar = cx.new(|cx| {
            let mut sb = StatusBar::new(workspace_for_status, focus_manager_for_status, cx);
            sb.set_sidebar_open(sidebar_initially_open, cx);
            sb
        });

        // Create overlay manager
        let overlay_manager = cx.new(|_cx| OverlayManager::new(window_id, workspace.clone(), focus_manager.clone(), request_broker.clone()));

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

        let last_data_replacement_epoch = workspace.read(cx).data_replacement_epoch();

        // Wrap PtyManager in LocalBackend for the TerminalBackend trait
        let backend: Arc<dyn TerminalBackend> = Arc::new(LocalBackend::new(pty_manager));

        // Wire up sidebar callbacks
        {
            let workspace_for_dispatch = workspace.clone();
            let focus_manager_for_dispatch = focus_manager.clone();
            let backend_for_dispatch = backend.clone();
            let terminals_for_dispatch = terminals.clone();
            sidebar.update(cx, |s, _cx| {
                // Dispatch action callback
                s.set_dispatch_action(Box::new(move |project_id, action, cx| {
                    if let Some(dispatcher) = crate::action_dispatch::dispatcher_for_project(
                        project_id,
                        window_id,
                        &workspace_for_dispatch,
                        &focus_manager_for_dispatch,
                        &Some(backend_for_dispatch.clone()),
                        &terminals_for_dispatch,
                        &None, // service_manager - wired later
                        &None, // remote_manager - wired later
                        cx,
                    ) {
                        dispatcher.dispatch(action, cx);
                    }
                }));

                // Settings callback
                s.set_settings(Box::new(|cx| {
                    let app_settings = crate::settings::settings(cx);
                    okena_views_sidebar::SidebarSettings {
                        worktree_path_template: app_settings.worktree.path_template.clone(),
                        hooks: app_settings.hooks.clone(),
                    }
                }));
            });
        }

        let mut view = Self {
            window_id,
            focus_manager,
            workspace,
            request_broker,
            backend,
            terminals,
            sidebar,
            sidebar_ctrl,
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
            service_manager: None,
            remote_manager: None,
            git_watcher: None,
            pane_switch_active: false,
            pane_switcher_entity: None,
            last_scroll_project: None,
            was_project_focused: false,
            pending_center_scroll: None,
            last_project_paths: HashMap::new(),
            last_data_replacement_epoch,
        };

        // Slice 07 cri 7: persist OS bounds back into this window's
        // `WindowState.os_bounds` whenever GPUI reports a bounds change
        // (move, resize, snap, monitor switch). The setter delegates to
        // `data.set_os_bounds` which silently no-ops on an unknown extra id
        // (close-race contract), so a debounced bounds-observer firing on
        // a window that's just been closed is safe. The auto-save observer
        // in `Okena::new` debounces persistence at 500ms; this observer
        // just bumps `data_version` per bounds change and lets the save
        // path coalesce. Conversion mirrors the inverse path in
        // `src/app/extras.rs::open_extra_window` (gpui `Bounds<Pixels>` ->
        // `PersistedWindowBounds` via four `f32::from(...)` calls).
        cx.observe_window_bounds(window, |this, window, cx| {
            let bounds = window.window_bounds().get_bounds();
            let persisted = PersistedWindowBounds {
                origin_x: f32::from(bounds.origin.x),
                origin_y: f32::from(bounds.origin.y),
                width: f32::from(bounds.size.width),
                height: f32::from(bounds.size.height),
            };
            let window_id = this.window_id;
            this.workspace.update(cx, |ws, cx| {
                ws.set_os_bounds(window_id, Some(persisted), cx);
            });
        })
        .detach();

        // Observe focus_manager to scroll focused project into view.
        // (Workspace observers no longer fire on focus changes since
        // focus moved off the Workspace entity in slice 03.)
        cx.observe(&view.focus_manager, |this, fm, cx| {
            let fm = fm.read(cx);
            let is_project_focused = fm.focused_project_id().is_some();
            let focused_terminal_project = fm
                .focused_terminal_state()
                .map(|f| f.project_id.clone());

            // When project zoom is cleared, defer centering until after next layout pass
            if this.was_project_focused && !is_project_focused {
                this.last_scroll_project = focused_terminal_project.clone();
                this.pending_center_scroll = focused_terminal_project;
            }
            // When the active terminal changes project, ensure it's visible
            else if focused_terminal_project != this.last_scroll_project && focused_terminal_project.is_some() {
                this.last_scroll_project = focused_terminal_project.clone();
                this.scroll_to_focused_project(focused_terminal_project.as_deref(), false, cx);
            }

            this.was_project_focused = is_project_focused;
            cx.notify();
        }).detach();

        // Observe workspace data changes so project path renames refresh
        // cached git providers / service paths.
        cx.observe(&view.workspace, |this, _workspace, cx| {
            let data_replacement_epoch = this.workspace.read(cx).data_replacement_epoch();
            if this.last_data_replacement_epoch != data_replacement_epoch {
                this.last_data_replacement_epoch = data_replacement_epoch;
                this.project_columns.clear();
                this.last_project_paths.clear();
                this.focus_manager.update(cx, |fm, cx| {
                    fm.clear_all();
                    cx.notify();
                });
                this.sync_project_columns(cx);
            }
            this.refresh_for_project_path_changes(cx);
        }).detach();

        // Initialize project columns
        view.sync_project_columns(cx);

        // Seed path snapshot so the observer only fires on real changes.
        view.last_project_paths = view.snapshot_local_project_paths(cx);

        view
    }

    /// Get the terminals registry (for sharing with detached windows)
    pub fn terminals(&self) -> &TerminalsRegistry {
        &self.terminals
    }

    /// Identifies which window-scoped slot on the shared `Workspace` this
    /// view addresses. Always `WindowId::Main` today (single-window runtime);
    /// slice 05 spawns extras that mint distinct `WindowId::Extra(uuid)`s.
    /// Field is read directly within the impl via `self.window_id`; this
    /// public getter exists for external callers (e.g. the slice 05 spawn
    /// flow on `Okena`) that need to address window-scoped state on
    /// `Workspace` in the same window this view inhabits.
    #[allow(dead_code)]
    pub fn window_id(&self) -> WindowId {
        self.window_id
    }

    /// Per-window focus state, owned by this WindowView. Returned as an
    /// `Entity<FocusManager>` handle so callers (children, sibling views)
    /// can `update`/`read` it without going through `WindowView`. Workspace
    /// action methods that mutate focus (`set_focused_terminal`,
    /// `set_focused_project`, etc.) take `&mut FocusManager` as a parameter,
    /// supplied via `focus_manager.update(cx, |fm, cx| ws.method(fm, ...))`.
    pub fn focus_manager(&self) -> Entity<FocusManager> {
        self.focus_manager.clone()
    }

    /// Set the git watcher entity (called by Okena after creation).
    pub fn set_git_watcher(&mut self, watcher: Entity<GitStatusWatcher>, cx: &mut Context<Self>) {
        self.git_watcher = Some(watcher);
        // Drop existing local columns so they get recreated with the watcher
        self.project_columns.retain(|id, _| id.starts_with("remote:"));
        self.sync_project_columns(cx);
    }

    /// Set the remote connection manager (called after creation by Okena).
    pub fn set_remote_manager(&mut self, manager: Entity<RemoteConnectionManager>, cx: &mut Context<Self>) {
        // Observe remote manager and sync remote projects into workspace
        let workspace = self.workspace.clone();
        let focus_manager = self.focus_manager.clone();
        let window_id = self.window_id;
        cx.observe(&manager, move |this, rm, cx| {
            Self::sync_remote_projects_into_workspace(
                window_id,
                &workspace,
                &focus_manager,
                &rm,
                cx,
            );
            this.sync_project_columns(cx);
            cx.notify();
        }).detach();

        // Wire up remote callbacks on sidebar
        {
            let rm_for_connections = manager.clone();
            let rm_for_send = manager.clone();
            let rm_for_folder = manager.clone();
            self.sidebar.update(cx, |sidebar, _cx| {
                // Get remote connections callback
                sidebar.set_remote_connections(Box::new(move |cx| {
                    rm_for_connections.read(cx).connections().iter().map(|(config, status, _state)| {
                        okena_views_sidebar::RemoteConnectionSnapshot {
                            config: (*config).clone(),
                            status: (*status).clone(),
                        }
                    }).collect()
                }));

                // Send remote action callback
                sidebar.set_send_remote_action(Box::new(move |conn_id, action, cx| {
                    rm_for_send.update(cx, |rm, cx| {
                        rm.send_action(conn_id, action, cx);
                    });
                }));

                // Get remote folder callback
                sidebar.set_get_remote_folder(Box::new(move |conn_id, prefixed_project_id, cx| {
                    let server_project_id = okena_core::client::strip_prefix(prefixed_project_id, conn_id);
                    rm_for_folder.read(cx).connections().iter()
                        .find(|(config, _, _)| config.id == conn_id)
                        .and_then(|(_, _, state)| state.as_ref())
                        .and_then(|state| {
                            state.folders.iter().find(|f| f.project_ids.contains(&server_project_id))
                                .map(|f| f.id.clone())
                        })
                }));
            });

            // Observe remote manager for sidebar updates
            let sidebar_for_observe = self.sidebar.clone();
            cx.observe(&manager, move |_this, _rm, cx| {
                sidebar_for_observe.update(cx, |_, cx| cx.notify());
            }).detach();
        }

        self.remote_manager = Some(manager);

        // Rebuild dispatch callback with remote manager
        self.rebuild_sidebar_dispatch(cx);
    }

    /// Set the service manager entity (called by Okena after creation).
    pub fn set_service_manager(&mut self, manager: Entity<ServiceManager>, cx: &mut Context<Self>) {
        cx.observe(&manager, |_this, _sm, cx| {
            cx.notify();
        }).detach();

        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.set_service_manager(manager.clone(), cx);
        });

        // Rebuild dispatch callback with service manager
        self.rebuild_sidebar_dispatch(cx);

        // Wire service manager into existing project columns
        for col in self.project_columns.values() {
            col.update(cx, |col, cx| {
                col.set_service_manager(manager.clone(), cx);
            });
        }

        self.service_manager = Some(manager);
    }

    /// Rebuild the sidebar dispatch action callback with current service/remote managers.
    fn rebuild_sidebar_dispatch(&self, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let focus_manager = self.focus_manager.clone();
        let backend = self.backend.clone();
        let terminals = self.terminals.clone();
        let service_manager = self.service_manager.clone();
        let remote_manager = self.remote_manager.clone();
        let window_id = self.window_id;
        self.sidebar.update(cx, |s, _cx| {
            s.set_dispatch_action(Box::new(move |project_id, action, cx| {
                if let Some(dispatcher) = crate::action_dispatch::dispatcher_for_project(
                    project_id,
                    window_id,
                    &workspace,
                    &focus_manager,
                    &Some(backend.clone()),
                    &terminals,
                    &service_manager,
                    &remote_manager,
                    cx,
                ) {
                    dispatcher.dispatch(action, cx);
                }
            }));
        });
    }

    /// Sync remote connection state into workspace as materialized ProjectData entries.
    fn sync_remote_projects_into_workspace(
        window_id: WindowId,
        workspace: &Entity<Workspace>,
        focus_manager: &Entity<FocusManager>,
        rm: &Entity<RemoteConnectionManager>,
        cx: &mut Context<Self>,
    ) {
        use crate::workspace::state::{FolderData, ProjectData, LayoutNode};
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

            if let Some(ref state) = snap.state {
                // Build the server folder lookup
                let server_folder_map: std::collections::HashMap<&str, &okena_core::api::ApiFolder> =
                    state.folders.iter().map(|f| (f.id.as_str(), f)).collect();

                // Build prefixed project_order and folder entries that mirror the server structure
                let mut remote_order: Vec<String> = Vec::new();
                let mut remote_folders: Vec<FolderData> = Vec::new();

                if !state.project_order.is_empty() {
                    for order_id in &state.project_order {
                        if let Some(sf) = server_folder_map.get(order_id.as_str()) {
                            // This is a folder — create a prefixed FolderData
                            let prefixed_folder_id = format!("remote:{}:{}", conn_id, sf.id);
                            let prefixed_project_ids: Vec<String> = sf.project_ids.iter()
                                .map(|pid| format!("remote:{}:{}", conn_id, pid))
                                .collect();
                            remote_folders.push(FolderData {
                                id: prefixed_folder_id.clone(),
                                name: sf.name.clone(),
                                project_ids: prefixed_project_ids,
                                folder_color: sf.folder_color,
                            });
                            remote_order.push(prefixed_folder_id);
                        } else {
                            // This is a top-level project
                            remote_order.push(format!("remote:{}:{}", conn_id, order_id));
                        }
                    }
                } else {
                    // Old server without project_order: put all projects as top-level
                    for api_project in &state.projects {
                        remote_order.push(format!("remote:{}:{}", conn_id, api_project.id));
                    }
                };

                for api_project in &state.projects {
                    let prefixed_id = format!("remote:{}:{}", conn_id, api_project.id);
                    expected_remote_ids.insert(prefixed_id.clone());

                    let layout = api_project.layout.as_ref().map(|l| {
                        LayoutNode::from_api_prefixed(l, &format!("remote:{}", conn_id))
                    });

                    let terminal_names: std::collections::HashMap<String, String> = api_project.terminal_names.iter()
                        .map(|(k, v)| (format!("remote:{}:{}", conn_id, k), v.clone()))
                        .collect();

                    let project_color = api_project.folder_color;
                    let conn_id_owned = conn_id.clone();

                    // Build remote services with prefixed terminal IDs
                    let remote_services: Vec<okena_core::api::ApiServiceInfo> = api_project.services.iter().map(|s| {
                        let mut svc = s.clone();
                        svc.terminal_id = s.terminal_id.as_ref()
                            .map(|tid| format!("remote:{}:{}", conn_id, tid));
                        svc
                    }).collect();
                    let remote_host = Some(snap.config.host.clone());
                    let remote_git_status = api_project.git_status.clone();

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
                            existing.folder_color = project_color;
                            existing.worktree_info = api_project.worktree_info.as_ref().map(|wt| {
                                crate::workspace::state::WorktreeMetadata {
                                    parent_project_id: format!("remote:{}:{}", conn_id, wt.parent_project_id),
                                    color_override: wt.color_override,
                                    main_repo_path: String::new(),
                                    worktree_path: String::new(),
                                    branch_name: String::new(),
                                }
                            });
                            existing.worktree_ids = api_project.worktree_ids.iter()
                                .map(|id| format!("remote:{}:{}", conn_id, id))
                                .collect();
                            // Don't overwrite the local hidden set — the
                            // user may have toggled visibility locally.
                        } else {
                            let worktree_info = api_project.worktree_info.as_ref().map(|wt| {
                                crate::workspace::state::WorktreeMetadata {
                                    parent_project_id: format!("remote:{}:{}", conn_id, wt.parent_project_id),
                                    color_override: wt.color_override,
                                    main_repo_path: String::new(),
                                    worktree_path: String::new(),
                                    branch_name: String::new(),
                                }
                            });
                            let worktree_ids: Vec<String> = api_project.worktree_ids.iter()
                                .map(|id| format!("remote:{}:{}", conn_id, id))
                                .collect();
                            // Translate the wire-format show_in_overview
                            // flag into a per-window hidden_project_ids
                            // entry on initial sync. Subsequent syncs hit
                            // the `existing` branch above and leave the
                            // local hidden set alone.
                            if !api_project.show_in_overview {
                                ws.data.hide_project_in_all_windows(&prefixed_id);
                            }
                            ws.data.projects.push(ProjectData {
                                id: prefixed_id.clone(),
                                name: api_project.name.clone(),
                                path: api_project.path.clone(),
                                layout,
                                terminal_names,
                                hidden_terminals: std::collections::HashMap::new(),
                                worktree_info,
                                worktree_ids,
                                folder_color: project_color,
                                hooks: HooksConfig::default(),
                                is_remote: true,
                                connection_id: Some(conn_id_owned),
                                service_terminals: std::collections::HashMap::new(),
                                default_shell: None,
                                hook_terminals: std::collections::HashMap::new(),
                            });
                        }
                        // Update the transient remote snapshot regardless of create/update path.
                        let snapshot = ws.remote_sync.snapshot_mut(&prefixed_id);
                        snapshot.services = remote_services;
                        snapshot.host = remote_host;
                        snapshot.git_status = remote_git_status;
                    });
                }

                // Sync remote folders and project_order into workspace
                let remote_prefix = format!("remote:{}:", conn_id);
                workspace.update(cx, |ws, _cx| {
                    let next_remote_folder_ids: std::collections::HashSet<String> =
                        remote_folders.iter().map(|f| f.id.clone()).collect();
                    let removed_folder_ids: Vec<String> = ws.data.folders.iter()
                        .filter(|f| f.id.starts_with(&remote_prefix) && !next_remote_folder_ids.contains(&f.id))
                        .map(|f| f.id.clone())
                        .collect();
                    for folder_id in removed_folder_ids {
                        ws.data.delete_folder_scrub_all_windows(&folder_id);
                    }

                    // Remove old remote folders for this connection
                    ws.data.folders.retain(|f| !f.id.starts_with(&remote_prefix));
                    // Remove old remote entries from project_order for this connection
                    ws.data.project_order.retain(|id| !id.starts_with(&remote_prefix));

                    // Add new remote folders
                    for rf in remote_folders {
                        ws.data.folders.push(rf);
                    }

                    // Add new remote project_order entries
                    ws.data.project_order.extend(remote_order);
                });
            } else {
                // No state (disconnected/connecting) — remove materialized projects and folders
                let prefix = format!("remote:{}:", conn_id);
                workspace.update(cx, |ws, _cx| {
                    let removed_project_ids: Vec<String> = ws.data.projects.iter()
                        .filter(|p| p.id.starts_with(&prefix))
                        .map(|p| p.id.clone())
                        .collect();
                    let removed_folder_ids: Vec<String> = ws.data.folders.iter()
                        .filter(|f| f.id.starts_with(&prefix))
                        .map(|f| f.id.clone())
                        .collect();
                    for project_id in removed_project_ids {
                        ws.data.delete_project_scrub_all_windows(&project_id);
                    }
                    for folder_id in removed_folder_ids {
                        ws.data.delete_folder_scrub_all_windows(&folder_id);
                    }

                    ws.data.projects.retain(|p| !p.id.starts_with(&prefix));
                    ws.data.folders.retain(|f| !f.id.starts_with(&prefix));
                    ws.data.project_order.retain(|id| !id.starts_with(&prefix));
                });
            }
        }

        // Remove stale remote projects/folders from connections that no longer exist
        workspace.update(cx, |ws, _cx| {
            let removed_project_ids: Vec<String> = ws.data.projects.iter()
                .filter(|p| p.is_remote && !expected_remote_ids.contains(&p.id))
                .map(|p| p.id.clone())
                .collect();
            let removed_folder_ids: Vec<String> = ws.data.folders.iter()
                .filter(|f| {
                    if f.id.starts_with("remote:") {
                        // Remote folder IDs are "remote:{conn_id}:{folder_id}"
                        // Extract conn_id (second segment)
                        let rest = f.id.strip_prefix("remote:").unwrap_or("");
                        let conn_id = rest.split(':').next().unwrap_or("");
                        !active_conn_ids.contains(conn_id)
                    } else {
                        false
                    }
                })
                .map(|f| f.id.clone())
                .collect();
            for project_id in removed_project_ids {
                ws.data.delete_project_scrub_all_windows(&project_id);
            }
            for folder_id in removed_folder_ids {
                ws.data.delete_folder_scrub_all_windows(&folder_id);
            }

            ws.data.projects.retain(|p| {
                if p.is_remote {
                    expected_remote_ids.contains(&p.id)
                } else {
                    true
                }
            });
            ws.data.folders.retain(|f| {
                if f.id.starts_with("remote:") {
                    // Remote folder IDs are "remote:{conn_id}:{folder_id}"
                    // Extract conn_id (second segment)
                    let rest = f.id.strip_prefix("remote:").unwrap_or("");
                    let conn_id = rest.split(':').next().unwrap_or("");
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

        // Focus newly appeared terminals for projects that had a pending CreateTerminal.
        focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| {
                let pending = ws.drain_pending_remote_focus(window_id);
                for pending_focus in pending {
                    let pid = pending_focus.project_id;
                    let new_ids = match ws.project(&pid).and_then(|p| p.layout.as_ref()) {
                        Some(layout) => layout.collect_terminal_ids(),
                        None => continue,
                    };
                    // Find the first terminal ID that wasn't present when the
                    // CreateTerminal action originated in this window.
                    let old_set: std::collections::HashSet<&str> = pending_focus
                        .old_terminal_ids
                        .iter()
                        .map(|s| s.as_str())
                        .collect();
                    if let Some(new_tid) = new_ids.iter().find(|id| !old_set.contains(id.as_str())) {
                        if let Some(path) = ws.project(&pid)
                            .and_then(|p| p.layout.as_ref())
                            .and_then(|l| l.find_terminal_path(new_tid))
                        {
                            ws.set_focused_terminal(fm, pid.clone(), path, cx);
                        }
                    }
                }
            });
            cx.notify();
        });

        // Notify UI without bumping data_version (remote changes shouldn't trigger auto-save)
        workspace.update(cx, |ws, cx| {
            ws.notify_ui_only(cx);
        });
    }

    /// Snapshot current on-disk paths for local projects (keyed by project_id).
    fn snapshot_local_project_paths(&self, cx: &Context<Self>) -> HashMap<String, String> {
        self.workspace.read(cx).projects().iter()
            .filter(|p| !p.is_remote)
            .map(|p| (p.id.clone(), p.path.clone()))
            .collect()
    }

    /// Detect local project directory renames and refresh caches that hold a
    /// snapshotted path (git provider inside GitHeader, ServiceManager paths).
    fn refresh_for_project_path_changes(&mut self, cx: &mut Context<Self>) {
        let current = self.snapshot_local_project_paths(cx);

        let changed: Vec<(String, String)> = current.iter()
            .filter(|(id, path)| self.last_project_paths.get(id.as_str()) != Some(*path))
            .map(|(id, path)| (id.clone(), path.clone()))
            .collect();

        if changed.is_empty() {
            // Still drop entries for projects that no longer exist
            if current.len() != self.last_project_paths.len() {
                self.last_project_paths = current;
            }
            return;
        }

        for (id, new_path) in &changed {
            if let Some(column) = self.project_columns.get(id).cloned() {
                if let Some(provider) = self.build_git_provider(id, cx) {
                    column.update(cx, |col, cx| col.set_git_provider(provider, cx));
                }
            }
            if let Some(sm) = self.service_manager.clone() {
                let id = id.clone();
                let new_path = new_path.clone();
                sm.update(cx, move |sm, _cx| sm.update_project_path(&id, &new_path));
            }
        }

        self.last_project_paths = current;
    }

    /// Ensure project columns exist for all visible projects
    fn sync_project_columns(&mut self, cx: &mut Context<Self>) {
        let visible_projects: Vec<(String, bool, Option<String>)> = {
            let ws = self.workspace.read(cx);
            let fm = self.focus_manager.read(cx);
            ws.visible_projects(self.window_id, fm.focused_project_id(), fm.is_focus_individual()).iter().map(|p| {
                (p.id.clone(), p.is_remote, p.connection_id.clone())
            }).collect()
        };

        // Clean up columns for projects that no longer exist
        let visible_ids: std::collections::HashSet<&str> = visible_projects.iter()
            .map(|(id, _, _)| id.as_str())
            .collect();
        self.project_columns.retain(|id, _| visible_ids.contains(id.as_str()));

        // Create columns for new projects
        for (project_id, is_remote, connection_id) in &visible_projects {
            if !self.project_columns.contains_key(project_id) {
                let entity = if *is_remote {
                    self.create_remote_column(project_id, connection_id.as_deref(), cx)
                } else {
                    Some(self.create_local_column(project_id, cx))
                };
                if let Some(entity) = entity {
                    self.project_columns.insert(project_id.clone(), entity);
                }
            }
        }
    }

    /// Create a ProjectColumn for a remote project.
    fn create_remote_column(
        &self,
        project_id: &str,
        connection_id: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Option<Entity<ProjectColumn>> {
        let conn_id = connection_id?;
        let backend = self.remote_manager.as_ref()
            .and_then(|rm| rm.read(cx).backend_for(conn_id))?;

        let workspace_clone = self.workspace.clone();
        let focus_manager_clone = self.focus_manager.clone();
        let request_broker_clone = self.request_broker.clone();
        let terminals_clone = self.terminals.clone();
        let active_drag_clone = self.active_drag.clone();
        let id = project_id.to_string();
        let workspace_for_dispatch = self.workspace.clone();
        let focus_manager_for_dispatch = self.focus_manager.clone();
        let window_id = self.window_id;
        let action_dispatcher = self.remote_manager.as_ref().map(|rm| {
            crate::action_dispatch::ActionDispatcher::Remote {
                connection_id: conn_id.to_string(),
                manager: rm.clone(),
                workspace: workspace_for_dispatch,
                focus_manager: focus_manager_for_dispatch,
                window_id,
            }
        });
        let ws_for_observe = self.workspace.clone();

        let git_provider = self.build_git_provider(project_id, cx)?;

        Some(cx.new(move |cx| {
            let mut col = ProjectColumn::new(
                window_id,
                workspace_clone,
                focus_manager_clone,
                request_broker_clone,
                id,
                backend,
                terminals_clone,
                active_drag_clone,
                None, // remote projects don't get git watcher
                git_provider,
                cx,
            );
            col.set_action_dispatcher(action_dispatcher);
            // Observe workspace for remote service state changes
            // (instead of local ServiceManager which has no data for remote projects)
            col.observe_remote_services(ws_for_observe, cx);
            col
        }))
    }

    /// Create a ProjectColumn for a local project.
    fn create_local_column(
        &self,
        project_id: &str,
        cx: &mut Context<Self>,
    ) -> Entity<ProjectColumn> {
        let workspace_clone = self.workspace.clone();
        let focus_manager_clone = self.focus_manager.clone();
        let request_broker_clone = self.request_broker.clone();
        let terminals_clone = self.terminals.clone();
        let active_drag_clone = self.active_drag.clone();
        let id = project_id.to_string();
        let backend_clone = self.backend.clone();
        let workspace_for_dispatch = self.workspace.clone();
        let focus_manager_for_dispatch = self.focus_manager.clone();
        let backend_for_dispatch = self.backend.clone();
        let terminals_for_dispatch = self.terminals.clone();
        let git_watcher = self.git_watcher.clone();

        let git_provider = match self.build_git_provider(project_id, cx) {
            Some(p) => p,
            None => {
                log::warn!("Cannot build git provider for project {}", project_id);
                let path = self.workspace.read(cx).project(project_id)
                    .map(|p| p.path.clone())
                    .unwrap_or_default();
                Arc::new(okena_views_git::diff_viewer::provider::LocalGitProvider::new(path))
            }
        };

        let window_id = self.window_id;
        let entity = cx.new(move |cx| {
            let mut col = ProjectColumn::new(
                window_id,
                workspace_clone,
                focus_manager_clone,
                request_broker_clone,
                id,
                backend_clone,
                terminals_clone,
                active_drag_clone,
                git_watcher,
                git_provider,
                cx,
            );
            col.set_action_dispatcher(Some(
                crate::action_dispatch::ActionDispatcher::Local {
                    workspace: workspace_for_dispatch,
                    focus_manager: focus_manager_for_dispatch,
                    backend: backend_for_dispatch,
                    terminals: terminals_for_dispatch,
                    service_manager: None, // set later via set_service_manager
                    window_id,
                },
            ));
            col
        });
        if let Some(ref sm) = self.service_manager {
            entity.update(cx, |col, cx| col.set_service_manager(sm.clone(), cx));
        }
        entity
    }
}

impl_focusable!(WindowView);

#[cfg(test)]
mod tests {
    use super::notify_pane_weaks;
    use gpui::AppContext as _;

    struct Stub;

    #[gpui::test]
    fn fans_out_to_every_alive_weak_and_prunes_dead(cx: &mut gpui::TestAppContext) {
        let (a, b, mut weaks) = cx.update(|cx| {
            let a = cx.new(|_| Stub);
            let b = cx.new(|_| Stub);
            let weaks = vec![a.downgrade(), b.downgrade()];
            (a, b, weaks)
        });

        cx.update(|cx| {
            assert!(notify_pane_weaks(&mut weaks, cx));
            assert_eq!(weaks.len(), 2, "both alive entries kept");
        });

        drop(b);

        cx.update(|cx| {
            assert!(notify_pane_weaks(&mut weaks, cx));
            assert_eq!(weaks.len(), 1, "dead entry pruned, live entry kept");
        });

        drop(a);

        cx.update(|cx| {
            assert!(!notify_pane_weaks(&mut weaks, cx));
            assert!(weaks.is_empty(), "all dead entries pruned");
        });
    }
}
