//! UI request workspace actions
//!
//! Actions for managing UI dialogs and menus (context menu, shell selector, etc.)

use crate::workspace::state::Workspace;
use gpui::*;

impl Workspace {
    /// Clear the worktree dialog request
    pub fn clear_worktree_dialog_request(&mut self, cx: &mut Context<Self>) {
        self.worktree_dialog_request = None;
        cx.notify();
    }

    /// Request showing the context menu for a project
    pub fn request_context_menu(&mut self, project_id: &str, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        self.context_menu_request = Some(crate::workspace::state::ContextMenuRequest {
            project_id: project_id.to_string(),
            position,
        });
        cx.notify();
    }

    /// Clear the context menu request
    pub fn clear_context_menu_request(&mut self, cx: &mut Context<Self>) {
        self.context_menu_request = None;
        cx.notify();
    }

    /// Request showing the folder context menu
    pub fn request_folder_context_menu(
        &mut self,
        folder_id: &str,
        folder_name: &str,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.folder_context_menu_request = Some(crate::workspace::state::FolderContextMenuRequest {
            folder_id: folder_id.to_string(),
            folder_name: folder_name.to_string(),
            position,
        });
        cx.notify();
    }

    /// Clear the folder context menu request
    pub fn clear_folder_context_menu_request(&mut self, cx: &mut Context<Self>) {
        self.folder_context_menu_request = None;
        cx.notify();
    }

    /// Request showing the shell selector for a terminal
    pub fn request_shell_selector(
        &mut self,
        project_id: &str,
        terminal_id: &str,
        current_shell: crate::terminal::shell_config::ShellType,
        cx: &mut Context<Self>,
    ) {
        self.shell_selector_request = Some(crate::workspace::state::ShellSelectorRequest {
            project_id: project_id.to_string(),
            terminal_id: terminal_id.to_string(),
            current_shell,
        });
        cx.notify();
    }

    /// Clear the shell selector request
    pub fn clear_shell_selector_request(&mut self, cx: &mut Context<Self>) {
        self.shell_selector_request = None;
        cx.notify();
    }

    /// Request renaming a project (from context menu)
    pub fn request_project_rename(
        &mut self,
        project_id: &str,
        project_name: &str,
        cx: &mut Context<Self>,
    ) {
        self.pending_project_rename = Some(crate::workspace::state::ProjectRenameRequest {
            project_id: project_id.to_string(),
            project_name: project_name.to_string(),
        });
        cx.notify();
    }

    /// Clear the project rename request
    pub fn clear_project_rename_request(&mut self, cx: &mut Context<Self>) {
        self.pending_project_rename = None;
        cx.notify();
    }

    /// Clear the folder rename request
    pub fn clear_folder_rename_request(&mut self, cx: &mut Context<Self>) {
        self.pending_folder_rename = None;
        cx.notify();
    }

    /// Request showing the add project dialog
    pub fn request_add_project_dialog(&mut self, cx: &mut Context<Self>) {
        self.add_project_requested = true;
        cx.notify();
    }

    /// Clear the add project dialog request
    pub fn clear_add_project_dialog_request(&mut self, cx: &mut Context<Self>) {
        self.add_project_requested = false;
        cx.notify();
    }
}
