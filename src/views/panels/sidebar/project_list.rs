//! Project and terminal list rendering for the sidebar

use crate::keybindings::{MinimizeTerminal, ToggleFullscreen};
use crate::theme::theme;
use crate::views::components::is_renaming;
use gpui::*;
use gpui::prelude::*;
use gpui_component::tooltip::Tooltip;
use okena_core::api::ActionRequest;
use okena_ui::color_dot::color_dot;
use okena_ui::icon_button::icon_button;

use super::item_widgets::*;
use super::{Sidebar, SidebarProjectInfo, ProjectDrag, ProjectDragView, FolderDrag, WorktreeDrag, WorktreeDragView};
use std::collections::HashMap;

/// Drag/drop configuration for group header rendering.
/// Determines how project drag and folder drag are handled.
pub(super) enum GroupHeaderDragConfig {
    /// Top-level group header: reorder projects/folders by index.
    TopLevel { index: usize },
    /// Group header inside a folder: move projects into folder at position.
    InFolder { folder_id: String },
}

impl Sidebar {
    pub(super) fn render_project_item(&self, project: &SidebarProjectInfo, index: usize, is_cursor: bool, is_focused_project: bool, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let has_worktrees = project.worktree_count > 0;
        let is_expanded = self.expanded_projects.contains(&project.id);
        let project_id = project.id.clone();
        let project_name = project.name.clone();

        let is_renaming = is_renaming(&self.project_rename, &project.id);

        let has_layout = project.has_layout;


        // Count idle terminals when project is collapsed (not expanded)
        let idle_count = if !is_expanded {
            self.count_waiting_terminals(&project.terminal_ids)
        } else {
            0
        };

        // Project row
        div()
            .id(ElementId::Name(format!("project-row-{}", project.id).into()))
            .group("project-item")
            .h(px(24.0))
            .pl(px(4.0))
            .pr(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .when(is_focused_project, |d| d.bg(rgb(t.bg_hover)))
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
                        ws.set_focused_project_individual(Some(project_id.clone()), cx);
                    });
                }
            }))
            .child({
                let has_expandable_content = has_layout || has_worktrees || !project.services.is_empty();
                if has_expandable_content {
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
                            cx.stop_propagation();
                        }
                    }))
                    .into_any_element()
                } else {
                    div().flex_shrink_0().w(px(12.0)).h(px(16.0)).into_any_element()
                }
            })
            .child({
                // Project color dot - clickable for color picker
                let folder_color = t.get_folder_color(project.folder_color);
                let project_id = project.id.clone();
                sidebar_color_indicator(
                    ElementId::Name(format!("folder-icon-{}", project.id).into()),
                    color_dot(folder_color, project.is_worktree),
                )
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.show_color_picker(project_id.clone(), f32::from(event.position.y), cx);
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
                                    ws.set_focused_project_individual(Some(project_id.clone()), cx);
                                });
                            }
                            cx.stop_propagation();
                        }
                    }))
                    .into_any_element()
                },
            )
            .when(idle_count > 0, |d| d.child(sidebar_idle_dot(&t)))
            .when(project.worktree_count > 0, |d| {
                d.child(sidebar_worktree_badge(project.worktree_count, &t))
            })
            .child(
                sidebar_visibility_button(
                    ElementId::Name(format!("visibility-{}", project.id).into()),
                    project.show_in_overview,
                    project.terminal_ids.len(),
                    "project-item",
                    if project.show_in_overview { "Hide Project" } else { "Show Project" },
                    &t,
                )
                .on_click(cx.listener({
                    let project_id = project_id.clone();
                    move |this, _, _window, cx| {
                        this.workspace.update(cx, |ws, cx| {
                            ws.toggle_project_overview_visibility(&project_id, cx);
                        });
                        cx.stop_propagation();
                    }
                }))
            )
    }

    /// Renders a worktree project row. Promoted worktrees use the same indent as their parent
    /// (solid dot, conditional expand arrow). Nested worktrees are indented with a hollow circle.
    pub(super) fn render_worktree_item(&self, project: &SidebarProjectInfo, indent: f32, worktree_index: usize, is_cursor: bool, is_focused_project: bool, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let is_expanded = self.expanded_projects.contains(&project.id);
        let is_closing = project.is_closing;
        let is_creating = project.is_creating;
        let is_busy = is_closing || is_creating;
        let project_id = project.id.clone();
        let project_name = project.name.clone();
        let parent_id = project.parent_project_id.clone().unwrap_or_default();

        let is_renaming = is_renaming(&self.project_rename, &project.id);

        let has_layout = project.has_layout;

        // Count idle terminals when project is collapsed (not expanded)
        let idle_count = if !is_expanded {
            self.count_waiting_terminals(&project.terminal_ids)
        } else {
            0
        };

        div()
            .id(ElementId::Name(format!("worktree-row-{}", project.id).into()))
            .group("worktree-item")
            .h(px(24.0))
            .pl(px(indent))
            .pr(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .when(!is_busy, |d| d.cursor_pointer())
            .when(is_busy, |d| d.opacity(0.5))
            .when(!is_busy, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
            .when(is_focused_project && !is_busy, |d| d.bg(rgb(t.bg_hover)))
            .when(is_cursor, |d| d.border_l_2().border_color(rgb(t.border_active)))
            // Drag source for worktree reordering
            .when(!parent_id.is_empty(), |d| {
                let wt_id = project_id.clone();
                let wt_name = project_name.clone();
                let pid = parent_id.clone();
                d.on_drag(WorktreeDrag { worktree_id: wt_id, parent_id: pid, worktree_name: wt_name }, move |drag, _position, _window, cx| {
                    cx.new(|_| WorktreeDragView { name: drag.worktree_name.clone() })
                })
            })
            // Drop target for worktree reordering within same parent
            .drag_over::<WorktreeDrag>(move |style, _, _, _| {
                style.border_t_2().border_color(rgb(t.border_active))
            })
            .on_drop(cx.listener({
                let project_id = project_id.clone();
                let parent_id = parent_id.clone();
                move |this, drag: &WorktreeDrag, _window, cx| {
                    if drag.worktree_id != project_id && drag.parent_id == parent_id {
                        this.workspace.update(cx, |ws, cx| {
                            ws.reorder_worktree(&parent_id, &drag.worktree_id, worktree_index, cx);
                        });
                    }
                }
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
                        ws.set_focused_project_individual(Some(project_id.clone()), cx);
                    });
                }
            }))
            .child({
                let has_expandable_content = has_layout || !project.services.is_empty();
                if has_expandable_content {
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
                            cx.stop_propagation();
                        }
                    }))
                    .into_any_element()
                } else {
                    div().flex_shrink_0().w(px(12.0)).h(px(16.0)).into_any_element()
                }
            })
            .child({
                // Hollow circle indicator for all worktrees
                let folder_color = t.get_folder_color(project.folder_color);
                let dot_color = if project.is_orphan { t.warning } else { folder_color };
                div()
                    .flex_shrink_0()
                    .w(px(14.0))
                    .h(px(16.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(color_dot(dot_color, true))
            })
            .child(
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
                                    ws.set_focused_project_individual(Some(project_id.clone()), cx);
                                });
                            }
                            cx.stop_propagation();
                        }
                    }))
                    .into_any_element()
                },
            )
            .when(idle_count > 0 && !is_busy, |d| d.child(sidebar_idle_dot(&t)))
            .when(is_busy, |d| {
                d.child(
                    div()
                        .ml_auto()
                        .text_size(px(10.0))
                        .text_color(rgb(t.text_secondary))
                        .child(if is_creating { "Creating\u{2026}" } else { "Closing\u{2026}" })
                )
            })
            .when(!is_busy, |d| {
                d.child(
                    sidebar_visibility_button(
                        ElementId::Name(format!("visibility-wt-{}", project_id).into()),
                        project.show_in_overview,
                        project.terminal_ids.len(),
                        "worktree-item",
                        if project.show_in_overview { "Hide Worktree" } else { "Show Worktree" },
                        &t,
                    )
                    .on_click(cx.listener({
                        let project_id = project_id.clone();
                        move |this, _, _window, cx| {
                            this.workspace.update(cx, |ws, cx| {
                                ws.toggle_worktree_visibility(&project_id, cx);
                            });
                            cx.stop_propagation();
                        }
                    }))
                )
            })
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

        // Priority: user-set custom name > non-prompt OSC title > directory fallback
        // Also check for bell notification and cached idle/waiting state
        let (terminal_name, has_bell, is_waiting, idle_label) = {
            let ws = self.workspace.read(cx);
            let project = ws.project(&project_id);
            let terminals = self.terminals.lock();
            let terminal = terminals.get(terminal_id.as_str());
            let osc_title = terminal.and_then(|t| t.title());
            let name = if let Some(custom_name) = terminal_names.get(terminal_id.as_str()) {
                custom_name.clone()
            } else if let Some(p) = project {
                p.terminal_display_name(terminal_id.as_str(), osc_title)
            } else {
                "Terminal".to_string()
            };
            let bell = terminal.map_or(false, |t| t.has_bell());
            let waiting = terminal.map_or(false, |t| t.is_waiting_for_input());
            let idle = if waiting { terminal.map(|t| t.idle_duration_display()) } else { None };
            (name, bell, waiting, idle)
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
                            } else if is_waiting {
                                rgb(t.border_idle)
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
            .children(idle_label.map(|d| {
                div()
                    .text_size(px(10.0))
                    .text_color(rgb(t.border_idle))
                    .flex_shrink_0()
                    .child(d)
            }))
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
                        icon_button(
                            ElementId::Name(format!("{}minimize-{}", id_prefix, terminal_id).into()),
                            "icons/minimize.svg",
                            &t,
                        )
                            .on_mouse_down(MouseButton::Left, cx.listener(|_this, _, _, cx| {
                                cx.stop_propagation();
                            }))
                            .on_click(cx.listener({
                                let project_id = project_id.clone();
                                let terminal_id = terminal_id.clone();
                                move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    if let Some(dispatcher) = this.dispatcher_for_project(&project_id, cx) {
                                        dispatcher.dispatch(ActionRequest::ToggleMinimized {
                                            project_id: project_id.clone(),
                                            terminal_id: terminal_id.clone(),
                                        }, cx);
                                    }
                                }
                            }))
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
                        icon_button(
                            ElementId::Name(format!("{}fullscreen-{}", id_prefix, terminal_id).into()),
                            "icons/fullscreen.svg",
                            &t,
                        )
                            .on_mouse_down(MouseButton::Left, cx.listener(|_this, _, _, cx| {
                                cx.stop_propagation();
                            }))
                            .on_click(cx.listener({
                                let project_id = project_id.clone();
                                let terminal_id = terminal_id.clone();
                                move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    if let Some(dispatcher) = this.dispatcher_for_project(&project_id, cx) {
                                        dispatcher.dispatch(ActionRequest::SetFullscreen {
                                            project_id: project_id.clone(),
                                            terminal_id: Some(terminal_id.clone()),
                                        }, cx);
                                    }
                                }
                            }))
                            .tooltip(|_window, cx| {
                                Tooltip::new("Fullscreen")
                                    .action(&ToggleFullscreen as &dyn Action, None)
                                    .build(_window, cx)
                            }),
                    ),
            )
    }

    /// Render project as a group header when it has worktrees.
    /// Click = show parent + all worktrees (non-individual focus).
    pub(super) fn render_project_group_header(
        &self,
        project: &SidebarProjectInfo,
        left_padding: f32,
        id_prefix: &str,
        group_name: &'static str,
        drag_config: GroupHeaderDragConfig,
        is_cursor: bool,
        is_focused_project: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let is_expanded = self.is_project_expanded(&project.id, true);
        let project_id = project.id.clone();
        let project_name = project.name.clone();
        let is_renaming = is_renaming(&self.project_rename, &project.id);

        let idle_count = if !is_expanded { self.count_waiting_terminals(&project.terminal_ids) } else { 0 };

        let base = div()
            .id(ElementId::Name(format!("{}-{}", id_prefix, project.id).into()))
            .group(group_name)
            .h(px(24.0))
            .pl(px(left_padding))
            .pr(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .when(is_focused_project, |d| d.bg(rgb(t.bg_hover)))
            .when(is_cursor, |d| d.border_l_2().border_color(rgb(t.border_active)))
            .on_drag(ProjectDrag { project_id: project_id.clone(), project_name: project_name.clone() }, move |drag, _position, _window, cx| {
                cx.new(|_| ProjectDragView { name: drag.project_name.clone() })
            })
            .drag_over::<ProjectDrag>(move |style, _, _, _| {
                style.border_t_2().border_color(rgb(t.border_active))
            });

        let base = match drag_config {
            GroupHeaderDragConfig::TopLevel { index } => {
                base
                    .on_drop(cx.listener({
                        let project_id = project_id.clone();
                        move |this, drag: &ProjectDrag, _window, cx| {
                            if drag.project_id != project_id {
                                this.workspace.update(cx, |ws, cx| { ws.move_project(&drag.project_id, index, cx); });
                            }
                        }
                    }))
                    .drag_over::<FolderDrag>(move |style, _, _, _| {
                        style.border_t_2().border_color(rgb(t.border_active))
                    })
                    .on_drop(cx.listener(move |this, drag: &FolderDrag, _window, cx| {
                        this.workspace.update(cx, |ws, cx| { ws.move_item_in_order(&drag.folder_id, index, cx); });
                    }))
            }
            GroupHeaderDragConfig::InFolder { folder_id } => {
                base
                    .on_drop(cx.listener({
                        let folder_id = folder_id.clone();
                        let project_id = project_id.clone();
                        move |this, drag: &ProjectDrag, _window, cx| {
                            if drag.project_id != project_id {
                                let pos = this.workspace.read(cx).folder(&folder_id)
                                    .and_then(|f| f.project_ids.iter().position(|id| id == &project_id));
                                if let Some(pos) = pos {
                                    this.workspace.update(cx, |ws, cx| { ws.move_project_to_folder(&drag.project_id, &folder_id, Some(pos), cx); });
                                }
                            }
                        }
                    }))
            }
        };

        base
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
                    ElementId::Name(format!("expand-{}-{}", id_prefix, project.id).into()),
                    is_expanded,
                    &t,
                )
                .on_click(cx.listener({
                    let project_id = project_id.clone();
                    move |this, _, _window, cx| {
                        this.toggle_worktrees_collapsed(&project_id);
                        cx.notify();
                        cx.stop_propagation();
                    }
                }))
            )
            .child({
                let folder_color = t.get_folder_color(project.folder_color);
                let project_id = project.id.clone();
                sidebar_color_indicator(
                    ElementId::Name(format!("{}-icon-{}", id_prefix, project.id).into()),
                    color_dot(folder_color, false),
                )
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.show_color_picker(project_id.clone(), f32::from(event.position.y), cx);
                    cx.stop_propagation();
                }))
            })
            .child(
                if is_renaming {
                    sidebar_rename_input(
                        ElementId::Name(format!("{}-rename-input", id_prefix).into()),
                        &self.project_rename, &t,
                    )
                        .map(|el| el.into_any_element())
                        .unwrap_or_else(|| div().flex_1().into_any_element())
                } else {
                    sidebar_name_label(
                        ElementId::Name(format!("{}-name-{}", id_prefix, project.id).into()),
                        project_name.clone(), &t,
                    )
                    .font_weight(FontWeight::MEDIUM)
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
            .when(idle_count > 0, |d| d.child(sidebar_idle_dot(&t)))
    }

    /// Render main project as a child row under a group header.
    /// Click = show just this project (individual focus).
    pub(super) fn render_project_group_child(
        &self,
        project: &SidebarProjectInfo,
        left_padding: f32,
        id_prefix: &str,
        group_name: &'static str,
        is_cursor: bool,
        is_focused_project: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let is_expanded = self.expanded_projects.contains(&project.id);
        let project_id = project.id.clone();
        let has_layout = project.has_layout;
        let idle_count = if !is_expanded { self.count_waiting_terminals(&project.terminal_ids) } else { 0 };

        div()
            .id(ElementId::Name(format!("{}-{}", id_prefix, project.id).into()))
            .group(group_name)
            .h(px(24.0))
            .pl(px(left_padding))
            .pr(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .when(is_focused_project, |d| d.bg(rgb(t.bg_hover)))
            .when(is_cursor, |d| d.border_l_2().border_color(rgb(t.border_active)))
            .on_click(cx.listener({
                let project_id = project_id.clone();
                move |this, _, _window, cx| {
                    this.cursor_index = None;
                    this.workspace.update(cx, |ws, cx| {
                        ws.set_focused_project_individual(Some(project_id.clone()), cx);
                    });
                }
            }))
            .on_mouse_down(MouseButton::Right, cx.listener({
                let project_id = project_id.clone();
                move |this, event: &MouseDownEvent, _window, cx| {
                    this.request_context_menu(project_id.clone(), event.position, cx);
                    cx.stop_propagation();
                }
            }))
            .child({
                let has_expandable = has_layout || !project.services.is_empty();
                if has_expandable {
                    sidebar_expand_arrow(ElementId::Name(format!("expand-{}-{}", id_prefix, project.id).into()), is_expanded, &t)
                    .on_click(cx.listener({
                        let project_id = project_id.clone();
                        move |this, _, _window, cx| { this.toggle_expanded(&project_id); cx.notify(); cx.stop_propagation(); }
                    }))
                    .into_any_element()
                } else {
                    div().flex_shrink_0().w(px(12.0)).h(px(16.0)).into_any_element()
                }
            })
            .child({
                let folder_color = t.get_folder_color(project.folder_color);
                div()
                    .flex_shrink_0()
                    .w(px(14.0))
                    .h(px(16.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(color_dot(folder_color, false))
            })
            .child(
                sidebar_name_label(ElementId::Name(format!("{}-name-{}", id_prefix, project.id).into()), project.name.clone(), &t)
                .on_click(cx.listener({
                    let project_id = project_id.clone();
                    move |this, _event: &ClickEvent, _window, cx| {
                        this.cursor_index = None;
                        this.workspace.update(cx, |ws, cx| {
                            ws.set_focused_project_individual(Some(project_id.clone()), cx);
                        });
                        cx.stop_propagation();
                    }
                }))
                .into_any_element(),
            )
            .when(idle_count > 0, |d| d.child(sidebar_idle_dot(&t)))
            .child(
                sidebar_visibility_button(
                    ElementId::Name(format!("{}-vis-{}", id_prefix, project.id).into()),
                    project.show_in_overview, project.terminal_ids.len(), group_name,
                    if project.show_in_overview { "Hide Project" } else { "Show Project" }, &t,
                )
                .on_click(cx.listener({
                    let project_id = project_id.clone();
                    move |this, _, _window, cx| {
                        this.workspace.update(cx, |ws, cx| { ws.toggle_project_overview_visibility(&project_id, cx); });
                        cx.stop_propagation();
                    }
                }))
            )
    }


}
