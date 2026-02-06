//! Project and terminal list rendering for the sidebar

use crate::theme::theme;
use crate::views::components::{is_renaming, rename_input, SimpleInput};
use gpui::*;
use gpui::prelude::*;
use gpui_component::tooltip::Tooltip;

use super::{Sidebar, ProjectDrag, ProjectDragView, FolderDrag};
use crate::workspace::state::ProjectData;

impl Sidebar {
    /// Renders a project item with its worktree children nested below it
    pub(super) fn render_project_item_with_worktrees(
        &self,
        project: &ProjectData,
        index: usize,
        worktree_children: Option<&Vec<ProjectData>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut container = div()
            .flex()
            .flex_col()
            .child(self.render_project_item(project, index, window, cx));

        // Worktree children are always visible below their parent
        if let Some(children) = worktree_children {
            for child in children {
                container = container.child(self.render_worktree_item(child, window, cx));
            }
        }

        container
    }

    pub(super) fn render_project_item(&self, project: &ProjectData, index: usize, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let is_expanded = self.expanded_projects.contains(&project.id);
        let workspace_for_focus = self.workspace.clone();
        let workspace_for_drop = self.workspace.clone();
        let project_id = project.id.clone();
        let project_id_for_focus = project.id.clone();
        let project_id_for_toggle = project.id.clone();
        let project_id_for_visibility = project.id.clone();
        let project_id_for_rename = project.id.clone();
        let project_id_for_context_menu = project.id.clone();
        let project_id_for_drag = project.id.clone();
        let project_name = project.name.clone();
        let project_name_for_rename = project.name.clone();
        let project_name_for_drag = project.name.clone();

        let is_focused = {
            let ws = self.workspace.read(cx);
            ws.focused_project_id.as_ref() == Some(&project.id)
        };

        let is_renaming = is_renaming(&self.project_rename, &project.id);

        let terminal_ids = project.layout.as_ref()
            .map(|l| l.collect_terminal_ids())
            .unwrap_or_default();
        let terminal_count = terminal_ids.len();
        let has_layout = project.layout.is_some();

        div()
            .flex()
            .flex_col()
            .child(
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
                    // Drag source
                    .on_drag(ProjectDrag { project_id: project_id_for_drag.clone(), project_name: project_name_for_drag.clone() }, move |drag, _position, _window, cx| {
                        cx.new(|_| ProjectDragView { name: drag.project_name.clone() })
                    })
                    // Drop target - show indicator line at top
                    .drag_over::<ProjectDrag>(move |style, _, _, _| {
                        style.border_t_2().border_color(rgb(t.border_active))
                    })
                    .on_drop(cx.listener(move |_this, drag: &ProjectDrag, _window, cx| {
                        if drag.project_id != project_id_for_drag {
                            workspace_for_drop.update(cx, |ws, cx| {
                                ws.move_project(&drag.project_id, index, cx);
                            });
                        }
                    }))
                    // Drop target for folder reordering among projects
                    .drag_over::<FolderDrag>(move |style, _, _, _| {
                        style.border_t_2().border_color(rgb(t.border_active))
                    })
                    .on_drop(cx.listener({
                        let workspace = self.workspace.clone();
                        let target_index = index;
                        move |_this, drag: &FolderDrag, _window, cx| {
                            workspace.update(cx, |ws, cx| {
                                ws.move_item_in_order(&drag.folder_id, target_index, cx);
                            });
                        }
                    }))
                    .on_mouse_down(MouseButton::Right, cx.listener({
                        let project_id = project_id_for_context_menu.clone();
                        move |this, event: &MouseDownEvent, _window, cx| {
                            this.request_context_menu(project_id.clone(), event.position, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        // Expand arrow
                        div()
                            .id(ElementId::Name(format!("expand-{}", project.id).into()))
                            .flex_shrink_0()
                            .w(px(16.0))
                            .h(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                svg()
                                    .path(if is_expanded { "icons/chevron-down.svg" } else { "icons/chevron-right.svg" })
                                    .size(px(12.0))
                                    .text_color(rgb(t.text_secondary))
                            )
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.toggle_expanded(&project_id_for_toggle);
                                cx.notify();
                            })),
                    )
                    .child({
                        // Project color dot - clickable for color picker
                        let folder_color = t.get_folder_color(project.folder_color);
                        let project_id_for_color = project.id.clone();
                        div()
                            .id(ElementId::Name(format!("folder-icon-{}", project.id).into()))
                            .flex_shrink_0()
                            .w(px(16.0))
                            .h(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.7))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .w(px(8.0))
                                    .h(px(8.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(folder_color))
                            )
                            .on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                                this.show_color_picker(project_id_for_color.clone(), cx);
                                cx.stop_propagation();
                            }))
                    })
                    .child(
                        // Project name (or input if renaming)
                        if is_renaming {
                            if let Some(input) = rename_input(&self.project_rename) {
                                div()
                                    .id("project-rename-input")
                                    .flex_1()
                                    .min_w_0()
                                    .bg(rgb(t.bg_hover))
                                    .rounded(px(2.0))
                                    .child(
                                        SimpleInput::new(input)
                                            .text_size(px(12.0))
                                    )
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_click(|_, _window, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                                        // Stop all keys from bubbling
                                        cx.stop_propagation();
                                        match event.keystroke.key.as_str() {
                                            "enter" => this.finish_project_rename(cx),
                                            "escape" => this.cancel_project_rename(cx),
                                            _ => {}
                                        }
                                    }))
                                    .into_any_element()
                            } else {
                                div().flex_1().into_any_element()
                            }
                        } else {
                            div()
                                .id(ElementId::Name(format!("project-name-{}", project.id).into()))
                                .flex_1()
                                .min_w_0()  // Allow shrinking below content size
                                .overflow_hidden()
                                .text_size(px(12.0))
                                .text_color(rgb(t.text_primary))
                                .text_ellipsis()
                                .child(project_name)
                                .on_click(cx.listener({
                                    let project_id = project_id_for_rename;
                                    let project_id_for_focus = project_id_for_focus.clone();
                                    let name = project_name_for_rename;
                                    move |this, _event: &ClickEvent, window, cx| {
                                        if this.check_project_double_click(&project_id) {
                                            this.start_project_rename(project_id.clone(), name.clone(), window, cx);
                                        } else {
                                            // Single click - focus the project
                                            workspace_for_focus.update(cx, |ws, cx| {
                                                ws.set_focused_project(Some(project_id_for_focus.clone()), cx);
                                            });
                                        }
                                        cx.stop_propagation();
                                    }
                                }))
                                .into_any_element()
                        },
                    )
                    .child(
                        // Terminal count badge or bookmark indicator
                        if has_layout {
                            div()
                                .flex_shrink_0()
                                .px(px(4.0))
                                .py(px(1.0))
                                .rounded(px(4.0))
                                .bg(rgb(t.bg_secondary))
                                .text_size(px(10.0))
                                .text_color(rgb(t.text_muted))
                                .child(format!("{}", terminal_count))
                                .into_any_element()
                        } else {
                            // Bookmark badge for terminal-less projects
                            div()
                                .flex_shrink_0()
                                .px(px(4.0))
                                .py(px(1.0))
                                .rounded(px(4.0))
                                .bg(rgb(t.bg_secondary))
                                .flex()
                                .items_center()
                                .gap(px(2.0))
                                .child(
                                    svg()
                                        .path("icons/bookmark.svg")
                                        .size(px(10.0))
                                        .text_color(rgb(t.text_muted))
                                )
                                .into_any_element()
                        },
                    )
                    .child(
                        // Visibility toggle
                        {
                            let workspace = self.workspace.clone();
                            let is_visible = project.is_visible;
                            let visibility_tooltip = if is_visible { "Hide Project" } else { "Show Project" };
                            div()
                                .id(ElementId::Name(format!("visibility-{}", project.id).into()))
                                .flex_shrink_0()
                                .cursor_pointer()
                                .w(px(18.0))
                                .h(px(18.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.0))
                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                .on_click(move |_, _window, cx| {
                                    workspace.update(cx, |ws, cx| {
                                        ws.toggle_project_visibility(&project_id_for_visibility, cx);
                                    });
                                })
                                .child(
                                    svg()
                                        .path(if is_visible { "icons/eye.svg" } else { "icons/eye-off.svg" })
                                        .size(px(12.0))
                                        .text_color(if is_visible {
                                            rgb(t.term_blue)
                                        } else {
                                            rgb(t.text_muted)
                                        })
                                )
                                .tooltip(move |_window, cx| Tooltip::new(visibility_tooltip).build(_window, cx))
                        },
                    ),
            )
            .when(is_expanded, |d| {
                // Collect minimized states first to avoid borrow checker issues
                let minimized_states: Vec<(String, bool)> = {
                    let ws = self.workspace.read(cx);
                    terminal_ids.iter().map(|id| {
                        let is_minimized = ws.is_terminal_minimized(&project_id, id);
                        (id.clone(), is_minimized)
                    }).collect()
                };

                // Show all terminals (minimized ones will be dimmed with different icon)
                let terminal_elements: Vec<_> = minimized_states.iter().map(|(id, is_minimized)| {
                    self.render_terminal_item(&project_id, id, project, *is_minimized, window, cx).into_any_element()
                }).collect();

                d.children(terminal_elements)
            })
    }

    /// Renders a worktree project nested under its parent
    pub(super) fn render_worktree_item(&self, project: &ProjectData, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let is_expanded = self.expanded_projects.contains(&project.id);
        let workspace_for_focus = self.workspace.clone();
        let project_id = project.id.clone();
        let project_id_for_toggle = project.id.clone();
        let project_id_for_visibility = project.id.clone();
        let project_id_for_rename = project.id.clone();
        let project_id_for_context_menu = project.id.clone();
        let project_id_for_focus = project.id.clone();
        let project_name = project.name.clone();
        let project_name_for_rename = project.name.clone();

        let is_focused = {
            let ws = self.workspace.read(cx);
            ws.focused_project_id.as_ref() == Some(&project.id)
        };

        let is_renaming = is_renaming(&self.project_rename, &project.id);

        let terminal_ids = project.layout.as_ref()
            .map(|l| l.collect_terminal_ids())
            .unwrap_or_default();
        let terminal_count = terminal_ids.len();
        let has_layout = project.layout.is_some();

        div()
            .flex()
            .flex_col()
            .child(
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
                    .on_mouse_down(MouseButton::Right, cx.listener({
                        let project_id = project_id_for_context_menu.clone();
                        move |this, event: &MouseDownEvent, _window, cx| {
                            this.request_context_menu(project_id.clone(), event.position, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        // Expand arrow
                        div()
                            .id(ElementId::Name(format!("expand-wt-{}", project.id).into()))
                            .flex_shrink_0()
                            .w(px(16.0))
                            .h(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                svg()
                                    .path(if is_expanded { "icons/chevron-down.svg" } else { "icons/chevron-right.svg" })
                                    .size(px(12.0))
                                    .text_color(rgb(t.text_secondary))
                            )
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.toggle_expanded(&project_id_for_toggle);
                                cx.notify();
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
                            if let Some(input) = rename_input(&self.project_rename) {
                                div()
                                    .id("worktree-rename-input")
                                    .flex_1()
                                    .min_w_0()
                                    .bg(rgb(t.bg_hover))
                                    .rounded(px(2.0))
                                    .child(
                                        SimpleInput::new(input)
                                            .text_size(px(12.0))
                                    )
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_click(|_, _window, cx| {
                                        cx.stop_propagation();
                                    })
                                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                                        // Stop all keys from bubbling
                                        cx.stop_propagation();
                                        match event.keystroke.key.as_str() {
                                            "enter" => this.finish_project_rename(cx),
                                            "escape" => this.cancel_project_rename(cx),
                                            _ => {}
                                        }
                                    }))
                                    .into_any_element()
                            } else {
                                div().flex_1().into_any_element()
                            }
                        } else {
                            div()
                                .id(ElementId::Name(format!("worktree-name-{}", project.id).into()))
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_size(px(12.0))
                                .text_color(rgb(t.text_primary))
                                .text_ellipsis()
                                .child(project_name)
                                .on_click(cx.listener({
                                    let project_id = project_id_for_rename;
                                    let project_id_for_focus = project_id_for_focus.clone();
                                    let name = project_name_for_rename;
                                    move |this, _event: &ClickEvent, window, cx| {
                                        if this.check_project_double_click(&project_id) {
                                            this.start_project_rename(project_id.clone(), name.clone(), window, cx);
                                        } else {
                                            workspace_for_focus.update(cx, |ws, cx| {
                                                ws.set_focused_project(Some(project_id_for_focus.clone()), cx);
                                            });
                                        }
                                        cx.stop_propagation();
                                    }
                                }))
                                .into_any_element()
                        },
                    )
                    .child(
                        // Terminal count badge or bookmark indicator
                        if has_layout {
                            div()
                                .flex_shrink_0()
                                .px(px(4.0))
                                .py(px(1.0))
                                .rounded(px(4.0))
                                .bg(rgb(t.bg_secondary))
                                .text_size(px(10.0))
                                .text_color(rgb(t.text_muted))
                                .child(format!("{}", terminal_count))
                                .into_any_element()
                        } else {
                            div()
                                .flex_shrink_0()
                                .px(px(4.0))
                                .py(px(1.0))
                                .rounded(px(4.0))
                                .bg(rgb(t.bg_secondary))
                                .flex()
                                .items_center()
                                .gap(px(2.0))
                                .child(
                                    svg()
                                        .path("icons/bookmark.svg")
                                        .size(px(10.0))
                                        .text_color(rgb(t.text_muted))
                                )
                                .into_any_element()
                        },
                    )
                    .child(
                        // Visibility toggle
                        {
                            let workspace = self.workspace.clone();
                            let is_visible = project.is_visible;
                            let visibility_tooltip = if is_visible { "Hide Project" } else { "Show Project" };
                            div()
                                .id(ElementId::Name(format!("visibility-wt-{}", project.id).into()))
                                .flex_shrink_0()
                                .cursor_pointer()
                                .w(px(18.0))
                                .h(px(18.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.0))
                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                .on_click(move |_, _window, cx| {
                                    workspace.update(cx, |ws, cx| {
                                        ws.toggle_project_visibility(&project_id_for_visibility, cx);
                                    });
                                })
                                .child(
                                    svg()
                                        .path(if is_visible { "icons/eye.svg" } else { "icons/eye-off.svg" })
                                        .size(px(12.0))
                                        .text_color(if is_visible {
                                            rgb(t.term_blue)
                                        } else {
                                            rgb(t.text_muted)
                                        })
                                )
                                .tooltip(move |_window, cx| Tooltip::new(visibility_tooltip).build(_window, cx))
                        },
                    ),
            )
            .when(is_expanded, |d| {
                // Render terminals with extra indentation for worktree items
                let minimized_states: Vec<(String, bool)> = {
                    let ws = self.workspace.read(cx);
                    terminal_ids.iter().map(|id| {
                        let is_minimized = ws.is_terminal_minimized(&project_id, id);
                        (id.clone(), is_minimized)
                    }).collect()
                };

                let terminal_elements: Vec<_> = minimized_states.iter().map(|(id, is_minimized)| {
                    self.render_worktree_terminal_item(&project_id, id, project, *is_minimized, window, cx).into_any_element()
                }).collect();

                d.children(terminal_elements)
            })
    }

    /// Renders a terminal item inside a worktree project (extra indentation)
    fn render_worktree_terminal_item(
        &self,
        project_id: &str,
        terminal_id: &str,
        project: &ProjectData,
        is_minimized: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Reuse the standard terminal item but override the left padding
        let t = theme(cx);
        let workspace = self.workspace.clone();
        let workspace_for_focus = self.workspace.clone();
        let workspace_for_minimize = self.workspace.clone();
        let project_id = project_id.to_string();
        let project_id_for_focus = project_id.clone();
        let project_id_for_minimize = project_id.clone();
        let project_id_for_rename = project_id.clone();
        let terminal_id_owned = terminal_id.to_string();
        let terminal_id_for_focus = terminal_id.to_string();
        let terminal_id_for_minimize = terminal_id.to_string();
        let terminal_id_for_rename = terminal_id.to_string();

        let (terminal_name, has_bell) = {
            let terminals = self.terminals.lock();
            if let Some(terminal) = terminals.get(terminal_id) {
                let name = if let Some(custom_name) = project.terminal_names.get(terminal_id) {
                    custom_name.clone()
                } else {
                    terminal.title().unwrap_or_else(|| terminal_id.chars().take(8).collect())
                };
                (name, terminal.has_bell())
            } else {
                let name = project.terminal_names.get(terminal_id)
                    .cloned()
                    .unwrap_or_else(|| terminal_id.chars().take(8).collect());
                (name, false)
            }
        };

        let is_renaming = is_renaming(&self.terminal_rename, &(project_id.clone(), terminal_id.to_string()));

        let is_focused = {
            let ws = self.workspace.read(cx);
            ws.focus_manager.focused_terminal_state().map_or(false, |ft| {
                if let Some(proj) = ws.project(&project_id) {
                    proj.layout.as_ref()
                        .and_then(|l| l.find_terminal_path(&terminal_id_for_focus))
                        .map_or(false, |path| ft.project_id == project_id && ft.layout_path == path)
                } else {
                    false
                }
            })
        };

        let terminal_name_for_rename = terminal_name.clone();

        div()
            .id(ElementId::Name(format!("wt-terminal-item-{}", terminal_id).into()))
            .group("terminal-item")
            .h(px(22.0))
            .pl(px(48.0))  // Extra indentation for worktree terminals
            .pr(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .when(is_minimized, |d| d.opacity(0.5))
            .when(is_focused, |d| d.bg(rgb(t.bg_selection)))
            .on_click({
                let workspace = workspace_for_focus.clone();
                let project_id = project_id_for_focus.clone();
                let terminal_id = terminal_id_for_focus.clone();
                move |_, _window, cx| {
                    workspace.update(cx, |ws, cx| {
                        ws.focus_terminal_by_id(&project_id, &terminal_id, cx);
                    });
                }
            })
            .child(
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
                            } else {
                                rgb(t.success)
                            })
                    ),
            )
            .child(
                if is_renaming {
                    if let Some(input) = rename_input(&self.terminal_rename) {
                        div()
                            .id("wt-terminal-rename-input")
                            .flex_1()
                            .min_w_0()
                            .bg(rgb(t.bg_hover))
                            .rounded(px(2.0))
                            .child(
                                SimpleInput::new(input)
                                    .text_size(px(12.0))
                            )
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(|_, _window, cx| {
                                cx.stop_propagation();
                            })
                            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                                // Stop all keys from bubbling
                                cx.stop_propagation();
                                match event.keystroke.key.as_str() {
                                    "enter" => this.finish_rename(cx),
                                    "escape" => this.cancel_rename(cx),
                                    _ => {}
                                }
                            }))
                            .into_any_element()
                    } else {
                        div().flex_1().min_w_0().into_any_element()
                    }
                } else {
                    div()
                        .id(ElementId::Name(format!("wt-terminal-name-{}", terminal_id).into()))
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_size(px(12.0))
                        .text_color(rgb(t.text_primary))
                        .text_ellipsis()
                        .child(terminal_name)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_click(cx.listener({
                            let workspace = workspace_for_focus.clone();
                            let project_id = project_id_for_rename;
                            let project_id_for_focus = project_id_for_focus.clone();
                            let terminal_id = terminal_id_for_rename;
                            let terminal_id_for_focus = terminal_id_for_focus.clone();
                            let name = terminal_name_for_rename;
                            move |this, _event: &ClickEvent, window, cx| {
                                if this.check_double_click(&terminal_id) {
                                    this.start_rename(project_id.clone(), terminal_id.clone(), name.clone(), window, cx);
                                } else {
                                    workspace.update(cx, |ws, cx| {
                                        ws.focus_terminal_by_id(&project_id_for_focus, &terminal_id_for_focus, cx);
                                    });
                                }
                                cx.stop_propagation();
                            }
                        }))
                        .into_any_element()
                },
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .gap(px(2.0))
                    .opacity(0.0)
                    .group_hover("terminal-item", |s| s.opacity(1.0))
                    .child(
                        div()
                            .id(ElementId::Name(format!("wt-minimize-{}", terminal_id).into()))
                            .cursor_pointer()
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(move |_, _window, cx| {
                                cx.stop_propagation();
                                workspace_for_minimize.update(cx, |ws, cx| {
                                    ws.toggle_terminal_minimized_by_id(&project_id_for_minimize, &terminal_id_for_minimize, cx);
                                });
                            })
                            .child(
                                svg()
                                    .path("icons/minimize.svg")
                                    .size(px(12.0))
                                    .text_color(rgb(t.text_secondary))
                            )
                            .tooltip({
                                let tooltip_text = if is_minimized { "Restore" } else { "Minimize" };
                                move |_window, cx| Tooltip::new(tooltip_text).build(_window, cx)
                            }),
                    )
                    .child(
                        div()
                            .id(ElementId::Name(format!("wt-fullscreen-{}", terminal_id).into()))
                            .cursor_pointer()
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(move |_, _window, cx| {
                                cx.stop_propagation();
                                workspace.update(cx, |ws, cx| {
                                    ws.set_fullscreen_terminal(
                                        project_id.clone(),
                                        terminal_id_owned.clone(),
                                        cx,
                                    );
                                });
                            })
                            .child(
                                svg()
                                    .path("icons/fullscreen.svg")
                                    .size(px(12.0))
                                    .text_color(rgb(t.text_secondary))
                            )
                            .tooltip(|_window, cx| Tooltip::new("Fullscreen").build(_window, cx)),
                    ),
            )
    }

    pub(super) fn render_terminal_item(
        &self,
        project_id: &str,
        terminal_id: &str,
        project: &ProjectData,
        is_minimized: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.clone();
        let workspace_for_focus = self.workspace.clone();
        let workspace_for_minimize = self.workspace.clone();
        let project_id = project_id.to_string();
        let project_id_for_focus = project_id.clone();
        let project_id_for_minimize = project_id.clone();
        let project_id_for_rename = project_id.clone();
        let terminal_id_owned = terminal_id.to_string();
        let terminal_id_for_focus = terminal_id.to_string();
        let terminal_id_for_minimize = terminal_id.to_string();
        let terminal_id_for_rename = terminal_id.to_string();

        // Priority: custom name > OSC title > terminal ID prefix
        // Also check for bell notification
        let (terminal_name, has_bell) = {
            let terminals = self.terminals.lock();
            if let Some(terminal) = terminals.get(terminal_id) {
                let name = if let Some(custom_name) = project.terminal_names.get(terminal_id) {
                    custom_name.clone()
                } else {
                    terminal.title().unwrap_or_else(|| terminal_id.chars().take(8).collect())
                };
                (name, terminal.has_bell())
            } else {
                let name = project.terminal_names.get(terminal_id)
                    .cloned()
                    .unwrap_or_else(|| terminal_id.chars().take(8).collect());
                (name, false)
            }
        };

        // Check if this terminal is being renamed
        let is_renaming = is_renaming(&self.terminal_rename, &(project_id.clone(), terminal_id.to_string()));

        // Check if this terminal is currently focused
        let is_focused = {
            let ws = self.workspace.read(cx);
            ws.focus_manager.focused_terminal_state().map_or(false, |ft| {
                if let Some(proj) = ws.project(&project_id) {
                    proj.layout.as_ref()
                        .and_then(|l| l.find_terminal_path(&terminal_id_for_focus))
                        .map_or(false, |path| ft.project_id == project_id && ft.layout_path == path)
                } else {
                    false
                }
            })
        };

        let terminal_name_for_rename = terminal_name.clone();

        div()
            .id(ElementId::Name(format!("terminal-item-{}", terminal_id).into()))
            .group("terminal-item")
            .h(px(22.0))
            .pl(px(28.0))
            .pr(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .when(is_minimized, |d| d.opacity(0.5))
            .when(is_focused, |d| d.bg(rgb(t.bg_selection)))
            // Click to focus this terminal
            .on_click({
                let workspace = workspace_for_focus.clone();
                let project_id = project_id_for_focus.clone();
                let terminal_id = terminal_id_for_focus.clone();
                move |_, _window, cx| {
                    workspace.update(cx, |ws, cx| {
                        ws.focus_terminal_by_id(&project_id, &terminal_id, cx);
                    });
                }
            })
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
                            } else {
                                rgb(t.success)
                            })
                    ),
            )
            .child(
                // Terminal name (or input if renaming)
                if is_renaming {
                    if let Some(input) = rename_input(&self.terminal_rename) {
                        div()
                            .id("terminal-rename-input")
                            .flex_1()
                            .min_w_0()
                            .bg(rgb(t.bg_hover))
                            .rounded(px(2.0))
                            .child(
                                SimpleInput::new(input)
                                    .text_size(px(12.0))
                            )
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(|_, _window, cx| {
                                cx.stop_propagation();
                            })
                            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                                // Stop all keys from bubbling
                                cx.stop_propagation();
                                match event.keystroke.key.as_str() {
                                    "enter" => this.finish_rename(cx),
                                    "escape" => this.cancel_rename(cx),
                                    _ => {}
                                }
                            }))
                            .into_any_element()
                    } else {
                        div().flex_1().min_w_0().into_any_element()
                    }
                } else {
                    div()
                        .id(ElementId::Name(format!("terminal-name-{}", terminal_id).into()))
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_size(px(12.0))
                        .text_color(rgb(t.text_primary))
                        .text_ellipsis()
                        .child(terminal_name)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_click(cx.listener({
                            let workspace = workspace_for_focus.clone();
                            let project_id = project_id_for_rename;
                            let project_id_for_focus = project_id_for_focus.clone();
                            let terminal_id = terminal_id_for_rename;
                            let terminal_id_for_focus = terminal_id_for_focus.clone();
                            let name = terminal_name_for_rename;
                            move |this, _event: &ClickEvent, window, cx| {
                                if this.check_double_click(&terminal_id) {
                                    this.start_rename(project_id.clone(), terminal_id.clone(), name.clone(), window, cx);
                                } else {
                                    // Single click - focus the terminal
                                    workspace.update(cx, |ws, cx| {
                                        ws.focus_terminal_by_id(&project_id_for_focus, &terminal_id_for_focus, cx);
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
                            .id(ElementId::Name(format!("minimize-{}", terminal_id).into()))
                            .cursor_pointer()
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(move |_, _window, cx| {
                                cx.stop_propagation();
                                workspace_for_minimize.update(cx, |ws, cx| {
                                    ws.toggle_terminal_minimized_by_id(&project_id_for_minimize, &terminal_id_for_minimize, cx);
                                });
                            })
                            .child(
                                svg()
                                    .path("icons/minimize.svg")
                                    .size(px(12.0))
                                    .text_color(rgb(t.text_secondary))
                            )
                            .tooltip({
                                let tooltip_text = if is_minimized { "Restore" } else { "Minimize" };
                                move |_window, cx| Tooltip::new(tooltip_text).build(_window, cx)
                            }),
                    )
                    .child(
                        // Fullscreen button
                        div()
                            .id(ElementId::Name(format!("fullscreen-{}", terminal_id).into()))
                            .cursor_pointer()
                            .w(px(18.0))
                            .h(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(move |_, _window, cx| {
                                cx.stop_propagation();
                                workspace.update(cx, |ws, cx| {
                                    ws.set_fullscreen_terminal(
                                        project_id.clone(),
                                        terminal_id_owned.clone(),
                                        cx,
                                    );
                                });
                            })
                            .child(
                                svg()
                                    .path("icons/fullscreen.svg")
                                    .size(px(12.0))
                                    .text_color(rgb(t.text_secondary))
                            )
                            .tooltip(|_window, cx| Tooltip::new("Fullscreen").build(_window, cx)),
                    ),
            )
    }
}
