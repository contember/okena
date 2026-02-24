//! UI request types for transient view-to-view communication.
//!
//! These types describe UI interactions (context menus, overlays, rename dialogs)
//! and are never persisted. They flow through `Workspace`'s request queues.

/// Request to show context menu at a position
#[derive(Clone, Debug)]
pub struct ContextMenuRequest {
    pub project_id: String,
    pub position: gpui::Point<gpui::Pixels>,
}

/// Request to show folder context menu at a position
#[derive(Clone, Debug)]
pub struct FolderContextMenuRequest {
    pub folder_id: String,
    pub folder_name: String,
    pub position: gpui::Point<gpui::Pixels>,
}

/// Requests consumed by RootView::process_pending_requests()
#[derive(Clone, Debug)]
pub enum OverlayRequest {
    ContextMenu { project_id: String, position: gpui::Point<gpui::Pixels> },
    FolderContextMenu { folder_id: String, folder_name: String, position: gpui::Point<gpui::Pixels> },
    ShellSelector { project_id: String, terminal_id: String, current_shell: crate::terminal::shell_config::ShellType },
    AddProjectDialog,
    DiffViewer { path: String, file: Option<String> },
    RemoteConnect,
    RemoteConnectionContextMenu { connection_id: String, connection_name: String, is_pairing: bool, position: gpui::Point<gpui::Pixels> },
    TerminalContextMenu {
        terminal_id: String,
        project_id: String,
        layout_path: Vec<usize>,
        position: gpui::Point<gpui::Pixels>,
        has_selection: bool,
    },
    TabContextMenu {
        tab_index: usize,
        num_tabs: usize,
        project_id: String,
        layout_path: Vec<usize>,
        position: gpui::Point<gpui::Pixels>,
    },
    ShowServiceLog { project_id: String, service_name: String },
}

/// Requests consumed by Sidebar::render()
#[derive(Clone, Debug)]
pub enum SidebarRequest {
    RenameProject { project_id: String, project_name: String },
    RenameFolder { folder_id: String, folder_name: String },
}
