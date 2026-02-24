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
    /// Visual/presentation actions (split sizes, minimize, fullscreen, active tab, focus)
    /// are executed locally on the client workspace to avoid server round-trips
    /// and to survive state syncs.
    Remote {
        connection_id: String,
        manager: Entity<RemoteConnectionManager>,
        workspace: Entity<Workspace>,
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
                workspace,
            } => {
                // Visual/presentation actions are executed locally on the client
                // workspace. They never reach the server, so each client has
                // independent visual state that survives state syncs.
                match &action {
                    ActionRequest::UpdateSplitSizes { project_id, path, sizes } => {
                        let pid = project_id.clone();
                        let p = path.clone();
                        let s = sizes.clone();
                        workspace.update(cx, |ws, cx| {
                            ws.update_split_sizes(&pid, &p, s, cx);
                        });
                        return;
                    }
                    ActionRequest::ToggleMinimized { project_id, terminal_id } => {
                        let pid = project_id.clone();
                        let tid = terminal_id.clone();
                        workspace.update(cx, |ws, cx| {
                            ws.toggle_terminal_minimized_by_id(&pid, &tid, cx);
                        });
                        return;
                    }
                    ActionRequest::SetFullscreen { project_id, terminal_id } => {
                        let pid = project_id.clone();
                        let tid = terminal_id.clone();
                        workspace.update(cx, |ws, cx| {
                            match tid {
                                Some(tid) => ws.set_fullscreen_terminal(pid, tid, cx),
                                None => ws.exit_fullscreen(cx),
                            }
                        });
                        return;
                    }
                    ActionRequest::SetActiveTab { project_id, path, index } => {
                        let pid = project_id.clone();
                        let p = path.clone();
                        let idx = *index;
                        workspace.update(cx, |ws, cx| {
                            ws.set_active_tab(&pid, &p, idx, cx);
                        });
                        return;
                    }
                    ActionRequest::FocusTerminal { project_id, terminal_id } => {
                        let pid = project_id.clone();
                        let tid = terminal_id.clone();
                        workspace.update(cx, |ws, cx| {
                            if let Some(project) = ws.project(&pid) {
                                if let Some(ref layout) = project.layout {
                                    if let Some(path) = layout.find_terminal_path(&tid) {
                                        ws.set_focused_terminal(pid, path, cx);
                                    }
                                }
                            }
                        });
                        return;
                    }
                    _ => {}
                }

                let action = strip_remote_ids(action, connection_id);
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

/// Strip the `remote:{connection_id}:` prefix from terminal and project IDs before sending to server.
fn strip_remote_ids(action: ActionRequest, connection_id: &str) -> ActionRequest {
    let s = |id: &str| strip_prefix(id, connection_id);
    match action {
        ActionRequest::SendText { terminal_id, text } => ActionRequest::SendText {
            terminal_id: s(&terminal_id),
            text,
        },
        ActionRequest::RunCommand {
            terminal_id,
            command,
        } => ActionRequest::RunCommand {
            terminal_id: s(&terminal_id),
            command,
        },
        ActionRequest::SendSpecialKey { terminal_id, key } => ActionRequest::SendSpecialKey {
            terminal_id: s(&terminal_id),
            key,
        },
        ActionRequest::SplitTerminal {
            project_id,
            path,
            direction,
        } => ActionRequest::SplitTerminal {
            project_id: s(&project_id),
            path,
            direction,
        },
        ActionRequest::CloseTerminal {
            project_id,
            terminal_id,
        } => ActionRequest::CloseTerminal {
            project_id: s(&project_id),
            terminal_id: s(&terminal_id),
        },
        ActionRequest::CloseTerminals {
            project_id,
            terminal_ids,
        } => ActionRequest::CloseTerminals {
            project_id: s(&project_id),
            terminal_ids: terminal_ids.iter().map(|id| s(id)).collect(),
        },
        ActionRequest::FocusTerminal {
            project_id,
            terminal_id,
        } => ActionRequest::FocusTerminal {
            project_id: s(&project_id),
            terminal_id: s(&terminal_id),
        },
        ActionRequest::ReadContent { terminal_id } => ActionRequest::ReadContent {
            terminal_id: s(&terminal_id),
        },
        ActionRequest::Resize {
            terminal_id,
            cols,
            rows,
        } => ActionRequest::Resize {
            terminal_id: s(&terminal_id),
            cols,
            rows,
        },
        ActionRequest::CreateTerminal { project_id } => ActionRequest::CreateTerminal {
            project_id: s(&project_id),
        },
        ActionRequest::UpdateSplitSizes {
            project_id,
            path,
            sizes,
        } => ActionRequest::UpdateSplitSizes {
            project_id: s(&project_id),
            path,
            sizes,
        },
        ActionRequest::ToggleMinimized {
            project_id,
            terminal_id,
        } => ActionRequest::ToggleMinimized {
            project_id: s(&project_id),
            terminal_id: s(&terminal_id),
        },
        ActionRequest::SetFullscreen {
            project_id,
            terminal_id,
        } => ActionRequest::SetFullscreen {
            project_id: s(&project_id),
            terminal_id: terminal_id.map(|id| s(&id)),
        },
        ActionRequest::RenameTerminal {
            project_id,
            terminal_id,
            name,
        } => ActionRequest::RenameTerminal {
            project_id: s(&project_id),
            terminal_id: s(&terminal_id),
            name,
        },
        ActionRequest::AddTab {
            project_id,
            path,
            in_group,
        } => ActionRequest::AddTab {
            project_id: s(&project_id),
            path,
            in_group,
        },
        ActionRequest::SetActiveTab {
            project_id,
            path,
            index,
        } => ActionRequest::SetActiveTab {
            project_id: s(&project_id),
            path,
            index,
        },
        ActionRequest::MoveTab {
            project_id,
            path,
            from_index,
            to_index,
        } => ActionRequest::MoveTab {
            project_id: s(&project_id),
            path,
            from_index,
            to_index,
        },
        ActionRequest::MoveTerminalToTabGroup {
            project_id,
            terminal_id,
            target_path,
            position,
        } => ActionRequest::MoveTerminalToTabGroup {
            project_id: s(&project_id),
            terminal_id: s(&terminal_id),
            target_path,
            position,
        },
        ActionRequest::MovePaneTo {
            project_id,
            terminal_id,
            target_project_id,
            target_terminal_id,
            zone,
        } => ActionRequest::MovePaneTo {
            project_id: s(&project_id),
            terminal_id: s(&terminal_id),
            target_project_id: s(&target_project_id),
            target_terminal_id: s(&target_terminal_id),
            zone,
        },
        ActionRequest::GitStatus { project_id } => ActionRequest::GitStatus {
            project_id: s(&project_id),
        },
        ActionRequest::GitDiffSummary { project_id } => ActionRequest::GitDiffSummary {
            project_id: s(&project_id),
        },
        ActionRequest::GitDiff {
            project_id,
            mode,
            ignore_whitespace,
        } => ActionRequest::GitDiff {
            project_id: s(&project_id),
            mode,
            ignore_whitespace,
        },
        ActionRequest::GitBranches { project_id } => ActionRequest::GitBranches {
            project_id: s(&project_id),
        },
        ActionRequest::GitFileContents {
            project_id,
            file_path,
            mode,
        } => ActionRequest::GitFileContents {
            project_id: s(&project_id),
            file_path,
            mode,
        },
        ActionRequest::CreateApp {
            project_id,
            app_kind,
            app_config,
        } => ActionRequest::CreateApp {
            project_id: s(&project_id),
            app_kind,
            app_config,
        },
        ActionRequest::CloseApp {
            project_id,
            app_id,
        } => ActionRequest::CloseApp {
            project_id: s(&project_id),
            app_id: s(&app_id),
        },
        ActionRequest::AppAction {
            project_id,
            app_id,
            payload,
        } => ActionRequest::AppAction {
            project_id: s(&project_id),
            app_id: s(&app_id),
            payload,
        },
    }
}
