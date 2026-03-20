//! Sidebar view with project and terminal list
//!
//! The sidebar provides navigation for projects and terminals, with features for:
//! - Adding/managing projects
//! - Renaming terminals and projects
//! - Drag-and-drop project reordering
//! - Folder color customization
//! - Organizing projects into collapsible folders

mod color_picker;
pub(super) mod drag;
mod folder_list;
mod item_widgets;
mod project_list;
mod remote_list;
mod hook_list;
mod service_list;
mod worktree_list;

use crate::action_dispatch::ActionDispatcher;
use crate::keybindings::{
    SidebarConfirm, SidebarDown, SidebarEscape, SidebarToggleExpand, SidebarUp,
};
use crate::remote_client::manager::RemoteConnectionManager;
use crate::services::manager::ServiceManager;
use crate::terminal::backend::TerminalBackend;
use crate::theme::{theme, FolderColor};
use crate::ui::ClickDetector;
use crate::views::components::{
    cancel_rename, finish_rename, start_rename_with_blur,
    RenameState,
};
use crate::views::root::TerminalsRegistry;
use crate::workspace::request_broker::RequestBroker;
use crate::workspace::requests::SidebarRequest;
use crate::workspace::state::{FolderData, ProjectData, Workspace};
use gpui::*;
use gpui_component::h_flex;
use gpui::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use drag::{ProjectDrag, ProjectDragView, FolderDrag, FolderDragView, WorktreeDrag, WorktreeDragView};

/// Sub-category group kind within an expanded project.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum GroupKind {
    Terminals,
    Services,
    Hooks,
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
pub(super) enum SidebarCursorItem {
    Folder { folder_id: String },
    Project { project_id: String },
    WorktreeProject { project_id: String },
    GroupHeader { project_id: String, group: GroupKind },
    Terminal { project_id: String, terminal_id: String },
    Service { project_id: String, service_name: String },
    #[allow(dead_code)]
    Hook { project_id: String, terminal_id: String },
    #[allow(dead_code)]
    RemoteConnection { connection_id: String },
    #[allow(dead_code)]
    RemoteProject { connection_id: String, project_id: String },
}

/// Sidebar view with project and terminal list
pub struct Sidebar {
    workspace: Entity<Workspace>,
    pub(super) request_broker: Entity<RequestBroker>,
    expanded_projects: HashSet<String>,
    /// Projects whose worktree children list is collapsed.
    /// Uses negative-sense (collapsed) because worktrees should be visible by default.
    /// This is the inverse of `expanded_projects` which uses positive-sense because
    /// terminal details should be hidden by default.
    collapsed_worktrees: HashSet<String>,
    pub(super) terminals: TerminalsRegistry,
    /// Terminal rename state: (project_id, terminal_id)
    pub(super) terminal_rename: Option<RenameState<(String, String)>>,
    /// Double-click detector for terminals
    terminal_click_detector: ClickDetector<String>,
    /// Project rename state
    pub(super) project_rename: Option<RenameState<String>>,
    /// Double-click detector for projects
    project_click_detector: ClickDetector<String>,
    /// Project ID for which color picker is shown
    color_picker_project_id: Option<String>,
    /// Folder rename state
    pub(super) folder_rename: Option<RenameState<String>>,
    /// Double-click detector for folders
    folder_click_detector: ClickDetector<String>,
    /// Folder ID for which color picker is shown
    color_picker_folder_id: Option<String>,
    /// Anchor position (window coords) for the currently open popover
    popover_anchor: gpui::Point<gpui::Pixels>,
    /// Sidebar requests drained from Workspace by observer, applied in render() (needs Window)
    pending_sidebar_requests: Vec<SidebarRequest>,
    /// Focus handle for keyboard event capture
    focus_handle: FocusHandle,
    /// Scroll handle for programmatic scrolling
    scroll_handle: ScrollHandle,
    /// Current keyboard cursor position (index into flat item list)
    cursor_index: Option<usize>,
    /// Saved focus handle to restore when leaving sidebar
    pub saved_focus: Option<FocusHandle>,
    /// Remote connection manager (optional - set after creation)
    pub(super) remote_manager: Option<Entity<RemoteConnectionManager>>,
    /// Collapsed state for remote connections
    pub(super) collapsed_connections: HashMap<String, bool>,
    /// Terminal backend for building dispatchers
    backend: Option<Arc<dyn TerminalBackend>>,
    /// Service manager (optional - set after creation)
    pub(super) service_manager: Option<Entity<ServiceManager>>,
    /// Collapsed state for group headers (Terminals/Services) per project
    collapsed_groups: HashSet<(String, GroupKind)>,
    /// Project IDs that have been auto-expanded due to hook terminals.
    /// Tracked so we only auto-expand once (user can collapse afterward).
    hook_auto_expanded: HashSet<String>,
    /// Parent project IDs with in-flight worktree creation (debounce guard)
    creating_worktree: HashSet<String>,
    /// Project ID for which the worktree list popover is open
    worktree_list_project_id: Option<String>,
    /// Cached git worktrees for the worktree list popover (computed eagerly on show)
    worktree_list_entries: Vec<(String, String)>,
}

impl Sidebar {
    pub fn new(workspace: Entity<Workspace>, request_broker: Entity<RequestBroker>, terminals: TerminalsRegistry, cx: &mut Context<Self>) -> Self {
        // Observe RequestBroker to drain sidebar requests outside of render().
        // Requests are stored in pending_sidebar_requests and applied in render()
        // where Window access is available (needed for focus/rename).
        cx.observe(&request_broker, |this, _broker, cx| {
            if !this.request_broker.read(cx).has_sidebar_requests() {
                return;
            }
            let requests = this.request_broker.update(cx, |broker, _cx| {
                broker.drain_sidebar_requests()
            });
            this.pending_sidebar_requests.extend(requests);
            cx.notify();
        }).detach();

        // Auto-expand projects that gain hook terminals (outside of render).
        // Tracked in hook_auto_expanded so we only expand once per project
        // (user can collapse afterward without it re-expanding).
        cx.observe(&workspace, |this, workspace, cx| {
            let ws = workspace.read(cx);
            let mut changed = false;
            for project in &ws.data().projects {
                // Auto-expand projects with hook terminals
                if !project.hook_terminals.is_empty() && this.hook_auto_expanded.insert(project.id.clone()) {
                    this.expanded_projects.insert(project.id.clone());
                    changed = true;
                }
            }
            let before_len = this.hook_auto_expanded.len();
            this.hook_auto_expanded.retain(|id| {
                ws.data().projects.iter().any(|p| p.id == *id && !p.hook_terminals.is_empty())
            });
            if changed || this.hook_auto_expanded.len() != before_len {
                cx.notify();
            }
        }).detach();

        Self {
            workspace,
            request_broker,
            expanded_projects: HashSet::new(),
            collapsed_worktrees: HashSet::new(),
            terminals,
            terminal_rename: None,
            terminal_click_detector: ClickDetector::new(),
            project_rename: None,
            project_click_detector: ClickDetector::new(),
            color_picker_project_id: None,
            folder_rename: None,
            folder_click_detector: ClickDetector::new(),
            color_picker_folder_id: None,
            popover_anchor: gpui::point(gpui::px(0.0), gpui::px(0.0)),
            pending_sidebar_requests: Vec::new(),
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            cursor_index: None,
            saved_focus: None,
            remote_manager: None,
            collapsed_connections: HashMap::new(),
            backend: None,
            service_manager: None,
            collapsed_groups: HashSet::new(),
            hook_auto_expanded: HashSet::new(),
            creating_worktree: HashSet::new(),
            worktree_list_project_id: None,
            worktree_list_entries: Vec::new(),
        }
    }

    /// Check for double-click on terminal and return true if detected
    pub(super) fn check_double_click(&mut self, terminal_id: &str) -> bool {
        self.terminal_click_detector.check(terminal_id.to_string())
    }

    /// Whether a project row should show its children (layout or worktrees).
    /// Worktree parents use negative-sense (collapsed set), others use positive-sense (expanded set).
    pub(super) fn is_project_expanded(&self, project_id: &str, has_worktrees: bool) -> bool {
        if has_worktrees {
            !self.collapsed_worktrees.contains(project_id)
        } else {
            self.expanded_projects.contains(project_id)
        }
    }

    fn toggle_expanded(&mut self, project_id: &str) {
        if self.expanded_projects.contains(project_id) {
            self.expanded_projects.remove(project_id);
        } else {
            self.expanded_projects.insert(project_id.to_string());
        }
    }

    pub(super) fn toggle_worktrees_collapsed(&mut self, project_id: &str) {
        if self.collapsed_worktrees.contains(project_id) {
            self.collapsed_worktrees.remove(project_id);
        } else {
            self.collapsed_worktrees.insert(project_id.to_string());
        }
    }

    /// Spawn quick worktree creation on a background thread.
    /// All blocking git operations (branch name generation, worktree creation)
    /// run off the main thread to avoid UI jank.
    pub(super) fn spawn_quick_create_worktree(&mut self, project_id: &str, cx: &mut Context<Self>) {
        // Debounce: prevent concurrent creation for the same parent
        if !self.creating_worktree.insert(project_id.to_string()) {
            return;
        }

        let workspace = self.workspace.clone();
        let parent_id = project_id.to_string();
        let parent_id_for_cleanup = parent_id.clone();

        // Collect data from workspace and settings (non-blocking reads)
        let prep = self.workspace.read(cx).prepare_quick_create(project_id);
        let path_template = crate::settings::settings(cx).worktree.path_template.clone();
        let Some((parent_path, main_repo_path)) = prep else {
            log::error!("Quick worktree creation failed: parent project not found");
            self.creating_worktree.remove(project_id);
            return;
        };

        cx.spawn(async move |sidebar_weak, cx| {
            // Phase 1 (fast): resolve git root, generate branch name, compute
            // paths — no network calls needed.
            let prep_result = smol::unblock(move || -> Result<(String, std::path::PathBuf, String, String, Option<String>), String> {
                let project_path = std::path::PathBuf::from(&parent_path);

                // Determine git root
                let git_root = main_repo_path
                    .map(std::path::PathBuf::from)
                    .or_else(|| crate::git::get_repo_root(&project_path))
                    .ok_or_else(|| "Not a git repository".to_string())?;

                // Compute subdir (project path relative to git root)
                let normalized_project = crate::git::repository::normalize_path(&project_path);
                let normalized_root = crate::git::repository::normalize_path(&git_root);
                let subdir = normalized_project.strip_prefix(&normalized_root)
                    .unwrap_or(std::path::Path::new(""))
                    .to_path_buf();

                // Generate branch name (username cached, branch listing is local)
                let branch = crate::git::branch_names::generate_branch_name(&git_root);

                // Fast local lookup for default branch (no network)
                let default_branch = crate::git::repository::get_default_branch(&git_root);

                // Compute target paths
                let (worktree_path, project_path) = crate::git::repository::compute_target_paths(
                    &git_root, &subdir, &path_template, &branch,
                );

                Ok((branch, git_root, worktree_path, project_path, default_branch))
            }).await;

            let (branch, git_root, worktree_path, project_path, default_branch) = match prep_result {
                Ok(v) => v,
                Err(e) => {
                    log::error!("Quick worktree creation failed: {}", e);
                    let _ = sidebar_weak.update(cx, |sidebar, cx| {
                        sidebar.creating_worktree.remove(&parent_id_for_cleanup);
                        cx.notify();
                    });
                    return;
                }
            };

            // Register project in sidebar immediately so it appears instantly.
            // Hooks are deferred until the worktree directory exists on disk.
            let project_id = cx.update(|cx| {
                workspace.update(cx, |ws, cx| {
                    let id = ws.register_worktree_project_deferred_hooks(
                        &parent_id, &branch, &git_root,
                        &worktree_path, &project_path, cx,
                    );
                    if let Ok(ref id) = id {
                        ws.creating_projects.insert(id.clone());
                    }
                    id
                })
            });

            let Ok(project_id) = project_id else {
                log::error!("Quick worktree creation failed: could not register project");
                let _ = sidebar_weak.update(cx, |sidebar, cx| {
                    sidebar.creating_worktree.remove(&parent_id_for_cleanup);
                    cx.notify();
                });
                return;
            };

            // Phase 2 (slow): fetch + git worktree add in background.
            // The project is already visible in the sidebar.
            let branch_clone = branch.clone();
            let worktree_path_clone = worktree_path.clone();
            let git_root_clone = git_root.clone();
            let create_result = smol::unblock(move || -> Result<(), String> {
                let target = std::path::PathBuf::from(&worktree_path_clone);

                // Fetch and create worktree — fetch runs first if we have a default branch
                if let Some(ref db) = default_branch {
                    if let Some(repo_str) = git_root_clone.to_str() {
                        let _ = crate::process::safe_output(
                            crate::process::command("git")
                                .args(["-C", repo_str, "fetch", "origin", db.as_str()]),
                        );
                    }
                }

                crate::git::repository::create_worktree_with_start_point(
                    &git_root_clone,
                    &branch_clone,
                    &target,
                    default_branch.as_deref(),
                )
            }).await;

            match create_result {
                Ok(()) => {
                    // Worktree directory exists — clear creating state and fire hooks
                    let _ = cx.update(|cx| {
                        workspace.update(cx, |ws, cx| {
                            ws.creating_projects.remove(&project_id);
                            ws.fire_worktree_hooks(&project_id, cx);
                            ws.notify_data(cx);
                        });
                    });
                }
                Err(e) => {
                    log::error!("Quick worktree git operation failed: {}", e);
                    // Remove the optimistically-added project since git worktree add failed
                    let _ = cx.update(|cx| {
                        workspace.update(cx, |ws, cx| {
                            ws.creating_projects.remove(&project_id);
                            ws.delete_project(&project_id, cx);
                        });
                    });
                }
            }

            // Clear debounce guard
            let _ = sidebar_weak.update(cx, |sidebar, cx| {
                sidebar.creating_worktree.remove(&parent_id_for_cleanup);
                cx.notify();
            });
        }).detach();
    }

    fn toggle_group(&mut self, project_id: &str, group: GroupKind) {
        let key = (project_id.to_string(), group);
        if self.collapsed_groups.contains(&key) {
            self.collapsed_groups.remove(&key);
        } else {
            self.collapsed_groups.insert(key);
        }
    }

    fn is_group_collapsed(&self, project_id: &str, group: &GroupKind) -> bool {
        self.collapsed_groups.contains(&(project_id.to_string(), group.clone()))
    }

    /// Render expanded children (terminals group + services group) for a project.
    /// Returns elements and advances flat_idx.
    fn render_expanded_children(
        &self,
        project: &SidebarProjectInfo,
        group_header_padding: f32,
        group_items_padding: f32,
        id_prefix: &str,
        cursor_index: Option<usize>,
        flat_idx: &mut usize,
        flat_elements: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        let t = theme(cx);

        // Terminals group
        if !project.terminal_ids.is_empty() {
            let is_collapsed = self.is_group_collapsed(&project.id, &GroupKind::Terminals);
            let is_cursor = cursor_index == Some(*flat_idx);
            let project_id = project.id.clone();
            flat_elements.push(
                item_widgets::sidebar_group_header(
                    ElementId::Name(format!("{}term-group-{}", id_prefix, project.id).into()),
                    GroupKind::Terminals.label(),
                    project.terminal_ids.len(),
                    is_collapsed,
                    is_cursor,
                    group_header_padding,
                    &t,
                )
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.toggle_group(&project_id, GroupKind::Terminals);
                    cx.notify();
                }))
                .into_any_element()
            );
            *flat_idx += 1;

            if !is_collapsed {
                let minimized_states: Vec<(String, bool)> = {
                    let ws = self.workspace.read(cx);
                    project.terminal_ids.iter().map(|id| {
                        (id.clone(), ws.is_terminal_minimized(&project.id, id))
                    }).collect()
                };
                for (tid, is_minimized) in &minimized_states {
                    let is_cursor = cursor_index == Some(*flat_idx);
                    let is_inactive_tab = project.inactive_tab_terminals.contains(tid.as_str());
                    let is_in_tab_group = project.tab_group_terminals.contains(tid.as_str());
                    flat_elements.push(
                        self.render_terminal_item(
                            &project.id, tid, &project.terminal_names,
                            *is_minimized, is_inactive_tab, is_in_tab_group,
                            group_items_padding, id_prefix, is_cursor, cx,
                        )
                        .into_any_element()
                    );
                    *flat_idx += 1;
                }
            }
        }

        // Services group
        if !project.services.is_empty() {
            let is_collapsed = self.is_group_collapsed(&project.id, &GroupKind::Services);
            let is_cursor = cursor_index == Some(*flat_idx);
            flat_elements.push(
                self.render_services_group_header(project, is_collapsed, is_cursor, group_header_padding, cx)
                    .into_any_element()
            );
            *flat_idx += 1;

            if !is_collapsed {
                for service in &project.services {
                    let is_cursor = cursor_index == Some(*flat_idx);
                    flat_elements.push(
                        self.render_service_item(project, service, group_items_padding, is_cursor, cx)
                            .into_any_element()
                    );
                    *flat_idx += 1;
                }
            }
        }

        // Hooks group
        if !project.hook_terminals.is_empty() {
            let is_collapsed = self.is_group_collapsed(&project.id, &GroupKind::Hooks);
            let is_cursor = cursor_index == Some(*flat_idx);
            flat_elements.push(
                self.render_hooks_group_header(project, is_collapsed, is_cursor, group_header_padding, cx)
                    .into_any_element()
            );
            *flat_idx += 1;

            if !is_collapsed {
                for hook in &project.hook_terminals {
                    let is_cursor = cursor_index == Some(*flat_idx);
                    flat_elements.push(
                        self.render_hook_item(project, hook, group_items_padding, is_cursor, cx)
                            .into_any_element()
                    );
                    *flat_idx += 1;
                }
            }
        }
    }

    pub(super) fn start_rename(&mut self, project_id: String, terminal_id: String, current_name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal_rename = Some(start_rename_with_blur(
            (project_id, terminal_id),
            &current_name,
            "Terminal name...",
            |this, _window, cx| this.finish_rename(cx),
            window,
            cx,
        ));
        self.workspace.update(cx, |ws, cx| ws.clear_focused_terminal(cx));
        cx.notify();
    }

    pub(super) fn finish_rename(&mut self, cx: &mut Context<Self>) {
        if let Some(((project_id, terminal_id), new_name)) = finish_rename(&mut self.terminal_rename, cx) {
            if let Some(dispatcher) = self.dispatcher_for_project(&project_id, cx) {
                dispatcher.dispatch(okena_core::api::ActionRequest::RenameTerminal {
                    project_id,
                    terminal_id,
                    name: new_name,
                }, cx);
            }
        }
        self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        cx.notify();
    }

    pub(super) fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        cancel_rename(&mut self.terminal_rename);
        self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        cx.notify();
    }

    /// Check for double-click on project and return true if detected
    pub(super) fn check_project_double_click(&mut self, project_id: &str) -> bool {
        self.project_click_detector.check(project_id.to_string())
    }

    pub(super) fn start_project_rename(&mut self, project_id: String, current_name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.project_rename = Some(start_rename_with_blur(
            project_id,
            &current_name,
            "Project name...",
            |this, _window, cx| this.finish_project_rename(cx),
            window,
            cx,
        ));
        self.workspace.update(cx, |ws, cx| ws.clear_focused_terminal(cx));
        cx.notify();
    }

    pub(super) fn finish_project_rename(&mut self, cx: &mut Context<Self>) {
        if let Some((project_id, new_name)) = finish_rename(&mut self.project_rename, cx) {
            self.workspace.update(cx, |ws, cx| {
                ws.rename_project(&project_id, new_name, cx);
            });
        }
        self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        cx.notify();
    }

    pub(super) fn cancel_project_rename(&mut self, cx: &mut Context<Self>) {
        cancel_rename(&mut self.project_rename);
        self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        cx.notify();
    }

    pub(super) fn show_color_picker(&mut self, project_id: String, anchor: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        self.color_picker_project_id = Some(project_id);
        self.color_picker_folder_id = None;
        self.popover_anchor = anchor;
        cx.notify();
    }

    pub(super) fn show_folder_color_picker(&mut self, folder_id: String, anchor: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        self.color_picker_folder_id = Some(folder_id);
        self.color_picker_project_id = None;
        self.popover_anchor = anchor;
        cx.notify();
    }

    fn hide_color_picker(&mut self, cx: &mut Context<Self>) {
        self.color_picker_project_id = None;
        self.color_picker_folder_id = None;
        cx.notify();
    }

    pub(super) fn show_worktree_list(&mut self, project_id: String, anchor: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let project_path = self.workspace.read(cx).project(&project_id)
            .map(|p| p.path.clone())
            .unwrap_or_default();
        self.worktree_list_entries = crate::git::repository::list_git_worktrees(
            std::path::Path::new(&project_path),
        );
        self.worktree_list_project_id = Some(project_id);
        self.popover_anchor = anchor;
        cx.notify();
    }

    fn hide_worktree_list(&mut self, cx: &mut Context<Self>) {
        self.worktree_list_project_id = None;
        self.worktree_list_entries.clear();
        cx.notify();
    }

    pub(super) fn set_folder_color(&mut self, project_id: &str, color: FolderColor, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| {
            ws.set_folder_color(project_id, color, cx);
        });
        // Send to server for remote projects
        if let Some(conn_id) = self.workspace.read(cx).project(project_id)
            .filter(|p| p.is_remote)
            .and_then(|p| p.connection_id.clone())
        {
            if let Some(ref manager) = self.remote_manager {
                let server_id = okena_core::client::strip_prefix(project_id, &conn_id);
                manager.update(cx, |rm, cx| {
                    rm.send_action(&conn_id, okena_core::api::ActionRequest::SetProjectColor {
                        project_id: server_id,
                        color,
                    }, cx);
                });
            }
        }
        self.hide_color_picker(cx);
    }

    pub(super) fn set_folder_item_color(&mut self, folder_id: &str, color: FolderColor, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| {
            ws.set_folder_item_color(folder_id, color, cx);
        });
        self.hide_color_picker(cx);
    }

    fn request_context_menu(&mut self, project_id: String, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.request_broker.update(cx, |broker, cx| {
            broker.push_overlay_request(crate::workspace::requests::OverlayRequest::ContextMenu {
                project_id,
                position,
            }, cx);
        });
    }

    /// Check for double-click on folder and return true if detected
    pub(super) fn check_folder_double_click(&mut self, folder_id: &str) -> bool {
        self.folder_click_detector.check(folder_id.to_string())
    }

    pub(super) fn start_folder_rename(&mut self, folder_id: String, current_name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.folder_rename = Some(start_rename_with_blur(
            folder_id,
            &current_name,
            "Folder name...",
            |this, _window, cx| this.finish_folder_rename(cx),
            window,
            cx,
        ));
        self.workspace.update(cx, |ws, cx| ws.clear_focused_terminal(cx));
        cx.notify();
    }

    pub(super) fn finish_folder_rename(&mut self, cx: &mut Context<Self>) {
        if let Some((folder_id, new_name)) = finish_rename(&mut self.folder_rename, cx) {
            self.workspace.update(cx, |ws, cx| {
                ws.rename_folder(&folder_id, new_name, cx);
            });
        }
        self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        cx.notify();
    }

    pub(super) fn cancel_folder_rename(&mut self, cx: &mut Context<Self>) {
        cancel_rename(&mut self.folder_rename);
        self.workspace.update(cx, |ws, cx| ws.restore_focused_terminal(cx));
        cx.notify();
    }

    fn create_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let folder_id = self.workspace.update(cx, |ws, cx| {
            ws.create_folder("New Folder".to_string(), cx)
        });
        // Immediately start renaming the new folder
        self.start_folder_rename(folder_id, "New Folder".to_string(), window, cx);
    }

    /// Public accessor for the focus handle (used by RootView for FocusSidebar)
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    pub fn set_backend(&mut self, backend: Arc<dyn TerminalBackend>) {
        self.backend = Some(backend);
    }

    /// Build an ActionDispatcher for the given project.
    /// Returns Remote dispatcher for remote projects, Local for local ones.
    fn dispatcher_for_project(&self, project_id: &str, cx: &Context<Self>) -> Option<ActionDispatcher> {
        crate::action_dispatch::dispatcher_for_project(
            project_id,
            &self.workspace,
            &self.backend,
            &self.terminals,
            &self.service_manager,
            &self.remote_manager,
            cx,
        )
    }

    pub fn set_remote_manager(&mut self, manager: Entity<RemoteConnectionManager>, cx: &mut Context<Self>) {
        // Observe remote manager for changes
        cx.observe(&manager, |_this, _rm, cx| {
            cx.notify();
        }).detach();
        self.remote_manager = Some(manager);
        cx.notify();
    }

    pub fn set_service_manager(&mut self, manager: Entity<ServiceManager>, cx: &mut Context<Self>) {
        cx.observe(&manager, |_this, _sm, cx| {
            cx.notify();
        }).detach();
        self.service_manager = Some(manager);
        cx.notify();
    }


    /// Initialize cursor to the focused project or first item
    pub fn activate_cursor(&mut self, cx: &mut Context<Self>) {
        let items = self.build_cursor_items(cx);
        if items.is_empty() {
            self.cursor_index = None;
            return;
        }
        // Try to place cursor on the focused project
        let focused_id = self.workspace.read(cx).focused_project_id().cloned();
        if let Some(ref focused_id) = focused_id {
            if let Some(pos) = items.iter().position(|item| match item {
                SidebarCursorItem::Project { project_id } |
                SidebarCursorItem::WorktreeProject { project_id } => project_id == focused_id,
                _ => false,
            }) {
                self.cursor_index = Some(pos);
                cx.notify();
                return;
            }
        }
        self.cursor_index = Some(0);
        cx.notify();
    }

    /// Build a flat list of cursor items matching the visual render order
    fn build_cursor_items(&self, cx: &mut Context<Self>) -> Vec<SidebarCursorItem> {
        let workspace = self.workspace.read(cx);
        let all_projects: HashMap<&str, &ProjectData> = workspace.data().projects.iter()
            .map(|p| (p.id.as_str(), p))
            .collect();
        let all_project_ids: HashSet<&str> = workspace.data().projects.iter()
            .map(|p| p.id.as_str()).collect();

        // Pre-collect service names per project (avoids borrow issues with cx)
        let service_names: HashMap<String, Vec<String>> = if let Some(ref sm) = self.service_manager {
            let sm = sm.read(cx);
            workspace.data().projects.iter()
                .filter(|p| sm.has_services(&p.id))
                .map(|p| {
                    let names = sm.services_for_project(&p.id)
                        .into_iter()
                        .map(|inst| inst.definition.name.clone())
                        .collect();
                    (p.id.clone(), names)
                })
                .collect()
        } else {
            HashMap::new()
        };

        // Pre-collect hook terminal IDs per project
        let hook_terminal_ids: HashMap<String, Vec<String>> = workspace.data().projects.iter()
            .filter(|p| !p.hook_terminals.is_empty())
            .map(|p| {
                let ids = p.hook_terminals.keys().cloned().collect();
                (p.id.clone(), ids)
            })
            .collect();

        // Build worktree children map
        let mut worktree_children_map: HashMap<String, Vec<&ProjectData>> = HashMap::new();
        for parent in &workspace.data().projects {
            if !parent.worktree_ids.is_empty() {
                let mut children = Vec::new();
                for wt_id in &parent.worktree_ids {
                    if let Some(&child) = all_projects.get(wt_id.as_str()) {
                        children.push(child);
                    }
                }
                if !children.is_empty() {
                    worktree_children_map.insert(parent.id.clone(), children);
                }
            }
        }

        let mut cursor_items = Vec::new();

        for id in &workspace.data().project_order {
            // Check if this is a folder
            if let Some(folder) = workspace.data().folders.iter().find(|f| &f.id == id) {
                cursor_items.push(SidebarCursorItem::Folder { folder_id: folder.id.clone() });

                if !folder.collapsed {
                    for pid in &folder.project_ids {
                        if let Some(&project) = all_projects.get(pid.as_str()) {
                            // Skip worktree children that have a parent in the project list
                            if project.worktree_info.as_ref().map_or(false, |w| {
                                all_project_ids.contains(w.parent_project_id.as_str())
                            }) {
                                continue;
                            }
                            self.push_project_cursor_items(project, &worktree_children_map, &service_names, &hook_terminal_ids, &mut cursor_items);
                        }
                    }
                }
                continue;
            }

            // Top-level project (not a worktree child of another)
            if let Some(&project) = all_projects.get(id.as_str()) {
                if project.worktree_info.as_ref().map_or(false, |w| {
                    all_project_ids.contains(w.parent_project_id.as_str())
                }) {
                    continue;
                }
                self.push_project_cursor_items(project, &worktree_children_map, &service_names, &hook_terminal_ids, &mut cursor_items);
            }
        }

        cursor_items
    }

    /// Helper: push a project row + its expanded terminals/services + worktree children into cursor items
    fn push_project_cursor_items(
        &self,
        project: &ProjectData,
        worktree_children_map: &HashMap<String, Vec<&ProjectData>>,
        service_names: &HashMap<String, Vec<String>>,
        hook_terminal_ids: &HashMap<String, Vec<String>>,
        cursor_items: &mut Vec<SidebarCursorItem>,
    ) {
        let has_worktrees = worktree_children_map.get(&project.id).map_or(false, |c| !c.is_empty());
        let is_orphan = project.worktree_info.is_some();

        if has_worktrees && !is_orphan {
            // Group header mode: Project = group header, WorktreeProject = main project child
            cursor_items.push(SidebarCursorItem::Project { project_id: project.id.clone() });

            let is_expanded = self.is_project_expanded(&project.id, true);
            if is_expanded {
                // Main project as first child
                cursor_items.push(SidebarCursorItem::WorktreeProject { project_id: project.id.clone() });

                if self.expanded_projects.contains(&project.id) {
                    self.push_group_cursor_items(&project.id, &project.layout, service_names, hook_terminal_ids, cursor_items);
                }

                // Worktree children as siblings
                if let Some(children) = worktree_children_map.get(&project.id) {
                    for child in children {
                        cursor_items.push(SidebarCursorItem::WorktreeProject { project_id: child.id.clone() });
                        if self.expanded_projects.contains(&child.id) {
                            self.push_group_cursor_items(&child.id, &child.layout, service_names, hook_terminal_ids, cursor_items);
                        }
                    }
                }
            }
        } else {
            // Standard mode: no worktrees
            cursor_items.push(SidebarCursorItem::Project { project_id: project.id.clone() });

            if self.expanded_projects.contains(&project.id) {
                self.push_group_cursor_items(&project.id, &project.layout, service_names, hook_terminal_ids, cursor_items);
            }
        }
    }

    /// Push group headers and their child cursor items for an expanded project.
    fn push_group_cursor_items(
        &self,
        project_id: &str,
        layout: &Option<crate::workspace::state::LayoutNode>,
        service_names: &HashMap<String, Vec<String>>,
        hook_terminal_ids: &HashMap<String, Vec<String>>,
        cursor_items: &mut Vec<SidebarCursorItem>,
    ) {
        // Terminals group
        if let Some(layout) = layout {
            let terminal_ids = layout.collect_terminal_ids();
            if !terminal_ids.is_empty() {
                cursor_items.push(SidebarCursorItem::GroupHeader {
                    project_id: project_id.to_string(),
                    group: GroupKind::Terminals,
                });

                if !self.is_group_collapsed(project_id, &GroupKind::Terminals) {
                    for tid in terminal_ids {
                        cursor_items.push(SidebarCursorItem::Terminal {
                            project_id: project_id.to_string(),
                            terminal_id: tid,
                        });
                    }
                }
            }
        }

        // Services group
        if let Some(names) = service_names.get(project_id) {
            if !names.is_empty() {
                cursor_items.push(SidebarCursorItem::GroupHeader {
                    project_id: project_id.to_string(),
                    group: GroupKind::Services,
                });

                if !self.is_group_collapsed(project_id, &GroupKind::Services) {
                    for name in names {
                        cursor_items.push(SidebarCursorItem::Service {
                            project_id: project_id.to_string(),
                            service_name: name.clone(),
                        });
                    }
                }
            }
        }

        // Hooks group
        if let Some(tids) = hook_terminal_ids.get(project_id) {
            if !tids.is_empty() {
                cursor_items.push(SidebarCursorItem::GroupHeader {
                    project_id: project_id.to_string(),
                    group: GroupKind::Hooks,
                });

                if !self.is_group_collapsed(project_id, &GroupKind::Hooks) {
                    for tid in tids {
                        cursor_items.push(SidebarCursorItem::Hook {
                            project_id: project_id.to_string(),
                            terminal_id: tid.clone(),
                        });
                    }
                }
            }
        }
    }

    /// Clamp cursor to valid range
    fn validate_cursor(&mut self, item_count: usize) {
        if item_count == 0 {
            self.cursor_index = None;
        } else if let Some(ref mut idx) = self.cursor_index {
            if *idx >= item_count {
                *idx = item_count - 1;
            }
        }
    }

    /// Check if any rename or color picker is active (blocks keyboard nav)
    fn is_interactive_mode_active(&self) -> bool {
        self.terminal_rename.is_some()
            || self.project_rename.is_some()
            || self.folder_rename.is_some()
            || self.color_picker_project_id.is_some()
            || self.color_picker_folder_id.is_some()
            || self.worktree_list_project_id.is_some()
    }

    fn handle_sidebar_up(&mut self, _: &SidebarUp, _window: &mut Window, cx: &mut Context<Self>) {
        if self.is_interactive_mode_active() { return; }
        let items = self.build_cursor_items(cx);
        if items.is_empty() { return; }
        match self.cursor_index {
            Some(idx) if idx > 0 => self.cursor_index = Some(idx - 1),
            None => self.cursor_index = Some(items.len() - 1),
            _ => {}
        }
        self.scroll_to_cursor(items.len());
        cx.notify();
    }

    fn handle_sidebar_down(&mut self, _: &SidebarDown, _window: &mut Window, cx: &mut Context<Self>) {
        if self.is_interactive_mode_active() { return; }
        let items = self.build_cursor_items(cx);
        if items.is_empty() { return; }
        match self.cursor_index {
            Some(idx) if idx < items.len() - 1 => self.cursor_index = Some(idx + 1),
            None => self.cursor_index = Some(0),
            _ => {}
        }
        self.scroll_to_cursor(items.len());
        cx.notify();
    }

    fn handle_sidebar_confirm(&mut self, _: &SidebarConfirm, window: &mut Window, cx: &mut Context<Self>) {
        if self.project_rename.is_some() {
            self.finish_project_rename(cx);
            return;
        }
        if self.folder_rename.is_some() {
            self.finish_folder_rename(cx);
            return;
        }
        if self.terminal_rename.is_some() {
            self.finish_rename(cx);
            return;
        }
        if self.is_interactive_mode_active() { return; }
        let items = self.build_cursor_items(cx);
        let Some(idx) = self.cursor_index else { return };
        let Some(item) = items.get(idx) else { return };

        match item.clone() {
            SidebarCursorItem::Project { project_id } => {
                // Project may be a group header (has worktrees) → non-individual focus
                let has_worktrees = !self.workspace.read(cx).worktree_child_ids(&project_id).is_empty();
                if has_worktrees {
                    self.workspace.update(cx, |ws, cx| {
                        ws.set_focused_project(Some(project_id.clone()), cx);
                    });
                } else {
                    self.workspace.update(cx, |ws, cx| {
                        ws.set_focused_project_individual(Some(project_id.clone()), cx);
                    });
                }
                self.cursor_index = None;
                if let Some(ref saved) = self.saved_focus {
                    window.focus(saved, cx);
                }
                self.saved_focus = None;
            }
            SidebarCursorItem::WorktreeProject { project_id } => {
                self.workspace.update(cx, |ws, cx| {
                    ws.set_focused_project_individual(Some(project_id.clone()), cx);
                });
                self.cursor_index = None;
                if let Some(ref saved) = self.saved_focus {
                    window.focus(saved, cx);
                }
                self.saved_focus = None;
            }
            SidebarCursorItem::Terminal { project_id, terminal_id } => {
                self.workspace.update(cx, |ws, cx| {
                    ws.focus_terminal_by_id(&project_id, &terminal_id, cx);
                });
                self.cursor_index = None;
                if let Some(ref saved) = self.saved_focus {
                    window.focus(saved, cx);
                }
                self.saved_focus = None;
            }
            SidebarCursorItem::Folder { folder_id } => {
                self.workspace.update(cx, |ws, cx| {
                    ws.toggle_folder_collapsed(&folder_id, cx);
                });
            }
            SidebarCursorItem::GroupHeader { project_id, group } => {
                self.toggle_group(&project_id, group);
            }
            SidebarCursorItem::Service { project_id, service_name } => {
                // Toggle start/stop for the service
                if let Some(ref sm) = self.service_manager {
                    sm.update(cx, |sm, cx| {
                        let key = (project_id.clone(), service_name.clone());
                        if let Some(inst) = sm.instances().get(&key) {
                            match inst.status {
                                crate::services::manager::ServiceStatus::Running |
                                crate::services::manager::ServiceStatus::Starting => {
                                    sm.stop_service(&project_id, &service_name, cx);
                                }
                                _ => {
                                    if let Some(path) = sm.project_path(&project_id) {
                                        let path = path.clone();
                                        sm.start_service(&project_id, &service_name, &path, cx);
                                    }
                                }
                            }
                        }
                    });
                }
            }
            SidebarCursorItem::RemoteConnection { connection_id } => {
                let collapsed = self.collapsed_connections.get(&connection_id).copied().unwrap_or(false);
                self.collapsed_connections.insert(connection_id, !collapsed);
            }
            SidebarCursorItem::RemoteProject { project_id, .. } => {
                // Remote projects are now materialized in workspace, use unified focus
                self.workspace.update(cx, |ws, cx| {
                    ws.set_focused_project_individual(Some(project_id.clone()), cx);
                });
                self.cursor_index = None;
                if let Some(ref saved) = self.saved_focus {
                    window.focus(saved, cx);
                }
                self.saved_focus = None;
            }
            SidebarCursorItem::Hook { project_id, terminal_id } => {
                self.workspace.update(cx, |ws, cx| {
                    ws.focus_terminal_by_id(&project_id, &terminal_id, cx);
                });
                self.cursor_index = None;
                if let Some(ref saved) = self.saved_focus {
                    window.focus(saved, cx);
                }
                self.saved_focus = None;
            }
        }
        cx.notify();
    }

    fn handle_sidebar_toggle_expand(&mut self, _: &SidebarToggleExpand, _window: &mut Window, cx: &mut Context<Self>) {
        if self.is_interactive_mode_active() { return; }
        let items = self.build_cursor_items(cx);
        let Some(idx) = self.cursor_index else { return };
        let Some(item) = items.get(idx) else { return };

        match item.clone() {
            SidebarCursorItem::Folder { folder_id } => {
                self.workspace.update(cx, |ws, cx| {
                    ws.toggle_folder_collapsed(&folder_id, cx);
                });
            }
            SidebarCursorItem::Project { project_id } => {
                // Mirror mouse behavior: toggle worktree collapse for parent projects,
                // terminal details for projects without worktrees
                let has_worktrees = !self.workspace.read(cx)
                    .worktree_child_ids(&project_id).is_empty();
                if has_worktrees {
                    self.toggle_worktrees_collapsed(&project_id);
                } else {
                    self.toggle_expanded(&project_id);
                }
            }
            SidebarCursorItem::WorktreeProject { project_id } => {
                self.toggle_expanded(&project_id);
            }
            SidebarCursorItem::GroupHeader { project_id, group } => {
                self.toggle_group(&project_id, group);
            }
            SidebarCursorItem::Terminal { .. } | SidebarCursorItem::Service { .. } | SidebarCursorItem::Hook { .. } => {}
            SidebarCursorItem::RemoteConnection { connection_id } => {
                let collapsed = self.collapsed_connections.get(&connection_id).copied().unwrap_or(false);
                self.collapsed_connections.insert(connection_id, !collapsed);
            }
            SidebarCursorItem::RemoteProject { .. } => {}
        }
        cx.notify();
    }

    fn handle_sidebar_escape(&mut self, _: &SidebarEscape, window: &mut Window, cx: &mut Context<Self>) {
        if self.project_rename.is_some() {
            self.cancel_project_rename(cx);
            return;
        }
        if self.folder_rename.is_some() {
            self.cancel_folder_rename(cx);
            return;
        }
        if self.terminal_rename.is_some() {
            self.cancel_rename(cx);
            return;
        }
        self.cursor_index = None;
        if let Some(ref saved) = self.saved_focus {
            window.focus(saved, cx);
        }
        self.saved_focus = None;
        cx.notify();
    }

    /// Scroll the sidebar to keep the cursor item visible
    fn scroll_to_cursor(&self, item_count: usize) {
        if let Some(idx) = self.cursor_index {
            if item_count > 0 {
                self.scroll_handle.scroll_to_item(idx);
            }
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        div()
            .h(px(35.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .justify_between()
            .bg(rgb(t.bg_header))
            .border_b_1()
            .border_color(rgb(t.border))
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_secondary))
                    .child("EXPLORER"),
            )
            .child(
                h_flex()
                    .gap(px(2.0))
                    .child(
                        // New folder button
                        div()
                            .id("new-folder-btn")
                            .cursor_pointer()
                            .px(px(4.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .child(
                                svg()
                                    .path("icons/folder.svg")
                                    .size(px(14.0))
                                    .text_color(rgb(t.text_secondary))
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.create_folder(window, cx);
                            })),
                    )
                    .child(
                        // Add project button
                        div()
                            .id("add-project-btn")
                            .cursor_pointer()
                            .px(px(4.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .text_color(rgb(t.text_secondary))
                                    .child("+"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(t.text_secondary))
                                    .child("Add Project"),
                            )
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.request_broker.update(cx, |broker, cx| {
                                    broker.push_overlay_request(
                                        crate::workspace::requests::OverlayRequest::AddProjectDialog,
                                        cx,
                                    );
                                });
                            })),
                    ),
            )
    }

    /// Count how many terminals from the given IDs are currently waiting for input
    pub(super) fn count_waiting_terminals(&self, terminal_ids: &[String]) -> usize {
        let terminals = self.terminals.lock();
        terminal_ids.iter()
            .filter(|id| terminals.get(id.as_str()).map_or(false, |t| t.is_waiting_for_input()))
            .count()
    }

    fn render_projects_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace_entity = self.workspace.clone();

        div()
            .h(px(28.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .justify_between()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .id("projects-header")
            .on_click(move |_, _window, cx| {
                workspace_entity.update(cx, |ws, cx| {
                    ws.set_focused_project(None, cx);
                    ws.set_folder_filter(None, cx);
                });
            })
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_secondary))
                    .child("PROJECTS"),
            )
    }
}

/// Service info for sidebar rendering.
#[derive(Clone)]
pub(super) struct SidebarServiceInfo {
    pub name: String,
    pub status: crate::services::manager::ServiceStatus,
    pub ports: Vec<u16>,
    /// Host for port badge URLs ("localhost" for local, remote host for remote)
    pub port_host: String,
    /// Whether this service is a Docker Compose service
    pub is_docker: bool,
}

/// Hook terminal info for sidebar rendering.
#[derive(Clone)]
pub(super) struct SidebarHookInfo {
    pub terminal_id: String,
    pub label: String,
    pub status: crate::workspace::state::HookTerminalStatus,
    pub command: String,
    pub cwd: String,
}

/// Lightweight projection of ProjectData for sidebar rendering.
/// Avoids cloning the full LayoutNode tree, path, hidden_terminals, and hooks
/// which are never used by the sidebar.
pub(super) struct SidebarProjectInfo {
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
    /// Whether this project is itself a worktree
    pub is_worktree: bool,
}

impl SidebarProjectInfo {
    fn from_project(project: &ProjectData) -> Self {
        let layout = project.layout.as_ref();
        Self {
            id: project.id.clone(),
            name: project.name.clone(),
            show_in_overview: project.show_in_overview,
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
            parent_project_id: project.worktree_info.as_ref().map(|w| w.parent_project_id.clone()),
            services: Vec::new(),
            hook_terminals: project.hook_terminals.iter().map(|(tid, entry)| {
                SidebarHookInfo {
                    terminal_id: tid.clone(),
                    label: entry.label.clone(),
                    status: entry.status.clone(),
                    command: entry.command.clone(),
                    cwd: entry.cwd.clone(),
                }
            }).collect(),
            is_closing: false,
            is_creating: false,
            is_worktree: project.worktree_info.is_some(),
        }
    }
}

/// An item in the sidebar's top-level ordering: either a project or a folder
enum SidebarItem {
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

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Process pending sidebar requests (drained from Workspace by observer)
        let pending = std::mem::take(&mut self.pending_sidebar_requests);
        for request in pending {
            match request {
                SidebarRequest::RenameProject { project_id, project_name } => {
                    self.start_project_rename(project_id, project_name, window, cx);
                }
                SidebarRequest::RenameFolder { folder_id, folder_name } => {
                    self.start_folder_rename(folder_id, folder_name, window, cx);
                }
                SidebarRequest::QuickCreateWorktree { project_id } => {
                    self.spawn_quick_create_worktree(&project_id, cx);
                }
                SidebarRequest::ShowWorktreeList { project_id } => {
                    // Use a default position since we don't have click coords from context menu
                    self.show_worktree_list(project_id, gpui::point(gpui::px(40.0), gpui::px(200.0)), cx);
                }
            }
        }


        // Clear cursor when sidebar loses focus
        if self.cursor_index.is_some() && !self.focus_handle.is_focused(window) {
            self.cursor_index = None;
        }

        let workspace = self.workspace.read(cx);

        // Collect all projects for lookup
        let all_projects: HashMap<&str, &ProjectData> = workspace.data().projects.iter()
            .map(|p| (p.id.as_str(), p))
            .collect();

        // Build worktree children map using parent's worktree_ids for deterministic ordering
        // Build worktree children map using parent's worktree_ids for deterministic ordering
        let mut worktree_children_map: HashMap<String, Vec<SidebarProjectInfo>> = HashMap::new();
        let all_project_ids: HashSet<&str> = workspace.data().projects.iter().map(|p| p.id.as_str()).collect();
        for parent in &workspace.data().projects {
            if !parent.worktree_ids.is_empty() {
                let mut children = Vec::new();
                for wt_id in &parent.worktree_ids {
                    if let Some(&p) = all_projects.get(wt_id.as_str()) {
                        let mut info = SidebarProjectInfo::from_project(p);
                        info.is_closing = workspace.closing_projects.contains(&p.id);
                        info.is_creating = workspace.creating_projects.contains(&p.id);
                        // Inherit parent project's color for visual association
                        info.folder_color = parent.folder_color;
                        children.push(info);
                    }
                }
                if !children.is_empty() {
                    worktree_children_map.insert(parent.id.clone(), children);
                }
            }
        }

        // Collect services from ServiceManager for all projects
        let mut project_services: HashMap<String, Vec<SidebarServiceInfo>> = if let Some(ref sm) = self.service_manager {
            let sm = sm.read(cx);
            workspace.data().projects.iter()
                .filter(|p| sm.has_services(&p.id))
                .map(|p| {
                    let services = sm.services_for_project(&p.id)
                        .into_iter()
                        .filter(|inst| !inst.is_extra)
                        .map(|inst| SidebarServiceInfo {
                            name: inst.definition.name.clone(),
                            status: inst.status.clone(),
                            ports: inst.detected_ports.clone(),
                            port_host: "localhost".to_string(),
                            is_docker: matches!(inst.kind, crate::services::manager::ServiceKind::DockerCompose { .. }),
                        })
                        .collect();
                    (p.id.clone(), services)
                })
                .collect()
        } else {
            HashMap::new()
        };

        // Also populate services from remote project data (for projects not covered by local ServiceManager)
        for project in &workspace.data().projects {
            if !project.remote_services.is_empty() && !project_services.contains_key(&project.id) {
                let port_host = project.remote_host.clone().unwrap_or_else(|| "localhost".to_string());
                let services = project.remote_services.iter()
                    .filter(|api_svc| !api_svc.is_extra)
                    .map(|api_svc| {
                        SidebarServiceInfo {
                            name: api_svc.name.clone(),
                            status: crate::services::manager::ServiceStatus::from_api(&api_svc.status, api_svc.exit_code),
                            ports: api_svc.ports.clone(),
                            port_host: port_host.clone(),
                            is_docker: api_svc.kind == "docker_compose",
                        }
                    }).collect();
                project_services.insert(project.id.clone(), services);
            }
        }

        // Build sidebar items from project_order
        let mut items: Vec<SidebarItem> = Vec::new();
        for (top_index, id) in workspace.data().project_order.iter().enumerate() {
            // Check if this is a folder
            if let Some(folder) = workspace.data().folders.iter().find(|f| &f.id == id) {
                let mut folder_projects: Vec<SidebarProjectInfo> = folder.project_ids.iter()
                    .filter_map(|pid| all_projects.get(pid.as_str()))
                    .filter(|p| p.worktree_info.is_none() || !all_project_ids.contains(
                        p.worktree_info.as_ref().map(|w| w.parent_project_id.as_str()).unwrap_or("")
                    ))
                    .map(|p| {
                        let mut info = SidebarProjectInfo::from_project(p);
                        info.is_orphan = p.worktree_info.as_ref().map_or(false, |wt| {
                            !all_project_ids.contains(wt.parent_project_id.as_str())
                        });
                        info.is_closing = workspace.closing_projects.contains(&p.id);
                        info.is_creating = workspace.creating_projects.contains(&p.id);
                        info
                    })
                    .collect();
                let mut folder_wt_children: HashMap<String, Vec<SidebarProjectInfo>> = HashMap::new();
                for fp in &mut folder_projects {
                    if let Some(mut children) = worktree_children_map.remove(&fp.id) {
                        fp.worktree_count = children.len();
                        for child in &mut children {
                            if let Some(services) = project_services.remove(&child.id) {
                                child.services = services;
                            }
                        }
                        folder_wt_children.insert(fp.id.clone(), children);
                    }
                    if let Some(services) = project_services.remove(&fp.id) {
                        fp.services = services;
                    }
                }
                items.push(SidebarItem::Folder {
                    folder: folder.clone(),
                    index: top_index,
                    projects: folder_projects,
                    worktree_children: folder_wt_children,
                });
                continue;
            }

            // Check if this is a top-level project (not a worktree child)
            if let Some(&project) = all_projects.get(id.as_str()) {
                if let Some(ref wt_info) = project.worktree_info {
                    if all_project_ids.contains(wt_info.parent_project_id.as_str()) {
                        // This is a worktree child shown under its parent, skip
                        continue;
                    }
                }
                let mut wt_children = worktree_children_map.remove(&project.id).unwrap_or_default();
                let mut project_info = SidebarProjectInfo::from_project(project);
                project_info.is_orphan = project.worktree_info.as_ref().map_or(false, |wt| {
                    !all_project_ids.contains(wt.parent_project_id.as_str())
                });
                project_info.is_closing = workspace.closing_projects.contains(&project.id);
                project_info.is_creating = workspace.creating_projects.contains(&project.id);
                project_info.worktree_count = wt_children.len();

                if !wt_children.is_empty() {
                    for child in &mut wt_children {
                        if let Some(services) = project_services.remove(&child.id) {
                            child.services = services;
                        }
                    }
                }
                if let Some(services) = project_services.remove(&project.id) {
                    project_info.services = services;
                }
                items.push(SidebarItem::Project {
                    project: project_info,
                    index: top_index,
                    worktree_children: wt_children,
                });
            }
        }

        // Index for trailing drop zone — must be project_order.len() to place after everything
        let end_index = workspace.data().project_order.len();

        let color_picker_project_id = self.color_picker_project_id.clone();
        let color_picker_folder_id = self.color_picker_folder_id.clone();
        let has_color_picker = color_picker_project_id.is_some() || color_picker_folder_id.is_some();
        let worktree_list_project_id = self.worktree_list_project_id.clone();
        let has_worktree_list = worktree_list_project_id.is_some();

        // Build cursor items and validate cursor position
        let cursor_items = self.build_cursor_items(cx);
        self.validate_cursor(cursor_items.len());
        let cursor_index = self.cursor_index;

        // Determine which project is focused — only highlight when explicitly focused via sidebar click
        let (focused_project_id, focus_individual) = {
            let ws = self.workspace.read(cx);
            (ws.focus_manager.focused_project_id().cloned(), ws.focus_manager.is_focus_individual())
        };

        // Build flat elements with cursor tracking
        let mut flat_elements: Vec<AnyElement> = Vec::new();
        let mut flat_idx: usize = 0;

        // Leading drop zone so items can be dropped before the first entry
        flat_elements.push(
            div()
                .id("sidebar-drop-head")
                .h(px(4.0))
                .w_full()
                .drag_over::<ProjectDrag>(move |style, _, _, _| {
                    style.h(px(8.0)).border_b_2().border_color(rgb(t.border_active))
                })
                .on_drop(cx.listener(move |this, drag: &ProjectDrag, _window, cx| {
                    this.workspace.update(cx, |ws, cx| {
                        ws.move_project(&drag.project_id, 0, cx);
                    });
                }))
                .drag_over::<FolderDrag>(move |style, _, _, _| {
                    style.h(px(8.0)).border_b_2().border_color(rgb(t.border_active))
                })
                .on_drop(cx.listener(move |this, drag: &FolderDrag, _window, cx| {
                    this.workspace.update(cx, |ws, cx| {
                        ws.move_item_in_order(&drag.folder_id, 0, cx);
                    });
                }))
                .into_any_element()
        );

        for item in items {
            match item {
                SidebarItem::Project { project, index, worktree_children } => {
                    let has_worktrees = !worktree_children.is_empty();

                    if has_worktrees && !project.is_orphan {
                        // Group header mode: project becomes a group, main project is first child
                        let is_cursor = cursor_index == Some(flat_idx);
                        // Group header highlights when focused non-individual (showing all)
                        let is_focused_group = focused_project_id.as_ref() == Some(&project.id) && !focus_individual;
                        let all_hidden = !project.show_in_overview && worktree_children.iter().all(|c| !c.show_in_overview);
                        flat_elements.push(
                            self.render_project_group_header(&project, 4.0, "gh", "group-header-item", project_list::GroupHeaderDragConfig::TopLevel { index }, all_hidden, is_cursor, is_focused_group, window, cx).into_any_element()
                        );
                        flat_idx += 1;

                        let is_expanded = self.is_project_expanded(&project.id, true);
                        if is_expanded {
                            // Main project as first child — highlights when focused individual
                            let is_cursor = cursor_index == Some(flat_idx);
                            let is_focused_project = focused_project_id.as_ref() == Some(&project.id) && focus_individual;
                            flat_elements.push(
                                self.render_project_group_child(&project, 20.0, "gc", "group-child-item", is_cursor, is_focused_project, window, cx).into_any_element()
                            );
                            flat_idx += 1;

                            if self.expanded_projects.contains(&project.id) {
                                self.render_expanded_children(&project, 34.0, 48.0, "gm-", cursor_index, &mut flat_idx, &mut flat_elements, cx);
                            }

                            // Worktree children as siblings
                            for (wt_idx, child) in worktree_children.iter().enumerate() {
                                let is_cursor = cursor_index == Some(flat_idx);
                                let is_focused_project = focused_project_id.as_ref() == Some(&child.id);
                                flat_elements.push(
                                    self.render_worktree_item(child, 20.0, wt_idx, is_cursor, is_focused_project, window, cx).into_any_element()
                                );
                                flat_idx += 1;

                                if self.expanded_projects.contains(&child.id) {
                                    self.render_expanded_children(child, 34.0, 48.0, "wt-", cursor_index, &mut flat_idx, &mut flat_elements, cx);
                                }
                            }
                        }
                    } else {
                        // No worktrees or orphan — standard rendering
                        let is_cursor = cursor_index == Some(flat_idx);
                        let is_focused_project = focused_project_id.as_ref() == Some(&project.id);
                        if project.is_orphan {
                            flat_elements.push(
                                self.render_worktree_item(&project, 8.0, 0, is_cursor, is_focused_project, window, cx).into_any_element()
                            );
                        } else {
                            flat_elements.push(
                                self.render_project_item(&project, index, is_cursor, is_focused_project, window, cx).into_any_element()
                            );
                        }
                        flat_idx += 1;

                        let show_children = self.expanded_projects.contains(&project.id);
                        if show_children {
                            self.render_expanded_children(&project, 20.0, 34.0, "", cursor_index, &mut flat_idx, &mut flat_elements, cx);
                        }
                    }
                }
                SidebarItem::Folder { folder, index, projects, worktree_children } => {
                    let is_cursor = cursor_index == Some(flat_idx);
                    let idle_terminal_count = if folder.collapsed {
                        let terminals = self.terminals.lock();
                        projects.iter()
                            .flat_map(|p| p.terminal_ids.iter())
                            .filter(|id| terminals.get(id.as_str()).map_or(false, |t| t.is_waiting_for_input()))
                            .count()
                    } else {
                        0
                    };
                    let all_hidden = projects.iter().all(|p| !p.show_in_overview) && worktree_children.values().flat_map(|c| c.iter()).all(|c| !c.show_in_overview);
                    flat_elements.push(
                        self.render_folder_header(&folder, index, projects.len(), idle_terminal_count, all_hidden, is_cursor, window, cx).into_any_element()
                    );
                    flat_idx += 1;

                    // Folder children when not collapsed
                    if !folder.collapsed {
                        for fp in &projects {
                            let fp_wt_children = worktree_children.get(&fp.id);
                            let has_worktrees = fp_wt_children.map_or(false, |c| !c.is_empty());

                            if has_worktrees && !fp.is_orphan {
                                // Group header mode within folder
                                let is_cursor = cursor_index == Some(flat_idx);
                                let is_focused_group = focused_project_id.as_ref() == Some(&fp.id) && !focus_individual;
                                flat_elements.push(
                                    {
                                    let all_hidden = !fp.show_in_overview && fp_wt_children.map_or(true, |c| c.iter().all(|c| !c.show_in_overview));
                                    self.render_project_group_header(fp, 20.0, "fgh", "fgh-item", project_list::GroupHeaderDragConfig::InFolder { folder_id: folder.id.clone() }, all_hidden, is_cursor, is_focused_group, window, cx).into_any_element()
                                    }
                                );
                                flat_idx += 1;

                                let is_expanded = self.is_project_expanded(&fp.id, true);
                                if is_expanded {
                                    // Main project as first child
                                    let is_cursor = cursor_index == Some(flat_idx);
                                    let is_focused_project = focused_project_id.as_ref() == Some(&fp.id) && focus_individual;
                                    flat_elements.push(
                                        self.render_project_group_child(fp, 36.0, "fgc", "fgc-item", is_cursor, is_focused_project, window, cx).into_any_element()
                                    );
                                    flat_idx += 1;

                                    if self.expanded_projects.contains(&fp.id) {
                                        self.render_expanded_children(fp, 50.0, 64.0, "gm-", cursor_index, &mut flat_idx, &mut flat_elements, cx);
                                    }

                                    // Worktree children as siblings
                                    if let Some(wt_children) = fp_wt_children {
                                        for (wt_idx, child) in wt_children.iter().enumerate() {
                                            let is_cursor = cursor_index == Some(flat_idx);
                                            let is_focused_project = focused_project_id.as_ref() == Some(&child.id);
                                            flat_elements.push(
                                                self.render_worktree_item(child, 36.0, wt_idx, is_cursor, is_focused_project, window, cx).into_any_element()
                                            );
                                            flat_idx += 1;

                                            if self.expanded_projects.contains(&child.id) {
                                                self.render_expanded_children(child, 50.0, 64.0, "wt-", cursor_index, &mut flat_idx, &mut flat_elements, cx);
                                            }
                                        }
                                    }
                                }
                            } else {
                                // No worktrees or orphan — standard folder project rendering
                                let is_cursor = cursor_index == Some(flat_idx);
                                let is_focused_project = focused_project_id.as_ref() == Some(&fp.id);
                                if fp.is_orphan {
                                    flat_elements.push(
                                        self.render_worktree_item(fp, 20.0, 0, is_cursor, is_focused_project, window, cx).into_any_element()
                                    );
                                } else {
                                    flat_elements.push(
                                        self.render_folder_project_item(fp, &folder.id, is_cursor, is_focused_project, window, cx).into_any_element()
                                    );
                                }
                                flat_idx += 1;

                                let show_children = self.expanded_projects.contains(&fp.id);
                                if show_children {
                                    self.render_expanded_children(fp, 36.0, 50.0, "", cursor_index, &mut flat_idx, &mut flat_elements, cx);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Trailing drop zone so items can be dropped after the last entry
        flat_elements.push(
            div()
                .id("sidebar-drop-tail")
                .h(px(24.0))
                .flex_1()
                .min_h(px(24.0))
                .drag_over::<ProjectDrag>(move |style, _, _, _| {
                    style.border_t_2().border_color(rgb(t.border_active))
                })
                .on_drop(cx.listener(move |this, drag: &ProjectDrag, _window, cx| {
                    this.workspace.update(cx, |ws, cx| {
                        ws.move_project(&drag.project_id, end_index, cx);
                    });
                }))
                .drag_over::<FolderDrag>(move |style, _, _, _| {
                    style.border_t_2().border_color(rgb(t.border_active))
                })
                .on_drop(cx.listener(move |this, drag: &FolderDrag, _window, cx| {
                    this.workspace.update(cx, |ws, cx| {
                        ws.move_item_in_order(&drag.folder_id, end_index, cx);
                    });
                }))
                .into_any_element()
        );

        div()
            .relative()
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(t.bg_secondary))
            .track_focus(&self.focus_handle)
            .key_context("Sidebar")
            .on_action(cx.listener(Self::handle_sidebar_up))
            .on_action(cx.listener(Self::handle_sidebar_down))
            .on_action(cx.listener(Self::handle_sidebar_confirm))
            .on_action(cx.listener(Self::handle_sidebar_toggle_expand))
            .on_action(cx.listener(Self::handle_sidebar_escape))
            .child(self.render_header(cx))
            .child(self.render_projects_header(cx))
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .children(flat_elements)
                    .child(self.render_remote_section(cx)),
            )
            // Color picker popover
            .when(has_color_picker, |d: Div| {
                let anchor = self.popover_anchor;
                d.child(
                    okena_ui::popover::popover_backdrop("color-picker-backdrop")
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.hide_color_picker(cx);
                        }))
                )
                .when(color_picker_project_id.is_some(), |d: Div| {
                    let project_id = color_picker_project_id.unwrap();
                    d.child(okena_ui::popover::popover_anchored(anchor, self.render_color_picker(&project_id, cx)))
                })
                .when(color_picker_folder_id.is_some(), |d: Div| {
                    let folder_id = color_picker_folder_id.unwrap();
                    d.child(okena_ui::popover::popover_anchored(anchor, self.render_folder_color_picker(&folder_id, cx)))
                })
            })
            // Worktree list popover
            .when(has_worktree_list, |d: Div| {
                let anchor = self.popover_anchor;
                d.child(
                    okena_ui::popover::popover_backdrop("worktree-list-backdrop")
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.hide_worktree_list(cx);
                        }))
                )
                .map(|d| {
                    let project_id = worktree_list_project_id.unwrap();
                    d.child(okena_ui::popover::popover_anchored(anchor, self.render_worktree_list(&project_id, cx)))
                })
            })
    }
}
