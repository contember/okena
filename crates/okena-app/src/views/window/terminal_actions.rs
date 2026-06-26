use gpui::*;

use super::WindowView;

impl WindowView {
    /// Switch terminal shell — dispatches to the daemon, which kills the old PTY
    /// and respawns at the same layout path with the new shell (resolving the
    /// default chain + applying shell-wrapper/on_create hooks). The new terminal
    /// id arrives via the next state snapshot.
    pub(super) fn switch_terminal_shell(
        &mut self,
        project_id: &str,
        old_terminal_id: &str,
        shell_type: crate::terminal::shell_config::ShellType,
        cx: &mut Context<Self>,
    ) {
        if let Some(dispatcher) = self.dispatcher_for_project(project_id, cx) {
            dispatcher.dispatch(
                okena_core::api::ActionRequest::SwitchTerminalShell {
                    project_id: project_id.to_string(),
                    terminal_id: old_terminal_id.to_string(),
                    shell: shell_type,
                },
                cx,
            );
        }
    }

    /// Create worktree from the focused project
    pub(super) fn create_worktree_from_focus(&mut self, cx: &mut Context<Self>) {
        // Get the focused project ID and info
        let project_info = {
            let ws = self.workspace.read(cx);
            let fm = self.focus_manager.read(cx);
            let project_id = fm.focused_terminal_state()
                .map(|f| f.project_id.clone())
                .or_else(|| {
                    // Fallback: use the first visible project
                    ws.visible_projects(self.window_id, fm.focused_project_id(), fm.is_focus_individual())
                        .first()
                        .map(|p| p.id.clone())
                });

            project_id.and_then(|id| {
                ws.project(&id).map(|p| {
                    let project_path = p.path.clone();
                    let is_worktree = p.worktree_info.is_some();
                    let is_git = ws
                        .remote_snapshot(&id)
                        .and_then(|s| s.git_status.as_ref())
                        .is_some();
                    (id, project_path, is_git, is_worktree)
                })
            })
        };

        if let Some((project_id, project_path, is_git, is_worktree)) = project_info {
            if is_git && !is_worktree {
                self.overlay_manager.update(cx, |om, cx| {
                    om.show_worktree_dialog(project_id, project_path, cx);
                });
            } else {
                log::info!("Cannot create worktree: project is not a git repo or is already a worktree");
            }
        }
    }
}
