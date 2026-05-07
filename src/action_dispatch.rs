//! Unified action dispatch — routes terminal actions to local or remote execution.
//!
//! The `ActionDispatcher` enum encapsulates the local-vs-remote routing decision.
//! Callers simply call `dispatcher.dispatch(action, cx)` without any conditionals.

use crate::remote_client::manager::RemoteConnectionManager;
use crate::services::manager::ServiceManager;
use crate::terminal::backend::TerminalBackend;
use crate::views::window::TerminalsRegistry;
use crate::workspace::actions::execute::execute_action;
use crate::workspace::focus::FocusManager;
use crate::workspace::state::{WindowId, Workspace};

use okena_core::api::ActionRequest;
use okena_core::client::strip_prefix;

use gpui::{AppContext, Entity};
use std::sync::Arc;

/// Build an ActionDispatcher for the given project.
///
/// Returns `Remote` variant for remote projects, `Local` for local ones.
/// Returns `None` if required dependencies (backend, remote manager) are unavailable.
///
/// `window_id` carries the originating `WindowView`'s window id so per-window
/// state mutations triggered by local UI actions (e.g. hide/show via the
/// sidebar context menu routed through `SetProjectShowInOverview`) land on
/// the right window's slot. Remote projects also carry `window_id` so a UI
/// action issued in W2 against a remote project mutates W2's per-window
/// state on the local mirror, not main's.
pub fn dispatcher_for_project(
    project_id: &str,
    window_id: WindowId,
    workspace: &Entity<Workspace>,
    focus_manager: &Entity<FocusManager>,
    backend: &Option<Arc<dyn TerminalBackend>>,
    terminals: &TerminalsRegistry,
    service_manager: &Option<Entity<ServiceManager>>,
    remote_manager: &Option<Entity<RemoteConnectionManager>>,
    cx: &gpui::App,
) -> Option<ActionDispatcher> {
    let ws = workspace.read(cx);
    let project = ws.project(project_id)?;
    if project.is_remote {
        let connection_id = project.connection_id.as_ref()?;
        let manager = remote_manager.as_ref()?;
        let _ = window_id; // Remote variant doesn't carry window_id today
        Some(ActionDispatcher::Remote {
            connection_id: connection_id.clone(),
            manager: manager.clone(),
            workspace: workspace.clone(),
            focus_manager: focus_manager.clone(),
        })
    } else {
        let backend = backend.as_ref()?;
        Some(ActionDispatcher::Local {
            workspace: workspace.clone(),
            focus_manager: focus_manager.clone(),
            backend: backend.clone(),
            terminals: terminals.clone(),
            service_manager: service_manager.clone(),
            window_id,
        })
    }
}

/// Routes terminal and service actions to either local execution or remote HTTP.
///
/// Passed through the view hierarchy (ProjectColumn → LayoutContainer → TerminalPane)
/// so all action handlers dispatch through this without knowing if the project is
/// local or remote.
#[derive(Clone)]
pub enum ActionDispatcher {
    /// Local project — execute actions directly in the workspace.
    Local {
        workspace: Entity<Workspace>,
        focus_manager: Entity<FocusManager>,
        backend: Arc<dyn TerminalBackend>,
        terminals: TerminalsRegistry,
        service_manager: Option<Entity<ServiceManager>>,
        /// Originating window's id (PRD cri 13). Per-window state mutations
        /// inside `execute_action` (e.g. `SetProjectShowInOverview`) target
        /// this slot.
        window_id: WindowId,
    },
    /// Remote project — send actions via HTTP to the remote server.
    /// Visual/presentation actions (split sizes, minimize, fullscreen, active tab, focus)
    /// are executed locally on the client workspace to avoid server round-trips
    /// and to survive state syncs. No `window_id` field today: visual mirror
    /// actions touch focus state via the (already-per-window) `focus_manager`,
    /// and per-window data mutations have no remote counterpart yet.
    Remote {
        connection_id: String,
        manager: Entity<RemoteConnectionManager>,
        workspace: Entity<Workspace>,
        focus_manager: Entity<FocusManager>,
    },
}

impl ActionDispatcher {
    #[allow(dead_code)]
    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Remote { .. })
    }

    /// Dispatch a standard action (split, close, create terminal, service action, etc.).
    pub fn dispatch(&self, action: ActionRequest, cx: &mut impl AppContext) {
        match self {
            Self::Local {
                workspace,
                focus_manager,
                backend,
                terminals,
                service_manager,
                window_id,
            } => {
                // Intercept service actions — these need ServiceManager, not execute_action
                if let Some(sm) = service_manager {
                    match &action {
                        ActionRequest::StartService { project_id, service_name } => {
                            sm.update(cx, |sm, cx| {
                                if let Some(path) = sm.project_path(project_id).cloned() {
                                    sm.start_service(project_id, service_name, &path, cx);
                                }
                            });
                            return;
                        }
                        ActionRequest::StopService { project_id, service_name } => {
                            sm.update(cx, |sm, cx| sm.stop_service(project_id, service_name, cx));
                            return;
                        }
                        ActionRequest::RestartService { project_id, service_name } => {
                            sm.update(cx, |sm, cx| {
                                if let Some(path) = sm.project_path(project_id).cloned() {
                                    sm.restart_service(project_id, service_name, &path, cx);
                                }
                            });
                            return;
                        }
                        ActionRequest::StartAllServices { project_id } => {
                            sm.update(cx, |sm, cx| {
                                if let Some(path) = sm.project_path(project_id).cloned() {
                                    sm.start_all(project_id, &path, cx);
                                }
                            });
                            return;
                        }
                        ActionRequest::StopAllServices { project_id } => {
                            sm.update(cx, |sm, cx| sm.stop_all(project_id, cx));
                            return;
                        }
                        ActionRequest::ReloadServices { project_id } => {
                            sm.update(cx, |sm, cx| {
                                if let Some(path) = sm.project_path(project_id).cloned() {
                                    sm.reload_project_services(project_id, &path, cx);
                                }
                            });
                            return;
                        }
                        _ => {}
                    }
                }

                let backend = backend.clone();
                let terminals = terminals.clone();
                let focus_manager = focus_manager.clone();
                let window_id = *window_id;
                focus_manager.update(cx, |fm, cx| {
                    workspace.update(cx, |ws, cx| {
                        execute_action(action, ws, window_id, fm, &*backend, &terminals, cx);
                    });
                });
            }
            Self::Remote {
                connection_id,
                manager,
                workspace,
                focus_manager,
            } => {
                // Visual/presentation actions are executed locally on the client
                // workspace. They never reach the server, so each client has
                // independent visual state that survives state syncs.
                match &action {
                    ActionRequest::UpdateSplitSizes { project_id, path, sizes } => {
                        let pid = project_id.clone();
                        let p = path.clone();
                        let s = sizes.clone();
                        // Use UI-only notify during drag to avoid auto-save spam;
                        // final sizes are persisted on mouse-up.
                        workspace.update(cx, |ws, cx| {
                            ws.update_split_sizes_ui_only(&pid, &p, s, cx);
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
                        let focus_manager = focus_manager.clone();
                        focus_manager.update(cx, |fm, cx| {
                            workspace.update(cx, |ws, cx| {
                                match tid {
                                    Some(tid) => ws.set_fullscreen_terminal(fm, pid, tid, cx),
                                    None => ws.exit_fullscreen(fm, cx),
                                }
                            });
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
                        let focus_manager = focus_manager.clone();
                        focus_manager.update(cx, |fm, cx| {
                            workspace.update(cx, |ws, cx| {
                                if let Some(project) = ws.project(&pid) {
                                    if let Some(ref layout) = project.layout {
                                        if let Some(path) = layout.find_terminal_path(&tid) {
                                            ws.set_focused_terminal(fm, pid, path, cx);
                                        }
                                    }
                                }
                            });
                        });
                        return;
                    }
                    ActionRequest::CreateTerminal { project_id } => {
                        // Record pending focus — the actual focus will happen when
                        // the next state sync brings the new terminal into the
                        // client's layout (see sync_remote_projects_into_workspace).
                        let pid = project_id.clone();
                        workspace.update(cx, |ws, _cx| {
                            ws.queue_pending_remote_focus(&pid);
                        });
                        // Don't return — action proceeds to be sent to server below
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

    /// Split a terminal (local: workspace layout operation; remote: via server).
    ///
    /// For local projects this only modifies the layout — the UI will lazily
    /// spawn the PTY with the correct shell.  Going through `execute_action`
    /// would eagerly call `spawn_uninitialized_terminals` with `None` shell,
    /// ignoring the project / global default shell (e.g. WSL).
    pub fn split_terminal(
        &self,
        project_id: &str,
        layout_path: &[usize],
        direction: crate::workspace::state::SplitDirection,
        cx: &mut impl AppContext,
    ) {
        match self {
            Self::Local { workspace, focus_manager, .. } => {
                let pid = project_id.to_string();
                let lp = layout_path.to_vec();
                let focus_manager = focus_manager.clone();
                focus_manager.update(cx, |fm, cx| {
                    workspace.update(cx, |ws, cx| {
                        ws.split_terminal(fm, &pid, &lp, direction, cx);
                    });
                });
            }
            Self::Remote { .. } => {
                self.dispatch(
                    ActionRequest::SplitTerminal {
                        project_id: project_id.to_string(),
                        path: layout_path.to_vec(),
                        direction,
                    },
                    cx,
                );
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
            Self::Local { workspace, focus_manager, .. } => {
                let pid = project_id.to_string();
                let lp = layout_path.to_vec();
                let focus_manager = focus_manager.clone();
                focus_manager.update(cx, |fm, cx| {
                    workspace.update(cx, |ws, cx| {
                        if in_group {
                            ws.add_tab_to_group(fm, &pid, &lp, cx);
                        } else {
                            ws.add_tab(fm, &pid, &lp, cx);
                        }
                    });
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

impl okena_views_terminal::ActionDispatch for ActionDispatcher {
    fn dispatch(&self, action: ActionRequest, cx: &mut gpui::App) {
        self.dispatch(action, cx);
    }

    fn is_remote(&self) -> bool {
        self.is_remote()
    }

    fn split_terminal(
        &self,
        project_id: &str,
        layout_path: &[usize],
        direction: crate::workspace::state::SplitDirection,
        cx: &mut gpui::App,
    ) {
        self.split_terminal(project_id, layout_path, direction, cx);
    }

    fn add_tab(
        &self,
        project_id: &str,
        layout_path: &[usize],
        in_group: bool,
        cx: &mut gpui::App,
    ) {
        self.add_tab(project_id, layout_path, in_group, cx);
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
            target_project_id,
        } => ActionRequest::MoveTerminalToTabGroup {
            project_id: s(&project_id),
            terminal_id: s(&terminal_id),
            target_path,
            position,
            target_project_id: target_project_id.map(|id| s(&id)),
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
        ActionRequest::AddProject { name, path } => ActionRequest::AddProject { name, path },
        ActionRequest::ReorderProjectInFolder {
            folder_id,
            project_id,
            new_index,
        } => ActionRequest::ReorderProjectInFolder {
            folder_id: s(&folder_id),
            project_id: s(&project_id),
            new_index,
        },
        ActionRequest::SetProjectColor { project_id, color } => {
            ActionRequest::SetProjectColor {
                project_id: s(&project_id),
                color,
            }
        }
        ActionRequest::SetFolderColor { folder_id, color } => {
            ActionRequest::SetFolderColor {
                folder_id: s(&folder_id),
                color,
            }
        }
        ActionRequest::StartService {
            project_id,
            service_name,
        } => ActionRequest::StartService {
            project_id: s(&project_id),
            service_name,
        },
        ActionRequest::StopService {
            project_id,
            service_name,
        } => ActionRequest::StopService {
            project_id: s(&project_id),
            service_name,
        },
        ActionRequest::RestartService {
            project_id,
            service_name,
        } => ActionRequest::RestartService {
            project_id: s(&project_id),
            service_name,
        },
        ActionRequest::StartAllServices { project_id } => ActionRequest::StartAllServices {
            project_id: s(&project_id),
        },
        ActionRequest::StopAllServices { project_id } => ActionRequest::StopAllServices {
            project_id: s(&project_id),
        },
        ActionRequest::ReloadServices { project_id } => ActionRequest::ReloadServices {
            project_id: s(&project_id),
        },
        ActionRequest::CreateWorktree {
            project_id,
            branch,
            create_branch,
        } => ActionRequest::CreateWorktree {
            project_id: s(&project_id),
            branch,
            create_branch,
        },
        ActionRequest::GitCommitGraph { project_id, count, branch } => ActionRequest::GitCommitGraph {
            project_id: s(&project_id),
            count,
            branch,
        },
        ActionRequest::GitListBranches { project_id } => ActionRequest::GitListBranches {
            project_id: s(&project_id),
        },
        ActionRequest::GitStageFile { project_id, file_path } => ActionRequest::GitStageFile {
            project_id: s(&project_id),
            file_path,
        },
        ActionRequest::GitUnstageFile { project_id, file_path } => ActionRequest::GitUnstageFile {
            project_id: s(&project_id),
            file_path,
        },
        ActionRequest::GitDiscardFile { project_id, file_path } => ActionRequest::GitDiscardFile {
            project_id: s(&project_id),
            file_path,
        },
        ActionRequest::ListFiles { project_id, show_ignored } => ActionRequest::ListFiles {
            project_id: s(&project_id),
            show_ignored,
        },
        ActionRequest::ReadFile { project_id, relative_path } => ActionRequest::ReadFile {
            project_id: s(&project_id),
            relative_path,
        },
        ActionRequest::FileSize { project_id, relative_path } => ActionRequest::FileSize {
            project_id: s(&project_id),
            relative_path,
        },
        ActionRequest::SearchContent { project_id, query, case_sensitive, mode, max_results, file_glob, context_lines } => {
            ActionRequest::SearchContent {
                project_id: s(&project_id),
                query,
                case_sensitive,
                mode,
                max_results,
                file_glob,
                context_lines,
            }
        }
        ActionRequest::RenameFile { project_id, relative_path, new_name } => ActionRequest::RenameFile {
            project_id: s(&project_id),
            relative_path,
            new_name,
        },
        ActionRequest::DeleteFile { project_id, relative_path } => ActionRequest::DeleteFile {
            project_id: s(&project_id),
            relative_path,
        },
        ActionRequest::CreateFile { project_id, relative_path } => ActionRequest::CreateFile {
            project_id: s(&project_id),
            relative_path,
        },
        ActionRequest::CreateDirectory { project_id, relative_path } => ActionRequest::CreateDirectory {
            project_id: s(&project_id),
            relative_path,
        },
        ActionRequest::RenameProject { project_id, name } => ActionRequest::RenameProject {
            project_id: s(&project_id),
            name,
        },
        ActionRequest::RenameProjectDirectory { project_id, new_name } => ActionRequest::RenameProjectDirectory {
            project_id: s(&project_id),
            new_name,
        },
        ActionRequest::DeleteProject { project_id } => ActionRequest::DeleteProject {
            project_id: s(&project_id),
        },
        ActionRequest::SetProjectShowInOverview { project_id, show } => ActionRequest::SetProjectShowInOverview {
            project_id: s(&project_id),
            show,
        },
        ActionRequest::RemoveWorktreeProject { project_id, force } => ActionRequest::RemoveWorktreeProject {
            project_id: s(&project_id),
            force,
        },
        ActionRequest::CreateFolder { name } => ActionRequest::CreateFolder { name },
        ActionRequest::DeleteFolder { folder_id } => ActionRequest::DeleteFolder { folder_id },
        ActionRequest::RenameFolder { folder_id, name } => ActionRequest::RenameFolder { folder_id, name },
        ActionRequest::MoveProjectToFolder { project_id, folder_id, position } => ActionRequest::MoveProjectToFolder {
            project_id: s(&project_id),
            folder_id,
            position,
        },
        ActionRequest::MoveProjectOutOfFolder { project_id, top_level_index } => ActionRequest::MoveProjectOutOfFolder {
            project_id: s(&project_id),
            top_level_index,
        },
    }
}
