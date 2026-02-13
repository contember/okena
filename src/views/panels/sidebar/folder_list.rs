//! Folder list rendering for the sidebar


use crate::theme::theme;
use crate::views::components::is_renaming;
use gpui::*;
use gpui::prelude::*;
use gpui_component::tooltip::Tooltip;

use super::item_widgets::*;
use super::{Sidebar, SidebarProjectInfo, ProjectDrag, FolderDrag, FolderDragView};
use crate::workspace::state::FolderData;

impl Sidebar {
    /// Renders only the folder header row (expand arrow, icon, name, badges)
    pub(super) fn render_folder_header(
        &self,
        folder: &FolderData,
        index: usize,
        project_count: usize,
        is_cursor: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let folder_id = folder.id.clone();
        let folder_name = folder.name.clone();
        let is_collapsed = folder.collapsed;

        let is_renaming = is_renaming(&self.folder_rename, &folder.id);

        // Folder header row
        div()
            .id(ElementId::Name(format!("folder-row-{}", folder.id).into()))
            .h(px(24.0))
            .pl(px(8.0))
            .pr(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .when(is_cursor, |d| d.border_l_2().border_color(rgb(t.border_active)))
            // Drag source for folder reordering
            .on_drag(FolderDrag { folder_id: folder_id.clone(), folder_name: folder_name.clone() }, move |drag, _position, _window, cx| {
                cx.new(|_| FolderDragView { name: drag.folder_name.clone() })
            })
            // Drop target for folder reordering
            .drag_over::<FolderDrag>(move |style, _, _, _| {
                style.border_t_2().border_color(rgb(t.border_active))
            })
            .on_drop(cx.listener({
                let folder_id = folder_id.clone();
                move |this, drag: &FolderDrag, _window, cx| {
                    if drag.folder_id != folder_id {
                        this.workspace.update(cx, |ws, cx| {
                            ws.move_item_in_order(&drag.folder_id, index, cx);
                        });
                    }
                }
            }))
            // Drop target for projects being dragged onto folder
            .drag_over::<ProjectDrag>(move |style, _, _, _| {
                style.bg(rgb(t.bg_selection))
            })
            .on_drop(cx.listener({
                let folder_id = folder_id.clone();
                move |this, drag: &ProjectDrag, _window, cx| {
                    this.workspace.update(cx, |ws, cx| {
                        ws.move_project_to_folder(&drag.project_id, &folder_id, None, cx);
                    });
                }
            }))
            // Right-click context menu
            .on_mouse_down(MouseButton::Right, cx.listener({
                let folder_id = folder_id.clone();
                let folder_name = folder_name.clone();
                move |this, event: &MouseDownEvent, _window, cx| {
                    this.request_broker.update(cx, |broker, cx| {
                        broker.push_overlay_request(crate::workspace::requests::OverlayRequest::FolderContextMenu {
                            folder_id: folder_id.clone(),
                            folder_name: folder_name.clone(),
                            position: event.position,
                        }, cx);
                    });
                    cx.stop_propagation();
                }
            }))
            .on_click(cx.listener({
                let folder_id = folder_id.clone();
                move |this, _, _window, cx| {
                    this.cursor_index = None;
                    this.workspace.update(cx, |ws, cx| {
                        ws.toggle_folder_collapsed(&folder_id, cx);
                    });
                }
            }))
            .child(
                sidebar_expand_arrow(
                    ElementId::Name(format!("folder-expand-{}", folder.id).into()),
                    !is_collapsed,
                    &t,
                )
                .on_click(cx.listener({
                    let folder_id = folder_id.clone();
                    move |this, _, _window, cx| {
                        this.workspace.update(cx, |ws, cx| {
                            ws.toggle_folder_collapsed(&folder_id, cx);
                        });
                        cx.stop_propagation();
                    }
                })),
            )
            .child({
                // Folder color icon
                let folder_color = t.get_folder_color(folder.folder_color);
                let folder_id = folder.id.clone();
                sidebar_color_indicator(
                    ElementId::Name(format!("folder-color-{}", folder.id).into()),
                    svg()
                        .path("icons/folder.svg")
                        .size(px(14.0))
                        .text_color(rgb(folder_color)),
                )
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.show_folder_color_picker(folder_id.clone(), f32::from(event.position.y), cx);
                    cx.stop_propagation();
                }))
            })
            .child(
                // Folder name (or input if renaming)
                if is_renaming {
                    sidebar_rename_input("folder-rename-input", &self.folder_rename, &t)
                        .map(|el| el.into_any_element())
                        .unwrap_or_else(|| div().flex_1().into_any_element())
                } else {
                    sidebar_name_label(
                        ElementId::Name(format!("folder-name-{}", folder.id).into()),
                        folder_name.clone(),
                        &t,
                    )
                    .font_weight(FontWeight::MEDIUM)
                    .on_click(cx.listener({
                        let folder_id = folder_id.clone();
                        let folder_name = folder_name.clone();
                        move |this, _event: &ClickEvent, window, cx| {
                            if this.check_folder_double_click(&folder_id) {
                                this.start_folder_rename(folder_id.clone(), folder_name.clone(), window, cx);
                            } else {
                                this.cursor_index = None;
                                this.workspace.update(cx, |ws, cx| {
                                    ws.toggle_folder_collapsed(&folder_id, cx);
                                });
                            }
                            cx.stop_propagation();
                        }
                    }))
                    .into_any_element()
                },
            )
            .child(
                // Project count badge
                div()
                    .flex_shrink_0()
                    .px(px(4.0))
                    .py(px(1.0))
                    .rounded(px(4.0))
                    .bg(rgb(t.bg_secondary))
                    .text_size(px(10.0))
                    .text_color(rgb(t.text_muted))
                    .child(format!("{}", project_count)),
            )
            .child(
                // Delete folder button (on hover)
                {
                    let folder_id = folder_id.clone();
                    div()
                        .id(ElementId::Name(format!("folder-delete-{}", folder_id).into()))
                        .flex_shrink_0()
                        .cursor_pointer()
                        .w(px(18.0))
                        .h(px(18.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(3.0))
                        .opacity(0.0)
                        .hover(|s| s.bg(rgb(t.bg_hover)).opacity(1.0))
                        .on_mouse_down(MouseButton::Left, cx.listener(|_this, _, _, cx| {
                            cx.stop_propagation();
                        }))
                        .on_click(cx.listener({
                            let folder_id = folder_id.clone();
                            move |this, _, _window, cx| {
                                cx.stop_propagation();
                                this.workspace.update(cx, |ws, cx| {
                                    ws.delete_folder(&folder_id, cx);
                                });
                            }
                        }))
                        .child(
                            svg()
                                .path("icons/close.svg")
                                .size(px(12.0))
                                .text_color(rgb(t.text_muted))
                        )
                        .tooltip(|_window, cx| Tooltip::new("Delete folder (keeps projects)").build(_window, cx))
                },
            )
    }

    /// Renders a project item inside a folder (indented)
    pub(super) fn render_folder_project_item(
        &self,
        project: &SidebarProjectInfo,
        folder_id: &str,
        is_cursor: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let is_expanded = self.expanded_projects.contains(&project.id);
        let project_id = project.id.clone();
        let project_name = project.name.clone();
        let folder_id = folder_id.to_string();

        let is_renaming = is_renaming(&self.project_rename, &project.id);

        let terminal_count = project.terminal_ids.len();
        let has_layout = project.has_layout;

        div()
            .id(ElementId::Name(format!("folder-project-row-{}", project.id).into()))
            .h(px(24.0))
            .pl(px(28.0))  // Indented for folder nesting
            .pr(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .when(is_cursor, |d| d.border_l_2().border_color(rgb(t.border_active)))
            // Drag source
            .on_drag(super::ProjectDrag { project_id: project_id.clone(), project_name: project_name.clone() }, move |drag, _position, _window, cx| {
                cx.new(|_| super::ProjectDragView { name: drag.project_name.clone() })
            })
            // Drop target for reordering within folder
            .drag_over::<super::ProjectDrag>(move |style, _, _, _| {
                style.border_t_2().border_color(rgb(t.border_active))
            })
            .on_drop(cx.listener({
                let folder_id = folder_id.clone();
                let project_id = project_id.clone();
                move |this, drag: &super::ProjectDrag, _window, cx| {
                    if drag.project_id != project_id {
                        let pos = this.workspace.read(cx).folder(&folder_id)
                            .and_then(|f| f.project_ids.iter().position(|id| id == &project_id));
                        if let Some(pos) = pos {
                            this.workspace.update(cx, |ws, cx| {
                                ws.move_project_to_folder(&drag.project_id, &folder_id, Some(pos), cx);
                            });
                        }
                    }
                }
            }))
            // Also accept FolderDrag for top-level reordering
            .drag_over::<super::FolderDrag>(move |style, _, _, _| {
                style.border_t_2().border_color(rgb(t.border_active))
            })
            .on_drop(cx.listener(move |this, drag: &super::FolderDrag, _window, cx| {
                this.workspace.update(cx, |ws, cx| {
                    ws.move_item_in_order(&drag.folder_id, 0, cx);
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
                    ElementId::Name(format!("expand-fp-{}", project.id).into()),
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
                })),
            )
            .child({
                // Project color dot
                let folder_color = t.get_folder_color(project.folder_color);
                let project_id = project.id.clone();
                sidebar_color_indicator(
                    ElementId::Name(format!("fp-folder-icon-{}", project.id).into()),
                    div()
                        .flex_shrink_0()
                        .w(px(8.0))
                        .h(px(8.0))
                        .rounded(px(4.0))
                        .bg(rgb(folder_color)),
                )
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.show_color_picker(project_id.clone(), f32::from(event.position.y), cx);
                    cx.stop_propagation();
                }))
            })
            .child(
                // Project name (or input if renaming)
                if is_renaming {
                    sidebar_rename_input("fp-project-rename-input", &self.project_rename, &t)
                        .map(|el| el.into_any_element())
                        .unwrap_or_else(|| div().flex_1().into_any_element())
                } else {
                    sidebar_name_label(
                        ElementId::Name(format!("fp-project-name-{}", project.id).into()),
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
                        ElementId::Name(format!("fp-visibility-{}", project.id).into()),
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
}
