use crate::git;
use crate::terminal::pty_manager::PtyManager;
use crate::theme::{theme, ThemeColors};
use crate::views::layout_container::LayoutContainer;
use crate::views::root::TerminalsRegistry;
use crate::workspace::state::{ProjectData, Workspace};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use std::path::Path;
use std::sync::Arc;

/// A single project column with header and layout
pub struct ProjectColumn {
    workspace: Entity<Workspace>,
    project_id: String,
    #[allow(dead_code)]
    pty_manager: Arc<PtyManager>,
    #[allow(dead_code)]
    terminals: TerminalsRegistry,
    /// Stored layout container entity (must be created in new(), not render())
    layout_container: Option<Entity<LayoutContainer>>,
}

impl ProjectColumn {
    pub fn new(
        workspace: Entity<Workspace>,
        project_id: String,
        pty_manager: Arc<PtyManager>,
        terminals: TerminalsRegistry,
    ) -> Self {
        Self {
            workspace,
            project_id,
            pty_manager,
            terminals,
            layout_container: None, // Will be initialized on first render with cx
        }
    }

    fn ensure_layout_container(&mut self, project_path: String, cx: &mut Context<Self>) {
        if self.layout_container.is_none() {
            let workspace = self.workspace.clone();
            let project_id = self.project_id.clone();
            let pty_manager = self.pty_manager.clone();
            let terminals = self.terminals.clone();

            self.layout_container = Some(cx.new(move |_cx| {
                LayoutContainer::new(
                    workspace,
                    project_id,
                    project_path,
                    vec![],
                    pty_manager,
                    terminals,
                )
            }));
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

        div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .px(px(4.0))
            .py(px(2.0))
            .rounded(px(4.0))
            .bg(rgb(t.bg_secondary))
            // Minimized terminals
            .children(
                minimized_terminals.into_iter().map(|(terminal_id, layout_path)| {
                    let workspace = self.workspace.clone();
                    let project_id = self.project_id.clone();

                    // Priority: custom name > OSC title > terminal ID prefix
                    let terminal_name = if let Some(custom_name) = project.terminal_names.get(&terminal_id) {
                        custom_name.clone()
                    } else {
                        // Try to get OSC title from terminal
                        let terminals = self.terminals.lock();
                        if let Some(terminal) = terminals.get(&terminal_id) {
                            terminal.title().unwrap_or_else(|| terminal_id.chars().take(8).collect())
                        } else {
                            terminal_id.chars().take(8).collect()
                        }
                    };

                    div()
                        .id(ElementId::Name(format!("minimized-{}", terminal_id).into()))
                        .cursor_pointer()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(2.0))
                        .bg(rgb(t.bg_hover))
                        .hover(|s| s.bg(rgb(t.border_active)))
                        .text_size(px(10.0))
                        .text_color(rgb(t.text_secondary))
                        .child(terminal_name)
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

                    // Priority: custom name > OSC title > terminal ID prefix
                    let terminal_name = if let Some(custom_name) = project.terminal_names.get(&terminal_id) {
                        custom_name.clone()
                    } else {
                        // Try to get OSC title from terminal
                        let terminals = self.terminals.lock();
                        if let Some(terminal) = terminals.get(&terminal_id) {
                            terminal.title().unwrap_or_else(|| terminal_id.chars().take(8).collect())
                        } else {
                            terminal_id.chars().take(8).collect()
                        }
                    };

                    div()
                        .id(ElementId::Name(format!("detached-{}", terminal_id).into()))
                        .cursor_pointer()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(2.0))
                        .bg(rgb(t.border_active))
                        .hover(|s| s.bg(rgb(0x4a90d9)))
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

    fn render_git_status(&self, project: &ProjectData, t: ThemeColors) -> impl IntoElement {
        let status = git::get_git_status(Path::new(&project.path));
        let is_worktree = project.worktree_info.is_some();
        let main_repo_path = project.worktree_info.as_ref()
            .map(|w| w.main_repo_path.clone())
            .unwrap_or_default();

        match status {
            Some(status) if status.branch.is_some() => {
                div()
                    .flex()
                    .items_center()
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
                                .text_color(rgb(0xffffff))
                                .child("WT")
                                .tooltip(move |_window, cx| Tooltip::new(tooltip_text.clone()).build(_window, cx))
                        )
                    })
                    // Branch name
                    .child(
                        div()
                            .flex()
                            .items_center()
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
                    // Diff stats (only if there are changes)
                    .when(status.has_changes(), |d: Div| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(3.0))
                                .child(
                                    div()
                                        .text_color(rgb(t.term_green))
                                        .child(format!("+{}", status.lines_added))
                                )
                                .child(
                                    div()
                                        .text_color(rgb(t.text_muted))
                                        .child("/")
                                )
                                .child(
                                    div()
                                        .text_color(rgb(t.term_red))
                                        .child(format!("-{}", status.lines_removed))
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

        div()
            .id("project-header")
            .group("project-header")
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
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(t.text_primary))
                            .line_height(px(14.0))
                            .text_ellipsis()
                            .child(project.name.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(rgb(t.text_muted))
                                    .line_height(px(12.0))
                                    .max_w(px(180.0))
                                    .text_ellipsis()
                                    .overflow_hidden()
                                    .child(project.path.clone()),
                            )
                            .child(self.render_git_status(project, t)),
                    ),
            )
            .child(
                // Right side: minimized taskbar + controls
                div()
                    .flex()
                    .items_center()
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
                                            ws.fullscreen_project(project_id.clone(), cx);
                                        });
                                    })
                                    .child(
                                        svg()
                                            .path("icons/fullscreen.svg")
                                            .size(px(14.0))
                                            .text_color(rgb(t.text_secondary))
                                    )
                                    .tooltip(|_window, cx| Tooltip::new("Fullscreen").build(_window, cx)),
                            ),
                    ),
            )
    }

    /// Render empty state for bookmark projects (no terminal)
    fn render_empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.clone();
        let project_id = self.project_id.clone();

        div()
            .flex()
            .flex_col()
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
                    .on_click(move |_, _window, cx| {
                        workspace.update(cx, |ws, cx| {
                            ws.start_terminal(&project_id, cx);
                        });
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
                    .flex()
                    .flex_col()
                    .size_full()
                    .min_h_0()
                    .bg(rgb(t.bg_primary))
                    .child(self.render_header(&project, cx))
                    .child(content)
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
