use crate::git::{self, PrStateColor, CiStatusColor};
use crate::git::watcher::GitStatusWatcher;
use crate::action_dispatch::ActionDispatcher;
use okena_views_git::git_header::GitHeader;
use crate::services::manager::ServiceManager;
use crate::terminal::backend::TerminalBackend;
use crate::theme::{theme, ThemeColors};
use crate::views::layout::layout_container::LayoutContainer;
use crate::views::layout::split_pane::ActiveDrag;
use crate::workspace::request_broker::RequestBroker;
use crate::workspace::state::{ProjectData, Workspace};
use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use gpui_component::{h_flex, v_flex};
use std::sync::Arc;

use okena_core::api::ActionRequest;
use okena_views_services::service_panel::ServicePanel;
use crate::views::root::TerminalsRegistry;

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

        let git_header = {
            let pid = project_id.clone();
            let rb = request_broker.clone();
            cx.new(move |cx| GitHeader::new(pid, rb, cx))
        };
        // Observe git_header so ProjectColumn re-renders when popovers change
        cx.observe(&git_header, |_, _, cx| cx.notify()).detach();

        let service_panel = {
            let pid = project_id.clone();
            let ws = workspace.clone();
            let rb = request_broker.clone();
            let be = backend.clone();
            let ts = terminals.clone();
            let ad = active_drag.clone();
            cx.new(move |cx| {
                ServicePanel::new(pid, ws, rb, be, ts, ad, initial_service_height, cx)
            })
        };
        // Observe service_panel so ProjectColumn re-renders when panel state changes
        cx.observe(&service_panel, |_, _, cx| cx.notify()).detach();

        Self {
            workspace,
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
        }
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

    /// Set the service manager and observe it for changes.
    pub fn set_service_manager(&mut self, manager: Entity<ServiceManager>, cx: &mut Context<Self>) {
        // Also update the action dispatcher so it can route service actions locally
        if let Some(ActionDispatcher::Local { ref mut service_manager, .. }) = self.action_dispatcher {
            *service_manager = Some(manager.clone());
        }
        // Sync dispatcher to service panel (may have been set before panel was created)
        self.sync_service_panel_dispatcher(cx);
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

    /// Observe workspace for remote service state changes (used for remote project columns).
    pub fn observe_remote_services(&mut self, workspace: Entity<Workspace>, cx: &mut Context<Self>) {
        // Sync dispatcher to service panel (may have been set before panel was created)
        self.sync_service_panel_dispatcher(cx);
        self.service_panel.update(cx, |sp, cx| {
            sp.observe_remote_services(workspace, cx);
        });
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
                        .child(format!("\u{2197} {}", terminal_name))
                        .on_click(move |_, _window, cx| {
                            workspace.update(cx, |ws, cx| {
                                ws.attach_terminal(&terminal_id_for_click, cx);
                            });
                        })
                })
            )
            .into_any_element()
    }

    fn render_header(&self, project: &ProjectData, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.clone();
        let workspace_for_hide = self.workspace.clone();
        let project_id = self.project_id.clone();
        let project_id_for_hide = self.project_id.clone();
        let effective_color = self.workspace.read(cx).effective_folder_color(project);
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

        let is_worktree = project.worktree_info.is_some();
        let project_path = project.path.clone();

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
                    // Git status (delegated to GitHeader entity)
                    .child({
                        self.git_header.update(cx, |gh, cx| {
                            gh.render_git_status(
                                &project_path,
                                is_worktree,
                                git_status,
                                &t,
                                cx,
                            )
                        })
                    }),
            )
            .child(
                // Right side: minimized taskbar + controls
                h_flex()
                    .gap(px(8.0))
                    .child(self.render_hidden_taskbar(project, t))
                    .child(
                        div()
                            .flex()
                            .gap(px(2.0))
                            .opacity(0.0)
                            .group_hover("project-header", |s| s.opacity(1.0))
                            .child(
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
                    // Service indicator (delegated to ServicePanel entity)
                    .child({
                        self.service_panel.update(cx, |sp, cx| {
                            sp.render_service_indicator(&t, cx)
                        })
                    }),
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
                                    ActionRequest::CreateTerminal {
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
                    self.ensure_layout_container(project.path.clone(), cx);

                    div()
                        .id("project-column-content")
                        .flex_1()
                        .min_h_0()
                        .overflow_hidden()
                        .child(AnyView::from(self.layout_container.clone().unwrap()).cached(
                            StyleRefinement::default().size_full()
                        ))
                        .into_any_element()
                } else if is_creating {
                    self.render_creating_state(cx).into_any_element()
                } else {
                    self.render_empty_state(cx).into_any_element()
                };

                // Get current branch for commit log popover and update git header
                let current_branch = self.git_watcher.as_ref()
                    .and_then(|w| w.read(cx).get(&self.project_id).cloned())
                    .and_then(|s| s.branch);
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
                    // Service panel (delegated to ServicePanel entity)
                    .child({
                        self.service_panel.update(cx, |sp, cx| {
                            sp.render_panel(&t, cx)
                        })
                    })
                    // Diff popover (delegated to GitHeader entity)
                    .child({
                        self.git_header.update(cx, |gh, cx| {
                            gh.render_diff_popover(&t, cx)
                        })
                    })
                    // Commit log popover (delegated to GitHeader entity)
                    .child({
                        self.git_header.update(cx, |gh, cx| {
                            gh.render_commit_log_popover(current_branch, &t, cx)
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
