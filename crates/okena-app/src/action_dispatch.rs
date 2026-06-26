//! Unified action dispatch — routes terminal actions to the local daemon.
//!
//! Every project is a remote project of the local daemon, so `ActionDispatcher`
//! carries a single `Remote` variant. Callers simply call
//! `dispatcher.dispatch(action, cx)` without any conditionals.

use crate::remote_client::manager::RemoteConnectionManager;
use crate::workspace::focus::FocusManager;
use crate::workspace::state::{WindowId, Workspace};

use okena_core::api::ActionRequest;
use okena_transport::client::strip_prefix;

use gpui::{AppContext, Entity};

/// Build an ActionDispatcher for the given project.
///
/// Every project is a remote project of the local daemon, so this always
/// returns the `Remote` variant. Returns `None` if the project is unknown or
/// the connection/remote manager required to reach it is unavailable.
///
/// `window_id` carries the originating `WindowView`'s window id so per-window
/// state mutations triggered by UI actions (e.g. hide/show via the sidebar
/// context menu routed through `SetProjectShowInOverview`) land on the right
/// window's slot. A UI action issued in W2 against a project mutates W2's
/// per-window state on the local mirror, not main's.
pub fn dispatcher_for_project(
    project_id: &str,
    window_id: WindowId,
    workspace: &Entity<Workspace>,
    focus_manager: &Entity<FocusManager>,
    remote_manager: &Option<Entity<RemoteConnectionManager>>,
    cx: &gpui::App,
) -> Option<ActionDispatcher> {
    let ws = workspace.read(cx);
    let project = ws.project(project_id)?;
    let connection_id = project.connection_id.as_ref()?;
    let manager = remote_manager.as_ref()?;
    Some(ActionDispatcher::Remote {
        connection_id: connection_id.clone(),
        manager: manager.clone(),
        workspace: workspace.clone(),
        focus_manager: focus_manager.clone(),
        window_id,
    })
}

/// Routes terminal and service actions to either local execution or remote HTTP.
///
/// Passed through the view hierarchy (ProjectColumn → LayoutContainer → TerminalPane)
/// so all action handlers dispatch through this without knowing if the project is
/// local or remote.
#[derive(Clone)]
pub enum ActionDispatcher {
    /// Remote project — send actions via HTTP to the remote server.
    /// Visual/presentation actions (split sizes, minimize, fullscreen, active tab, focus)
    /// are executed locally on the client workspace to avoid server round-trips
    /// and to survive state syncs. `window_id` carries the originating window
    /// for deferred focus after remote terminal creation.
    Remote {
        connection_id: String,
        manager: Entity<RemoteConnectionManager>,
        workspace: Entity<Workspace>,
        focus_manager: Entity<FocusManager>,
        window_id: WindowId,
    },
}

impl ActionDispatcher {
    #[allow(dead_code)]
    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Remote { .. })
    }

    /// Dispatch a standard action (split, close, create terminal, service action, etc.).
    pub fn dispatch(&self, action: ActionRequest, cx: &mut impl AppContext) {
        let Self::Remote {
            connection_id,
            manager,
            workspace,
            focus_manager,
            window_id,
        } = self;

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
            ActionRequest::SetFullscreen { project_id, terminal_id, .. } => {
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
                    cx.notify();
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
            ActionRequest::FocusTerminal { project_id, terminal_id, .. } => {
                let pid = project_id.clone();
                let tid = terminal_id.clone();
                let focus_manager = focus_manager.clone();
                focus_manager.update(cx, |fm, cx| {
                    workspace.update(cx, |ws, cx| {
                        if let Some(project) = ws.project(&pid)
                            && let Some(ref layout) = project.layout
                            && let Some(path) = layout.find_terminal_path(&tid) {
                                ws.set_focused_terminal(fm, pid, path, cx);
                            }
                    });
                    cx.notify();
                });
                return;
            }
            ActionRequest::CreateTerminal { project_id } => {
                // Record pending focus — the actual focus will happen when
                // the next state sync brings the new terminal into the
                // client's layout (see sync_remote_projects_into_workspace).
                let pid = project_id.clone();
                let window_id = *window_id;
                workspace.update(cx, |ws, _cx| {
                    let old_terminal_ids = ws
                        .project(&pid)
                        .and_then(|p| p.layout.as_ref())
                        .map(|layout| layout.collect_terminal_ids())
                        .unwrap_or_default();
                    ws.queue_pending_remote_focus(window_id, &pid, old_terminal_ids);
                });
                // Don't return — action proceeds to be sent to server below
            }
            ActionRequest::CreateWorktree { branch, .. } => {
                // Record pending project visibility — the server assigns
                // the new worktree project ID, so the next state sync
                // applies the spawning-window rule when the branch-named
                // project first appears.
                let window_id = *window_id;
                let cid = connection_id.clone();
                let branch = branch.clone();
                workspace.update(cx, |ws, _cx| {
                    ws.queue_pending_remote_project_visibility(
                        window_id,
                        &cid,
                        &branch,
                        None,
                    );
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

    /// Split a terminal via the server.
    pub fn split_terminal(
        &self,
        project_id: &str,
        layout_path: &[usize],
        direction: crate::workspace::state::SplitDirection,
        cx: &mut impl AppContext,
    ) {
        self.dispatch(
            ActionRequest::SplitTerminal {
                project_id: project_id.to_string(),
                path: layout_path.to_vec(),
                direction,
            },
            cx,
        );
    }

    /// Add a tab via the server.
    pub fn add_tab(
        &self,
        project_id: &str,
        layout_path: &[usize],
        in_group: bool,
        cx: &mut impl AppContext,
    ) {
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

impl ActionDispatcher {
    /// Upload a pasted clipboard image to the remote server.
    ///
    /// The server writes the bytes to a temp file on its own filesystem and
    /// bracketed-pastes that path into the terminal, so a server-side TUI like
    /// Claude Code can read it. `terminal_id` is the local (prefixed) id; the
    /// `remote:{cid}:` prefix is stripped before upload.
    pub fn upload_remote_paste_image(
        &self,
        terminal_id: &str,
        mime: &str,
        bytes: Vec<u8>,
        cx: &mut impl AppContext,
    ) {
        let Self::Remote { connection_id, manager, .. } = self;
        let remote_terminal_id = strip_prefix(terminal_id, connection_id);
        let cid = connection_id.clone();
        let mime = mime.to_string();
        manager.update(cx, |rm, cx| {
            rm.upload_paste_image(&cid, &remote_terminal_id, &mime, bytes, cx);
        });
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

    fn upload_remote_paste_image(
        &self,
        terminal_id: &str,
        mime: &str,
        bytes: Vec<u8>,
        cx: &mut gpui::App,
    ) {
        self.upload_remote_paste_image(terminal_id, mime, bytes, cx);
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
            window,
        } => ActionRequest::FocusTerminal {
            project_id: s(&project_id),
            terminal_id: s(&terminal_id),
            window,
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
            window,
        } => ActionRequest::SetFullscreen {
            project_id: s(&project_id),
            terminal_id: terminal_id.map(|id| s(&id)),
            window,
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
        ActionRequest::SwitchTerminalShell {
            project_id,
            terminal_id,
            shell,
        } => ActionRequest::SwitchTerminalShell {
            project_id: s(&project_id),
            terminal_id: s(&terminal_id),
            shell,
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
        ActionRequest::AddDiscoveredWorktree {
            parent_project_id,
            worktree_path,
            branch,
        } => ActionRequest::AddDiscoveredWorktree {
            parent_project_id: s(&parent_project_id),
            worktree_path,
            branch,
        },
        ActionRequest::RerunHook {
            project_id,
            terminal_id,
        } => ActionRequest::RerunHook {
            project_id: s(&project_id),
            terminal_id: s(&terminal_id),
        },
        ActionRequest::GitCommitGraph { project_id, count, branch } => ActionRequest::GitCommitGraph {
            project_id: s(&project_id),
            count,
            branch,
        },
        ActionRequest::GitListBranches { project_id } => ActionRequest::GitListBranches {
            project_id: s(&project_id),
        },
        ActionRequest::GitListWorktrees { project_id } => ActionRequest::GitListWorktrees {
            project_id: s(&project_id),
        },
        ActionRequest::GitListBranchesClassified { project_id } => ActionRequest::GitListBranchesClassified {
            project_id: s(&project_id),
        },
        ActionRequest::GitCheckoutLocalBranch { project_id, branch } => ActionRequest::GitCheckoutLocalBranch {
            project_id: s(&project_id),
            branch,
        },
        ActionRequest::GitCheckoutRemoteBranch { project_id, remote_branch } => ActionRequest::GitCheckoutRemoteBranch {
            project_id: s(&project_id),
            remote_branch,
        },
        ActionRequest::GitCreateAndCheckoutBranch { project_id, new_name, start_point } => ActionRequest::GitCreateAndCheckoutBranch {
            project_id: s(&project_id),
            new_name,
            start_point,
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
        ActionRequest::GitBlame { project_id, relative_path } => ActionRequest::GitBlame {
            project_id: s(&project_id),
            relative_path,
        },
        ActionRequest::ListFiles { project_id, show_ignored } => ActionRequest::ListFiles {
            project_id: s(&project_id),
            show_ignored,
        },
        ActionRequest::ListDirectory { project_id, relative_path, show_ignored } => {
            ActionRequest::ListDirectory {
                project_id: s(&project_id),
                relative_path,
                show_ignored,
            }
        }
        ActionRequest::ReadFile { project_id, relative_path } => ActionRequest::ReadFile {
            project_id: s(&project_id),
            relative_path,
        },
        ActionRequest::ReadFileBytes { project_id, relative_path } => ActionRequest::ReadFileBytes {
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
        ActionRequest::UpdateProjectHooks { project_id, hooks } => ActionRequest::UpdateProjectHooks {
            project_id: s(&project_id),
            hooks,
        },
        ActionRequest::RenameProjectDirectory { project_id, new_name } => ActionRequest::RenameProjectDirectory {
            project_id: s(&project_id),
            new_name,
        },
        ActionRequest::DeleteProject { project_id } => ActionRequest::DeleteProject {
            project_id: s(&project_id),
        },
        ActionRequest::SetProjectShowInOverview { project_id, show, window } => ActionRequest::SetProjectShowInOverview {
            project_id: s(&project_id),
            show,
            window,
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
        // Session + app-scoped actions carry no project/terminal ids to remap.
        a @ (ActionRequest::LoadSession { .. }
        | ActionRequest::SaveSession { .. }
        | ActionRequest::ImportWorkspace { .. }
        | ActionRequest::ExportWorkspace { .. }
        | ActionRequest::GetSettings
        | ActionRequest::GetSettingsSchema
        | ActionRequest::SetSettings { .. }
        | ActionRequest::GetThemes
        | ActionRequest::GetTheme { .. }
        | ActionRequest::SetTheme { .. }
        | ActionRequest::SaveCustomTheme { .. }
        | ActionRequest::ListActions
        | ActionRequest::InvokeAction { .. }) => a,
    }
}
