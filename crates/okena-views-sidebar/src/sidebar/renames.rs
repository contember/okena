//! Rename dialogs (terminal/project/folder), color picker requests, and
//! folder creation.

use super::Sidebar;
use gpui::*;
use okena_core::api::ActionRequest;
use okena_core::theme::FolderColor;
use okena_ui::rename_state::{cancel_rename, finish_rename, start_rename_with_blur};

impl Sidebar {
    pub fn start_rename(&mut self, project_id: String, terminal_id: String, current_name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal_rename = Some(start_rename_with_blur(
            (project_id, terminal_id),
            &current_name,
            "Terminal name...",
            |this, _window, cx| this.finish_rename(cx),
            window,
            cx,
        ));
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| ws.clear_focused_terminal(fm, cx));
        });
        cx.notify();
    }

    pub fn finish_rename(&mut self, cx: &mut Context<Self>) {
        if let Some(((project_id, terminal_id), new_name)) = finish_rename(&mut self.terminal_rename, cx) {
            self.dispatch_action_for_project(&project_id, ActionRequest::RenameTerminal {
                project_id: project_id.clone(),
                terminal_id,
                name: new_name,
            }, cx);
        }
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| ws.restore_focused_terminal(fm, cx));
        });
        cx.notify();
    }

    pub fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        cancel_rename(&mut self.terminal_rename);
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| ws.restore_focused_terminal(fm, cx));
        });
        cx.notify();
    }

    pub fn start_project_rename(&mut self, project_id: String, current_name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.project_rename = Some(start_rename_with_blur(
            project_id,
            &current_name,
            "Project name...",
            |this, _window, cx| this.finish_project_rename(cx),
            window,
            cx,
        ));
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| ws.clear_focused_terminal(fm, cx));
        });
        cx.notify();
    }

    pub fn finish_project_rename(&mut self, cx: &mut Context<Self>) {
        if let Some((project_id, new_name)) = finish_rename(&mut self.project_rename, cx) {
            self.workspace.update(cx, |ws, cx| {
                ws.rename_project(&project_id, new_name, cx);
            });
        }
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| ws.restore_focused_terminal(fm, cx));
        });
        cx.notify();
    }

    pub fn cancel_project_rename(&mut self, cx: &mut Context<Self>) {
        cancel_rename(&mut self.project_rename);
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| ws.restore_focused_terminal(fm, cx));
        });
        cx.notify();
    }

    /// Request to show color picker for a project (routed via OverlayManager).
    pub fn show_color_picker(&mut self, project_id: String, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        self.request_broker.update(cx, |broker, cx| {
            broker.push_overlay_request(okena_workspace::requests::OverlayRequest::Project(okena_workspace::requests::ProjectOverlay {
                project_id,
                kind: okena_workspace::requests::ProjectOverlayKind::ColorPicker { position },
            }), cx);
        });
    }

    /// Request to show color picker for a folder (routed via OverlayManager).
    pub fn show_folder_color_picker(&mut self, folder_id: String, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        self.request_broker.update(cx, |broker, cx| {
            broker.push_overlay_request(okena_workspace::requests::OverlayRequest::Folder(okena_workspace::requests::FolderOverlay {
                folder_id,
                kind: okena_workspace::requests::FolderOverlayKind::ColorPicker { position },
            }), cx);
        });
    }

    /// Sync a project color change to remote server (called when color picker emits event).
    pub fn sync_remote_color(&mut self, project_id: &str, color: FolderColor, cx: &mut Context<Self>) {
        if let Some(conn_id) = self.workspace.read(cx).project(project_id)
            .filter(|p| p.is_remote)
            .and_then(|p| p.connection_id.clone())
        {
            if let Some(ref send_action) = self.send_remote_action {
                let server_id = okena_core::client::strip_prefix(project_id, &conn_id);
                (send_action)(&conn_id, ActionRequest::SetProjectColor {
                    project_id: server_id,
                    color,
                }, cx);
            }
        }
    }

    pub fn start_folder_rename(&mut self, folder_id: String, current_name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.folder_rename = Some(start_rename_with_blur(
            folder_id,
            &current_name,
            "Folder name...",
            |this, _window, cx| this.finish_folder_rename(cx),
            window,
            cx,
        ));
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| ws.clear_focused_terminal(fm, cx));
        });
        cx.notify();
    }

    pub fn finish_folder_rename(&mut self, cx: &mut Context<Self>) {
        if let Some((folder_id, new_name)) = finish_rename(&mut self.folder_rename, cx) {
            self.workspace.update(cx, |ws, cx| {
                ws.rename_folder(&folder_id, new_name, cx);
            });
        }
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| ws.restore_focused_terminal(fm, cx));
        });
        cx.notify();
    }

    pub fn cancel_folder_rename(&mut self, cx: &mut Context<Self>) {
        cancel_rename(&mut self.folder_rename);
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| ws.restore_focused_terminal(fm, cx));
        });
        cx.notify();
    }

    pub(super) fn create_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let folder_id = self.workspace.update(cx, |ws, cx| {
            ws.create_folder("New Folder".to_string(), cx)
        });
        // Immediately start renaming the new folder
        self.start_folder_rename(folder_id, "New Folder".to_string(), window, cx);
    }
}
