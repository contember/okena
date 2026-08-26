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

/// Project-scoped overlay request. Carries a `project_id` once;
/// the specific overlay is in `kind`.
#[derive(Clone, Debug)]
pub struct ProjectOverlay {
    pub project_id: String,
    pub kind: ProjectOverlayKind,
}

/// How the terminal menu was opened and which contextual sections it needs.
#[derive(Clone, Debug)]
pub enum TerminalMenuInvocation {
    Content {
        has_selection: bool,
        link_url: Option<String>,
    },
    Header {
        include_primary_actions: bool,
    },
}

impl TerminalMenuInvocation {
    /// Selection/clipboard entries only make sense on a right-click in the content.
    pub fn includes_content_actions(&self) -> bool {
        matches!(self, Self::Content { .. })
    }

    /// Add-tab/split/close: shown unless the trigger already sits next to them.
    pub fn includes_primary_actions(&self) -> bool {
        match self {
            Self::Content { .. } => true,
            Self::Header {
                include_primary_actions,
            } => *include_primary_actions,
        }
    }
}

/// The specific overlay to show for a project.
#[derive(Clone, Debug)]
pub enum ProjectOverlayKind {
    ContextMenu {
        position: gpui::Point<gpui::Pixels>,
    },
    ShellSelector {
        terminal_id: String,
        current_shell: okena_terminal::shell_config::ShellType,
    },
    DiffViewer {
        file: Option<String>,
        mode: Option<okena_core::types::DiffMode>,
        commit_message: Option<String>,
        /// Commit list for navigation (prev/next) in the diff viewer.
        commits: Option<Vec<okena_git::CommitLogEntry>>,
        /// Current index into the commits list.
        commit_index: Option<usize>,
    },
    TerminalMenu {
        terminal_id: String,
        layout_path: Vec<usize>,
        position: gpui::Point<gpui::Pixels>,
        /// Resolved at the trigger, which already holds the pane's backend.
        can_export_buffer: bool,
        invocation: TerminalMenuInvocation,
    },
    /// Annotate the terminal's current selection and send it back to it.
    /// `position` anchors the composer, normally just under the selection.
    AnnotateSelection {
        terminal_id: String,
        position: gpui::Point<gpui::Pixels>,
    },
    TabContextMenu {
        tab_index: usize,
        num_tabs: usize,
        layout_path: Vec<usize>,
        position: gpui::Point<gpui::Pixels>,
    },
    ShowServiceLog {
        service_name: String,
    },
    ShowHookTerminal {
        terminal_id: String,
    },
    FileSearch,
    ContentSearch,
    FileBrowser,
    /// Open a specific file (project-relative path) in the file viewer.
    FileViewer {
        relative_path: String,
    },
    /// Resolve a terminal-emitted path on the daemon that owns the terminal.
    TerminalPathViewer {
        terminal_id: String,
        path: String,
        line: Option<u32>,
        column: Option<u32>,
    },
    ColorPicker {
        position: gpui::Point<gpui::Pixels>,
    },
    WorktreeList {
        position: gpui::Point<gpui::Pixels>,
    },
}

/// Folder-scoped overlay request. Carries a `folder_id` once;
/// the specific overlay is in `kind`.
#[derive(Clone, Debug)]
pub struct FolderOverlay {
    pub folder_id: String,
    pub kind: FolderOverlayKind,
}

/// The specific overlay to show for a folder.
#[derive(Clone, Debug)]
pub enum FolderOverlayKind {
    ContextMenu {
        folder_name: String,
        position: gpui::Point<gpui::Pixels>,
    },
    ColorPicker {
        position: gpui::Point<gpui::Pixels>,
    },
}

/// Requests consumed by WindowView::process_pending_requests().
///
/// Project-scoped and folder-scoped variants are grouped into
/// `ProjectOverlay` and `FolderOverlay` to avoid duplicating
/// `project_id` / `folder_id` across every variant. Global and
/// remote variants remain flat.
#[derive(Clone, Debug)]
pub enum OverlayRequest {
    Project(ProjectOverlay),
    Folder(FolderOverlay),
    AddProjectDialog,
    RemoteConnect,
    RemoteConnectionContextMenu {
        connection_id: String,
        connection_name: String,
        is_pairing: bool,
        /// Whether the connection currently uses TLS (drives the Upgrade item).
        tls: bool,
        position: gpui::Point<gpui::Pixels>,
    },
}

/// Requests consumed by Sidebar::render()
#[derive(Clone, Debug)]
pub enum SidebarRequest {
    RenameProject {
        project_id: String,
        project_name: String,
    },
    RenameFolder {
        folder_id: String,
        folder_name: String,
    },
    QuickCreateWorktree {
        project_id: String,
    },
}
