//! Folder context menu overlay.

use crate::Cancel;
use okena_ui::menu::{context_menu_panel, menu_item, menu_item_with_color, menu_separator};
use okena_ui::theme::theme;
use okena_workspace::requests::FolderContextMenuRequest;
use okena_workspace::state::{WindowId, Workspace};
use gpui::prelude::*;
use gpui::*;

/// Event emitted by FolderContextMenu
pub enum FolderContextMenuEvent {
    Close,
    RenameFolder { folder_id: String, folder_name: String },
    DeleteFolder { folder_id: String },
    FilterToFolder { folder_id: String },
}

impl okena_ui::overlay::CloseEvent for FolderContextMenuEvent {
    fn is_close(&self) -> bool { matches!(self, Self::Close) }
}

/// Folder context menu component
pub struct FolderContextMenu {
    /// Identifies which window-scoped slot on the shared `Workspace` this
    /// folder context menu addresses. Always `WindowId::Main` today
    /// (single-window runtime); slice 05 spawns extras that mint distinct
    /// `WindowId::Extra(uuid)`s. Read directly in `render` via
    /// `ws.active_folder_filter(self.window_id)` -- a direct `&self` field
    /// access on a disjoint field from the `ws` read borrow, no hoist
    /// needed (WindowId is Copy).
    pub(crate) window_id: WindowId,
    workspace: Entity<Workspace>,
    request: FolderContextMenuRequest,
    focus_handle: FocusHandle,
}

impl FolderContextMenu {
    pub fn new(
        window_id: WindowId,
        workspace: Entity<Workspace>,
        request: FolderContextMenuRequest,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        Self {
            window_id,
            workspace,
            request,
            focus_handle,
        }
    }

    /// Identifies which window-scoped slot on the shared `Workspace` this
    /// folder context menu addresses. Always `WindowId::Main` today
    /// (single-window runtime); slice 05 spawns extras that mint distinct
    /// `WindowId::Extra(uuid)`s. The field is read directly within `render`
    /// via `self.window_id`; this public getter exists for external callers
    /// (e.g. the slice 05 spawn flow on `Okena`) that need to address
    /// window-scoped state on `Workspace` in the same window this menu
    /// inhabits. `#[allow(dead_code)]` because no external caller reads it
    /// yet -- rustc tracks fields and methods separately, so the field
    /// being used by the ctor + the render path does NOT mark the getter
    /// as used.
    #[allow(dead_code)]
    pub fn window_id(&self) -> WindowId {
        self.window_id
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

    fn toggle_folder_filter(&self, cx: &mut Context<Self>) {
        cx.emit(FolderContextMenuEvent::FilterToFolder {
            folder_id: self.request.folder_id.clone(),
        });
    }
}

impl EventEmitter<FolderContextMenuEvent> for FolderContextMenu {}

impl Render for FolderContextMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Focus on first render
        if !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }

        let position = self.request.position;

        // Get folder info
        let ws = self.workspace.read(cx);
        let folder = ws.folder(&self.request.folder_id);
        let project_count = folder.map(|f| f.project_ids.len()).unwrap_or(0);
        let is_active_filter = ws.active_folder_filter(self.window_id) == Some(&self.request.folder_id);

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
                        context_menu_panel("folder-context-menu", &t)
                    // Filter option
                    .child(
                        menu_item(
                            "folder-ctx-filter",
                            if is_active_filter { "icons/eye-off.svg" } else { "icons/eye.svg" },
                            if is_active_filter { "Show All Projects" } else { "Show Only This Folder" },
                            &t,
                        )
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.toggle_folder_filter(cx);
                        })),
                    )
                    // Rename option
                    .child(
                        menu_item("folder-ctx-rename", "icons/edit.svg", "Rename Folder", &t)
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.rename_folder(cx);
                            })),
                    )
                    // Separator
                    .child(menu_separator(&t))
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

okena_ui::impl_focusable!(FolderContextMenu);
