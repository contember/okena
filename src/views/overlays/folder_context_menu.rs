//! Folder context menu overlay.

use crate::keybindings::Cancel;
use crate::theme::theme;
use crate::views::components::{menu_item, menu_item_with_color};
use crate::workspace::state::{FolderContextMenuRequest, Workspace};
use gpui::prelude::*;
use gpui::*;

/// Event emitted by FolderContextMenu
pub enum FolderContextMenuEvent {
    Close,
    RenameFolder { folder_id: String, folder_name: String },
    DeleteFolder { folder_id: String },
}

/// Folder context menu component
pub struct FolderContextMenu {
    workspace: Entity<Workspace>,
    request: FolderContextMenuRequest,
    focus_handle: FocusHandle,
}

impl FolderContextMenu {
    pub fn new(
        workspace: Entity<Workspace>,
        request: FolderContextMenuRequest,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        Self {
            workspace,
            request,
            focus_handle,
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(FolderContextMenuEvent::Close);
    }

    fn rename_folder(&self, cx: &mut Context<Self>) {
        cx.emit(FolderContextMenuEvent::RenameFolder {
            folder_id: self.request.folder_id.clone(),
            folder_name: self.request.folder_name.clone(),
        });
    }

    fn delete_folder(&self, cx: &mut Context<Self>) {
        cx.emit(FolderContextMenuEvent::DeleteFolder {
            folder_id: self.request.folder_id.clone(),
        });
    }
}

impl EventEmitter<FolderContextMenuEvent> for FolderContextMenu {}

impl Render for FolderContextMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Focus on first render
        window.focus(&self.focus_handle, cx);

        let position = self.request.position;

        // Get folder info
        let ws = self.workspace.read(cx);
        let folder = ws.folder(&self.request.folder_id);
        let project_count = folder.map(|f| f.project_ids.len()).unwrap_or(0);

        div()
            .track_focus(&self.focus_handle)
            .key_context("FolderContextMenu")
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            .absolute()
            .inset_0()
            .id("folder-context-menu-backdrop")
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                this.close(cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _window, cx| {
                this.close(cx);
            }))
            .child(deferred(
                anchored()
                    .position(position)
                    .snap_to_window()
                    .child(
                        div()
                            .bg(rgb(t.bg_primary))
                            .border_1()
                            .border_color(rgb(t.border))
                            .rounded(px(4.0))
                            .shadow_xl()
                            .min_w(px(160.0))
                            .py(px(4.0))
                            .id("folder-context-menu")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_scroll_wheel(|_, _, cx| {
                        cx.stop_propagation();
                    })
                    // Rename option
                    .child(
                        menu_item("folder-ctx-rename", "icons/edit.svg", "Rename Folder", &t)
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.rename_folder(cx);
                            })),
                    )
                    // Separator
                    .child(
                        div()
                            .h(px(1.0))
                            .mx(px(8.0))
                            .my(px(4.0))
                            .bg(rgb(t.border)),
                    )
                    // Delete option
                    .child(
                        menu_item_with_color(
                            "folder-ctx-delete",
                            "icons/trash.svg",
                            if project_count > 0 {
                                format!("Delete Folder ({} projects will be ungrouped)", project_count)
                            } else {
                                "Delete Folder".to_string()
                            },
                            t.error,
                            t.error,
                            &t,
                        )
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.delete_folder(cx);
                        })),
                    ),
                ),
            ))
    }
}

impl_focusable!(FolderContextMenu);
