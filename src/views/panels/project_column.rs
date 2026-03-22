use crate::git::{self, FileDiffSummary, PrStateColor, CiStatusColor};
use crate::git::watcher::GitStatusWatcher;
use crate::action_dispatch::ActionDispatcher;
use okena_views_git::project_header;
use crate::services::manager::{ServiceManager, ServiceStatus};
use crate::terminal::backend::TerminalBackend;
use crate::theme::{theme, ThemeColors};
use crate::views::layout::layout_container::LayoutContainer;
use crate::views::layout::terminal_pane::TerminalPane;
use crate::views::root::TerminalsRegistry;
use crate::elements::resize_handle::ResizeHandle;
use crate::views::layout::split_pane::{ActiveDrag, DragState};
use crate::workspace::request_broker::RequestBroker;
use crate::workspace::requests::OverlayRequest;
use crate::workspace::state::{ProjectData, Workspace};
use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use gpui_component::{h_flex, v_flex};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use okena_core::api::ActionRequest;
use okena_views_services::types::ServiceSnapshot;

/// Delay before showing diff summary popover (ms)
const HOVER_DELAY_MS: u64 = 400;

// ServiceSnapshot is now imported from okena_views_services::types

/// A single project column with header and layout
pub struct ProjectColumn {
    workspace: Entity<Workspace>,
    request_broker: Entity<RequestBroker>,
    project_id: String,
    #[allow(dead_code)]
    backend: Arc<dyn TerminalBackend>,
    #[allow(dead_code)]
    terminals: TerminalsRegistry,
    /// Stored layout container entity (must be created in new(), not render())
    layout_container: Option<Entity<LayoutContainer<ActionDispatcher>>>,
    /// Whether the diff summary popover is visible
    diff_popover_visible: bool,
    /// Cached file summaries for popover
    diff_file_summaries: Vec<FileDiffSummary>,
    /// Project path for the current popover
    diff_popover_project_path: String,
    /// Hover token to cancel pending popover show
    hover_token: Arc<AtomicU64>,
    /// Git status watcher (centralized polling)
    git_watcher: Option<Entity<GitStatusWatcher>>,
    /// Shared drag state for resize operations
    active_drag: ActiveDrag,
    /// Action dispatcher for routing terminal actions (local or remote)
    action_dispatcher: Option<ActionDispatcher>,
    /// Service manager reference (set after creation)
    service_manager: Option<Entity<ServiceManager>>,
    /// Whether the per-project service log panel is open
    service_panel_open: bool,
    /// Currently active service name in the service panel
    active_service_name: Option<String>,
    /// Terminal pane showing the active service's log output
    service_terminal_pane: Option<Entity<TerminalPane<ActionDispatcher>>>,
    /// Height of the service panel in pixels
    service_panel_height: f32,
    /// Bounds of the git diff stats badge (for popover positioning)
    diff_stats_bounds: Bounds<Pixels>,
    /// Whether the commit log popover is visible
    commit_log_visible: bool,
    /// Cached commit graph rows for the popover
    commit_log_entries: Vec<git::GraphRow>,
    /// Whether commit log is currently loading
    commit_log_loading: bool,
    /// Bounds of the commit log trigger button (for popover positioning)
    commit_log_bounds: Bounds<Pixels>,
    /// How many commits have been loaded so far (for pagination)
    commit_log_count: usize,
    /// Project path for loading more commits
    commit_log_project_path: String,
    /// Whether there are potentially more commits to load
    commit_log_has_more: bool,
    /// Scroll handle for the commit log scroll area
    commit_log_scroll: ScrollHandle,
    /// Currently viewed branch in commit log (None = HEAD)
    commit_log_branch: Option<String>,
    /// Available branches for the branch picker
    commit_log_branches: Vec<String>,
    /// Whether the branch picker is open
    commit_log_branch_picker: bool,
    /// Text filter for branch picker
    commit_log_branch_filter: String,
    /// Whether compare mode UI is shown
    commit_log_compare_mode: bool,
    /// Selected base branch for comparison
    commit_log_compare_base: Option<String>,
    /// Selected head branch for comparison
    commit_log_compare_head: Option<String>,
    /// Which slot the branch picker is targeting
    commit_log_picker_target: BranchPickerTarget,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum BranchPickerTarget {
    /// Picking branch to view graph for
    #[default]
    Graph,
    /// Picking base branch for compare
    CompareBase,
    /// Picking head branch for compare
    CompareHead,
}

impl ProjectColumn {
    pub fn new(
        workspace: Entity<Workspace>,
        request_broker: Entity<RequestBroker>,
        project_id: String,
        backend: Arc<dyn TerminalBackend>,
        terminals: TerminalsRegistry,
        active_drag: ActiveDrag,
        git_watcher: Option<Entity<GitStatusWatcher>>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Observe git watcher for re-renders (replaces per-column polling)
        if let Some(ref watcher) = git_watcher {
            cx.observe(watcher, |_, _, cx| cx.notify()).detach();
        }

        let initial_service_height = workspace.read(cx).data.service_panel_heights
            .get(&project_id).copied().unwrap_or(200.0);

        Self {
            workspace,
            request_broker,
            project_id,
            backend,
            terminals,
            layout_container: None, // Will be initialized on first render with cx
            diff_popover_visible: false,
            diff_file_summaries: Vec::new(),
            diff_popover_project_path: String::new(),
            hover_token: Arc::new(AtomicU64::new(0)),
            git_watcher,
            active_drag,
            action_dispatcher: None,
            service_manager: None,
            service_panel_open: false,
            active_service_name: None,
            service_terminal_pane: None,
            service_panel_height: initial_service_height,
            diff_stats_bounds: Bounds::default(),
            commit_log_visible: false,
            commit_log_entries: Vec::new(),
            commit_log_loading: false,
            commit_log_bounds: Bounds::default(),
            commit_log_count: 0,
            commit_log_project_path: String::new(),
            commit_log_has_more: false,
            commit_log_scroll: ScrollHandle::new(),
            commit_log_branch: None,
            commit_log_branches: Vec::new(),
            commit_log_branch_picker: false,
            commit_log_branch_filter: String::new(),
            commit_log_compare_mode: false,
            commit_log_compare_base: None,
            commit_log_compare_head: None,
            commit_log_picker_target: BranchPickerTarget::default(),
        }
    }

    /// Set the action dispatcher (used for remote projects).
    pub fn set_action_dispatcher(&mut self, dispatcher: Option<ActionDispatcher>) {
        self.action_dispatcher = dispatcher;
    }

    /// Set the service manager and observe it for changes.
    pub fn set_service_manager(&mut self, manager: Entity<ServiceManager>, cx: &mut Context<Self>) {
        // Also update the action dispatcher so it can route service actions locally
        if let Some(ActionDispatcher::Local { ref mut service_manager, .. }) = self.action_dispatcher {
            *service_manager = Some(manager.clone());
        }
        let project_id = self.project_id.clone();
        cx.observe(&manager, move |this, sm, cx| {
            let Some(ref active_name) = this.active_service_name else { return };
            let current_tid = sm.read(cx)
                .terminal_id_for(&project_id, active_name)
                .cloned();

            match current_tid {
                Some(new_tid) => {
                    // Check if terminal changed (service restarted)
                    let pane_tid = this.service_terminal_pane.as_ref()
                        .and_then(|p| p.read(cx).terminal_id());
                    if pane_tid.as_deref() != Some(&new_tid) {
                        let name = active_name.clone();
                        this.show_service(&name, cx);
                    }
                }
                None => {
                    // For Docker services that are still running/restarting,
                    // re-open the log viewer instead of showing "not running".
                    let is_active_docker = sm.read(cx)
                        .instances()
                        .get(&(project_id.clone(), active_name.clone()))
                        .is_some_and(|i| {
                            matches!(i.kind, crate::services::manager::ServiceKind::DockerCompose { .. })
                                && matches!(i.status, ServiceStatus::Running | ServiceStatus::Restarting)
                        });

                    if is_active_docker {
                        let name = active_name.clone();
                        this.show_service(&name, cx);
                    } else {
                        // Service stopped — clear the terminal pane but keep panel open
                        this.service_terminal_pane = None;
                        cx.notify();
                    }
                }
            }
        }).detach();

        self.service_manager = Some(manager);
    }

    /// Show a service's log output in the per-project panel.
    pub fn show_service(&mut self, service_name: &str, cx: &mut Context<Self>) {
        // For Docker services with no terminal_id, spawn a log viewer PTY on demand
        if let Some(ref sm) = self.service_manager {
            let is_docker = sm.read(cx).instances()
                .get(&(self.project_id.clone(), service_name.to_string()))
                .is_some_and(|i| matches!(i.kind, crate::services::manager::ServiceKind::DockerCompose { .. }));
            let has_terminal = sm.read(cx).terminal_id_for(&self.project_id, service_name).is_some();
            if is_docker && !has_terminal {
                let pid = self.project_id.clone();
                let name = service_name.to_string();
                sm.update(cx, |sm, cx| {
                    sm.open_docker_logs(&pid, &name, cx);
                });
            }
        }

        // Look up terminal_id from either ServiceManager or remote services
        let terminal_id = if let Some(ref sm) = self.service_manager {
            sm.read(cx).terminal_id_for(&self.project_id, service_name).cloned()
        } else {
            // Fall back to remote services
            self.workspace.read(cx).project(&self.project_id)
                .and_then(|p| {
                    p.remote_services.iter()
                        .find(|s| s.name == service_name)
                        .and_then(|s| s.terminal_id.clone())
                })
        };

        self.active_service_name = Some(service_name.to_string());
        self.service_panel_open = true;

        if let Some(tid) = terminal_id {
            let project_path = self.service_manager.as_ref()
                .and_then(|sm| sm.read(cx).project_path(&self.project_id).cloned())
                .or_else(|| {
                    self.workspace.read(cx).project(&self.project_id)
                        .map(|p| p.path.clone())
                })
                .unwrap_or_default();

            let ws = self.workspace.clone();
            let rb = self.request_broker.clone();
            let backend = self.backend.clone();
            let terminals = self.terminals.clone();
            let pid = self.project_id.clone();

            let pane = cx.new(move |cx| {
                TerminalPane::new(
                    ws,
                    rb,
                    pid,
                    project_path,
                    vec![usize::MAX],
                    Some(tid),
                    false,
                    false,
                    backend,
                    terminals,
                    None,
                    cx,
                )
            });

            self.service_terminal_pane = Some(pane);
        } else {
            self.service_terminal_pane = None;
        }

        cx.notify();
    }

    /// Set the service panel height (called during drag resize).
    pub fn set_service_panel_height(&mut self, height: f32, cx: &mut Context<Self>) {
        self.service_panel_height = height.clamp(80.0, 600.0);
        let project_id = self.project_id.clone();
        let h = self.service_panel_height;
        self.workspace.update(cx, |ws, cx| {
            ws.update_service_panel_height(&project_id, h, cx);
        });
        cx.notify();
    }

    /// Show the service overview tab (no specific service selected).
    fn show_overview(&mut self, cx: &mut Context<Self>) {
        self.active_service_name = None;
        self.service_terminal_pane = None;
        self.service_panel_open = true;
        cx.notify();
    }

    /// Close the per-project service log panel.
    pub fn close_service_panel(&mut self, cx: &mut Context<Self>) {
        self.service_panel_open = false;
        self.service_terminal_pane = None;
        self.active_service_name = None;
        cx.notify();
    }

    /// Get the list of services for this project, from either ServiceManager (local)
    /// or remote_services on ProjectData (remote).
    fn get_service_list(&self, cx: &Context<Self>) -> Vec<ServiceSnapshot> {
        // Try ServiceManager first (local projects)
        if let Some(ref sm) = self.service_manager {
            let services = sm.read(cx).services_for_project(&self.project_id);
            if !services.is_empty() {
                return services.iter().map(|inst| ServiceSnapshot {
                    name: inst.definition.name.clone(),
                    status: inst.status.clone(),
                    terminal_id: inst.terminal_id.clone(),
                    ports: inst.detected_ports.clone(),
                    is_docker: matches!(inst.kind, crate::services::manager::ServiceKind::DockerCompose { .. }),
                    is_extra: inst.is_extra,
                }).collect();
            }
        }
        // Fall back to remote services from ProjectData
        let ws = self.workspace.read(cx);
        ws.project(&self.project_id)
            .map(|p| p.remote_services.iter().map(|api_svc| ServiceSnapshot {
                name: api_svc.name.clone(),
                status: ServiceStatus::from_api(&api_svc.status, api_svc.exit_code),
                terminal_id: api_svc.terminal_id.clone(),
                ports: api_svc.ports.clone(),
                is_docker: api_svc.kind == "docker_compose",
                is_extra: api_svc.is_extra,
            }).collect())
            .unwrap_or_default()
    }

    /// Observe workspace for remote service state changes (used for remote project columns).
    pub fn observe_remote_services(&mut self, workspace: Entity<Workspace>, cx: &mut Context<Self>) {
        let project_id = self.project_id.clone();
        cx.observe(&workspace, move |this, ws, cx| {
            let Some(ref active_name) = this.active_service_name else { return };

            // Look up current terminal_id from remote_services
            let current_tid = ws.read(cx).project(&project_id)
                .and_then(|p| {
                    p.remote_services.iter()
                        .find(|s| s.name == *active_name)
                        .and_then(|s| s.terminal_id.clone())
                });

            match current_tid {
                Some(new_tid) => {
                    let pane_tid = this.service_terminal_pane.as_ref()
                        .and_then(|p| p.read(cx).terminal_id());
                    if pane_tid.as_deref() != Some(&new_tid) {
                        let name = active_name.clone();
                        this.show_service(&name, cx);
                    }
                }
                None => {
                    this.service_terminal_pane = None;
                    cx.notify();
                }
            }
        }).detach();
    }

    fn show_diff_popover(&mut self, project_path: String, cx: &mut Context<Self>) {
        // Skip if already visible
        if self.diff_popover_visible {
            return;
        }

        // Increment token to invalidate any pending show
        let token = self.hover_token.fetch_add(1, Ordering::SeqCst) + 1;
        let hover_token = self.hover_token.clone();

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            smol::Timer::after(Duration::from_millis(HOVER_DELAY_MS)).await;

            // Check if token is still valid (mouse hasn't left)
            if hover_token.load(Ordering::SeqCst) != token {
                return;
            }

            // Load file summaries
            let summaries = git::get_diff_file_summary(Path::new(&project_path));

            let _ = this.update(cx, |this, cx| {
                // Re-check token after loading
                if hover_token.load(Ordering::SeqCst) == token && !summaries.is_empty() {
                    this.diff_file_summaries = summaries;
                    this.diff_popover_project_path = project_path;
                    this.diff_popover_visible = true;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn hide_diff_popover(&mut self, cx: &mut Context<Self>) {
        // Always increment token to cancel any pending show task
        let token = self.hover_token.fetch_add(1, Ordering::SeqCst) + 1;

        if !self.diff_popover_visible {
            return;
        }

        let hover_token = self.hover_token.clone();

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            // Small delay to allow mouse to reach popover
            smol::Timer::after(Duration::from_millis(100)).await;

            // Check if hide was cancelled (mouse entered popover)
            if hover_token.load(Ordering::SeqCst) != token {
                return;
            }

            let _ = this.update(cx, |this, cx| {
                if hover_token.load(Ordering::SeqCst) == token && this.diff_popover_visible {
                    this.diff_popover_visible = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    const COMMIT_PAGE_SIZE: usize = 50;

    fn toggle_commit_log(&mut self, project_path: String, cx: &mut Context<Self>) {
        if self.commit_log_visible {
            self.commit_log_visible = false;
            cx.notify();
            return;
        }
        // Hide diff popover when opening commit log
        self.diff_popover_visible = false;

        self.commit_log_visible = true;
        self.commit_log_loading = true;
        self.commit_log_entries.clear();
        self.commit_log_count = 0;
        self.commit_log_project_path = project_path.clone();
        self.commit_log_has_more = false;
        self.commit_log_branch = None;
        self.commit_log_branch_picker = false;
        self.commit_log_branch_filter.clear();
        self.commit_log_compare_mode = false;
        self.commit_log_compare_base = None;
        self.commit_log_compare_head = None;
        self.commit_log_picker_target = BranchPickerTarget::Graph;
        cx.notify();

        let page = Self::COMMIT_PAGE_SIZE;
        let path_for_branches = project_path.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let (entries, branches) = smol::unblock(move || {
                let entries = git::get_commit_graph(std::path::Path::new(&project_path), page, None);
                let branches = git::list_branches(std::path::Path::new(&path_for_branches));
                (entries, branches)
            })
            .await;

            let _ = this.update(cx, |this, cx| {
                this.commit_log_loading = false;
                let commit_count = entries.iter().filter(|r| matches!(r, git::GraphRow::Commit(_))).count();
                this.commit_log_has_more = commit_count >= page;
                this.commit_log_count = commit_count;
                this.commit_log_entries = entries;
                this.commit_log_branches = branches;
                cx.notify();
            });
        })
        .detach();
    }

    fn switch_commit_log_branch(&mut self, branch: Option<String>, cx: &mut Context<Self>) {
        self.commit_log_branch = branch.clone();
        self.commit_log_branch_picker = false;
        self.commit_log_branch_filter.clear();
        self.commit_log_loading = true;
        self.commit_log_entries.clear();
        self.commit_log_count = 0;
        self.commit_log_has_more = false;
        cx.notify();

        let project_path = self.commit_log_project_path.clone();
        let page = Self::COMMIT_PAGE_SIZE;

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let entries = smol::unblock(move || {
                git::get_commit_graph(
                    std::path::Path::new(&project_path),
                    page,
                    branch.as_deref(),
                )
            })
            .await;

            let _ = this.update(cx, |this, cx| {
                this.commit_log_loading = false;
                let commit_count = entries.iter().filter(|r| matches!(r, git::GraphRow::Commit(_))).count();
                this.commit_log_has_more = commit_count >= page;
                this.commit_log_count = commit_count;
                this.commit_log_entries = entries;
                cx.notify();
            });
        })
        .detach();
    }

    fn load_more_commits(&mut self, cx: &mut Context<Self>) {
        if self.commit_log_loading || !self.commit_log_has_more {
            return;
        }

        self.commit_log_loading = true;
        cx.notify();

        let project_path = self.commit_log_project_path.clone();
        let branch = self.commit_log_branch.clone();
        let already_loaded = self.commit_log_count;
        let page = Self::COMMIT_PAGE_SIZE;
        let new_total = already_loaded + page;

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            // Reload full graph with larger limit — git log --graph requires
            // the full history to compute lane positions correctly
            let entries = smol::unblock(move || {
                git::get_commit_graph(std::path::Path::new(&project_path), new_total, branch.as_deref())
            })
            .await;

            let _ = this.update(cx, |this, cx| {
                this.commit_log_loading = false;
                let commit_count = entries.iter().filter(|r| matches!(r, git::GraphRow::Commit(_))).count();
                this.commit_log_has_more = commit_count >= new_total;
                this.commit_log_count = commit_count;
                this.commit_log_entries = entries;
                cx.notify();
            });
        })
        .detach();
    }

    fn hide_commit_log(&mut self, cx: &mut Context<Self>) {
        if self.commit_log_visible {
            self.commit_log_visible = false;
            cx.notify();
        }
    }

    fn render_commit_log_popover(&self, t: &ThemeColors, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.commit_log_visible {
            return div().size_0().into_any_element();
        }

        let bounds = self.commit_log_bounds;
        let position = point(
            bounds.origin.x - px(8.0),
            bounds.origin.y + bounds.size.height + px(6.0),
        );

        // Resolve branch name for the header
        let branch_name = self.git_watcher.as_ref()
            .and_then(|w| w.read(cx).get(&self.project_id).cloned())
            .and_then(|s| s.branch);

        let content = {
            let entity_handle = cx.entity().clone();
            let project_id = self.project_id.clone();
            let request_broker = self.request_broker.clone();
            let on_commit_click: Option<Arc<dyn Fn(&str, &str, usize, &mut Window, &mut App)>> =
                if self.commit_log_entries.is_empty() {
                    None
                } else {
                    // Extract commit list for navigation in the diff viewer
                    let all_commits: Vec<git::CommitLogEntry> = self.commit_log_entries.iter()
                        .filter_map(|r| match r { git::GraphRow::Commit(e) => Some(e.clone()), _ => None })
                        .collect();
                    Some(Arc::new(move |hash: &str, msg: &str, _commit_idx: usize, _window: &mut Window, cx: &mut App| {
                        let commit_hash = hash.to_string();
                        let commit_msg = msg.to_string();
                        let commits_vec = all_commits.clone();
                        let commit_idx = commits_vec.iter().position(|c| c.hash == commit_hash).unwrap_or(0);
                        let _ = entity_handle.update(cx, |this: &mut ProjectColumn, cx| {
                            this.hide_commit_log(cx);
                        });
                        request_broker.update(cx, |broker, cx| {
                            broker.push_overlay_request(OverlayRequest::DiffViewer {
                                project_id: project_id.clone(),
                                file: None,
                                mode: Some(okena_core::types::DiffMode::Commit(commit_hash)),
                                commit_message: Some(commit_msg),
                                commits: Some(commits_vec),
                                commit_index: Some(commit_idx),
                            }, cx);
                        });
                    }))
                };
            project_header::render_commit_log_content(
                &self.commit_log_entries,
                self.commit_log_loading,
                on_commit_click,
                t,
            )
        };

        div()
            .size_full()
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("commit-log-backdrop")
                    .absolute()
                    .inset_0()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.hide_commit_log(cx);
                    }))
                    .on_scroll_wheel(|_, _, cx| {
                        cx.stop_propagation();
                    }),
            )
            .child(
                deferred(
                    anchored()
                        .position(position)
                        .snap_to_window()
                        .child(
                            v_flex()
                                .id("commit-log-popover")
                                .occlude()
                                .w(px(520.0))
                                .max_h(px(420.0))
                                .bg(rgb(t.bg_primary))
                                .border_1()
                                .border_color(rgb(t.border))
                                .rounded(px(8.0))
                                .shadow_lg()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_scroll_wheel(|_, _, cx| {
                                    cx.stop_propagation();
                                })
                                // Header
                                .child(
                                    h_flex()
                                        .px(px(10.0))
                                        .py(px(6.0))
                                        .gap(px(6.0))
                                        .items_center()
                                        .border_b_1()
                                        .border_color(rgb(t.border))
                                        .child(
                                            svg()
                                                .path("icons/git-commit.svg")
                                                .size(px(11.0))
                                                .text_color(rgb(t.text_muted)),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .text_color(rgb(t.text_secondary))
                                                .child("GRAPH"),
                                        )
                                        // Right side: Compare toggle + branch selector
                                        .child({
                                            let display_branch = self.commit_log_branch.clone()
                                                .or(branch_name);
                                            let is_compare = self.commit_log_compare_mode;
                                            h_flex()
                                                .flex_1()
                                                .justify_end()
                                                .gap(px(4.0))
                                                .items_center()
                                                // Compare toggle
                                                .child(
                                                    div()
                                                        .id("commit-log-compare-toggle")
                                                        .cursor_pointer()
                                                        .px(px(6.0))
                                                        .py(px(2.0))
                                                        .rounded(px(4.0))
                                                        .bg(rgb(if is_compare { t.bg_selection } else { t.bg_hover }))
                                                        .hover(|s| s.bg(rgb(t.bg_selection)))
                                                        .text_size(px(10.0))
                                                        .text_color(rgb(if is_compare { t.term_cyan } else { t.text_muted }))
                                                        .on_mouse_down(MouseButton::Left, |_, _, cx| { cx.stop_propagation(); })
                                                        .on_click(cx.listener(|this, _, _window, cx| {
                                                            this.commit_log_compare_mode = !this.commit_log_compare_mode;
                                                            if this.commit_log_compare_mode {
                                                                // Pre-fill base with current branch
                                                                let current = this.git_watcher.as_ref()
                                                                    .and_then(|w| w.read(cx).get(&this.project_id).cloned())
                                                                    .and_then(|s| s.branch);
                                                                this.commit_log_compare_base = current;
                                                                this.commit_log_compare_head = this.commit_log_branch.clone();
                                                            }
                                                            this.commit_log_branch_picker = false;
                                                            cx.notify();
                                                        }))
                                                        .child("Compare"),
                                                )
                                                // Branch selector pill (only when not in compare mode)
                                                .when(!is_compare, |d| {
                                                    d.when_some(display_branch, |d, name| {
                                                        d.child(
                                                            h_flex()
                                                                .id("commit-log-branch-btn")
                                                                .gap(px(4.0))
                                                                .items_center()
                                                                .px(px(6.0))
                                                                .py(px(2.0))
                                                                .rounded(px(4.0))
                                                                .bg(rgb(t.bg_hover))
                                                                .cursor_pointer()
                                                                .hover(|s| s.bg(rgb(t.bg_selection)))
                                                                .on_mouse_down(MouseButton::Left, |_, _, cx| { cx.stop_propagation(); })
                                                                .on_click(cx.listener(|this, _, _window, cx| {
                                                                    this.commit_log_picker_target = BranchPickerTarget::Graph;
                                                                    this.commit_log_branch_picker = !this.commit_log_branch_picker;
                                                                    this.commit_log_branch_filter.clear();
                                                                    cx.notify();
                                                                }))
                                                                .child(svg().path("icons/git-branch.svg").size(px(10.0)).text_color(rgb(t.term_green)))
                                                                .child(
                                                                    div().text_size(px(10.0)).text_color(rgb(t.text_secondary))
                                                                        .max_w(px(140.0)).text_ellipsis().overflow_hidden().child(name),
                                                                ),
                                                        )
                                                    })
                                                })
                                        }),
                                )
                                // Compare bar — two branch selectors + view diff button
                                .when(self.commit_log_compare_mode, |d| {
                                    let base = self.commit_log_compare_base.clone();
                                    let head = self.commit_log_compare_head.clone();
                                    let pid = self.project_id.clone();
                                    let broker = self.request_broker.clone();
                                    let both_selected = base.is_some() && head.is_some();
                                    d.child(
                                        h_flex()
                                            .px(px(10.0))
                                            .py(px(6.0))
                                            .gap(px(6.0))
                                            .items_center()
                                            .border_b_1()
                                            .border_color(rgb(t.border))
                                            // Base branch pill
                                            .child(
                                                div()
                                                    .id("compare-base-btn")
                                                    .cursor_pointer()
                                                    .px(px(6.0))
                                                    .py(px(2.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(t.bg_hover))
                                                    .hover(|s| s.bg(rgb(t.bg_selection)))
                                                    .text_size(px(10.0))
                                                    .on_mouse_down(MouseButton::Left, |_, _, cx| { cx.stop_propagation(); })
                                                    .on_click(cx.listener(|this, _, _window, cx| {
                                                        this.commit_log_picker_target = BranchPickerTarget::CompareBase;
                                                        this.commit_log_branch_picker = !this.commit_log_branch_picker;
                                                        this.commit_log_branch_filter.clear();
                                                        cx.notify();
                                                    }))
                                                    .child(
                                                        h_flex().gap(px(3.0)).items_center()
                                                            .child(svg().path("icons/git-branch.svg").size(px(9.0)).text_color(rgb(t.term_green)))
                                                            .child(
                                                                div().text_color(rgb(t.text_secondary))
                                                                    .max_w(px(120.0)).text_ellipsis().overflow_hidden()
                                                                    .child(base.clone().unwrap_or_else(|| "base...".to_string())),
                                                            ),
                                                    ),
                                            )
                                            // Arrow
                                            .child(div().text_size(px(10.0)).text_color(rgb(t.text_muted)).child("\u{2192}"))
                                            // Head branch pill
                                            .child(
                                                div()
                                                    .id("compare-head-btn")
                                                    .cursor_pointer()
                                                    .px(px(6.0))
                                                    .py(px(2.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(t.bg_hover))
                                                    .hover(|s| s.bg(rgb(t.bg_selection)))
                                                    .text_size(px(10.0))
                                                    .on_mouse_down(MouseButton::Left, |_, _, cx| { cx.stop_propagation(); })
                                                    .on_click(cx.listener(|this, _, _window, cx| {
                                                        this.commit_log_picker_target = BranchPickerTarget::CompareHead;
                                                        this.commit_log_branch_picker = !this.commit_log_branch_picker;
                                                        this.commit_log_branch_filter.clear();
                                                        cx.notify();
                                                    }))
                                                    .child(
                                                        h_flex().gap(px(3.0)).items_center()
                                                            .child(svg().path("icons/git-branch.svg").size(px(9.0)).text_color(rgb(t.term_cyan)))
                                                            .child(
                                                                div().text_color(rgb(t.text_secondary))
                                                                    .max_w(px(120.0)).text_ellipsis().overflow_hidden()
                                                                    .child(head.clone().unwrap_or_else(|| "head...".to_string())),
                                                            ),
                                                    ),
                                            )
                                            // View Diff button
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .flex()
                                                    .justify_end()
                                                    .child(
                                                        div()
                                                            .id("compare-view-diff")
                                                            .cursor_pointer()
                                                            .px(px(8.0))
                                                            .py(px(3.0))
                                                            .rounded(px(4.0))
                                                            .when(both_selected, |d| {
                                                                d.bg(rgb(t.term_cyan))
                                                                    .text_color(rgb(t.bg_primary))
                                                                    .hover(|s| s.opacity(0.9))
                                                            })
                                                            .when(!both_selected, |d| {
                                                                d.bg(rgb(t.bg_hover))
                                                                    .text_color(rgb(t.text_muted))
                                                            })
                                                            .text_size(px(10.0))
                                                            .font_weight(FontWeight::MEDIUM)
                                                            .on_mouse_down(MouseButton::Left, |_, _, cx| { cx.stop_propagation(); })
                                                            .when(both_selected, |d| {
                                                                d.on_click(cx.listener(move |this, _, _window, cx| {
                                                                    let base = this.commit_log_compare_base.clone().unwrap();
                                                                    let head = this.commit_log_compare_head.clone().unwrap();
                                                                    this.hide_commit_log(cx);
                                                                    broker.update(cx, |broker, cx| {
                                                                        broker.push_overlay_request(OverlayRequest::DiffViewer {
                                                                            project_id: pid.clone(),
                                                                            file: None,
                                                                            mode: Some(okena_core::types::DiffMode::BranchCompare {
                                                                                base,
                                                                                head,
                                                                            }),
                                                                            commit_message: None,
                                                                            commits: None,
                                                                            commit_index: None,
                                                                        }, cx);
                                                                    });
                                                                }))
                                                            })
                                                            .child("View Diff"),
                                                    ),
                                            ),
                                    )
                                })
                                // Branch picker (inline, between header and commit list)
                                .when(self.commit_log_branch_picker, |d| {
                                    let filter = self.commit_log_branch_filter.to_lowercase();
                                    let filtered: Vec<&String> = self.commit_log_branches.iter()
                                        .filter(|b| filter.is_empty() || b.to_lowercase().contains(&filter))
                                        .collect();
                                    d.child(
                                        v_flex()
                                            .border_b_1()
                                            .border_color(rgb(t.border))
                                            .max_h(px(200.0))
                                            // Filter input
                                            .child(
                                                div()
                                                    .px(px(10.0))
                                                    .py(px(6.0))
                                                    .child(
                                                        div()
                                                            .px(px(8.0))
                                                            .py(px(4.0))
                                                            .rounded(px(4.0))
                                                            .bg(rgb(t.bg_secondary))
                                                            .text_size(px(11.0))
                                                            .text_color(rgb(t.text_primary))
                                                            .child(
                                                                if filter.is_empty() {
                                                                    format!("{} branches", self.commit_log_branches.len())
                                                                } else {
                                                                    format!("\"{}\" \u{2014} {} matches", self.commit_log_branch_filter, filtered.len())
                                                                }
                                                            ),
                                                    ),
                                            )
                                            // Branch list
                                            .child(
                                                div()
                                                    .id("branch-picker-scroll")
                                                    .flex_1()
                                                    .min_h_0()
                                                    .overflow_y_scroll()
                                                    .children(
                                                        filtered.iter().enumerate().map(|(i, branch)| {
                                                            let b = (*branch).clone();
                                                            let target = self.commit_log_picker_target;
                                                            let is_selected = match target {
                                                                BranchPickerTarget::Graph => self.commit_log_branch.as_ref() == Some(*branch),
                                                                BranchPickerTarget::CompareBase => self.commit_log_compare_base.as_ref() == Some(*branch),
                                                                BranchPickerTarget::CompareHead => self.commit_log_compare_head.as_ref() == Some(*branch),
                                                            };
                                                            div()
                                                                .id(ElementId::Name(format!("branch-{}-{}", i, branch).into()))
                                                                .px(px(10.0))
                                                                .py(px(3.0))
                                                                .cursor_pointer()
                                                                .text_size(px(11.0))
                                                                .text_color(rgb(if is_selected { t.text_primary } else { t.text_secondary }))
                                                                .when(is_selected, |d| d.font_weight(FontWeight::SEMIBOLD))
                                                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                                                .on_click(cx.listener(move |this, _, _window, cx| {
                                                                    match target {
                                                                        BranchPickerTarget::Graph => {
                                                                            this.switch_commit_log_branch(Some(b.clone()), cx);
                                                                        }
                                                                        BranchPickerTarget::CompareBase => {
                                                                            this.commit_log_compare_base = Some(b.clone());
                                                                            this.commit_log_branch_picker = false;
                                                                            cx.notify();
                                                                        }
                                                                        BranchPickerTarget::CompareHead => {
                                                                            this.commit_log_compare_head = Some(b.clone());
                                                                            this.commit_log_branch_picker = false;
                                                                            cx.notify();
                                                                        }
                                                                    }
                                                                }))
                                                                .child((*branch).clone())
                                                                .into_any_element()
                                                        }),
                                                    ),
                                            ),
                                    )
                                })
                                // Scrollable commit list
                                .child(
                                    div()
                                        .id("commit-log-scroll")
                                        .flex_1()
                                        .min_h_0()
                                        .overflow_y_scroll()
                                        .track_scroll(&self.commit_log_scroll)
                                        .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                                            // Scrolling down: check if near bottom to auto-load
                                            let delta_y = f32::from(event.delta.pixel_delta(px(1.0)).y);
                                            if delta_y >= 0.0 {
                                                return; // scrolling up
                                            }
                                            if !this.commit_log_has_more || this.commit_log_loading {
                                                return;
                                            }
                                            // Estimate total content height vs visible area
                                            let row_count = this.commit_log_entries.len();
                                            let est_content_h = row_count as f32 * 20.0; // rough avg row height
                                            let scroll_y = -f32::from(this.commit_log_scroll.offset().y);
                                            let viewport_h = 380.0; // approximate visible height
                                            if scroll_y + viewport_h > est_content_h - 200.0 {
                                                this.load_more_commits(cx);
                                            }
                                        }))
                                        .py(px(4.0))
                                        .child(content),
                                ),
                        ),
                ),
            )
            .into_any_element()
    }

    fn render_diff_popover(&self, t: &ThemeColors, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.diff_popover_visible || self.diff_file_summaries.is_empty() {
            return div().size_0().into_any_element();
        }

        let entity_handle = cx.entity().clone();
        let request_broker = self.request_broker.clone();
        let project_id = self.project_id.clone();
        let tree_elements = project_header::render_diff_file_list_interactive(
            &self.diff_file_summaries,
            move |file_path, _window, cx| {
                let file_path = file_path.to_string();
                let pid = project_id.clone();
                let _ = entity_handle.update(cx, |this: &mut ProjectColumn, cx| {
                    this.hide_diff_popover(cx);
                });
                request_broker.update(cx, |broker, cx| {
                    broker.push_overlay_request(OverlayRequest::DiffViewer {
                        project_id: pid,
                        file: Some(file_path),
                        mode: None,
                        commit_message: None,
                        commits: None,
                        commit_index: None,
                    }, cx);
                });
            },
            t,
        );

        // Position below the git-diff-stats badge
        let bounds = self.diff_stats_bounds;
        let position = point(
            bounds.origin.x,
            bounds.origin.y + bounds.size.height + px(4.0),
        );

        deferred(
            anchored()
                .position(position)
                .snap_to_window()
                .child(
                    div()
                        .id("diff-summary-popover")
                        .occlude()
                        .min_w(px(280.0))
                        .max_w(px(400.0))
                        .max_h(px(300.0))
                        .overflow_y_scroll()
                        .bg(rgb(t.bg_primary))
                        .border_1()
                        .border_color(rgb(t.border))
                        .rounded(px(6.0))
                        .shadow_lg()
                        .py(px(6.0))
                        // Keep popover open when hovering over it
                        .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                            if *hovered {
                                // Cancel any pending hide by updating token
                                this.hover_token.fetch_add(1, Ordering::SeqCst);
                            } else {
                                this.hide_diff_popover(cx);
                            }
                        }))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_scroll_wheel(|_, _, cx| {
                            cx.stop_propagation();
                        })
                        .children(tree_elements),
                ),
        )
        .into_any_element()
    }

    fn ensure_layout_container(&mut self, project_path: String, cx: &mut Context<Self>) {
        if self.layout_container.is_none() {
            let workspace = self.workspace.clone();
            let request_broker = self.request_broker.clone();
            let project_id = self.project_id.clone();
            let backend = self.backend.clone();
            let terminals = self.terminals.clone();
            let active_drag = self.active_drag.clone();
            let action_dispatcher = self.action_dispatcher.clone();

            self.layout_container = Some(cx.new(move |_cx| {
                LayoutContainer::new(
                    workspace,
                    request_broker,
                    project_id,
                    project_path,
                    vec![],
                    backend,
                    terminals,
                    active_drag,
                    action_dispatcher,
                )
            }));
        } else if let Some(container) = &self.layout_container {
            // Update project_path if it changed
            container.update(cx, |c, _| {
                c.set_project_path(project_path);
            });
        }
    }

    fn get_project<'a>(&self, workspace: &'a Workspace) -> Option<&'a ProjectData> {
        workspace.project(&self.project_id)
    }

    fn render_hidden_taskbar(&self, project: &ProjectData, t: ThemeColors) -> impl IntoElement {
        let minimized_terminals = project.layout.as_ref()
            .map(|l| l.collect_minimized_terminals())
            .unwrap_or_default();
        let detached_terminals = project.layout.as_ref()
            .map(|l| l.collect_detached_terminals())
            .unwrap_or_default();

        if minimized_terminals.is_empty() && detached_terminals.is_empty() {
            return div().into_any_element();
        }

        h_flex()
            // Minimized terminals
            .children(
                minimized_terminals.into_iter().map(|(terminal_id, layout_path)| {
                    let workspace = self.workspace.clone();
                    let project_id = self.project_id.clone();

                    // Priority: user-set custom name > non-prompt OSC title > directory fallback
                    let terminal_name = {
                        let osc_title = self.terminals.lock().get(&terminal_id).and_then(|t| t.title());
                        project.terminal_display_name(&terminal_id, osc_title)
                    };

                    div()
                        .id(ElementId::Name(format!("minimized-{}", terminal_id).into()))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .border_l_1()
                        .border_color(rgb(t.border))
                        .hover(|s| s.bg(rgb(t.bg_hover)))
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .text_size(px(10.0))
                        .child(
                            svg()
                                .path("icons/terminal-minimized.svg")
                                .size(px(10.0))
                                .text_color(rgb(t.text_muted))
                        )
                        .child(
                            div()
                                .text_color(rgb(t.text_primary))
                                .child(terminal_name)
                        )
                        .on_click(move |_, _window, cx| {
                            workspace.update(cx, |ws, cx| {
                                ws.restore_terminal(&project_id, &layout_path, cx);
                            });
                        })
                })
            )
            // Detached terminals (with different styling)
            .children(
                detached_terminals.into_iter().map(|(terminal_id, _layout_path)| {
                    let workspace = self.workspace.clone();
                    let terminal_id_for_click = terminal_id.clone();

                    // Priority: user-set custom name > non-prompt OSC title > directory fallback
                    let terminal_name = {
                        let osc_title = self.terminals.lock().get(&terminal_id).and_then(|t| t.title());
                        project.terminal_display_name(&terminal_id, osc_title)
                    };

                    div()
                        .id(ElementId::Name(format!("detached-{}", terminal_id).into()))
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(4.0))
                        .border_l_1()
                        .border_color(rgb(t.border))
                        .bg(rgb(t.bg_hover))
                        .hover(|s| s.bg(rgb(t.bg_selection)))
                        .text_size(px(10.0))
                        .text_color(rgb(t.text_primary))
                        .child(format!("↗ {}", terminal_name))
                        .on_click(move |_, _window, cx| {
                            // Re-attach the terminal (closes detached window)
                            workspace.update(cx, |ws, cx| {
                                ws.attach_terminal(&terminal_id_for_click, cx);
                            });
                        })
                })
            )
            .into_any_element()
    }

    fn render_git_status(&self, project: &ProjectData, status: Option<git::GitStatus>, t: ThemeColors, cx: &mut Context<Self>) -> impl IntoElement {
        let is_worktree = project.worktree_info.is_some();
        let entity_handle = cx.entity().clone();

        match status {
            Some(status) if status.branch.is_some() => {
                let has_changes = status.has_changes();
                let lines_added = status.lines_added;
                let lines_removed = status.lines_removed;
                let project_id = self.project_id.clone();
                let project_path_for_hover = project.path.clone();

                h_flex()
                    .flex_shrink_0()
                    .gap(px(6.0))
                    .text_size(px(10.0))
                    .line_height(px(12.0))
                    // Branch name (hidden for worktrees — shown in header badge instead)
                    .when(!is_worktree, |d| {
                        let pr_url = status.pr_info.as_ref().map(|p| p.url.clone());
                        d.child(project_header::render_branch_status(
                            &status,
                            pr_url.map(|url| {
                                move |_: &mut Window, _: &mut App| {
                                    crate::process::open_url(&url);
                                }
                            }),
                            &t,
                        ))
                    })
                    // Commit log button
                    .child({
                        let project_path_for_log = project.path.clone();
                        let entity_for_bounds = entity_handle.clone();
                        div()
                            .id(ElementId::Name(format!("commit-log-btn-{}", project_id).into()))
                            .relative()
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(18.0))
                            .h(px(16.0))
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                cx.stop_propagation();
                                this.toggle_commit_log(project_path_for_log.clone(), cx);
                            }))
                            .child(
                                svg()
                                    .path("icons/git-commit.svg")
                                    .size(px(10.0))
                                    .text_color(rgb(t.text_muted)),
                            )
                            .child(
                                canvas(
                                    move |bounds, _window, app| {
                                        let _ = entity_for_bounds.update(app, |this: &mut ProjectColumn, _cx| {
                                            this.commit_log_bounds = bounds;
                                        });
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            )
                            .tooltip(move |_window, cx| Tooltip::new("Commit Log").build(_window, cx))
                    })
                    // Diff stats (clickable, only if there are changes)
                    .when(has_changes, |d: Div| {
                        d.child(
                            project_header::render_diff_stats_badge(lines_added, lines_removed, &t)
                                .id(ElementId::Name(format!("git-diff-stats-{}", project_id).into()))
                                .relative()
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                                    if *hovered {
                                        this.show_diff_popover(project_path_for_hover.clone(), cx);
                                    } else {
                                        this.hide_diff_popover(cx);
                                    }
                                }))
                                .on_click(cx.listener(move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.hide_diff_popover(cx);
                                    let pid = this.project_id.clone();
                                    this.request_broker.update(cx, |broker, cx| {
                                        broker.push_overlay_request(OverlayRequest::DiffViewer {
                                            project_id: pid,
                                            file: None,
                                            mode: None,
                                            commit_message: None,
                                            commits: None,
                                            commit_index: None,
                                        }, cx);
                                    });
                                }))
                                // Invisible canvas to capture bounds for popover positioning
                                .child(canvas(
                                    {
                                        let entity_handle = entity_handle.clone();
                                        move |bounds, _window, app| {
                                            entity_handle.update(app, |this, _cx| {
                                                this.diff_stats_bounds = bounds;
                                            });
                                        }
                                    },
                                    |_, _, _, _| {},
                                ).absolute().size_full())
                        )
                    })
                    .into_any_element()
            }
            _ => div().into_any_element(), // Not a git repo - show nothing
        }
    }

    fn render_header(&self, project: &ProjectData, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.clone();
        let workspace_for_hide = self.workspace.clone();
        let project_id = self.project_id.clone();
        let project_id_for_hide = self.project_id.clone();
        // Worktree projects inherit their parent's folder color
        let effective_color = if let Some(ref wt_info) = project.worktree_info {
            let ws = self.workspace.read(cx);
            ws.project(&wt_info.parent_project_id)
                .map(|p| p.folder_color)
                .unwrap_or(project.folder_color)
        } else {
            project.folder_color
        };
        let folder_color = t.get_folder_color(effective_color);

        // Fetch git status once for both header badge and git status area
        let git_status = self.git_watcher.as_ref()
            .and_then(|w| w.read(cx).get(&self.project_id).cloned())
            .or_else(|| {
                project.remote_git_status.as_ref().map(|g| git::GitStatus {
                    branch: g.branch.clone(),
                    lines_added: g.lines_added,
                    lines_removed: g.lines_removed,
                    pr_info: None,
                })
            });
        v_flex()
            // Colored accent bar
            .child(
                div()
                    .h(px(1.0))
                    .w_full()
                    .flex_shrink_0()
                    .bg(rgb(folder_color))
            )
            .child(div()
            .id("project-header")
            .group("project-header")
            .h(px(34.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .justify_between()
            .bg(rgb(t.bg_header))
            .border_b_1()
            .border_color(rgb(t.border))
            .child(
                h_flex()
                    .gap(px(6.0))
                    .overflow_hidden()
                    .child(
                        if project.worktree_info.is_some() {
                            div()
                                .flex_shrink_0()
                                .w(px(8.0))
                                .h(px(8.0))
                                .rounded(px(4.0))
                                .border_1()
                                .border_color(rgb(folder_color))
                                .into_any_element()
                        } else {
                            div()
                                .flex_shrink_0()
                                .w(px(8.0))
                                .h(px(8.0))
                                .rounded(px(4.0))
                                .bg(rgb(folder_color))
                                .into_any_element()
                        }
                    )
                    .child({
                        // For worktree projects, show parent project's name
                        let display_name = if let Some(ref wt_info) = project.worktree_info {
                            let ws = self.workspace.read(cx);
                            ws.project(&wt_info.parent_project_id)
                                .map(|p| p.name.clone())
                                .unwrap_or_else(|| project.name.clone())
                        } else {
                            project.name.clone()
                        };
                        div()
                            .flex_shrink_0()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(t.text_primary))
                            .line_height(px(14.0))
                            .text_ellipsis()
                            .child(display_name)
                    })
                    // Branch badge — for worktrees, also acts as PR button with color-coded icon
                    .when(project.worktree_info.is_some(), |d| {
                        let branch = git_status.as_ref()
                            .and_then(|s| s.branch.clone())
                            .unwrap_or_else(|| project.name.clone());
                        let pr_info = git_status.as_ref().and_then(|s| s.pr_info.clone());

                        let (icon_path, icon_color, tooltip_text) = if let Some(ref pr) = pr_info {
                            ("icons/git-pull-request.svg", pr.state.color(&t), format!("Pull Request ({})", pr.state.label()))
                        } else {
                            ("icons/git-branch.svg", t.text_muted, branch.clone())
                        };

                        let pr_number = pr_info.as_ref().map(|p| p.number);
                        let ci_checks = pr_info.as_ref().and_then(|p| p.ci_checks.clone());
                        let has_pr = pr_info.is_some();
                        let pr_url = pr_info.map(|p| p.url);

                        d.child(
                            h_flex()
                                .id("branch-badge")
                                .flex_shrink_0()
                                .gap(px(3.0))
                                .px(px(4.0))
                                .py(px(1.0))
                                .rounded(px(3.0))
                                .items_center()
                                .when(has_pr, |d| {
                                    d.cursor_pointer()
                                        .hover(|s| s.bg(rgb(t.bg_hover)))
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation();
                                        })
                                })
                                .when_some(pr_url, |d, url| {
                                    d.on_click(move |_, _, _cx| {
                                        crate::process::open_url(&url);
                                    })
                                })
                                .child(
                                    svg()
                                        .path(icon_path)
                                        .size(px(10.0))
                                        .text_color(rgb(icon_color))
                                )
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(rgb(t.text_secondary))
                                        .line_height(px(12.0))
                                        .max_w(px(120.0))
                                        .text_ellipsis()
                                        .overflow_hidden()
                                        .child(branch)
                                )
                                .when_some(pr_number, |d, num| {
                                    d.child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(rgb(t.text_muted))
                                            .line_height(px(12.0))
                                            .child(format!("#{num}"))
                                    )
                                })
                                .when_some(ci_checks, |d, checks| {
                                    let ci_tooltip = checks.tooltip_text();
                                    d.child(
                                        div()
                                            .id("ci-status-wt")
                                            .child(
                                                svg()
                                                    .path(checks.status.icon())
                                                    .size(px(8.0))
                                                    .text_color(rgb(checks.status.color(&t)))
                                            )
                                            .tooltip(move |_window, cx| Tooltip::new(ci_tooltip.clone()).build(_window, cx))
                                    )
                                })
                                .tooltip(move |_window, cx| Tooltip::new(tooltip_text.clone()).build(_window, cx))
                        )
                    })
                    .child({
                        let path_for_copy = project.path.clone();
                        // Left-truncate: flex + justify_end clips from the left
                        div()
                            .id("project-path")
                            .max_w(px(300.0))
                            .overflow_hidden()
                            .flex()
                            .justify_end()
                            .cursor_pointer()
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(path_for_copy.clone()));
                                crate::views::panels::toast::ToastManager::success("Path copied to clipboard".to_string(), cx);
                            })
                            .tooltip(move |_window, cx| Tooltip::new("Copy path").build(_window, cx))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(10.0))
                                    .text_color(rgb(t.text_muted))
                                    .line_height(px(12.0))
                                    .child(project.path.clone()),
                            )
                    })
                    .child(self.render_git_status(project, git_status, t, cx)),
            )
            .child(
                // Right side: minimized taskbar + controls
                h_flex()
                    .gap(px(8.0))
                    // Hidden terminals taskbar (minimized and detached)
                    .child(self.render_hidden_taskbar(project, t))
                    // Project controls
                    .child(
                        div()
                            .flex()
                            .gap(px(2.0))
                            .opacity(0.0)
                            .group_hover("project-header", |s| s.opacity(1.0))
                            .child(
                                // Hide project button
                                div()
                                    .id("hide-project-btn")
                                    .cursor_pointer()
                                    .w(px(24.0))
                                    .h(px(24.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.0))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_click(move |_, _window, cx| {
                                        cx.stop_propagation();
                                        workspace_for_hide.update(cx, |ws, cx| {
                                            ws.toggle_project_overview_visibility(&project_id_for_hide, cx);
                                        });
                                    })
                                    .child(
                                        svg()
                                            .path("icons/eye-off.svg")
                                            .size(px(14.0))
                                            .text_color(rgb(t.text_secondary))
                                    )
                                    .tooltip(|_window, cx| Tooltip::new("Hide Project").build(_window, cx)),
                            )
                            .child(
                                // Fullscreen button
                                div()
                                    .id("fullscreen-project-btn")
                                    .cursor_pointer()
                                    .w(px(24.0))
                                    .h(px(24.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.0))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_click(move |_, _window, cx| {
                                        cx.stop_propagation();
                                        workspace.update(cx, |ws, cx| {
                                            ws.set_focused_project(Some(project_id.clone()), cx);
                                        });
                                    })
                                    .child(
                                        svg()
                                            .path("icons/fullscreen.svg")
                                            .size(px(14.0))
                                            .text_color(rgb(t.text_secondary))
                                    )
                                    .tooltip(|_window, cx| Tooltip::new("Focus Project").build(_window, cx)),
                            ),
                    )
                    // Service indicator (rightmost)
                    .child(self.render_service_indicator(&t, cx)),
            ))
    }

    /// Render empty state for bookmark projects (no terminal)
    fn render_creating_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        v_flex()
            .items_center()
            .justify_center()
            .size_full()
            .gap(px(12.0))
            .bg(rgb(t.bg_primary))
            .child(
                svg()
                    .path("icons/git-branch.svg")
                    .size(px(48.0))
                    .text_color(rgb(t.text_muted))
            )
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(rgb(t.text_secondary))
                    .child("Setting up worktree\u{2026}")
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(t.text_muted))
                    .max_w(px(240.0))
                    .text_center()
                    .child("Fetching latest changes and creating the branch. Terminals will start automatically.")
            )
    }

    fn render_empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let project_id = self.project_id.clone();

        v_flex()
            .items_center()
            .justify_center()
            .size_full()
            .gap(px(16.0))
            .bg(rgb(t.bg_primary))
            .child(
                // Folder icon
                svg()
                    .path("icons/folder.svg")
                    .size(px(48.0))
                    .text_color(rgb(t.text_muted))
            )
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(rgb(t.text_muted))
                    .child("No terminal attached")
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(t.text_muted))
                    .max_w(px(200.0))
                    .text_center()
                    .child("This project is saved as a bookmark. Start a terminal to begin working.")
            )
            .child(
                // Start Terminal button
                div()
                    .id("start-terminal-btn")
                    .cursor_pointer()
                    .px(px(16.0))
                    .py(px(8.0))
                    .rounded(px(6.0))
                    .bg(rgb(t.button_primary_bg))
                    .hover(|s| s.bg(rgb(t.button_primary_hover)))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        svg()
                            .path("icons/terminal.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.button_primary_fg))
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(t.button_primary_fg))
                            .child("Start Terminal")
                    )
                    .on_click({
                        let dispatcher = self.action_dispatcher.clone();
                        move |_, _window, cx| {
                            if let Some(ref dispatcher) = dispatcher {
                                dispatcher.dispatch(
                                    okena_core::api::ActionRequest::CreateTerminal {
                                        project_id: project_id.clone(),
                                    },
                                    cx,
                                );
                            }
                        }
                    })
            )
    }
}

impl ProjectColumn {
    /// Dispatch a service action through ActionDispatcher (handles both local and remote).
    fn dispatch_service_action(&self, action: ActionRequest, cx: &mut Context<Self>) {
        if let Some(ref dispatcher) = self.action_dispatcher {
            dispatcher.dispatch(action, cx);
        }
    }

    /// Render the per-project service log panel (tab header + terminal pane).
    fn render_service_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.service_panel_open {
            return div().into_any_element();
        }

        let t = theme(cx);
        let services = self.get_service_list(cx);

        if services.is_empty() {
            return div().into_any_element();
        }

        let active_name = self.active_service_name.clone();
        let is_overview = active_name.is_none();

        // Read active service status for action buttons (detail tab only)
        let active_status = active_name.as_ref().and_then(|name| {
            services.iter()
                .find(|s| s.name == *name)
                .map(|s| s.status.clone())
        });

        let project_id = self.project_id.clone();
        let active_drag = self.active_drag.clone();
        let panel_height = self.service_panel_height;
        let entity = cx.entity().downgrade();

        div()
            .id("service-panel")
            .flex()
            .flex_col()
            .h(px(panel_height))
            .flex_shrink_0()
            .child(
                ResizeHandle::new(
                    true, // horizontal divider (full width, 1px tall)
                    t.border,
                    t.border_active,
                    move |mouse_pos, _cx| {
                        *active_drag.borrow_mut() = Some(DragState::ServicePanel {
                            project_id: project_id.clone(),
                            initial_mouse_y: f32::from(mouse_pos.y),
                            initial_height: panel_height,
                        });
                    },
                ),
            )
            .child(
                okena_views_services::panel::render_service_panel_header(
                    &services,
                    active_name.as_deref(),
                    &t,
                    // on_overview_click
                    {
                        let entity = entity.clone();
                        move |_window, cx| {
                            if let Some(e) = entity.upgrade() {
                                e.update(cx, |this, cx| this.show_overview(cx));
                            }
                        }
                    },
                    // on_tab_click
                    {
                        let entity = entity.clone();
                        move |name: String, _window, cx| {
                            if let Some(e) = entity.upgrade() {
                                e.update(cx, |this, cx| this.show_service(&name, cx));
                            }
                        }
                    },
                    // on_start_all
                    {
                        let entity = entity.clone();
                        move |_window, cx| {
                            if let Some(e) = entity.upgrade() {
                                e.update(cx, |this, cx| {
                                    this.dispatch_service_action(ActionRequest::StartAllServices {
                                        project_id: this.project_id.clone(),
                                    }, cx);
                                });
                            }
                        }
                    },
                    // on_stop_all
                    {
                        let entity = entity.clone();
                        move |_window, cx| {
                            if let Some(e) = entity.upgrade() {
                                e.update(cx, |this, cx| {
                                    this.dispatch_service_action(ActionRequest::StopAllServices {
                                        project_id: this.project_id.clone(),
                                    }, cx);
                                });
                            }
                        }
                    },
                    // on_reload
                    {
                        let entity = entity.clone();
                        move |_window, cx| {
                            if let Some(e) = entity.upgrade() {
                                e.update(cx, |this, cx| {
                                    this.dispatch_service_action(ActionRequest::ReloadServices {
                                        project_id: this.project_id.clone(),
                                    }, cx);
                                });
                            }
                        }
                    },
                    // on_start (active service)
                    {
                        let entity = entity.clone();
                        move |_window, cx| {
                            if let Some(e) = entity.upgrade() {
                                e.update(cx, |this, cx| {
                                    if let Some(name) = this.active_service_name.clone() {
                                        this.dispatch_service_action(ActionRequest::StartService {
                                            project_id: this.project_id.clone(),
                                            service_name: name,
                                        }, cx);
                                    }
                                });
                            }
                        }
                    },
                    // on_stop (active service)
                    {
                        let entity = entity.clone();
                        move |_window, cx| {
                            if let Some(e) = entity.upgrade() {
                                e.update(cx, |this, cx| {
                                    if let Some(name) = this.active_service_name.clone() {
                                        this.dispatch_service_action(ActionRequest::StopService {
                                            project_id: this.project_id.clone(),
                                            service_name: name,
                                        }, cx);
                                    }
                                });
                            }
                        }
                    },
                    // on_restart (active service)
                    {
                        let entity = entity.clone();
                        move |_window, cx| {
                            if let Some(e) = entity.upgrade() {
                                e.update(cx, |this, cx| {
                                    if let Some(name) = this.active_service_name.clone() {
                                        this.dispatch_service_action(ActionRequest::RestartService {
                                            project_id: this.project_id.clone(),
                                            service_name: name,
                                        }, cx);
                                    }
                                });
                            }
                        }
                    },
                    // on_close
                    {
                        let entity = entity.clone();
                        move |_window, cx| {
                            if let Some(e) = entity.upgrade() {
                                e.update(cx, |this, cx| this.close_service_panel(cx));
                            }
                        }
                    },
                    active_status.as_ref(),
                ),
            )
            .child(
                // Content area
                if is_overview {
                    self.render_overview_content(&services, cx).into_any_element()
                } else if self.service_terminal_pane.is_some() {
                    div()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .overflow_hidden()
                        .children(self.service_terminal_pane.clone())
                        .into_any_element()
                } else {
                    okena_views_services::panel::render_not_running_placeholder(
                        &t,
                        {
                            let entity = entity.clone();
                            move |_window, cx| {
                                if let Some(e) = entity.upgrade() {
                                    e.update(cx, |this, cx| {
                                        if let Some(name) = this.active_service_name.clone() {
                                            this.dispatch_service_action(ActionRequest::StartService {
                                                project_id: this.project_id.clone(),
                                                service_name: name,
                                            }, cx);
                                        }
                                    });
                                }
                            }
                        },
                    ).into_any_element()
                },
            )
            .into_any_element()
    }

    /// Render the overview content showing all services in a table layout.
    fn render_overview_content(&self, services: &[ServiceSnapshot], cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Determine host for port badge URLs
        let remote_host = self.workspace.read(cx).project(&self.project_id)
            .and_then(|p| p.remote_host.clone());

        let project_id = self.project_id.clone();
        let entity = cx.entity().downgrade();

        okena_views_services::panel::render_service_overview(
            services,
            &project_id,
            remote_host.as_deref(),
            &t,
            // on_service_click
            {
                let entity = entity.clone();
                move |name: String, _window, cx| {
                    if let Some(e) = entity.upgrade() {
                        e.update(cx, |this, cx| this.show_service(&name, cx));
                    }
                }
            },
            // on_start
            {
                let entity = entity.clone();
                move |name: String, _window, cx| {
                    if let Some(e) = entity.upgrade() {
                        e.update(cx, |this, cx| {
                            this.dispatch_service_action(ActionRequest::StartService {
                                project_id: this.project_id.clone(),
                                service_name: name.clone(),
                            }, cx);
                        });
                    }
                }
            },
            // on_stop
            {
                let entity = entity.clone();
                move |name: String, _window, cx| {
                    if let Some(e) = entity.upgrade() {
                        e.update(cx, |this, cx| {
                            this.dispatch_service_action(ActionRequest::StopService {
                                project_id: this.project_id.clone(),
                                service_name: name.clone(),
                            }, cx);
                        });
                    }
                }
            },
            // on_restart
            {
                let entity = entity.clone();
                move |name: String, _window, cx| {
                    if let Some(e) = entity.upgrade() {
                        e.update(cx, |this, cx| {
                            this.dispatch_service_action(ActionRequest::RestartService {
                                project_id: this.project_id.clone(),
                                service_name: name.clone(),
                            }, cx);
                        });
                    }
                }
            },
            // on_port_click
            |port: u16| {
                let url = format!("http://localhost:{}", port);
                crate::process::open_url(&url);
            },
        )
    }

    /// Render the service indicator button for the project header.
    fn render_service_indicator(&self, t: &ThemeColors, cx: &mut Context<Self>) -> impl IntoElement {
        let services = self.get_service_list(cx);
        let entity = cx.entity().downgrade();

        okena_views_services::panel::render_service_indicator(
            &services,
            t,
            move |_window, cx| {
                if let Some(e) = entity.upgrade() {
                    e.update(cx, |this, cx| {
                        if this.service_panel_open {
                            this.close_service_panel(cx);
                        } else {
                            this.show_overview(cx);
                        }
                    });
                }
            },
        )
    }
}

impl Render for ProjectColumn {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.read(cx);
        let project = self.get_project(workspace).cloned();

        match project {
            Some(project) => {
                let has_layout = project.layout.is_some();

                let is_creating = workspace.creating_projects.contains(&self.project_id);

                // Soft tinted background based on folder color (when enabled)
                let bg_color = if crate::settings::settings(cx).color_tinted_background {
                    let color = workspace.effective_folder_color(&project);
                    if color != crate::theme::FolderColor::Default {
                        rgb(crate::ui::tint_color(t.bg_primary, t.get_folder_color(color), 0.025))
                    } else {
                        rgb(t.bg_primary)
                    }
                } else {
                    rgb(t.bg_primary)
                };

                // Content: layout, creating state, or empty bookmark state
                let content = if has_layout {
                    // Ensure layout container exists (created once, not every render)
                    self.ensure_layout_container(project.path.clone(), cx);

                    div()
                        .id("project-column-content")
                        .flex_1()
                        .min_h_0()
                        .overflow_hidden()
                        .child(self.layout_container.clone().unwrap())
                        .into_any_element()
                } else if is_creating {
                    self.render_creating_state(cx).into_any_element()
                } else {
                    // Empty state for bookmark projects
                    self.render_empty_state(cx).into_any_element()
                };

                div()
                    .id("project-column-main")
                    .relative()
                    .flex()
                    .flex_col()
                    .size_full()
                    .min_h_0()
                    .bg(bg_color)
                    .child(self.render_header(&project, cx))
                    .child(content)
                    .child(self.render_service_panel(cx))
                    .child(self.render_diff_popover(&t, cx))
                    .child(self.render_commit_log_popover(&t, cx))
                    .into_any_element()
            }

            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(t.text_muted))
                .child("Project not found")
                .into_any_element(),
        }
    }
}

