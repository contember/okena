//! Folder list rendering for the sidebar


use okena_ui::theme::theme;
use okena_ui::rename_state::is_renaming;
use gpui::*;
use gpui::prelude::*;
use gpui_component::tooltip::Tooltip;
use okena_ui::color_dot::color_dot;
use okena_ui::icon_button::icon_button;

use crate::item_widgets::*;
use crate::sidebar::{Sidebar, SidebarProjectInfo};
use crate::drag::{ProjectDrag, ProjectDragView, FolderDrag, FolderDragView};
use okena_workspace::state::FolderData;

impl Sidebar {
    /// Send a reorder action to the remote server when a project is reordered
    /// within a remote folder on the client.
    fn send_remote_reorder(this: &mut Self, conn_id: &str, prefixed_project_id: &str, new_index: usize, cx: &mut App) {
        let server_project_id = okena_core::client::strip_prefix(prefixed_project_id, conn_id);

        // Look up the server's folder structure from the cached state
        let server_folder_id = if let Some(ref get_folder) = this.get_remote_folder {
            (get_folder)(conn_id, prefixed_project_id, cx)
        } else {
            None
        };

        if let Some(folder_id) = server_folder_id {
            if let Some(ref send_action) = this.send_remote_action {
                (send_action)(conn_id, okena_core::api::ActionRequest::ReorderProjectInFolder {
                    folder_id,
                    project_id: server_project_id,
                    new_index,
                }, cx);
            }
        }
    }

    /// Renders only the folder header row (expand arrow, icon, name, badges)
    pub fn render_folder_header(
        &self,
        folder: &FolderData,
        index: usize,
        _project_count: usize,
        idle_terminal_count: usize,
        all_hidden: bool,
        is_cursor: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let folder_id = folder.id.clone();
        let folder_name = folder.name.clone();

        let is_renaming = is_renaming(&self.folder_rename, &folder.id);
        let ws = self.workspace.read(cx);
        let is_collapsed = ws.is_folder_collapsed(self.window_id, &folder.id);
        let is_active_filter = ws.active_folder_filter(self.window_id) == Some(&folder.id)
            && self.focus_manager.read(cx).focused_project_id().is_none();

        // Folder header row
        div()
            .id(ElementId::Name(format!("folder-row-{}", folder.id).into()))
            .h(px(24.0))
            .pl(px(4.0))
            .pr(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .when(is_active_filter, |d| d.bg(rgb(t.bg_hover)))
            .when(is_cursor, |d| d.border_l_2().border_color(rgb(t.border_active)))
            .when(all_hidden, |d| d.opacity(0.75))
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
            // Drop target for moving projects into this folder
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
                        broker.push_overlay_request(okena_workspace::requests::OverlayRequest::Folder(okena_workspace::requests::FolderOverlay {
                            folder_id: folder_id.clone(),
                            kind: okena_workspace::requests::FolderOverlayKind::ContextMenu {
                                folder_name: folder_name.clone(),
                                position: event.position,
                            },
                        }), cx);
                    });
                    cx.stop_propagation();
                }
            }))
            .on_click(cx.listener({
                let folder_id = folder_id.clone();
                move |this, _, _window, cx| {
                    this.cursor_index = None;
                    let window_id = this.window_id;
                    let workspace = this.workspace.clone();
                    this.focus_manager.update(cx, |fm, cx| {
                        workspace.update(cx, |ws, cx| {
                            ws.toggle_folder_focus(fm, window_id, &folder_id, cx);
                        });
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
                        let window_id = this.window_id;
                        this.workspace.update(cx, |ws, cx| {
                            ws.toggle_folder_collapsed(window_id, &folder_id, cx);
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
                    this.show_folder_color_picker(folder_id.clone(), event.position, cx);
                    cx.stop_propagation();
                }))
            })
            .child(
                // Folder name (or input if renaming)
                if is_renaming {
                    sidebar_rename_input("folder-rename-input", &self.folder_rename, &t, cx)
                        .map(|el| el.into_any_element())
                        .unwrap_or_else(|| div().flex_1().into_any_element())
                } else {
                    sidebar_name_label(
                        ElementId::Name(format!("folder-name-{}", folder.id).into()),
                        folder_name.clone(),
                        &t,
                        cx,
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
                                let window_id = this.window_id;
                                let workspace = this.workspace.clone();
                                let fid = folder_id.clone();
                                this.focus_manager.update(cx, |fm, cx| {
                                    workspace.update(cx, |ws, cx| {
                                        ws.toggle_folder_focus(fm, window_id, &fid, cx);
                                    });
                                });
                            }
                            cx.stop_propagation();
                        }
                    }))
                    .into_any_element()
                },
            )
            .when(idle_terminal_count > 0, |d| d.child(sidebar_idle_dot(&t)))
            .child(
                // Delete folder button (on hover)
                {
                    let folder_id = folder_id.clone();
                    icon_button(
                        ElementId::Name(format!("folder-delete-{}", folder_id).into()),
                        "icons/close.svg",
                        &t,
                    )
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
                        .tooltip(|_window, cx| Tooltip::new("Delete folder (keeps projects)").build(_window, cx))
                },
            )
    }

    /// Renders a project item inside a folder (indented)
    pub fn render_folder_project_item(
        &self,
        project: &SidebarProjectInfo,
        folder_id: &str,
        is_cursor: bool,
        is_focused_project: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let has_worktrees = project.worktree_count > 0;
        let is_expanded = self.expanded_projects.contains(&project.id);
        let project_id = project.id.clone();
        let project_name = project.name.clone();
        let folder_id = folder_id.to_string();

        let is_renaming = is_renaming(&self.project_rename, &project.id);

        let has_layout = project.has_layout;

        // Count idle terminals when project is collapsed (not expanded)
        let idle_count = if !is_expanded {
            self.count_waiting_terminals(&project.terminal_ids)
        } else {
            0
        };

        div()
            .id(ElementId::Name(format!("folder-project-row-{}", project.id).into()))
            .group("folder-project-item")
            .h(px(24.0))
            .pl(px(20.0))  // Indented for folder nesting
            .pr(px(8.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .when(is_focused_project, |d| d.bg(rgb(t.bg_hover)))
            .when(is_cursor, |d| d.border_l_2().border_color(rgb(t.border_active)))
            .when(!project.show_in_overview, |d| d.opacity(0.75))
            // Drag source
            .on_drag(ProjectDrag { project_id: project_id.clone(), project_name: project_name.clone() }, move |drag, _position, _window, cx| {
                cx.new(|_| ProjectDragView { name: drag.project_name.clone() })
            })
            // Drop target for reordering within folder
            .drag_over::<ProjectDrag>(move |style, _, _, _| {
                style.border_t_2().border_color(rgb(t.border_active))
            })
            .on_drop(cx.listener({
                let folder_id = folder_id.clone();
                let project_id = project_id.clone();
                move |this, drag: &ProjectDrag, _window, cx| {
                    if drag.project_id != project_id {
                        let pos = this.workspace.read(cx).folder(&folder_id)
                            .and_then(|f| f.project_ids.iter().position(|id| id == &project_id));
                        if let Some(pos) = pos {
                            this.workspace.update(cx, |ws, cx| {
                                ws.move_project_to_folder(&drag.project_id, &folder_id, Some(pos), cx);
                            });
                            // Send reorder to server for remote folders
                            if folder_id.starts_with("remote:") {
                                // Folder ID is "remote:{conn_id}:{folder_id}" — extract conn_id
                                if let Some(rest) = folder_id.strip_prefix("remote:") {
                                    if let Some(conn_id) = rest.split(':').next() {
                                        Self::send_remote_reorder(this, conn_id, &drag.project_id, pos, cx);
                                    }
                                }
                            }
                        }
                    }
                }
            }))
            // Also accept FolderDrag for top-level reordering
            .drag_over::<FolderDrag>(move |style, _, _, _| {
                style.border_t_2().border_color(rgb(t.border_active))
            })
            .on_drop(cx.listener(move |this, drag: &FolderDrag, _window, cx| {
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
                    let workspace = this.workspace.clone();
                    this.focus_manager.update(cx, |fm, cx| {
                        workspace.update(cx, |ws, cx| {
                            ws.set_focused_project_individual(fm, Some(project_id.clone()), cx);
                        });
                    });
                }
            }))
            .child({
                let has_expandable_content = has_layout || has_worktrees || !project.services.is_empty();
                if has_expandable_content {
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
                    }))
                    .into_any_element()
                } else {
                    div().flex_shrink_0().w(px(12.0)).h(px(16.0)).into_any_element()
                }
            })
            .child({
                // Project color dot
                let folder_color = t.get_folder_color(project.folder_color);
                let project_id = project.id.clone();
                sidebar_color_indicator(
                    ElementId::Name(format!("fp-folder-icon-{}", project.id).into()),
                    color_dot(folder_color, project.is_worktree),
                )
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.show_color_picker(project_id.clone(), event.position, cx);
                    cx.stop_propagation();
                }))
            })
            .child(
                // Project name (or input if renaming)
                if is_renaming {
                    sidebar_rename_input("fp-project-rename-input", &self.project_rename, &t, cx)
                        .map(|el| el.into_any_element())
                        .unwrap_or_else(|| div().flex_1().into_any_element())
                } else {
                    let name_label = sidebar_name_label(
                        ElementId::Name(format!("fp-project-name-{}", project.id).into()),
                        project_name.clone(),
                        &t,
                        cx,
                    )
                    .on_click(cx.listener({
                        let project_id = project_id.clone();
                        let project_name = project_name.clone();
                        move |this, _event: &ClickEvent, window, cx| {
                            if this.check_project_double_click(&project_id) {
                                this.start_project_rename(project_id.clone(), project_name.clone(), window, cx);
                            } else {
                                this.cursor_index = None;
                                let workspace = this.workspace.clone();
                                let pid = project_id.clone();
                                this.focus_manager.update(cx, |fm, cx| {
                                    workspace.update(cx, |ws, cx| {
                                        ws.set_focused_project_individual(fm, Some(pid), cx);
                                    });
                                });
                            }
                            cx.stop_propagation();
                        }
                    }));
                    sidebar_name_or_badge(name_label, &project_name, is_expanded || project.show_in_overview, project.terminal_ids.len(), &t, cx)
                },
            )
            .when(idle_count > 0, |d| d.child(sidebar_idle_dot(&t)))
            .child(
                sidebar_visibility_button(
                    ElementId::Name(format!("fp-visibility-{}", project.id).into()),
                    project.show_in_overview,
                    "folder-project-item",
                    if project.show_in_overview { "Hide Project" } else { "Show Project" },
                    &t,
                )
                .on_click(cx.listener({
                    let project_id = project_id.clone();
                    move |this, _, _window, cx| {
                        let window_id = this.window_id;
                        this.workspace.update(cx, |ws, cx| {
                            ws.toggle_project_overview_visibility(window_id, &project_id, cx);
                        });
                        cx.stop_propagation();
                    }
                }))
            )
    }
}
