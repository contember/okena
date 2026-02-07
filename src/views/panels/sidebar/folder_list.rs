//! Folder list rendering for the sidebar

use crate::keybindings::Cancel;
use crate::theme::theme;
use crate::views::components::{is_renaming, rename_input, SimpleInput};
use gpui::*;
use gpui::prelude::*;
use gpui_component::tooltip::Tooltip;

use super::{Sidebar, ProjectDrag, FolderDrag, FolderDragView};
use crate::workspace::state::{FolderData, ProjectData};
use std::collections::HashMap;

impl Sidebar {
    /// Renders a folder item with its contained projects
    pub(super) fn render_folder_item(
        &self,
        folder: &FolderData,
        index: usize,
        projects: &[ProjectData],
        worktree_children: &HashMap<String, Vec<ProjectData>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let folder_id = folder.id.clone();
        let folder_id_for_toggle = folder.id.clone();
        let folder_id_for_drag = folder.id.clone();
        let folder_id_for_rename = folder.id.clone();
        let folder_id_for_drop = folder.id.clone();
        let folder_name = folder.name.clone();
        let folder_name_for_drag = folder.name.clone();
        let folder_name_for_rename = folder.name.clone();
        let is_collapsed = folder.collapsed;
        let workspace_for_drop = self.workspace.clone();
        let workspace_for_toggle = self.workspace.clone();
        let workspace_for_reorder = self.workspace.clone();

        let is_renaming = is_renaming(&self.folder_rename, &folder.id);
        let project_count = projects.len();

        let mut container = div()
            .flex()
            .flex_col();

        // Folder header row
        container = container.child(
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
                // Drag source for folder reordering
                .on_drag(FolderDrag { folder_id: folder_id_for_drag.clone(), folder_name: folder_name_for_drag.clone() }, move |drag, _position, _window, cx| {
                    cx.new(|_| FolderDragView { name: drag.folder_name.clone() })
                })
                // Drop target for folder reordering
                .drag_over::<FolderDrag>(move |style, _, _, _| {
                    style.border_t_2().border_color(rgb(t.border_active))
                })
                .on_drop(cx.listener({
                    let workspace = workspace_for_reorder.clone();
                    let target_index = index;
                    move |_this, drag: &FolderDrag, _window, cx| {
                        if drag.folder_id != folder_id_for_drag {
                            workspace.update(cx, |ws, cx| {
                                ws.move_item_in_order(&drag.folder_id, target_index, cx);
                            });
                        }
                    }
                }))
                // Drop target for projects being dragged onto folder
                .drag_over::<ProjectDrag>(move |style, _, _, _| {
                    style.bg(rgb(t.bg_selection))
                })
                .on_drop(cx.listener({
                    let workspace = workspace_for_drop.clone();
                    let folder_id = folder_id_for_drop.clone();
                    move |_this, drag: &ProjectDrag, _window, cx| {
                        workspace.update(cx, |ws, cx| {
                            ws.move_project_to_folder(&drag.project_id, &folder_id, None, cx);
                        });
                    }
                }))
                // Right-click context menu
                .on_mouse_down(MouseButton::Right, cx.listener({
                    let folder_id = folder.id.clone();
                    let folder_name = folder.name.clone();
                    move |_this, event: &MouseDownEvent, _window, cx| {
                        _this.workspace.update(cx, |ws, cx| {
                            ws.push_overlay_request(crate::workspace::state::OverlayRequest::FolderContextMenu {
                                folder_id: folder_id.clone(),
                                folder_name: folder_name.clone(),
                                position: event.position,
                            }, cx);
                        });
                        cx.stop_propagation();
                    }
                }))
                .child(
                    // Expand/collapse arrow
                    div()
                        .id(ElementId::Name(format!("folder-expand-{}", folder.id).into()))
                        .flex_shrink_0()
                        .w(px(16.0))
                        .h(px(16.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            svg()
                                .path(if is_collapsed { "icons/chevron-right.svg" } else { "icons/chevron-down.svg" })
                                .size(px(12.0))
                                .text_color(rgb(t.text_secondary))
                        )
                        .on_click(cx.listener(move |_this, _, _window, cx| {
                            workspace_for_toggle.update(cx, |ws, cx| {
                                ws.toggle_folder_collapsed(&folder_id_for_toggle, cx);
                            });
                        })),
                )
                .child({
                    // Folder color dot
                    let folder_color = t.get_folder_color(folder.folder_color);
                    let folder_id_for_color = folder.id.clone();
                    div()
                        .id(ElementId::Name(format!("folder-color-{}", folder.id).into()))
                        .flex_shrink_0()
                        .w(px(16.0))
                        .h(px(16.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.opacity(0.7))
                        .child(
                            svg()
                                .path("icons/folder.svg")
                                .size(px(14.0))
                                .text_color(rgb(folder_color))
                        )
                        .on_click(cx.listener(move |this, _event: &ClickEvent, _window, cx| {
                            this.show_folder_color_picker(folder_id_for_color.clone(), cx);
                            cx.stop_propagation();
                        }))
                })
                .child(
                    // Folder name (or input if renaming)
                    if is_renaming {
                        if let Some(input) = rename_input(&self.folder_rename) {
                            div()
                                .id("folder-rename-input")
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
                                .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                                    this.cancel_folder_rename(cx);
                                }))
                                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                    match event.keystroke.key.as_str() {
                                        "enter" => this.finish_folder_rename(cx),
                                        _ => {}
                                    }
                                }))
                                .into_any_element()
                        } else {
                            div().flex_1().into_any_element()
                        }
                    } else {
                        div()
                            .id(ElementId::Name(format!("folder-name-{}", folder.id).into()))
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(t.text_primary))
                            .text_ellipsis()
                            .child(folder_name)
                            .on_click(cx.listener({
                                let folder_id = folder_id_for_rename;
                                let name = folder_name_for_rename;
                                move |this, _event: &ClickEvent, window, cx| {
                                    if this.check_folder_double_click(&folder_id) {
                                        this.start_folder_rename(folder_id.clone(), name.clone(), window, cx);
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
                        let workspace = self.workspace.clone();
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
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(move |_, _window, cx| {
                                cx.stop_propagation();
                                workspace.update(cx, |ws, cx| {
                                    ws.delete_folder(&folder_id, cx);
                                });
                            })
                            .child(
                                svg()
                                    .path("icons/close.svg")
                                    .size(px(12.0))
                                    .text_color(rgb(t.text_muted))
                            )
                            .tooltip(|_window, cx| Tooltip::new("Delete folder (keeps projects)").build(_window, cx))
                    },
                ),
        );

        // Render folder children when not collapsed
        if !is_collapsed {
            for project in projects {
                let wt_children = worktree_children.get(&project.id);
                container = container.child(
                    self.render_folder_project_item(project, &folder.id, wt_children, window, cx)
                );
            }
        }

        container
    }

    /// Renders a project item inside a folder (indented)
    fn render_folder_project_item(
        &self,
        project: &ProjectData,
        folder_id: &str,
        worktree_children: Option<&Vec<ProjectData>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let is_expanded = self.expanded_projects.contains(&project.id);
        let workspace_for_focus = self.workspace.clone();
        let workspace_for_drop = self.workspace.clone();
        let workspace_for_drop_out = self.workspace.clone();
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
        let folder_id = folder_id.to_string();
        let folder_id_for_drop = folder_id.clone();

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

        let mut container = div()
            .flex()
            .flex_col();

        container = container.child(
            div()
                .id(ElementId::Name(format!("folder-project-row-{}", project.id).into()))
                .h(px(24.0))
                .pl(px(28.0))  // Indented for folder nesting
                .pr(px(8.0))
                .flex()
                .items_center()
                .gap(px(4.0))
                .cursor_pointer()
                .when(is_focused, |d| d.bg(rgb(t.bg_selection)))
                .when(!is_focused, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
                // Drag source
                .on_drag(super::ProjectDrag { project_id: project_id_for_drag.clone(), project_name: project_name_for_drag.clone() }, move |drag, _position, _window, cx| {
                    cx.new(|_| super::ProjectDragView { name: drag.project_name.clone() })
                })
                // Drop target for reordering within folder
                .drag_over::<super::ProjectDrag>(move |style, _, _, _| {
                    style.border_t_2().border_color(rgb(t.border_active))
                })
                .on_drop(cx.listener({
                    let folder_id = folder_id_for_drop.clone();
                    let workspace = workspace_for_drop.clone();
                    let project_id = project_id_for_drag.clone();
                    move |_this, drag: &super::ProjectDrag, _window, cx| {
                        if drag.project_id != project_id {
                            // Find position of this project in the folder
                            let pos = workspace.read(cx).folder(&folder_id)
                                .and_then(|f| f.project_ids.iter().position(|id| id == &project_id));
                            if let Some(pos) = pos {
                                workspace.update(cx, |ws, cx| {
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
                .on_drop(cx.listener({
                    let workspace = workspace_for_drop_out;
                    move |_this, drag: &super::FolderDrag, _window, cx| {
                        // Reorder folder in project_order (no-op if same position)
                        workspace.update(cx, |ws, cx| {
                            ws.move_item_in_order(&drag.folder_id, 0, cx);
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
                        .id(ElementId::Name(format!("expand-fp-{}", project.id).into()))
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
                    // Project color dot
                    let folder_color = t.get_folder_color(project.folder_color);
                    let project_id_for_color = project.id.clone();
                    div()
                        .id(ElementId::Name(format!("fp-folder-icon-{}", project.id).into()))
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
                                .id("fp-project-rename-input")
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
                                .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                                    this.cancel_project_rename(cx);
                                }))
                                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                    match event.keystroke.key.as_str() {
                                        "enter" => this.finish_project_rename(cx),
                                        _ => {}
                                    }
                                }))
                                .into_any_element()
                        } else {
                            div().flex_1().into_any_element()
                        }
                    } else {
                        div()
                            .id(ElementId::Name(format!("fp-project-name-{}", project.id).into()))
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
                    // Terminal count badge
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
                            .id(ElementId::Name(format!("fp-visibility-{}", project.id).into()))
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
        );

        // Terminal items when expanded
        if is_expanded {
            let minimized_states: Vec<(String, bool)> = {
                let ws = self.workspace.read(cx);
                terminal_ids.iter().map(|id| {
                    let is_minimized = ws.is_terminal_minimized(&project_id, id);
                    (id.clone(), is_minimized)
                }).collect()
            };

            for (id, is_minimized) in &minimized_states {
                container = container.child(
                    self.render_terminal_item(&project_id, id, project, *is_minimized, 28.0, "", cx)
                );
            }
        }

        // Worktree children
        if let Some(children) = worktree_children {
            for child in children {
                container = container.child(self.render_worktree_item(child, window, cx));
            }
        }

        container
    }
}
