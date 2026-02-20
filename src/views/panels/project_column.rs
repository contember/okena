use crate::git::{self, FileDiffSummary};
use crate::git::watcher::GitStatusWatcher;
use crate::action_dispatch::ActionDispatcher;
use crate::terminal::backend::TerminalBackend;
use crate::theme::{theme, ThemeColors};
use crate::views::layout::layout_container::LayoutContainer;
use crate::views::root::TerminalsRegistry;
use crate::views::layout::split_pane::ActiveDrag;
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

/// Delay before showing diff summary popover (ms)
const HOVER_DELAY_MS: u64 = 400;

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
    layout_container: Option<Entity<LayoutContainer>>,
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
        }
    }

    /// Set the action dispatcher (used for remote projects).
    pub fn set_action_dispatcher(&mut self, dispatcher: Option<ActionDispatcher>) {
        self.action_dispatcher = dispatcher;
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

    fn render_diff_popover(&self, t: &ThemeColors, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.diff_popover_visible || self.diff_file_summaries.is_empty() {
            return div().absolute().size_0().into_any_element();
        }

        let max_files = 15;
        let total_files = self.diff_file_summaries.len();
        let show_more = total_files > max_files;
        let project_path = self.diff_popover_project_path.clone();

        div()
            .id("diff-summary-popover")
            .absolute()
            .top(px(30.0))
            .left(px(12.0))
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
            .children(
                self.diff_file_summaries
                    .iter()
                    .take(max_files)
                    .enumerate()
                    .map(|(idx, summary)| {
                        let filename = summary.path.rsplit('/').next().unwrap_or(&summary.path);
                        let dir = if summary.path.contains('/') {
                            let parts: Vec<&str> = summary.path.rsplitn(2, '/').collect();
                            if parts.len() > 1 { Some(parts[1]) } else { None }
                        } else {
                            None
                        };
                        let is_new = summary.is_new;
                        let added = summary.added;
                        let removed = summary.removed;
                        let file_path = summary.path.clone();
                        let project_path_for_click = project_path.clone();

                        div()
                            .id(ElementId::Name(format!("diff-file-{}", idx).into()))
                            .px(px(10.0))
                            .py(px(4.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .rounded(px(4.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.0))
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.hide_diff_popover(cx);
                                this.request_broker.update(cx, |broker, cx| {
                                    broker.push_overlay_request(OverlayRequest::DiffViewer {
                                        path: project_path_for_click.clone(),
                                        file: Some(file_path.clone()),
                                    }, cx);
                                });
                            }))
                            .child(
                                v_flex()
                                    .overflow_hidden()
                                    .child(
                                        h_flex()
                                            .gap(px(4.0))
                                            .child(
                                                div()
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(t.text_primary))
                                                    .text_ellipsis()
                                                    .child(filename.to_string()),
                                            )
                                            .when(is_new, |d| {
                                                d.child(
                                                    div()
                                                        .px(px(4.0))
                                                        .py(px(1.0))
                                                        .rounded(px(2.0))
                                                        .bg(rgb(t.term_green))
                                                        .text_size(px(8.0))
                                                        .text_color(rgb(0x000000))
                                                        .child("new"),
                                                )
                                            }),
                                    )
                                    .when_some(dir, |d, dir| {
                                        d.child(
                                            div()
                                                .text_size(px(9.0))
                                                .text_color(rgb(t.text_muted))
                                                .text_ellipsis()
                                                .child(dir.to_string()),
                                        )
                                    }),
                            )
                            .child(
                                h_flex()
                                    .gap(px(6.0))
                                    .flex_shrink_0()
                                    .text_size(px(10.0))
                                    .when(added > 0, |d| {
                                        d.child(
                                            div()
                                                .text_color(rgb(t.term_green))
                                                .child(format!("+{}", added)),
                                        )
                                    })
                                    .when(removed > 0, |d| {
                                        d.child(
                                            div()
                                                .text_color(rgb(t.term_red))
                                                .child(format!("-{}", removed)),
                                        )
                                    }),
                            )
                    }),
            )
            .when(show_more, |d: Stateful<Div>| {
                d.child(
                    div()
                        .px(px(10.0))
                        .py(px(4.0))
                        .text_size(px(10.0))
                        .text_color(rgb(t.text_muted))
                        .child(format!("... and {} more files", total_files - max_files)),
                )
            })
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

    fn render_git_status(&self, project: &ProjectData, t: ThemeColors, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.git_watcher.as_ref()
            .and_then(|w| w.read(cx).get(&self.project_id).cloned());
        let is_worktree = project.worktree_info.is_some();
        let main_repo_path = project.worktree_info.as_ref()
            .map(|w| w.main_repo_path.clone())
            .unwrap_or_default();

        match status {
            Some(status) if status.branch.is_some() => {
                let has_changes = status.has_changes();
                let lines_added = status.lines_added;
                let lines_removed = status.lines_removed;
                let project_id = self.project_id.clone();
                let project_path = project.path.clone();
                let project_path_for_hover = project.path.clone();

                h_flex()
                    .flex_shrink_0()
                    .gap(px(6.0))
                    .text_size(px(10.0))
                    .line_height(px(12.0))
                    // Worktree indicator
                    .when(is_worktree, |d| {
                        let tooltip_text = format!("Worktree of {}", main_repo_path);
                        d.child(
                            div()
                                .id("worktree-indicator")
                                .px(px(4.0))
                                .py(px(1.0))
                                .rounded(px(3.0))
                                .bg(rgb(t.border_active))
                                .text_size(px(9.0))
                                .text_color(rgb(t.bg_primary))
                                .child("WT")
                                .tooltip(move |_window, cx| Tooltip::new(tooltip_text.clone()).build(_window, cx))
                        )
                    })
                    // Branch name
                    .child(
                        h_flex()
                            .gap(px(3.0))
                            .child(
                                svg()
                                    .path("icons/git-branch.svg")
                                    .size(px(10.0))
                                    .text_color(rgb(t.text_muted))
                            )
                            .child(
                                div()
                                    .text_color(rgb(t.text_secondary))
                                    .max_w(px(100.0))
                                    .text_ellipsis()
                                    .overflow_hidden()
                                    .child(status.branch.clone().unwrap_or_default())
                            )
                    )
                    // Diff stats (clickable, only if there are changes)
                    .when(has_changes, |d: Div| {
                        d.child(
                            div()
                                .id(ElementId::Name(format!("git-diff-stats-{}", project_id).into()))
                                .relative()
                                .flex()
                                .items_center()
                                .gap(px(3.0))
                                .cursor_pointer()
                                .px(px(4.0))
                                .py(px(1.0))
                                .rounded(px(3.0))
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
                                    this.request_broker.update(cx, |broker, cx| {
                                        broker.push_overlay_request(OverlayRequest::DiffViewer {
                                            path: project_path.clone(),
                                            file: None,
                                        }, cx);
                                    });
                                }))
                                .child(
                                    div()
                                        .text_color(rgb(t.term_green))
                                        .child(format!("+{}", lines_added))
                                )
                                .child(
                                    div()
                                        .text_color(rgb(t.text_muted))
                                        .child("/")
                                )
                                .child(
                                    div()
                                        .text_color(rgb(t.term_red))
                                        .child(format!("-{}", lines_removed))
                                )
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
        let folder_color = t.get_folder_color(project.folder_color);
        let is_remote = project.is_remote;

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
                        div()
                            .flex_shrink_0()
                            .w(px(8.0))
                            .h(px(8.0))
                            .rounded(px(4.0))
                            .bg(rgb(folder_color))
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(t.text_primary))
                            .line_height(px(14.0))
                            .text_ellipsis()
                            .child(project.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgb(t.text_muted))
                            .line_height(px(12.0))
                            .text_ellipsis()
                            .overflow_hidden()
                            .child(project.path.clone()),
                    )
                    .when(!is_remote, |d| d.child(self.render_git_status(project, t, cx))),
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
                            .when(!is_remote, |d| d.child(
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
                                            ws.toggle_project_visibility(&project_id_for_hide, cx);
                                        });
                                    })
                                    .child(
                                        svg()
                                            .path("icons/eye-off.svg")
                                            .size(px(14.0))
                                            .text_color(rgb(t.text_secondary))
                                    )
                                    .tooltip(|_window, cx| Tooltip::new("Hide Project").build(_window, cx)),
                            ))
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
                    ),
            ))
    }

    /// Render empty state for bookmark projects (no terminal)
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

impl Render for ProjectColumn {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.read(cx);
        let project = self.get_project(workspace).cloned();

        match project {
            Some(project) => {
                let has_layout = project.layout.is_some();

                // Content: either layout container or empty state
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
                    .bg(rgb(t.bg_primary))
                    .child(self.render_header(&project, cx))
                    .child(content)
                    .child(self.render_diff_popover(&t, cx))
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
