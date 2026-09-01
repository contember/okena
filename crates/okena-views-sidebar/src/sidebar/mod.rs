//! Sidebar view with project and terminal list
//!
//! The sidebar provides navigation for projects and terminals, with features for:
//! - Adding/managing projects
//! - Renaming terminals and projects
//! - Drag-and-drop project reordering
//! - Folder color customization
//! - Organizing projects into collapsible folders

mod cursor;
mod renames;
mod render;
mod worktree;

#[cfg(test)]
mod from_project_test;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use okena_core::api::ActionRequest;
use okena_core::theme::FolderColor;
use okena_services::manager::ServiceManager;
use okena_terminal::TerminalsRegistry;
use okena_transport::client::{ConnectionStatus, RemoteConnectionConfig};
use okena_ui::click_detector::ClickDetector;
use okena_ui::menu::{context_menu_panel, menu_item, menu_section, menu_separator};
use okena_ui::rename_state::RenameState;
use okena_ui::theme::theme;
use okena_ui::tokens::ui_text_ms;
use okena_workspace::request_broker::RequestBroker;
use okena_workspace::state::{FolderData, ProjectData, WindowId, Workspace};
use std::collections::{HashMap, HashSet};

/// Callback for dispatching actions for a given project.
/// Arguments: (project_id, action, cx)
pub type DispatchActionFn = Box<dyn Fn(&str, ActionRequest, &mut App)>;

/// Callback to get current app settings needed by the sidebar.
pub type GetSettingsFn = Box<dyn Fn(&App) -> SidebarSettings>;

/// Settings needed by the sidebar.
#[derive(Default, Clone)]
pub struct SidebarSettings {
    pub worktree_path_template: String,
    pub hooks: okena_workspace::settings::HooksConfig,
}

/// Snapshot of a remote connection for rendering.
pub struct RemoteConnectionSnapshot {
    pub config: RemoteConnectionConfig,
    pub status: ConnectionStatus,
}

/// Callback to get remote connection snapshots for rendering.
pub type GetRemoteConnectionsFn = Box<dyn Fn(&App) -> Vec<RemoteConnectionSnapshot>>;

/// Callback to send a remote action to a connection.
/// Arguments: (conn_id, action, cx)
pub type SendRemoteActionFn = Box<dyn Fn(&str, ActionRequest, &mut App)>;

/// Callback to dispatch an action to a connection by id, with id-stripping (the
/// connection-targeted analog of [`DispatchActionFn`], for folder-scoped and
/// workspace-global actions that carry no project to resolve a connection from).
/// Arguments: (conn_id, action, cx)
pub type DispatchForConnectionFn = Box<dyn Fn(&str, ActionRequest, &mut App)>;

/// Callback to get the server folder ID for a remote folder reorder operation.
/// Arguments: (conn_id, prefixed_project_id, cx) -> Option<folder_id>
pub type GetRemoteFolderFn = Box<dyn Fn(&str, &str, &App) -> Option<String>>;

/// Sub-category group kind within an expanded project.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum GroupKind {
    Terminals,
    Services,
    Hooks,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SidebarHeaderMenu {
    Create,
    View,
}

impl GroupKind {
    pub fn label(&self) -> &'static str {
        match self {
            GroupKind::Terminals => "Terminals",
            GroupKind::Services => "Services",
            GroupKind::Hooks => "Hooks",
        }
    }
}

/// Identifies each visible row in the sidebar for keyboard cursor navigation.
#[derive(Clone, Debug)]
pub enum SidebarCursorItem {
    Folder {
        folder_id: String,
    },
    Project {
        project_id: String,
    },
    WorktreeProject {
        project_id: String,
    },
    GroupHeader {
        project_id: String,
        group: GroupKind,
    },
    Terminal {
        project_id: String,
        terminal_id: String,
    },
    Service {
        project_id: String,
        service_name: String,
    },
    #[allow(dead_code)]
    Hook {
        project_id: String,
        terminal_id: String,
    },
    #[allow(dead_code)]
    RemoteConnection {
        connection_id: String,
    },
    #[allow(dead_code)]
    RemoteProject {
        connection_id: String,
        project_id: String,
    },
}

/// Sidebar view with project and terminal list
pub struct Sidebar {
    /// Identifies which window-scoped slot on the shared `Workspace` this
    /// sidebar addresses (folder filter, hidden set, widths, collapse, focus
    /// zoom). Today every reader of window-scoped state inside this entity's
    /// impl still passes the literal `WindowId::Main` at the call site;
    /// subsequent slice 03 commits migrate those readers (cursor.rs, mod.rs,
    /// folder_list.rs, project_list.rs -- all `impl Sidebar`) to route
    /// through `self.window_id`. Slice 05 then spawns extra windows that
    /// mint `WindowId::Extra(uuid)` and thread it in here so each `Sidebar`
    /// sees only its own per-window state.
    pub(crate) window_id: WindowId,
    pub(crate) workspace: Entity<Workspace>,
    pub(crate) focus_manager: Entity<okena_workspace::focus::FocusManager>,
    pub request_broker: Entity<RequestBroker>,
    pub(crate) expanded_projects: HashSet<String>,
    /// Projects whose worktree children list is collapsed.
    /// Uses negative-sense (collapsed) because worktrees should be visible by default.
    /// This is the inverse of `expanded_projects` which uses positive-sense because
    /// terminal details should be hidden by default.
    pub(crate) collapsed_worktrees: HashSet<String>,
    pub terminals: TerminalsRegistry,
    /// Terminal rename state: (project_id, terminal_id)
    pub terminal_rename: Option<RenameState<(String, String)>>,
    /// Double-click detector for terminals
    pub(crate) terminal_click_detector: ClickDetector<String>,
    /// Project rename state
    pub project_rename: Option<RenameState<String>>,
    /// Double-click detector for projects
    pub(crate) project_click_detector: ClickDetector<String>,
    /// Folder rename state
    pub folder_rename: Option<RenameState<String>>,
    /// Double-click detector for folders
    pub(crate) folder_click_detector: ClickDetector<String>,
    /// Sidebar requests drained from Workspace by observer, applied in render() (needs Window)
    pub(crate) pending_sidebar_requests: Vec<okena_workspace::requests::SidebarRequest>,
    /// Focus handle for keyboard event capture
    pub(crate) focus_handle: FocusHandle,
    /// Scroll handle for programmatic scrolling
    pub(crate) scroll_handle: ScrollHandle,
    /// Current keyboard cursor position (index into flat item list)
    pub(crate) cursor_index: Option<usize>,
    /// Saved focus handle to restore when leaving sidebar
    pub saved_focus: Option<FocusHandle>,
    /// Collapsed state for remote connections
    pub collapsed_connections: HashMap<String, bool>,
    /// Callback for dispatching actions (replaces ActionDispatcher + backend)
    pub(crate) dispatch_action: Option<DispatchActionFn>,
    /// Service manager (optional - set after creation)
    pub service_manager: Option<Entity<ServiceManager>>,
    /// Collapsed state for group headers (Terminals/Services) per project
    pub(crate) collapsed_groups: HashSet<(String, GroupKind)>,
    /// Parent project IDs with in-flight worktree creation (debounce guard)
    pub(crate) creating_worktree: HashSet<String>,
    /// Callback to get remote connections
    pub(crate) get_remote_connections: Option<GetRemoteConnectionsFn>,
    /// Callback to send remote actions
    pub(crate) send_remote_action: Option<SendRemoteActionFn>,
    /// Callback to dispatch a folder-scoped / workspace-global action to a
    /// connection by id (with id-stripping). See [`DispatchForConnectionFn`].
    pub(crate) dispatch_for_connection: Option<DispatchForConnectionFn>,
    /// Callback to get remote folder ID for reordering
    pub(crate) get_remote_folder: Option<GetRemoteFolderFn>,
    /// Last computed activity-view ordering `(tier, project_id)`. Reused while
    /// the pointer is inside the sidebar (hover-freeze) so rows don't reshuffle
    /// out from under the cursor; recomputed once the pointer leaves.
    pub(crate) activity_order_cache: Vec<(crate::activity_order::ActivityTier, String)>,
    /// True while the pointer is hovering the sidebar scroll area. Freezes the
    /// activity-view ordering so a finishing command / bell doesn't reshuffle
    /// rows mid-interaction.
    pub(crate) activity_pointer_inside: bool,
    /// Cursor-navigable items for the activity view, in exact rendered order
    /// (project rows + their expanded group/terminal children; tier headers
    /// and the attention mirror are not navigable). Rebuilt every render of the
    /// activity path; `build_cursor_items` returns this verbatim in activity
    /// mode so keyboard nav lines up 1:1 with what's on screen. Empty in manual
    /// mode.
    pub(crate) activity_cursor_items: Vec<SidebarCursorItem>,
    /// Scroll-child index for each `cursor_index`, populated every render by
    /// both paths. The scroll container interleaves non-navigable children
    /// (the leading drop zone and attention section in the manual view; tier
    /// headers in the activity view) with the navigable rows, so `cursor_index`
    /// is not the scroll-child index — this maps one to the other.
    pub(crate) cursor_scroll_indices: Vec<usize>,
    header_menu: Option<SidebarHeaderMenu>,
    create_button_bounds: Bounds<Pixels>,
    view_button_bounds: Bounds<Pixels>,
}

impl Sidebar {
    pub fn new(
        window_id: WindowId,
        workspace: Entity<Workspace>,
        focus_manager: Entity<okena_workspace::focus::FocusManager>,
        request_broker: Entity<RequestBroker>,
        terminals: TerminalsRegistry,
        cx: &mut Context<Self>,
    ) -> Self {
        // Observe RequestBroker to drain sidebar requests outside of render().
        // Requests are stored in pending_sidebar_requests and applied in render()
        // where Window access is available (needed for focus/rename).
        cx.observe(&request_broker, |this, _broker, cx| {
            if !this.request_broker.read(cx).has_sidebar_requests() {
                return;
            }
            let requests = this
                .request_broker
                .update(cx, |broker, _cx| broker.drain_sidebar_requests());
            this.pending_sidebar_requests.extend(requests);
            cx.notify();
        })
        .detach();

        // The Sidebar renders inside a `.cached()` view (window render), so it
        // only repaints when a notify comes from an entity it observes. It
        // derives per-project state straight from the Workspace — project data,
        // git status, and transient lifecycle flags (`is_closing`/`is_creating`)
        // read via `is_project_closing`/`is_creating_project`. Without observing
        // the Workspace, a workspace-only mutation (e.g. `mark_closing_project`,
        // which notifies the Workspace but pushes no remote snapshot) never
        // invalidates the cache, so the row stays stale until an unrelated
        // snapshot happens to repaint. Matches status_bar/project_column, which
        // observe the workspace for the same reason.
        cx.observe(&workspace, |_this, _ws, cx| cx.notify())
            .detach();

        // Hook terminals are displayed in the dedicated HookPanel, so we no
        // longer auto-expand the sidebar project when hooks appear.

        Self {
            window_id,
            workspace,
            focus_manager,
            request_broker,
            expanded_projects: HashSet::new(),
            collapsed_worktrees: HashSet::new(),
            terminals,
            terminal_rename: None,
            terminal_click_detector: ClickDetector::new(),
            project_rename: None,
            project_click_detector: ClickDetector::new(),
            folder_rename: None,
            folder_click_detector: ClickDetector::new(),
            pending_sidebar_requests: Vec::new(),
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            cursor_index: None,
            saved_focus: None,
            collapsed_connections: HashMap::new(),
            dispatch_action: None,
            service_manager: None,
            collapsed_groups: HashSet::new(),
            creating_worktree: HashSet::new(),
            get_remote_connections: None,
            send_remote_action: None,
            dispatch_for_connection: None,
            get_remote_folder: None,
            activity_order_cache: Vec::new(),
            activity_pointer_inside: false,
            activity_cursor_items: Vec::new(),
            cursor_scroll_indices: Vec::new(),
            header_menu: None,
            create_button_bounds: Bounds::default(),
            view_button_bounds: Bounds::default(),
        }
    }

    /// Identifies which window-scoped slot on the shared `Workspace` this
    /// sidebar addresses. Always `WindowId::Main` today (single-window
    /// runtime); slice 05 spawns extras that mint distinct
    /// `WindowId::Extra(uuid)`s. Field is read directly within the impl via
    /// `self.window_id`; this public getter exists for external callers
    /// (e.g. the slice 05 spawn flow on `Okena`) that need to address
    /// window-scoped state on `Workspace` in the same window this sidebar
    /// inhabits. Note: the dead_code lint tracks fields and methods as
    /// separate items, so a future runtime read of `self.window_id` does
    /// NOT mark this getter as used; the attribute stays until an external
    /// caller of the getter lands.
    #[allow(dead_code)]
    pub fn window_id(&self) -> WindowId {
        self.window_id
    }

    /// Set the dispatch action callback.
    pub fn set_dispatch_action(&mut self, f: DispatchActionFn) {
        self.dispatch_action = Some(f);
    }

    /// Set the remote connections callback.
    pub fn set_remote_connections(&mut self, f: GetRemoteConnectionsFn) {
        self.get_remote_connections = Some(f);
    }

    /// Set the send remote action callback.
    pub fn set_send_remote_action(&mut self, f: SendRemoteActionFn) {
        self.send_remote_action = Some(f);
    }

    /// Set the connection-targeted dispatch callback.
    pub fn set_dispatch_for_connection(&mut self, f: DispatchForConnectionFn) {
        self.dispatch_for_connection = Some(f);
    }

    /// Set the get remote folder callback.
    pub fn set_get_remote_folder(&mut self, f: GetRemoteFolderFn) {
        self.get_remote_folder = Some(f);
    }

    /// Dispatch an action for a project using the dispatch callback.
    pub(crate) fn dispatch_action_for_project(
        &self,
        project_id: &str,
        action: ActionRequest,
        cx: &mut App,
    ) {
        if let Some(ref dispatch) = self.dispatch_action {
            (dispatch)(project_id, action, cx);
        }
    }

    /// Dispatch a folder-scoped action to the connection that owns the folder.
    /// Folder ids are `remote:<conn>:<id>`; the dispatcher strips the prefix
    /// against the resolved connection. Falls back to the local daemon for an
    /// unprefixed id.
    pub(crate) fn dispatch_action_for_folder(
        &self,
        folder_id: &str,
        action: ActionRequest,
        cx: &mut App,
    ) {
        let conn_id = folder_id
            .strip_prefix("remote:")
            .and_then(|rest| rest.split(':').next())
            .unwrap_or(okena_transport::client::LOCAL_DAEMON_CONNECTION_ID);
        if let Some(ref dispatch) = self.dispatch_for_connection {
            (dispatch)(conn_id, action, cx);
        }
    }

    /// Dispatch a workspace-global action (e.g. creating a folder) to the local
    /// daemon, which owns the authoritative workspace; the result mirrors back
    /// on the next snapshot.
    pub(crate) fn dispatch_daemon_action(&self, action: ActionRequest, cx: &mut App) {
        if let Some(ref dispatch) = self.dispatch_for_connection {
            (dispatch)(
                okena_transport::client::LOCAL_DAEMON_CONNECTION_ID,
                action,
                cx,
            );
        }
    }

    /// Check for double-click on terminal and return true if detected
    pub fn check_double_click(&mut self, terminal_id: &str) -> bool {
        self.terminal_click_detector.check(terminal_id.to_string())
    }

    /// Whether a project row should show its children (layout or worktrees).
    /// Worktree parents use negative-sense (collapsed set), others use positive-sense (expanded set).
    pub fn is_project_expanded(&self, project_id: &str, has_worktrees: bool) -> bool {
        if has_worktrees {
            !self.collapsed_worktrees.contains(project_id)
        } else {
            self.expanded_projects.contains(project_id)
        }
    }

    pub(crate) fn toggle_expanded(&mut self, project_id: &str) {
        if self.expanded_projects.contains(project_id) {
            self.expanded_projects.remove(project_id);
        } else {
            self.expanded_projects.insert(project_id.to_string());
        }
    }

    pub fn toggle_worktrees_collapsed(&mut self, project_id: &str) {
        if self.collapsed_worktrees.contains(project_id) {
            self.collapsed_worktrees.remove(project_id);
        } else {
            self.collapsed_worktrees.insert(project_id.to_string());
        }
    }

    pub(crate) fn toggle_group(&mut self, project_id: &str, group: GroupKind) {
        let key = (project_id.to_string(), group);
        if self.collapsed_groups.contains(&key) {
            self.collapsed_groups.remove(&key);
        } else {
            self.collapsed_groups.insert(key);
        }
    }

    pub(crate) fn is_group_collapsed(&self, project_id: &str, group: &GroupKind) -> bool {
        self.collapsed_groups
            .contains(&(project_id.to_string(), group.clone()))
    }

    pub(crate) fn request_context_menu(
        &mut self,
        project_id: String,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.request_broker.update(cx, |broker, cx| {
            broker.push_overlay_request(
                okena_workspace::requests::OverlayRequest::Project(
                    okena_workspace::requests::ProjectOverlay {
                        project_id,
                        kind: okena_workspace::requests::ProjectOverlayKind::ContextMenu {
                            position,
                        },
                    },
                ),
                cx,
            );
        });
    }

    /// Check for double-click on project and return true if detected
    pub fn check_project_double_click(&mut self, project_id: &str) -> bool {
        self.project_click_detector.check(project_id.to_string())
    }

    /// Check for double-click on folder and return true if detected
    pub fn check_folder_double_click(&mut self, folder_id: &str) -> bool {
        self.folder_click_detector.check(folder_id.to_string())
    }

    /// Public accessor for the focus handle (used by WindowView for FocusSidebar)
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    pub fn set_service_manager(&mut self, manager: Entity<ServiceManager>, cx: &mut Context<Self>) {
        cx.observe(&manager, |_this, _sm, cx| {
            cx.notify();
        })
        .detach();
        self.service_manager = Some(manager);
        cx.notify();
    }

    /// Count how many terminals from the given IDs are currently waiting for input
    pub fn count_waiting_terminals(&self, terminal_ids: &[String]) -> usize {
        let terminals = self.terminals.lock();
        terminal_ids
            .iter()
            .filter(|id| {
                terminals
                    .get(id.as_str())
                    .is_some_and(|t| t.is_waiting_for_input())
            })
            .count()
    }

    pub(super) fn render_overview_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace_entity = self.workspace.clone();
        let focus_manager = self.focus_manager.clone();
        let window_id = self.window_id;
        let activity_mode = self
            .workspace
            .read(cx)
            .data()
            .window(window_id)
            .is_some_and(|window| window.project_sort_mode.is_activity());
        let is_active = self.focus_manager.read(cx).focused_project_id().is_none()
            && self
                .workspace
                .read(cx)
                .active_folder_filter(window_id)
                .is_none();
        let icon_color = if is_active {
            t.text_primary
        } else {
            t.text_secondary
        };
        let view_menu_open = self.header_menu == Some(SidebarHeaderMenu::View);
        let create_menu_open = self.header_menu == Some(SidebarHeaderMenu::Create);
        let view_bounds_entity = cx.entity().clone();
        let create_bounds_entity = cx.entity().clone();

        div()
            .relative()
            .h(px(35.0))
            .when(is_active, |row| row.bg(rgb(t.bg_hover)))
            .child(
                div()
                    .id("projects-overview")
                    .h_full()
                    .pl(px(20.0))
                    .pr(px(12.0))
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .on_click(move |_, _window, cx| {
                        focus_manager.update(cx, |fm, cx| {
                            workspace_entity.update(cx, |ws, cx| {
                                ws.set_focused_project(fm, None, cx);
                                ws.set_folder_filter(window_id, None, cx);
                            });
                            cx.notify();
                        });
                    })
                    .child(
                        svg()
                            .path("icons/select-all.svg")
                            .size(px(13.0))
                            .text_color(rgb(icon_color)),
                    )
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(if is_active {
                                t.text_primary
                            } else {
                                t.text_secondary
                            }))
                            .child("Overview"),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .id("project-view-menu-btn")
                                    .cursor_pointer()
                                    .size(px(24.0))
                                    .rounded(px(4.0))
                                    .when(view_menu_open, |button| button.bg(rgb(t.bg_hover)))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        svg()
                                            .path("icons/arrow-up-down.svg")
                                            .size(px(13.0))
                                            .text_color(rgb(if activity_mode {
                                                t.border_active
                                            } else {
                                                t.text_secondary
                                            })),
                                    )
                                    .child(
                                        canvas(
                                            move |bounds, _window, app| {
                                                view_bounds_entity.update(app, |this, _cx| {
                                                    this.view_button_bounds = bounds;
                                                });
                                            },
                                            |_, _, _, _| {},
                                        )
                                        .absolute()
                                        .size_full(),
                                    )
                                    .tooltip(move |window, cx| {
                                        Tooltip::new(if activity_mode {
                                            "Project ordering: Activity"
                                        } else {
                                            "Project ordering: Manual"
                                        })
                                        .build(window, cx)
                                    })
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        cx.stop_propagation();
                                        this.header_menu =
                                            if this.header_menu == Some(SidebarHeaderMenu::View) {
                                                None
                                            } else {
                                                Some(SidebarHeaderMenu::View)
                                            };
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("project-create-menu-btn")
                                    .cursor_pointer()
                                    .size(px(24.0))
                                    .rounded(px(4.0))
                                    .when(create_menu_open, |button| button.bg(rgb(t.bg_hover)))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        svg()
                                            .path("icons/plus.svg")
                                            .size(px(14.0))
                                            .text_color(rgb(t.text_secondary)),
                                    )
                                    .child(
                                        canvas(
                                            move |bounds, _window, app| {
                                                create_bounds_entity.update(app, |this, _cx| {
                                                    this.create_button_bounds = bounds;
                                                });
                                            },
                                            |_, _, _, _| {},
                                        )
                                        .absolute()
                                        .size_full(),
                                    )
                                    .tooltip(|window, cx| {
                                        Tooltip::new("New project or folder").build(window, cx)
                                    })
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        cx.stop_propagation();
                                        this.header_menu = if this.header_menu
                                            == Some(SidebarHeaderMenu::Create)
                                        {
                                            None
                                        } else {
                                            Some(SidebarHeaderMenu::Create)
                                        };
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(self.render_header_menu(activity_mode, cx))
    }

    fn render_header_menu(&self, activity_mode: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let window_id = self.window_id;
        let Some(menu) = self.header_menu else {
            return div().size_0().into_any_element();
        };
        let bounds = match menu {
            SidebarHeaderMenu::Create => self.create_button_bounds,
            SidebarHeaderMenu::View => self.view_button_bounds,
        };
        let position = point(
            bounds.origin.x + bounds.size.width,
            bounds.origin.y + bounds.size.height + px(2.0),
        );

        let panel = match menu {
            SidebarHeaderMenu::Create => context_menu_panel("project-create-menu", &t)
                .min_w(px(180.0))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.header_menu = None;
                    cx.notify();
                }))
                .child(
                    menu_item(
                        "create-project-menu-item",
                        "icons/plus.svg",
                        "New project",
                        &t,
                    )
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.header_menu = None;
                        this.request_broker.update(cx, |broker, cx| {
                            broker.push_overlay_request(
                                okena_workspace::requests::OverlayRequest::AddProjectDialog,
                                cx,
                            );
                        });
                        cx.notify();
                    })),
                )
                .child(
                    menu_item(
                        "create-folder-menu-item",
                        "icons/folder.svg",
                        "New folder",
                        &t,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.header_menu = None;
                        this.create_folder(window, cx);
                    })),
                )
                .into_any_element(),
            SidebarHeaderMenu::View => {
                let show_attention = self
                    .workspace
                    .read(cx)
                    .data()
                    .window(window_id)
                    .is_some_and(|window| window.show_attention_section);
                context_menu_panel("project-view-menu", &t)
                    .min_w(px(200.0))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.header_menu = None;
                        cx.notify();
                    }))
                    .child(menu_section("Order", &t))
                    .child(
                        menu_item(
                            "project-view-manual",
                            if activity_mode {
                                "icons/folder.svg"
                            } else {
                                "icons/check.svg"
                            },
                            "Manual order",
                            &t,
                        )
                        .on_click(cx.listener(
                            move |this, _, _window, cx| {
                                this.header_menu = None;
                                if activity_mode {
                                    this.workspace.update(cx, |ws, cx| {
                                        ws.toggle_project_sort_mode(window_id, cx);
                                    });
                                }
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        menu_item(
                            "project-view-activity",
                            if activity_mode {
                                "icons/check.svg"
                            } else {
                                "icons/bell.svg"
                            },
                            "By activity",
                            &t,
                        )
                        .on_click(cx.listener(
                            move |this, _, _window, cx| {
                                this.header_menu = None;
                                if !activity_mode {
                                    this.workspace.update(cx, |ws, cx| {
                                        ws.toggle_project_sort_mode(window_id, cx);
                                    });
                                }
                                cx.notify();
                            },
                        )),
                    )
                    .when(!activity_mode, |panel| {
                        panel.child(menu_separator(&t)).child(
                            menu_item(
                                "attention-toggle",
                                if show_attention {
                                    "icons/check.svg"
                                } else {
                                    "icons/bell.svg"
                                },
                                "Needs attention section",
                                &t,
                            )
                            .on_click(cx.listener(
                                move |this, _, _window, cx| {
                                    this.header_menu = None;
                                    this.workspace.update(cx, |ws, cx| {
                                        ws.toggle_show_attention_section(window_id, cx);
                                    });
                                    cx.notify();
                                },
                            )),
                        )
                    })
                    .into_any_element()
            }
        };

        deferred(
            anchored()
                .position(position)
                .anchor(Anchor::TopRight)
                .snap_to_window()
                .child(panel),
        )
        .into_any_element()
    }
}

/// Service info for sidebar rendering.
#[derive(Clone)]
pub struct SidebarServiceInfo {
    pub name: String,
    pub status: okena_services::manager::ServiceStatus,
    pub ports: Vec<u16>,
    /// Host for port badge URLs ("localhost" for local, remote host for remote)
    pub port_host: String,
    /// Whether this service is a Docker Compose service
    pub is_docker: bool,
}

/// Hook terminal info for sidebar rendering.
#[derive(Clone)]
pub struct SidebarHookInfo {
    pub terminal_id: String,
    pub label: String,
    pub status: okena_workspace::state::HookTerminalStatus,
    pub command: String,
    pub cwd: String,
}

/// Lightweight projection of ProjectData for sidebar rendering.
/// Avoids cloning the full LayoutNode tree, path, hidden_terminals, and hooks
/// which are never used by the sidebar.
pub struct SidebarProjectInfo {
    pub id: String,
    pub name: String,
    pub show_in_overview: bool,
    pub folder_color: FolderColor,
    pub has_layout: bool,
    pub terminal_ids: Vec<String>,
    pub terminal_names: HashMap<String, String>,
    /// Terminal IDs that are behind a non-active tab (not currently visible)
    pub inactive_tab_terminals: HashSet<String>,
    /// Terminal IDs that belong to a tab group (Tabs node with 2+ children)
    pub tab_group_terminals: HashSet<String>,
    /// True if this is a worktree whose parent project no longer exists
    pub is_orphan: bool,
    /// Total number of active worktree children (for badge display and expand arrow logic)
    pub worktree_count: usize,
    /// Parent project ID (for worktree children, used for drag-and-drop reordering)
    pub parent_project_id: Option<String>,
    /// Services defined in okena.yaml for this project
    pub services: Vec<SidebarServiceInfo>,
    /// Hook terminals currently running for this project
    pub hook_terminals: Vec<SidebarHookInfo>,
    /// True if this worktree is being closed (hook running or git remove in progress)
    pub is_closing: bool,
    /// True if this worktree is being created (git fetch + worktree add in progress)
    pub is_creating: bool,
    /// How far the in-flight create has got, when it reports progress at all
    /// (clones do, worktree checkouts do not). Replaces the generic
    /// "Creating…" label so a long clone does not look stuck.
    pub creating_progress: Option<String>,
    /// Whether this project is itself a worktree
    pub is_worktree: bool,
    /// Whether this project is pinned (shows a pin marker; drives the pinned
    /// tier in the activity-sorted view).
    pub pinned: bool,
}

impl SidebarProjectInfo {
    /// Build a sidebar projection of a project.
    ///
    /// `show_in_overview` on the projection is derived from
    /// `workspace.is_project_hidden(window_id, &project.id)` (the per-window
    /// viewport model — each window has its own `hidden_project_ids`). The
    /// `window_id` is the id of the window that owns the sidebar instance
    /// rendering this projection.
    pub(crate) fn from_project(
        project: &ProjectData,
        workspace: &Workspace,
        window_id: WindowId,
    ) -> Self {
        let layout = project.layout.as_ref();
        // For worktree projects, show the git branch instead of the stored name.
        // The branch comes from the daemon's git poll over the wire
        // (`remote_snapshot().git_status.branch`) — a local `get_git_status` would
        // return None in client mode since the daemon owns the working tree.
        let name = if project.worktree_info.is_some() {
            workspace
                .remote_snapshot(&project.id)
                .and_then(|s| s.git_status.as_ref())
                .and_then(|g| g.branch.clone())
                .unwrap_or_else(|| project.name.clone())
        } else {
            project.name.clone()
        };
        Self {
            id: project.id.clone(),
            name,
            show_in_overview: !workspace.is_project_hidden(window_id, &project.id),
            folder_color: project.folder_color,
            has_layout: layout.is_some(),
            terminal_ids: layout
                .map(|l| {
                    l.collect_terminal_ids()
                        .into_iter()
                        .filter(|tid| !project.hook_terminals.contains_key(tid))
                        .collect()
                })
                .unwrap_or_default(),
            inactive_tab_terminals: layout
                .map(|l| l.collect_inactive_tab_terminal_ids())
                .unwrap_or_default(),
            tab_group_terminals: layout
                .map(|l| l.collect_tab_group_terminal_ids())
                .unwrap_or_default(),
            terminal_names: project.terminal_names.clone(),
            is_orphan: false,
            worktree_count: 0,
            parent_project_id: project
                .worktree_info
                .as_ref()
                .map(|w| w.parent_project_id.clone()),
            services: Vec::new(),
            hook_terminals: project
                .hook_terminals
                .iter()
                .map(|(tid, entry)| SidebarHookInfo {
                    terminal_id: tid.clone(),
                    label: entry.label.clone(),
                    status: entry.status.clone(),
                    command: entry.command.clone(),
                    cwd: entry.cwd.clone(),
                })
                .collect(),
            is_closing: false,
            // Explicit mid-create marker, set by the daemon while the git
            // checkout is in flight and mirrored over the wire. NOT derived from
            // `layout.is_none()` — a fully-created worktree whose last terminal
            // the user closed is a legitimate bookmark with layout None, and must
            // not render the "Setting up worktree…" placeholder.
            is_creating: project.is_creating,
            creating_progress: project.creating_progress.clone(),
            is_worktree: project.worktree_info.is_some(),
            pinned: project.pinned,
        }
    }
}

/// An item in the sidebar's top-level ordering: either a project or a folder
pub(crate) enum SidebarItem {
    Project {
        project: SidebarProjectInfo,
        index: usize,
        worktree_children: Vec<SidebarProjectInfo>,
    },
    Folder {
        folder: FolderData,
        index: usize,
        projects: Vec<SidebarProjectInfo>,
        worktree_children: HashMap<String, Vec<SidebarProjectInfo>>,
    },
}
