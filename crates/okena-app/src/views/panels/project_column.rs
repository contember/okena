use crate::action_dispatch::ActionDispatcher;
use crate::git;
use crate::git::watcher::GitStatusWatcher;
use crate::services::manager::ServiceManager;
use crate::terminal::backend::TerminalBackend;
use crate::theme::{ThemeColors, theme};
use crate::ui::tokens::{ui_text_md, ui_text_ms, ui_text_sm, ui_text_xl};
use crate::views::layout::layout_container::LayoutContainer;
use crate::views::layout::split_pane::ActiveDrag;
use crate::workspace::request_broker::RequestBroker;
use crate::workspace::state::{FocusedTerminalState, LayoutNode, ProjectData, WindowId, Workspace};
use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use gpui_component::{h_flex, v_flex};
use okena_views_git::git_header::GitHeader;
use std::sync::Arc;

use crate::views::panels::hook_panel::HookPanel;
use crate::views::window::TerminalsRegistry;
use okena_core::api::ActionRequest;
use okena_views_services::service_panel::ServicePanel;
use okena_workspace::requests::{OverlayRequest, ProjectOverlay, ProjectOverlayKind};

fn project_header_display_name(project: &ProjectData) -> String {
    project.name.clone()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FocusedTerminalHeaderTarget {
    terminal_id: String,
    layout_path: Vec<usize>,
}

/// The terminal at `layout_path`, if it is the kind of pane whose header the
/// `auto_hide_single_terminal_header` setting hides — a live, standalone pane.
/// A pane inside a tab group keeps its own action bar, so it is never a target.
fn standalone_terminal_header_target(
    layout: &LayoutNode,
    layout_path: &[usize],
) -> Option<FocusedTerminalHeaderTarget> {
    if layout.is_in_tab_group(layout_path) {
        return None;
    }

    match layout.get_at_path(layout_path) {
        Some(LayoutNode::Terminal {
            terminal_id: Some(terminal_id),
            minimized: false,
            detached: false,
            ..
        }) => Some(FocusedTerminalHeaderTarget {
            terminal_id: terminal_id.clone(),
            layout_path: layout_path.to_vec(),
        }),
        _ => None,
    }
}

fn focused_terminal_header_target(
    project_id: &str,
    layout: Option<&LayoutNode>,
    focused: Option<&FocusedTerminalState>,
    auto_hide_single_terminal_header: bool,
    has_fullscreen: bool,
) -> Option<FocusedTerminalHeaderTarget> {
    if !auto_hide_single_terminal_header || has_fullscreen {
        return None;
    }

    let layout = layout?;

    if let Some(focused) = focused.filter(|focused| focused.project_id == project_id)
        && let Some(target) = standalone_terminal_header_target(layout, &focused.layout_path)
    {
        return Some(target);
    }

    let (_, layout_path) = layout.single_terminal()?;
    standalone_terminal_header_target(layout, &layout_path)
}

/// What the project column paints in its main content area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnContent {
    /// Worktree teardown is underway and its terminals are already gone.
    Closing,
    /// Normal case: render the pane tree.
    Layout,
    /// Worktree checkout is still being created.
    Creating,
    /// Bookmark project with no terminal attached.
    Empty,
}

/// Pick the content branch for a project column.
///
/// `Closing` wins over `Layout` because a closing worktree keeps `layout: Some`
/// — `prepare_background_worktree_removal` nulls every leaf's `terminal_id` but
/// leaves the tree shape standing so `ensure_terminal` can't resurrect a PTY
/// inside the doomed checkout. Without this branch those id-less panes each
/// render the "Starting terminal…" placeholder for the whole removal window,
/// which is seconds on a real checkout.
///
/// Deliberately gated on the terminals being *gone*, not on `closing` alone:
/// the merge/`before_remove`-hook phases also set the closing flag while the
/// terminals are still live and useful, and those must keep painting.
fn column_content(project: &ProjectData, closing: bool) -> ColumnContent {
    let has_terminals = project
        .layout
        .as_ref()
        .is_some_and(|layout| layout.has_terminal_ids());

    if closing && !has_terminals {
        ColumnContent::Closing
    } else if project.layout.is_some() {
        ColumnContent::Layout
    } else if project.is_creating {
        // Explicit mid-create marker set by the daemon while the git checkout
        // runs and mirrored over the wire. NOT derived from a missing layout: a
        // worktree whose last terminal the user closed is a legitimate bookmark
        // (layout None) and must fall through to the empty state with its Start
        // Terminal button, not the creating placeholder.
        ColumnContent::Creating
    } else {
        ColumnContent::Empty
    }
}

/// A single project column with header and layout
pub struct ProjectColumn {
    /// Identifies which window-scoped slot on the shared `Workspace` this
    /// project column addresses. Always `WindowId::Main` today (single-window
    /// runtime); slice 05 spawns extras that mint distinct
    /// `WindowId::Extra(uuid)`s. Read in-impl via `self.window_id` -- the
    /// hide-project button's `on_click` listener in `render_header`
    /// captures it as a `window_id_for_hide` local hoisted alongside
    /// `workspace_for_hide` and `project_id_for_hide`, which the move
    /// closure then captures by Copy for the
    /// `toggle_project_overview_visibility` call.
    pub(crate) window_id: WindowId,
    workspace: Entity<Workspace>,
    focus_manager: Entity<crate::workspace::focus::FocusManager>,
    request_broker: Entity<RequestBroker>,
    project_id: String,
    #[allow(dead_code)]
    backend: Arc<dyn TerminalBackend>,
    #[allow(dead_code)]
    terminals: TerminalsRegistry,
    /// Stored layout container entity (must be created in new(), not render())
    layout_container: Option<Entity<LayoutContainer<ActionDispatcher>>>,
    /// Git status watcher (centralized polling)
    git_watcher: Option<Entity<GitStatusWatcher>>,
    /// Shared drag state for resize operations
    active_drag: ActiveDrag,
    /// Action dispatcher for routing terminal actions (local or remote)
    action_dispatcher: Option<ActionDispatcher>,
    /// Self-contained git header entity (diff popover, commit log)
    git_header: Entity<GitHeader>,
    /// Self-contained service panel entity
    service_panel: Entity<ServicePanel<ActionDispatcher>>,
    /// Self-contained hook panel entity
    hook_panel: Entity<HookPanel>,
    /// Index into the discoverability tip pool shown on the empty state.
    /// Advanced by the "Another tip" control; seeded per column so different
    /// empty columns don't all open on the same tip.
    tip_index: usize,
    /// Whether the pointer is over the header. Drives revealing the hide/focus
    /// controls. Tracked in state (not `group_hover`) because the reveal frees
    /// layout space, and toggling `display` via hover styles crashes GPUI
    /// (`must call prepaint before paint`) — the hover state can differ between
    /// prepaint and paint. State + `notify` re-renders a consistent tree.
    header_hovered: bool,
}

impl ProjectColumn {
    // GPUI view constructor: each param is a distinct injected dependency.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        window_id: WindowId,
        workspace: Entity<Workspace>,
        focus_manager: Entity<crate::workspace::focus::FocusManager>,
        request_broker: Entity<RequestBroker>,
        project_id: String,
        backend: Arc<dyn TerminalBackend>,
        terminals: TerminalsRegistry,
        active_drag: ActiveDrag,
        git_watcher: Option<Entity<GitStatusWatcher>>,
        git_provider: Arc<dyn okena_views_git::diff_viewer::provider::GitProvider>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Observe git watcher for re-renders (replaces per-column polling).
        // In daemon-client mode this is always `None` (every project is remote,
        // so git status arrives via the remote snapshot); the immediate refresh
        // for a newly visible project is requested by `WindowView` sending a
        // `GitStatus` action to the daemon (see
        // `request_git_poll_for_visible_project`).
        if let Some(ref watcher) = git_watcher {
            cx.observe(watcher, |_, _, cx| cx.notify()).detach();
        }

        // Observe the workspace itself. In daemon-client mode there is no local
        // git_watcher (it's `None`); the header reads git status from the remote
        // snapshot, which is refreshed via `apply_remote_snapshot` +
        // `notify_ui_only` on the Workspace. Since ProjectColumn renders inside a
        // `.cached()` view, only a notify from an entity it observes repaints it
        // — without this observer remote git-status updates (branch, ahead/behind,
        // diff stats) never reach the header chip and go stale. Services don't
        // hit this because ServicePanel already observes the workspace.
        cx.observe(&workspace, |_, _, cx| cx.notify()).detach();

        let initial_service_height = workspace
            .read(cx)
            .data
            .service_panel_heights
            .get(&project_id)
            .copied()
            .unwrap_or(200.0);

        let git_header = {
            let pid = project_id.clone();
            let rb = request_broker.clone();
            let ws = workspace.clone();
            let fm = focus_manager.clone();
            let gw = git_watcher.clone();
            cx.new(move |cx| GitHeader::new(pid, rb, ws, fm, git_provider, gw, cx))
        };
        // Observe git_header so ProjectColumn re-renders when popovers change
        cx.observe(&git_header, |_, _, cx| cx.notify()).detach();

        let service_panel = {
            let pid = project_id.clone();
            let ws = workspace.clone();
            let fm = focus_manager.clone();
            let rb = request_broker.clone();
            let be = backend.clone();
            let ts = terminals.clone();
            let ad = active_drag.clone();
            cx.new(move |cx| {
                ServicePanel::new(
                    pid,
                    ws,
                    fm,
                    rb,
                    be,
                    ts,
                    ad,
                    window_id,
                    initial_service_height,
                    cx,
                )
            })
        };
        // Observe service_panel so ProjectColumn re-renders when panel state changes
        cx.observe(&service_panel, |_, _, cx| cx.notify()).detach();

        let initial_hook_height = workspace
            .read(cx)
            .data
            .hook_panel_heights
            .get(&project_id)
            .copied()
            .unwrap_or(200.0);

        let hook_panel = {
            let pid = project_id.clone();
            let ws = workspace.clone();
            let fm = focus_manager.clone();
            let rb = request_broker.clone();
            let be = backend.clone();
            let ts = terminals.clone();
            let ad = active_drag.clone();
            cx.new(move |cx| {
                HookPanel::new(
                    pid,
                    ws,
                    fm,
                    rb,
                    be,
                    ts,
                    ad,
                    window_id,
                    initial_hook_height,
                    cx,
                )
            })
        };
        cx.observe(&hook_panel, |_, _, cx| cx.notify()).detach();

        Self {
            window_id,
            workspace,
            focus_manager,
            request_broker,
            project_id,
            backend,
            terminals,
            layout_container: None,
            git_watcher,
            active_drag,
            action_dispatcher: None,
            git_header,
            service_panel,
            hook_panel,
            tip_index: crate::views::tips::next_start_index(),
            header_hovered: false,
        }
    }

    /// Identifies which window-scoped slot on the shared `Workspace` this
    /// project column addresses. Always `WindowId::Main` today (single-window
    /// runtime); slice 05 spawns extras that mint distinct
    /// `WindowId::Extra(uuid)`s. The field is read directly within `render_header`
    /// via the `window_id_for_hide` hoist captured by the hide-project button's
    /// `on_click` move closure. This public getter exists for external callers
    /// (e.g. the slice 05 spawn flow on `Okena`) that need to address
    /// window-scoped state on `Workspace` in the same window this project
    /// column inhabits. Marked `#[allow(dead_code)]` because rustc tracks
    /// fields and methods separately -- the field being used at runtime does
    /// NOT mark the getter as used.
    #[allow(dead_code)]
    pub fn window_id(&self) -> WindowId {
        self.window_id
    }

    /// Set the action dispatcher (used for remote projects).
    ///
    /// NOTE: This only sets the dispatcher on ProjectColumn itself.
    /// The ServicePanel's dispatcher is synced lazily on first render
    /// (via `sync_service_panel_dispatcher`), because `set_action_dispatcher`
    /// is called inside `cx.new()` closures where no `Context<Self>` is available.
    pub fn set_action_dispatcher(&mut self, dispatcher: Option<ActionDispatcher>) {
        self.action_dispatcher = dispatcher;
    }

    /// Sync the action dispatcher to the service panel entity.
    fn sync_service_panel_dispatcher(&self, cx: &mut Context<Self>) {
        let dispatcher = self.action_dispatcher.clone();
        self.service_panel.update(cx, |sp, _cx| {
            sp.set_action_dispatcher(dispatcher);
        });
    }

    /// Sync the action dispatcher to the hook panel entity (mirrors the service
    /// panel sync — the hook panel needs it to dispatch RerunHook to the daemon).
    fn sync_hook_panel_dispatcher(&self, cx: &mut Context<Self>) {
        let dispatcher = self.action_dispatcher.clone();
        self.hook_panel.update(cx, |hp, _cx| {
            hp.set_action_dispatcher(dispatcher);
        });
    }

    /// Set the service manager and observe it for changes.
    pub fn set_service_manager(&mut self, manager: Entity<ServiceManager>, cx: &mut Context<Self>) {
        // Sync dispatcher to service + hook panels (may have been set before the
        // panels were created).
        self.sync_service_panel_dispatcher(cx);
        self.sync_hook_panel_dispatcher(cx);
        self.service_panel.update(cx, |sp, cx| {
            sp.set_service_manager(manager, cx);
        });
    }

    /// Show a service's log output in the per-project panel.
    pub fn show_service(&mut self, service_name: &str, cx: &mut Context<Self>) {
        let name = service_name.to_string();
        self.service_panel.update(cx, |sp, cx| {
            sp.show_service(&name, cx);
        });
    }

    /// Set the service panel height (called during drag resize).
    pub fn set_service_panel_height(&mut self, height: f32, cx: &mut Context<Self>) {
        self.service_panel.update(cx, |sp, cx| {
            sp.set_service_panel_height(height, cx);
        });
    }

    /// Close the per-project service log panel.
    #[allow(dead_code)]
    pub fn close_service_panel(&mut self, cx: &mut Context<Self>) {
        self.service_panel.update(cx, |sp, cx| {
            sp.close(cx);
        });
    }

    /// Replace the git provider used by the project's `GitHeader`.
    /// Called when the project's on-disk path changes (e.g. directory rename),
    /// so cached commit/diff data stops referring to the stale path.
    pub fn set_git_provider(
        &mut self,
        provider: Arc<dyn okena_views_git::diff_viewer::provider::GitProvider>,
        cx: &mut Context<Self>,
    ) {
        self.git_header
            .update(cx, |gh, cx| gh.set_git_provider(provider, cx));
    }

    /// Open the branch switcher popover for this project's header.
    /// No-op when the provider is read-only (remote-mirrored project).
    pub fn show_branch_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.git_header
            .update(cx, |gh, cx| gh.show_branch_picker(window, cx));
    }

    /// Show a hook terminal in the hook panel.
    pub fn show_hook_terminal(&mut self, terminal_id: &str, cx: &mut Context<Self>) {
        let tid = terminal_id.to_string();
        self.hook_panel.update(cx, |hp, cx| {
            hp.show_hook(&tid, cx);
        });
    }

    /// Set the hook panel height (called during drag resize).
    pub fn set_hook_panel_height(&mut self, height: f32, cx: &mut Context<Self>) {
        self.hook_panel.update(cx, |hp, cx| {
            hp.set_panel_height(height, cx);
        });
    }

    /// Observe workspace for remote service state changes (used for remote project columns).
    pub fn observe_remote_services(
        &mut self,
        workspace: Entity<Workspace>,
        cx: &mut Context<Self>,
    ) {
        // Sync dispatcher to service + hook panels (may have been set before the
        // panels were created). This is the sync point for daemon-client columns,
        // which is how the hook panel gets its dispatcher to route RerunHook.
        self.sync_service_panel_dispatcher(cx);
        self.sync_hook_panel_dispatcher(cx);
        self.service_panel.update(cx, |sp, cx| {
            sp.observe_remote_services(workspace, cx);
        });
    }

    fn ensure_layout_container(&mut self, project_path: String, cx: &mut Context<Self>) {
        if self.layout_container.is_none() {
            let workspace = self.workspace.clone();
            let focus_manager = self.focus_manager.clone();
            let request_broker = self.request_broker.clone();
            let project_id = self.project_id.clone();
            let backend = self.backend.clone();
            let terminals = self.terminals.clone();
            let active_drag = self.active_drag.clone();
            let action_dispatcher = self.action_dispatcher.clone();
            let window_id = self.window_id;

            self.layout_container = Some(cx.new(move |_cx| {
                LayoutContainer::new(
                    workspace,
                    focus_manager,
                    request_broker,
                    window_id,
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

    fn render_hidden_taskbar(
        &self,
        project: &ProjectData,
        t: ThemeColors,
        cx: &App,
    ) -> impl IntoElement {
        let minimized_terminals = project
            .layout
            .as_ref()
            .map(|l| l.collect_minimized_terminals())
            .unwrap_or_default();
        let detached_terminals = project
            .layout
            .as_ref()
            .map(|l| l.collect_detached_terminals())
            .unwrap_or_default();

        if minimized_terminals.is_empty() && detached_terminals.is_empty() {
            // `hidden` (not an empty visible div) so it claims no flex gap.
            return div().hidden().into_any_element();
        }

        h_flex()
            // Minimized terminals
            .children(
                minimized_terminals
                    .into_iter()
                    .map(|(terminal_id, layout_path)| {
                        let workspace = self.workspace.clone();
                        let project_id = self.project_id.clone();

                        // A minimized terminal has no pane to carry the attention
                        // border, so its chip reports the same two signals.
                        let (terminal_name, has_bell) = {
                            let terminals = self.terminals.lock();
                            let terminal = terminals.get(&terminal_id);
                            let osc_title = terminal.and_then(|t| t.title());
                            let bell =
                                terminal.is_some_and(|t| t.has_bell() || t.has_notification());
                            (project.terminal_display_name(&terminal_id, osc_title), bell)
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
                            .text_size(ui_text_sm(cx))
                            .child(
                                svg()
                                    .path(if has_bell {
                                        "icons/bell.svg"
                                    } else {
                                        "icons/terminal-minimized.svg"
                                    })
                                    .size(px(10.0))
                                    .text_color(if has_bell {
                                        rgb(t.border_bell)
                                    } else {
                                        rgb(t.text_muted)
                                    }),
                            )
                            .child(div().text_color(rgb(t.text_primary)).child(terminal_name))
                            .on_click(move |_, _window, cx| {
                                workspace.update(cx, |ws, cx| {
                                    ws.restore_terminal(&project_id, &layout_path, cx);
                                });
                            })
                    }),
            )
            // Detached terminals (with different styling)
            .children(
                detached_terminals
                    .into_iter()
                    .map(|(terminal_id, _layout_path)| {
                        let workspace = self.workspace.clone();
                        let terminal_id_for_click = terminal_id.clone();

                        let (terminal_name, has_bell) = {
                            let terminals = self.terminals.lock();
                            let terminal = terminals.get(&terminal_id);
                            let osc_title = terminal.and_then(|t| t.title());
                            let bell =
                                terminal.is_some_and(|t| t.has_bell() || t.has_notification());
                            (project.terminal_display_name(&terminal_id, osc_title), bell)
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
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_primary))
                            .when(has_bell, |d| {
                                d.child(
                                    svg()
                                        .path("icons/bell.svg")
                                        .size(px(10.0))
                                        .text_color(rgb(t.border_bell)),
                                )
                            })
                            .child(format!("\u{2197} {}", terminal_name))
                            .on_click(move |_, _window, cx| {
                                workspace.update(cx, |ws, cx| {
                                    ws.attach_terminal(&terminal_id_for_click, cx);
                                });
                            })
                    }),
            )
            .into_any_element()
    }

    /// Resolve the project's git status: prefer the local watcher, fall back to
    /// the remote snapshot (daemon-client mode, where `git_watcher` is `None`).
    /// Both the header badge and the CI-checks popover MUST go through this so
    /// they agree on the source — otherwise the badge renders from the snapshot
    /// while the popover reads an empty watcher and shows nothing (a pill you
    /// can't open).
    fn resolve_git_status(&self, cx: &Context<Self>) -> Option<git::GitStatus> {
        self.git_watcher
            .as_ref()
            .and_then(|w| w.read(cx).get(&self.project_id).cloned())
            .or_else(|| {
                self.workspace
                    .read(cx)
                    .remote_snapshot(&self.project_id)
                    .and_then(|snap| snap.git_status.as_ref())
                    .map(|g| git::GitStatus {
                        branch: g.branch.clone(),
                        lines_added: g.lines_added,
                        lines_removed: g.lines_removed,
                        pr_info: g.pr_info.clone(),
                        ci_checks: g.ci_checks.clone(),
                        ahead: g.ahead,
                        behind: g.behind,
                        unpushed: g.unpushed,
                        // Carried over the wire (ApiGitStatus.review_base /
                        // .default_branch) so the "Review changes" chip renders
                        // and the base label hides on the default branch for
                        // daemon-backed projects too.
                        review_base: g.review_base.clone(),
                        default_branch: g.default_branch.clone(),
                    })
            })
    }

    fn render_header(&self, project: &ProjectData, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.clone();
        let focus_manager = self.focus_manager.clone();
        let focus_manager_for_hide = self.focus_manager.clone();
        let workspace_for_hide = self.workspace.clone();
        let project_id = self.project_id.clone();
        let project_id_for_hide = self.project_id.clone();
        let window_id_for_hide = self.window_id;
        let effective_color = self.workspace.read(cx).effective_folder_color(project);
        let folder_color = t.get_folder_color(effective_color);
        let app_settings = crate::settings::settings(cx);
        let density = app_settings.header_density;
        // In the rows layout each project is short, so vertical space is
        // precious: collapse the comfortable two-row header back to a single
        // row (git info still shows, just inline) when the grid is stacked.
        let is_rows = self
            .workspace
            .read(cx)
            .project_layout_mode(self.window_id)
            .is_rows();
        let is_comfortable =
            density == crate::workspace::settings::HeaderDensity::Comfortable && !is_rows;

        // Reading the focus state clones a String + Vec, so stay behind the
        // opt-in rather than paying for it on every repaint by default.
        let focused_terminal_target =
            app_settings
                .auto_hide_single_terminal_header
                .then(|| {
                    let focus_manager = self.focus_manager.read(cx);
                    let focused = focus_manager.focused_terminal_state();
                    focused_terminal_header_target(
                        &self.project_id,
                        project.layout.as_ref(),
                        focused.as_ref(),
                        true,
                        focus_manager.has_fullscreen(),
                    )
                })
                .flatten();

        // Fetch git status once for both header badge and git status area.
        // Goes through resolve_git_status so the CI popover (below) sees the
        // same source — watcher locally, remote snapshot in daemon-client mode.
        let git_status = self.resolve_git_status(cx);

        // Worktree indicator: filled dot for normal project, ring for worktree.
        let worktree_dot = if project.worktree_info.is_some() {
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
        };

        let project_name_el = {
            let display_name = project_header_display_name(project);
            let path_for_tooltip = project.path.clone();
            let project_id_for_click = self.project_id.clone();
            let request_broker_for_click = self.request_broker.clone();
            div()
                .id("project-name")
                .flex_shrink_0()
                .text_size(ui_text_md(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(t.text_primary))
                .line_height(px(14.0))
                .text_ellipsis()
                .cursor_pointer()
                .rounded(px(3.0))
                .px(px(2.0))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .on_click(move |_, _, cx| {
                    request_broker_for_click.update(cx, |broker, cx| {
                        broker.push_overlay_request(
                            OverlayRequest::Project(ProjectOverlay {
                                project_id: project_id_for_click.clone(),
                                kind: ProjectOverlayKind::FileBrowser,
                            }),
                            cx,
                        );
                    });
                })
                .tooltip(move |_window, cx| {
                    Tooltip::new(path_for_tooltip.clone()).build(_window, cx)
                })
                .child(display_name)
                .into_any_element()
        };

        // The focused project's column is shown even while the project is
        // hidden from the overview (see compute_visible_projects focus
        // override). In that state this button un-hides, so its icon and
        // tooltip must reflect the real hidden state rather than always
        // reading "Hide Project".
        let is_hidden = self
            .workspace
            .read(cx)
            .is_project_hidden(self.window_id, &self.project_id);
        let (vis_icon, vis_tooltip): (&'static str, &'static str) = if is_hidden {
            ("icons/eye.svg", "Show Project")
        } else {
            ("icons/eye-off.svg", "Hide Project")
        };

        // Whether this column is the currently focused (zoomed) project. When
        // it is, the header controls stay pinned (no hover-reveal) and the
        // focus button toggles focus back off, returning to the overview.
        let is_focused_view =
            self.focus_manager.read(cx).focused_project_id() == Some(&self.project_id);
        let (focus_icon, focus_tooltip): (&'static str, &'static str) = if is_focused_view {
            ("icons/fullscreen-exit.svg", "Exit Focus")
        } else {
            ("icons/fullscreen.svg", "Focus Project")
        };

        // Reveal the hide/focus controls only while the header is hovered (or
        // always, in the focused view). Tracked via `header_hovered` state +
        // conditional rendering rather than a `group_hover` style: the hidden
        // controls must take no layout space, and toggling `display` on hover
        // crashes GPUI (prepaint and paint can see different hover states).
        let show_reveal = is_focused_view || self.header_hovered;

        let has_git = git_status
            .as_ref()
            .and_then(|g| g.branch.as_ref())
            .is_some();

        let reveal_controls: Option<AnyElement> = show_reveal.then(|| {
            h_flex()
                .gap(px(2.0))
                .child(
                    div()
                        .id("hide-project-btn")
                        .cursor_pointer()
                        // Uniform horizontal padding (not a fixed width)
                        // so every header button sits 5px from its
                        // neighbours regardless of glyph size — a small
                        // dot no longer floats in a wide box.
                        .px(px(5.0))
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
                            focus_manager_for_hide.update(cx, |fm, cx| {
                                workspace_for_hide.update(cx, |ws, cx| {
                                    ws.toggle_project_overview_visibility(
                                        fm,
                                        window_id_for_hide,
                                        &project_id_for_hide,
                                        cx,
                                    );
                                });
                            });
                        })
                        .child(
                            svg()
                                .path(vis_icon)
                                .size(px(14.0))
                                .text_color(rgb(t.text_secondary)),
                        )
                        .tooltip(move |_window, cx| Tooltip::new(vis_tooltip).build(_window, cx)),
                )
                .child(
                    div()
                        .id("fullscreen-project-btn")
                        .cursor_pointer()
                        .px(px(5.0))
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
                            let pid = project_id.clone();
                            focus_manager.update(cx, |fm, cx| {
                                workspace.update(cx, |ws, cx| {
                                    // Toggle: when already focused, clear
                                    // focus to return to the overview.
                                    let target = if is_focused_view { None } else { Some(pid) };
                                    ws.set_focused_project(fm, target, cx);
                                });
                                cx.notify();
                            });
                        })
                        .child(
                            svg()
                                .path(focus_icon)
                                .size(px(14.0))
                                .text_color(rgb(t.text_secondary)),
                        )
                        .tooltip(move |_window, cx| Tooltip::new(focus_tooltip).build(_window, cx)),
                )
                .into_any_element()
        });

        // In the compact row the base-compare chip is pinned to the right edge
        // of the git status area, so growing the button cluster on its right
        // shoved the chip sideways on every hover. Hand the buttons to the git
        // status row instead: they land left of the chip and the flex spacer
        // absorbs their width. The comfortable layout keeps them in the header
        // row — there the chip lives on a row of its own.
        let (inline_reveal, header_reveal) = if !is_comfortable && has_git {
            (reveal_controls, None)
        } else {
            (None, reveal_controls)
        };

        let git_status_el = self.git_header.update(cx, |gh, cx| {
            gh.render_git_status(git_status.clone(), inline_reveal, &t, cx)
        });

        let terminal_action_controls = focused_terminal_target.map(|target| {
            okena_views_terminal::layout::layout_container::terminal_actions_button(
                okena_ui::header_buttons::HeaderAction::TerminalActions,
                &format!("terminal-actions-{:?}", target.layout_path),
                self.project_id.clone(),
                self.request_broker.clone(),
                target.layout_path,
                Some(target.terminal_id),
                self.backend.supports_buffer_capture(),
                true,
                cx,
            )
            .into_any_element()
        });

        let right_controls = h_flex()
            .gap(px(8.0))
            .child(self.render_hidden_taskbar(project, t, cx))
            .child(
                h_flex()
                    .gap(px(2.0))
                    .when_some(header_reveal, |d, controls| d.child(controls))
                    .child({
                        self.hook_panel
                            .update(cx, |hp, cx| hp.render_hook_indicator(&t, cx))
                    })
                    .child({
                        self.service_panel
                            .update(cx, |sp, cx| sp.render_service_indicator(&t, cx))
                    })
                    .when_some(terminal_action_controls, |d, controls| {
                        d.child(
                            h_flex()
                                .ml(px(4.0))
                                .pl(px(6.0))
                                .border_l_1()
                                .border_color(rgb(t.border))
                                .child(controls),
                        )
                    }),
            );

        let context_menu_handler = {
            let request_broker = self.request_broker.clone();
            let project_id = self.project_id.clone();
            move |event: &MouseDownEvent, _window: &mut Window, cx: &mut App| {
                cx.stop_propagation();
                request_broker.update(cx, |broker, cx| {
                    broker.push_overlay_request(
                        OverlayRequest::Project(ProjectOverlay {
                            project_id: project_id.clone(),
                            kind: ProjectOverlayKind::ContextMenu {
                                position: event.position,
                            },
                        }),
                        cx,
                    );
                });
            }
        };

        let header_body = if is_comfortable && has_git {
            // Two-row comfortable layout.
            v_flex()
                .id("project-header")
                .group("project-header")
                .px(px(12.0))
                .py(px(4.0))
                .gap(px(2.0))
                .bg(rgb(t.bg_header))
                .border_b_1()
                .border_color(rgb(t.border))
                .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                    if this.header_hovered != *hovered {
                        this.header_hovered = *hovered;
                        cx.notify();
                    }
                }))
                .on_mouse_down(MouseButton::Right, context_menu_handler)
                // Row 1: name + right controls
                .child(
                    h_flex()
                        .h(px(22.0))
                        .items_center()
                        .justify_between()
                        .child(
                            h_flex()
                                .gap(px(6.0))
                                .overflow_hidden()
                                .child(worktree_dot)
                                .child(project_name_el),
                        )
                        .child(right_controls),
                )
                // Row 2: full git info row
                .child(
                    h_flex()
                        .h(px(18.0))
                        .pl(px(14.0))
                        .items_center()
                        .child(git_status_el),
                )
                .into_any_element()
        } else {
            // Compact single-row layout (current default).
            div()
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
                .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                    if this.header_hovered != *hovered {
                        this.header_hovered = *hovered;
                        cx.notify();
                    }
                }))
                .on_mouse_down(MouseButton::Right, context_menu_handler)
                .child(
                    h_flex()
                        // Fill the row so the git status row can right-align its
                        // base-compare chip via its internal flex spacer.
                        .flex_1()
                        .min_w_0()
                        .gap(px(6.0))
                        .overflow_hidden()
                        .child(worktree_dot)
                        .child(project_name_el)
                        .child(git_status_el),
                )
                .child(right_controls)
                .into_any_element()
        };

        v_flex()
            // Colored accent bar
            .child(
                div()
                    .h(px(1.0))
                    .w_full()
                    .flex_shrink_0()
                    .bg(rgb(folder_color)),
            )
            .child(header_body)
    }

    /// Render the closing state shown while a worktree is being torn down.
    fn render_closing_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .text_color(rgb(t.text_muted)),
            )
            .child(
                div()
                    .text_size(ui_text_xl(cx))
                    .text_color(rgb(t.text_secondary))
                    .child("Closing worktree\u{2026}"),
            )
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .max_w(px(240.0))
                    .text_center()
                    // Deliberately not "removing the checkout": the before_remove
                    // hook phase reaches this screen too, and that phase is still
                    // abortable — nothing has been deleted yet.
                    .child("This project disappears once the worktree is gone."),
            )
    }

    /// Placeholder shown while the daemon is still materializing the project's
    /// directory — a worktree checkout, or a clone of a remote repository.
    fn render_creating_state(
        &self,
        is_worktree: bool,
        progress: Option<&str>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let (icon, title, detail) = if is_worktree {
            (
                "icons/git-branch.svg",
                "Setting up worktree\u{2026}",
                "Fetching latest changes and creating the branch. Terminals will start automatically.",
            )
        } else {
            (
                "icons/refresh.svg",
                "Cloning repository\u{2026}",
                "Fetching the repository. Terminals will start automatically once the clone finishes.",
            )
        };
        v_flex()
            .items_center()
            .justify_center()
            .size_full()
            .gap(px(12.0))
            .bg(rgb(t.bg_primary))
            .child(
                svg()
                    .path(icon)
                    .size(px(48.0))
                    .text_color(rgb(t.text_muted)),
            )
            .child(
                div()
                    .text_size(ui_text_xl(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(title),
            )
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .max_w(px(240.0))
                    .text_center()
                    .child(detail),
            )
            // Only a clone reports progress; a worktree checkout is local and
            // usually over before a percentage would be readable.
            .when_some(progress, |d, progress: &str| {
                d.child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_secondary))
                        .child(progress.to_string()),
                )
            })
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
                svg()
                    .path("icons/folder.svg")
                    .size(px(48.0))
                    .text_color(rgb(t.text_muted)),
            )
            .child(
                div()
                    .text_size(ui_text_xl(cx))
                    .text_color(rgb(t.text_muted))
                    .child("No terminal attached"),
            )
            .child(
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
                            .text_color(rgb(t.button_primary_fg)),
                    )
                    .child(
                        div()
                            .text_size(ui_text_md(cx))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(t.button_primary_fg))
                            .child("Start Terminal"),
                    )
                    .on_click({
                        let dispatcher = self.action_dispatcher.clone();
                        move |_, _window, cx| {
                            if let Some(ref dispatcher) = dispatcher {
                                dispatcher.dispatch(
                                    ActionRequest::CreateTerminal {
                                        project_id: project_id.clone(),
                                    },
                                    cx,
                                );
                            }
                        }
                    }),
            )
            .child(self.render_tip(cx))
    }

    /// Render a single rotating discoverability tip below the empty-state CTA.
    fn render_tip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let tip = crate::views::tips::tip_at(self.tip_index);
        // Live keybinding chip when the tip maps to an action; otherwise the
        // static trigger hint. `None` => no chip at all.
        let chip = tip
            .action
            .and_then(crate::views::tips::shortcut_for_action)
            .or_else(|| tip.hint.map(str::to_string));

        v_flex()
            .items_center()
            .gap(px(10.0))
            .mt(px(20.0))
            .max_w(px(340.0))
            .px(px(20.0))
            .py(px(16.0))
            .rounded(px(10.0))
            .bg(rgb(t.bg_secondary))
            .border_1()
            .border_color(rgb(t.border))
            .child(
                h_flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        svg()
                            .path("icons/lightbulb.svg")
                            .size(px(13.0))
                            .text_color(rgb(t.button_primary_bg)),
                    )
                    .child(
                        div()
                            .text_size(ui_text_sm(cx))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(t.button_primary_bg))
                            .child("TIP"),
                    ),
            )
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .text_center()
                    .child(tip.text),
            )
            .children(chip.map(|c| {
                div()
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(5.0))
                    .bg(rgb(t.bg_primary))
                    .border_1()
                    .border_color(rgb(t.border))
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(c)
            }))
            .child(
                h_flex()
                    .id("next-tip-btn")
                    .cursor_pointer()
                    .items_center()
                    .gap(px(4.0))
                    .mt(px(2.0))
                    .text_color(rgb(t.text_muted))
                    .hover(|s| s.text_color(rgb(t.text_secondary)))
                    .child(
                        svg()
                            .path("icons/refresh.svg")
                            .size(px(11.0))
                            .text_color(rgb(t.text_muted)),
                    )
                    .child(div().text_size(ui_text_sm(cx)).child("Another tip"))
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.tip_index = this.tip_index.wrapping_add(1);
                        cx.notify();
                    })),
            )
    }
}

impl Render for ProjectColumn {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.read(cx);
        let project = self.get_project(workspace).cloned();

        match project {
            Some(project) => {
                // Daemon-authoritative flag mirrored over the wire, OR'd with the
                // initiating client's optimistic tracker — same pair the sidebar
                // row reads for its "Closing…" label.
                let is_closing = project.is_closing || workspace.is_project_closing(&project.id);
                let content_kind = column_content(&project, is_closing);

                // Soft tinted background based on folder color (when enabled)
                let bg_color = if crate::settings::settings(cx).color_tinted_background {
                    let color = workspace.effective_folder_color(&project);
                    if color != crate::theme::FolderColor::Default {
                        rgb(crate::ui::tint_color(
                            t.bg_primary,
                            t.get_folder_color(color),
                            0.025,
                        ))
                    } else {
                        rgb(t.bg_primary)
                    }
                } else {
                    rgb(t.bg_primary)
                };

                // Content: layout, closing/creating placeholder, or empty bookmark state
                let content = match content_kind {
                    ColumnContent::Layout => {
                        self.ensure_layout_container(project.path.clone(), cx);

                        div()
                            .id("project-column-content")
                            .flex_1()
                            .min_h_0()
                            .overflow_hidden()
                            .when_some(self.layout_container.clone(), |d, container| {
                                d.child(
                                    AnyView::from(container)
                                        .cached(StyleRefinement::default().size_full()),
                                )
                            })
                            .into_any_element()
                    }
                    ColumnContent::Closing => self.render_closing_state(cx).into_any_element(),
                    ColumnContent::Creating => self
                        .render_creating_state(
                            project.worktree_info.is_some(),
                            project.creating_progress.as_deref(),
                            cx,
                        )
                        .into_any_element(),
                    ColumnContent::Empty => self.render_empty_state(cx).into_any_element(),
                };

                // Get current branch for commit log popover and update git header.
                // Same source as the badge (resolve_git_status) so the branch is
                // present in daemon-client mode too, where git_watcher is None.
                let current_branch = self.resolve_git_status(cx).and_then(|s| s.branch);
                self.git_header.update(cx, |gh, _cx| {
                    gh.set_current_branch(current_branch.clone());
                });

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
                    // Hook panel (delegated to HookPanel entity)
                    .child(self.hook_panel.update(cx, |hp, cx| hp.render_panel(&t, cx)))
                    // Service panel (delegated to ServicePanel entity)
                    .child({
                        self.service_panel
                            .update(cx, |sp, cx| sp.render_panel(&t, cx))
                    })
                    // Diff popover (delegated to GitHeader entity)
                    .child({
                        self.git_header
                            .update(cx, |gh, cx| gh.render_diff_popover(&t, cx))
                    })
                    // Commit log popover (delegated to GitHeader entity)
                    .child({
                        self.git_header.update(cx, |gh, cx| {
                            gh.render_commit_log_popover(current_branch, &t, cx)
                        })
                    })
                    // Branch picker popover (delegated to GitHeader entity)
                    .child({
                        self.git_header
                            .update(cx, |gh, cx| gh.render_branch_picker(window, &t, cx))
                    })
                    // CI checks popover (delegated to GitHeader entity).
                    // Resolve via the same path as the badge: in daemon-client
                    // mode git_watcher is None, so the watcher-only fetch left
                    // ci_checks empty and the popover rendered nothing (the pill
                    // toggled but never opened). Fall back to the remote snapshot.
                    .child({
                        let git_status = self.resolve_git_status(cx);
                        let ci_checks = git_status.as_ref().and_then(|g| g.ci_checks.clone());
                        let pr_info = git_status.and_then(|g| g.pr_info);
                        self.git_header.update(cx, |gh, cx| {
                            gh.render_ci_checks_popover(
                                ci_checks.as_ref(),
                                pr_info.as_ref(),
                                &t,
                                cx,
                            )
                        })
                    })
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

#[cfg(test)]
mod tests {
    use super::{
        ColumnContent, FocusedTerminalHeaderTarget, column_content, focused_terminal_header_target,
        project_header_display_name,
    };
    use crate::workspace::settings::HooksConfig;
    use crate::workspace::state::{
        FocusedTerminalState, LayoutNode, ProjectData, SplitDirection, WorktreeMetadata,
    };
    use okena_core::theme::FolderColor;
    use std::collections::HashMap;

    fn project_with_name(name: &str) -> ProjectData {
        ProjectData {
            id: "p1".to_string(),
            name: name.to_string(),
            path: "/tmp/repo-worktree".to_string(),
            layout: None,
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            default_shell: None,
            hook_terminals: HashMap::new(),
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
            creating_progress: None,
        }
    }

    /// A layout tree in the exact shape `prepare_background_worktree_removal`
    /// leaves behind: structure intact, every leaf's terminal id nulled.
    fn split_layout(terminal_ids: [Option<&str>; 2]) -> LayoutNode {
        let children = terminal_ids
            .into_iter()
            .map(|id| {
                let mut node = LayoutNode::new_terminal();
                if let LayoutNode::Terminal { terminal_id, .. } = &mut node {
                    *terminal_id = id.map(str::to_string);
                }
                node
            })
            .collect();

        LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            sizes: vec![0.5, 0.5],
            children,
        }
    }

    fn terminal_layout(terminal_id: &str) -> LayoutNode {
        let mut node = LayoutNode::new_terminal();
        if let LayoutNode::Terminal {
            terminal_id: slot, ..
        } = &mut node
        {
            *slot = Some(terminal_id.to_string());
        }
        node
    }

    #[test]
    fn hidden_standalone_header_actions_target_the_focused_terminal() {
        let layout = split_layout([Some("t1"), Some("t2")]);
        let focused = FocusedTerminalState {
            project_id: "p1".to_string(),
            layout_path: vec![1],
        };

        assert_eq!(
            focused_terminal_header_target("p1", Some(&layout), Some(&focused), true, false,),
            Some(FocusedTerminalHeaderTarget {
                terminal_id: "t2".to_string(),
                layout_path: vec![1],
            }),
        );
    }

    #[test]
    fn single_standalone_terminal_is_an_implicit_header_target() {
        let layout = terminal_layout("t1");
        let focused_other_project = FocusedTerminalState {
            project_id: "p2".to_string(),
            layout_path: vec![],
        };
        let expected = Some(FocusedTerminalHeaderTarget {
            terminal_id: "t1".to_string(),
            layout_path: vec![],
        });

        assert_eq!(
            focused_terminal_header_target("p1", Some(&layout), None, true, false),
            expected,
        );
        assert_eq!(
            focused_terminal_header_target(
                "p1",
                Some(&layout),
                Some(&focused_other_project),
                true,
                false,
            ),
            expected,
        );
    }

    #[test]
    fn implicit_header_target_does_not_duplicate_a_single_tab_header() {
        let tabs = LayoutNode::Tabs {
            children: vec![terminal_layout("t1")],
            active_tab: 0,
        };

        assert_eq!(
            focused_terminal_header_target("p1", Some(&tabs), None, true, false),
            None,
        );
    }

    #[test]
    fn project_header_does_not_duplicate_visible_terminal_actions() {
        let tabs = LayoutNode::Tabs {
            children: vec![terminal_layout("t1"), terminal_layout("t2")],
            active_tab: 1,
        };
        let focused_tab = FocusedTerminalState {
            project_id: "p1".to_string(),
            layout_path: vec![1],
        };
        let focused_other_project = FocusedTerminalState {
            project_id: "p2".to_string(),
            layout_path: vec![1],
        };

        assert_eq!(
            focused_terminal_header_target("p1", Some(&tabs), Some(&focused_tab), false, false,),
            None,
            "the opt-in must remain off by default",
        );
        assert_eq!(
            focused_terminal_header_target("p1", Some(&tabs), Some(&focused_tab), true, false,),
            None,
            "a tab group already has a visible action bar",
        );
        assert_eq!(
            focused_terminal_header_target(
                "p1",
                Some(&tabs),
                Some(&focused_other_project),
                true,
                false,
            ),
            None,
            "one project's header must not control another project's terminal",
        );
        assert_eq!(
            focused_terminal_header_target("p1", Some(&tabs), Some(&focused_tab), true, true,),
            None,
            "fullscreen has its own zoom header",
        );
    }

    #[test]
    fn closing_worktree_with_nulled_terminal_ids_shows_closing_state() {
        let mut project = project_with_name("feature-login");
        project.layout = Some(split_layout([None, None]));
        project.is_closing = true;

        assert_eq!(
            column_content(&project, true),
            ColumnContent::Closing,
            "id-less panes mid-teardown must not fall through to \"Starting terminal…\"",
        );
    }

    #[test]
    fn closing_worktree_keeps_painting_live_terminals() {
        let mut project = project_with_name("feature-login");
        project.layout = Some(split_layout([Some("t1"), None]));
        project.is_closing = true;

        assert_eq!(
            column_content(&project, true),
            ColumnContent::Layout,
            "merge and before_remove-hook phases keep the terminals alive and visible",
        );
    }

    #[test]
    fn closing_bookmark_without_layout_shows_closing_not_empty_state() {
        let mut project = project_with_name("feature-login");
        project.is_closing = true;

        assert_eq!(
            column_content(&project, true),
            ColumnContent::Closing,
            "a closing project must not offer a Start Terminal button",
        );
    }

    #[test]
    fn optimistic_client_closing_flag_alone_triggers_closing_state() {
        let project = project_with_name("feature-login");

        assert_eq!(
            column_content(&project, true),
            ColumnContent::Closing,
            "the initiating client marks closing in its tracker before the mirror catches up",
        );
    }

    #[test]
    fn non_closing_project_branches_are_unchanged() {
        let mut project = project_with_name("feature-login");
        assert_eq!(column_content(&project, false), ColumnContent::Empty);

        project.is_creating = true;
        assert_eq!(column_content(&project, false), ColumnContent::Creating);

        project.layout = Some(split_layout([None, None]));
        assert_eq!(
            column_content(&project, false),
            ColumnContent::Layout,
            "a fresh worktree's uninitialized slots still render panes, not a placeholder",
        );
    }

    #[test]
    fn project_header_uses_worktree_project_name() {
        let mut project = project_with_name("feature-login");
        project.worktree_info = Some(WorktreeMetadata {
            parent_project_id: "parent".to_string(),
            color_override: None,
            main_repo_path: "/tmp/repo".to_string(),
            worktree_path: "/tmp/repo-worktree".to_string(),
            branch_name: "feature/login".to_string(),
        });

        assert_eq!(project_header_display_name(&project), "feature-login");
    }
}
