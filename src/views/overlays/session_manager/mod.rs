mod actions;
mod render;

use crate::views::components::SimpleInputState;
use crate::workspace::persistence::{list_sessions, SessionInfo};
use crate::workspace::state::{Workspace, WorkspaceData};
use gpui::*;

/// Session Manager overlay for managing multiple workspaces
pub struct SessionManager {
    pub(crate) workspace: Entity<Workspace>,
    pub(crate) focus_handle: FocusHandle,
    pub(crate) sessions: Vec<SessionInfo>,
    /// Input for new session name
    pub(crate) new_session_input: Entity<SimpleInputState>,
    /// Input for renaming session (created when rename starts)
    pub(crate) rename_input: Option<Entity<SimpleInputState>>,
    pub(crate) renaming_session: Option<String>,
    pub(crate) error_message: Option<String>,
    pub(crate) show_delete_confirmation: Option<String>,
    /// Input for export path
    pub(crate) export_path_input: Entity<SimpleInputState>,
    /// Input for import path
    pub(crate) import_path_input: Entity<SimpleInputState>,
    pub(crate) active_tab: SessionManagerTab,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum SessionManagerTab {
    Sessions,
    ExportImport,
}

impl SessionManager {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let sessions = list_sessions().unwrap_or_default();
        let focus_handle = cx.focus_handle();

        // Default export path
        let default_export_path = dirs::home_dir()
            .map(|p| p.join("workspace-export.json").to_string_lossy().to_string())
            .unwrap_or_else(|| "workspace-export.json".to_string());

        let new_session_input = cx.new(|cx| {
            SimpleInputState::new(cx).placeholder("Enter session name...")
        });

        let export_path_input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Enter export path...")
                .default_value(default_export_path)
        });

        let import_path_input = cx.new(|cx| {
            SimpleInputState::new(cx).placeholder("Enter path to import...")
        });

        Self {
            workspace,
            focus_handle,
            sessions,
            new_session_input,
            rename_input: None,
            renaming_session: None,
            error_message: None,
            show_delete_confirmation: None,
            export_path_input,
            import_path_input,
            active_tab: SessionManagerTab::Sessions,
        }
    }
}

pub enum SessionManagerEvent {
    Close,
    SwitchWorkspace(WorkspaceData),
}

impl EventEmitter<SessionManagerEvent> for SessionManager {}

impl_focusable!(SessionManager);
