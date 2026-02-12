//! Project and terminal list rendering for the sidebar

use crate::keybindings::{MinimizeTerminal, ToggleFullscreen};
use crate::theme::theme;
use crate::views::components::is_renaming;
use gpui::*;
use gpui::prelude::*;
use gpui_component::tooltip::Tooltip;

use super::item_widgets::*;
use super::{Sidebar, SidebarProjectInfo, ProjectDrag, ProjectDragView, FolderDrag};
use std::collections::HashMap;

impl Sidebar {
    pub(super) fn render_project_item(&self, project: &SidebarProjectInfo, index: usize, is_cursor: bool, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let is_expanded = self.expanded_projects.contains(&project.id);
        let project_id = project.id.clone();
        let project_name = project.name.clone();

        let is_focused = {
            let ws = self.workspace.read(cx);
            ws.focused_project_id() == Some(&project.id)
        };

        let is_renaming = is_renaming(&self.project_rename, &project.id);

        let terminal_count = project.terminal_ids.len();
        let has_layout = project.has_layout;

        // Project row
        div()
            .id(ElementId::Name(format!("project-row-{}", project.id).into()))
            .h(px(24.0))
            .pl(px(8.0))
            .pr(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .when(is_focused, |d| d.bg(rgb(t.bg_selection)))
            .when(!is_focused, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
            .when(is_cursor, |d| d.border_l_2().border_color(rgb(t.border_active)))
            // Drag source
            .on_drag(ProjectDrag { project_id: project_id.clone(), project_name: project_name.clone() }, move |drag, _position, _window, cx| {
                cx.new(|_| ProjectDragView { name: drag.project_name.clone() })
            })
            // Drop target - show indicator line at top
            .drag_over::<ProjectDrag>(move |style, _, _, _| {
                style.border_t_2().border_color(rgb(t.border_active))
            })
            .on_drop(cx.listener({
                let project_id = project_id.clone();
                move |this, drag: &ProjectDrag, _window, cx| {
                    if drag.project_id != project_id {
                        this.workspace.update(cx, |ws, cx| {
                            ws.move_project(&drag.project_id, index, cx);
                        });
                    }
                }
            }))
            // Drop target for folder reordering among projects
            .drag_over::<FolderDrag>(move |style, _, _, _| {
                style.border_t_2().border_color(rgb(t.border_active))
            })
            .on_drop(cx.listener(move |this, drag: &FolderDrag, _window, cx| {
                this.workspace.update(cx, |ws, cx| {
                    ws.move_item_in_order(&drag.folder_id, index, cx);
                });
            }))
            .on_mouse_down(MouseButton::Right, cx.listener({
                let project_id = project_id.clone();
                move |this, event: &MouseDownEvent, _window, cx| {
                    this.request_context_menu(project_id.clone(), event.position, cx);
                    cx.stop_propagation();
                }
            }))
            .on_click(cx.listener({
                let project_id = project_id.clone();
                move |this, _, _window, cx| {
                    this.cursor_index = None;
                    this.workspace.update(cx, |ws, cx| {
                        ws.set_focused_project(Some(project_id.clone()), cx);
                    });
                }
            }))
            .child(
                sidebar_expand_arrow(
                    ElementId::Name(format!("expand-{}", project.id).into()),
                    is_expanded,
                    &t,
                )
                .on_click(cx.listener({
                    let project_id = project_id.clone();
                    move |this, _, _window, cx| {
                        this.toggle_expanded(&project_id);
                        cx.notify();
                    }
                })),
            )
            .child({
                // Project color dot - clickable for color picker
                let folder_color = t.get_folder_color(project.folder_color);
                let project_id = project.id.clone();
                sidebar_color_indicator(
                    ElementId::Name(format!("folder-icon-{}", project.id).into()),
                    div()
                        .flex_shrink_0()
                        .w(px(8.0))
                        .h(px(8.0))
                        .rounded(px(4.0))
                        .bg(rgb(folder_color)),
                )
                .on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                    this.show_color_picker(project_id.clone(), cx);
                    cx.stop_propagation();
                }))
            })
            .child(
                // Project name (or input if renaming)
                if is_renaming {
                    sidebar_rename_input("project-rename-input", &self.project_rename, &t)
                        .map(|el| el.into_any_element())
                        .unwrap_or_else(|| div().flex_1().into_any_element())
                } else {
                    sidebar_name_label(
                        ElementId::Name(format!("project-name-{}", project.id).into()),
                        project_name.clone(),
                        &t,
                    )
                    .on_click(cx.listener({
                        let project_id = project_id.clone();
                        let project_name = project_name.clone();
                        move |this, _event: &ClickEvent, window, cx| {
                            if this.check_project_double_click(&project_id) {
                                this.start_project_rename(project_id.clone(), project_name.clone(), window, cx);
                            } else {
                                this.cursor_index = None;
                                this.workspace.update(cx, |ws, cx| {
                                    ws.set_focused_project(Some(project_id.clone()), cx);
                                });
                            }
                            cx.stop_propagation();
                        }
                    }))
                    .into_any_element()
                },
            )
            .child(sidebar_terminal_badge(has_layout, terminal_count, &t))
            .child(
                {
                    let is_visible = project.is_visible;
                    let visibility_tooltip = if is_visible { "Hide Project" } else { "Show Project" };
                    sidebar_visibility_toggle(
                        ElementId::Name(format!("visibility-{}", project.id).into()),
                        is_visible,
                        &t,
                    )
                    .on_click(cx.listener({
                        let project_id = project_id.clone();
                        move |this, _, _window, cx| {
                            this.workspace.update(cx, |ws, cx| {
                                ws.toggle_project_visibility(&project_id, cx);
                            });
                            cx.stop_propagation();
                        }
                    }))
                    .tooltip(move |_window, cx| Tooltip::new(visibility_tooltip).build(_window, cx))
                },
            )
    }

    /// Renders a worktree project nested under its parent
    pub(super) fn render_worktree_item(&self, project: &SidebarProjectInfo, is_cursor: bool, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let is_expanded = self.expanded_projects.contains(&project.id);
        let project_id = project.id.clone();
        let project_name = project.name.clone();

        let is_focused = {
            let ws = self.workspace.read(cx);
            ws.focused_project_id() == Some(&project.id)
        };

        let is_renaming = is_renaming(&self.project_rename, &project.id);

        let terminal_count = project.terminal_ids.len();
        let has_layout = project.has_layout;

        // Worktree project row - indented under parent
        div()
            .id(ElementId::Name(format!("worktree-row-{}", project.id).into()))
            .h(px(24.0))
            .pl(px(28.0))  // Indented to align with terminal items
            .pr(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .when(is_focused, |d| d.bg(rgb(t.bg_selection)))
            .when(!is_focused, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
            .when(is_cursor, |d| d.border_l_2().border_color(rgb(t.border_active)))
            .on_mouse_down(MouseButton::Right, cx.listener({
                let project_id = project_id.clone();
                move |this, event: &MouseDownEvent, _window, cx| {
                    this.request_context_menu(project_id.clone(), event.position, cx);
                    cx.stop_propagation();
                }
            }))
            .on_click(cx.listener({
                let project_id = project_id.clone();
                move |this, _, _window, cx| {
                    this.cursor_index = None;
                    this.workspace.update(cx, |ws, cx| {
                        ws.set_focused_project(Some(project_id.clone()), cx);
                    });
                }
            }))
            .child(
                sidebar_expand_arrow(
                    ElementId::Name(format!("expand-wt-{}", project.id).into()),
                    is_expanded,
                    &t,
                )
                .on_click(cx.listener({
                    let project_id = project_id.clone();
                    move |this, _, _window, cx| {
                        this.toggle_expanded(&project_id);
                        cx.notify();
                    }
                })),
            )
            .child(
                // Git branch icon
                div()
                    .flex_shrink_0()
                    .w(px(16.0))
                    .h(px(16.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        svg()
                            .path("icons/git-branch.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary))
                    )
            )
            .child(
                // Project name (or input if renaming)
                if is_renaming {
                    sidebar_rename_input("worktree-rename-input", &self.project_rename, &t)
                        .map(|el| el.into_any_element())
                        .unwrap_or_else(|| div().flex_1().into_any_element())
                } else {
                    sidebar_name_label(
                        ElementId::Name(format!("worktree-name-{}", project.id).into()),
                        project_name.clone(),
                        &t,
                    )
                    .on_click(cx.listener({
                        let project_id = project_id.clone();
                        let project_name = project_name.clone();
                        move |this, _event: &ClickEvent, window, cx| {
                            if this.check_project_double_click(&project_id) {
                                this.start_project_rename(project_id.clone(), project_name.clone(), window, cx);
                            } else {
                                this.cursor_index = None;
                                this.workspace.update(cx, |ws, cx| {
                                    ws.set_focused_project(Some(project_id.clone()), cx);
                                });
                            }
                            cx.stop_propagation();
                        }
                    }))
                    .into_any_element()
                },
            )
            .child(sidebar_terminal_badge(has_layout, terminal_count, &t))
            .child(
                {
                    let is_visible = project.is_visible;
                    let visibility_tooltip = if is_visible { "Hide Project" } else { "Show Project" };
                    sidebar_visibility_toggle(
                        ElementId::Name(format!("visibility-wt-{}", project.id).into()),
                        is_visible,
                        &t,
                    )
                    .on_click(cx.listener({
                        let project_id = project_id.clone();
                        move |this, _, _window, cx| {
                            this.workspace.update(cx, |ws, cx| {
                                ws.toggle_project_visibility(&project_id, cx);
                            });
                            cx.stop_propagation();
                        }
                    }))
                    .tooltip(move |_window, cx| Tooltip::new(visibility_tooltip).build(_window, cx))
                },
            )
    }

    pub(super) fn render_terminal_item(
        &self,
        project_id: &str,
        terminal_id: &str,
        terminal_names: &HashMap<String, String>,
        is_minimized: bool,
        is_inactive_tab: bool,
        is_in_tab_group: bool,
        left_padding: f32,
        id_prefix: &str,
        is_cursor: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let project_id = project_id.to_string();
        let terminal_id = terminal_id.to_string();

        // Priority: custom name > OSC title > terminal ID prefix
        // Also check for bell notification
        let (terminal_name, has_bell) = {
            let terminals = self.terminals.lock();
            if let Some(terminal) = terminals.get(terminal_id.as_str()) {
                let name = if let Some(custom_name) = terminal_names.get(terminal_id.as_str()) {
                    custom_name.clone()
                } else {
                    terminal.title().unwrap_or_else(|| terminal_id.chars().take(8).collect())
                };
                (name, terminal.has_bell())
            } else {
                let name = terminal_names.get(terminal_id.as_str())
                    .cloned()
                    .unwrap_or_else(|| terminal_id.chars().take(8).collect());
                (name, false)
            }
        };

        // Check if this terminal is being renamed
        let is_renaming = is_renaming(&self.terminal_rename, &(project_id.clone(), terminal_id.clone()));

        // Check if this terminal is currently focused
        let is_focused = {
            let ws = self.workspace.read(cx);
            ws.focus_manager.focused_terminal_state().map_or(false, |ft| {
                if let Some(proj) = ws.project(&project_id) {
                    proj.layout.as_ref()
                        .and_then(|l| l.find_terminal_path(&terminal_id))
                        .map_or(false, |path| ft.project_id == project_id && ft.layout_path == path)
                } else {
                    false
                }
            })
        };

        div()
            .id(ElementId::Name(format!("{}terminal-item-{}", id_prefix, terminal_id).into()))
            .group("terminal-item")
            .h(px(22.0))
            .when(is_in_tab_group, |d| {
                d.ml(px(left_padding - 6.0))
                    .pl(px(4.0))
                    .border_l_2()
                    .border_color(rgb(t.border))
            })
            .when(!is_in_tab_group, |d| d.pl(px(left_padding)))
            .pr(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .when(is_minimized, |d| d.opacity(0.5))
            .when(is_inactive_tab && !is_minimized, |d| d.opacity(0.5))
            .when(is_focused, |d| d.bg(rgb(t.bg_selection)))
            .when(is_cursor && !is_in_tab_group, |d| d.border_l_2().border_color(rgb(t.border_active)))
            // Click to focus this terminal
            .on_click(cx.listener({
                let project_id = project_id.clone();
                let terminal_id = terminal_id.clone();
                move |this, _, _window, cx| {
                    this.cursor_index = None;
                    this.workspace.update(cx, |ws, cx| {
                        ws.focus_terminal_by_id(&project_id, &terminal_id, cx);
                    });
                }
            }))
            .child(
                // Terminal icon - different for minimized and bell state
                div()
                    .flex_shrink_0()
                    .w(px(14.0))
                    .h(px(14.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        svg()
                            .path(if has_bell {
                                "icons/bell.svg"
                            } else if is_minimized {
                                "icons/terminal-minimized.svg"
                            } else {
                                "icons/terminal.svg"
                            })
                            .size(px(12.0))
                            .text_color(if has_bell {
                                rgb(t.border_bell)
                            } else if is_minimized {
                                rgb(t.text_muted)
                            } else if is_inactive_tab {
                                rgb(t.text_muted)
                            } else {
                                rgb(t.success)
                            })
                    ),
            )
            .child(
                // Terminal name (or input if renaming)
                if is_renaming {
                    sidebar_rename_input(
                        ElementId::Name(format!("{}terminal-rename-input", id_prefix).into()),
                        &self.terminal_rename,
                        &t,
                    )
                        .map(|el| el.into_any_element())
                        .unwrap_or_else(|| div().flex_1().min_w_0().into_any_element())
                } else {
                    sidebar_name_label(
                        ElementId::Name(format!("{}terminal-name-{}", id_prefix, terminal_id).into()),
                        terminal_name.clone(),
                        &t,
                    )
                        .on_mouse_down(MouseButton::Left, cx.listener(|_this, _, _, cx| {
                            cx.stop_propagation();
                        }))
                        .on_click(cx.listener({
                            let project_id = project_id.clone();
                            let terminal_id = terminal_id.clone();
                            let terminal_name = terminal_name.clone();
                            move |this, _event: &ClickEvent, window, cx| {
                                if this.check_double_click(&terminal_id) {
                                    this.start_rename(project_id.clone(), terminal_id.clone(), terminal_name.clone(), window, cx);
                                } else {
                                    this.cursor_index = None;
                                    this.workspace.update(cx, |ws, cx| {
                                        ws.focus_terminal_by_id(&project_id, &terminal_id, cx);
                                    });
                                }
                                cx.stop_propagation();
                            }
                        }))
                        .into_any_element()
                },
            )
            .child(
                // Action buttons - show on hover
                div()
                    .flex()
                    .flex_shrink_0()
                    .gap(px(2.0))
                    .opacity(0.0)
                    .group_hover("terminal-item", |s| s.opacity(1.0))
                    .child(
                        // Minimize/restore button
                        div()
                            .id(ElementId::Name(format!("{}minimize-{}", id_prefix, terminal_id).into()))
                            .cursor_pointer()
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .on_mouse_down(MouseButton::Left, cx.listener(|_this, _, _, cx| {
                                cx.stop_propagation();
                            }))
                            .on_click(cx.listener({
                                let project_id = project_id.clone();
                                let terminal_id = terminal_id.clone();
                                move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.workspace.update(cx, |ws, cx| {
                                        ws.toggle_terminal_minimized_by_id(&project_id, &terminal_id, cx);
                                    });
                                }
                            }))
                            .child(
                                svg()
                                    .path("icons/minimize.svg")
                                    .size(px(12.0))
                                    .text_color(rgb(t.text_secondary))
                            )
                            .tooltip({
                                let tooltip_text = if is_minimized { "Restore" } else { "Minimize" };
                                move |_window, cx| {
                                    Tooltip::new(tooltip_text)
                                        .action(&MinimizeTerminal as &dyn Action, None)
                                        .build(_window, cx)
                                }
                            }),
                    )
                    .child(
                        // Fullscreen button
                        div()
                            .id(ElementId::Name(format!("{}fullscreen-{}", id_prefix, terminal_id).into()))
                            .cursor_pointer()
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .on_mouse_down(MouseButton::Left, cx.listener(|_this, _, _, cx| {
                                cx.stop_propagation();
                            }))
                            .on_click(cx.listener({
                                let project_id = project_id.clone();
                                let terminal_id = terminal_id.clone();
                                move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.workspace.update(cx, |ws, cx| {
                                        ws.set_fullscreen_terminal(
                                            project_id.clone(),
                                            terminal_id.clone(),
                                            cx,
                                        );
                                    });
                                }
                            }))
                            .child(
                                svg()
                                    .path("icons/fullscreen.svg")
                                    .size(px(12.0))
                                    .text_color(rgb(t.text_secondary))
                            )
                            .tooltip(|_window, cx| {
                                Tooltip::new("Fullscreen")
                                    .action(&ToggleFullscreen as &dyn Action, None)
                                    .build(_window, cx)
                            }),
                    ),
            )
    }
}
