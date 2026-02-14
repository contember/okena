//! Unified action dispatch — routes terminal actions to local or remote execution.
//!
//! The `ActionDispatcher` enum encapsulates the local-vs-remote routing decision.
//! Callers simply call `dispatcher.dispatch(action, cx)` without any conditionals.

use crate::remote_client::manager::RemoteConnectionManager;
use crate::terminal::backend::TerminalBackend;
use crate::views::root::TerminalsRegistry;
use crate::workspace::actions::execute::execute_action;
use crate::workspace::state::Workspace;

use okena_core::api::ActionRequest;
use okena_core::client::strip_prefix;

use gpui::{AppContext, Entity};
use std::sync::Arc;

/// Routes terminal actions to either local execution or remote HTTP.
///
/// Passed through the view hierarchy (ProjectColumn → LayoutContainer → TerminalPane)
/// so all action handlers dispatch through this without knowing if the project is
/// local or remote.
#[derive(Clone)]
pub enum ActionDispatcher {
    /// Local project — execute actions directly in the workspace.
    Local {
        workspace: Entity<Workspace>,
        backend: Arc<dyn TerminalBackend>,
        terminals: TerminalsRegistry,
    },
    /// Remote project — send actions via HTTP to the remote server.
    Remote {
        connection_id: String,
        manager: Entity<RemoteConnectionManager>,
    },
}

impl ActionDispatcher {
    #[allow(dead_code)]
    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Remote { .. })
    }

    /// Dispatch a standard action (split, close, create terminal, etc.).
    pub fn dispatch(&self, action: ActionRequest, cx: &mut impl AppContext) {
        match self {
            Self::Local {
                workspace,
                backend,
                terminals,
            } => {
                let backend = backend.clone();
                let terminals = terminals.clone();
                workspace.update(cx, |ws, cx| {
                    execute_action(action, ws, &*backend, &terminals, cx);
                });
            }
            Self::Remote {
                connection_id,
                manager,
            } => {
                let action = strip_remote_terminal_prefix(action, connection_id);
                let cid = connection_id.clone();
                manager.update(cx, |rm, cx| {
                    rm.send_action(&cid, action, cx);
                });
            }
        }
    }

    /// Add a tab (local: workspace layout operation; remote: create terminal).
    pub fn add_tab(
        &self,
        project_id: &str,
        layout_path: &[usize],
        in_group: bool,
        cx: &mut impl AppContext,
    ) {
        match self {
            Self::Local { workspace, .. } => {
                let pid = project_id.to_string();
                let lp = layout_path.to_vec();
                workspace.update(cx, |ws, cx| {
                    if in_group {
                        ws.add_tab_to_group(&pid, &lp, cx);
                    } else {
                        ws.add_tab(&pid, &lp, cx);
                    }
                });
            }
            Self::Remote { .. } => {
                self.dispatch(
                    ActionRequest::AddTab {
                        project_id: project_id.to_string(),
                        path: layout_path.to_vec(),
                        in_group,
                    },
                    cx,
                );
            }
        }
    }
}

/// Strip the `remote:{connection_id}:` prefix from terminal IDs before sending to server.
fn strip_remote_terminal_prefix(action: ActionRequest, connection_id: &str) -> ActionRequest {
    let prefix = format!("remote:{}", connection_id);
    match action {
        ActionRequest::CloseTerminal {
            project_id,
            terminal_id,
        } => ActionRequest::CloseTerminal {
            project_id,
            terminal_id: strip_prefix(&terminal_id, &prefix),
        },
        ActionRequest::CloseTerminals {
            project_id,
            terminal_ids,
        } => ActionRequest::CloseTerminals {
            project_id,
            terminal_ids: terminal_ids
                .into_iter()
                .map(|id| strip_prefix(&id, &prefix))
                .collect(),
        },
        ActionRequest::ToggleMinimized {
            project_id,
            terminal_id,
        } => ActionRequest::ToggleMinimized {
            project_id,
            terminal_id: strip_prefix(&terminal_id, &prefix),
        },
        ActionRequest::SetFullscreen {
            project_id,
            terminal_id,
        } => ActionRequest::SetFullscreen {
            project_id,
            terminal_id: terminal_id.map(|id| strip_prefix(&id, &prefix)),
        },
        ActionRequest::RenameTerminal {
            project_id,
            terminal_id,
            name,
        } => ActionRequest::RenameTerminal {
            project_id,
            terminal_id: strip_prefix(&terminal_id, &prefix),
            name,
        },
        other => other,
    }
}
