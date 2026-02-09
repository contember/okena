use crate::settings::settings;
use crate::terminal::terminal::{Terminal, TerminalSize};
use crate::views::panels::toast::ToastManager;
use crate::workspace::actions::execute::spawn_uninitialized_terminals;
use gpui::*;
use std::sync::Arc;

use super::RootView;

impl RootView {
    /// Spawn terminals for all layout slots in a project that have terminal_id: None
    /// Used after creating a worktree project to immediately populate terminals
    pub(super) fn spawn_terminals_for_project(&mut self, project_id: String, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let terminals = self.terminals.clone();
        self.workspace.update(cx, |ws, cx| {
            spawn_uninitialized_terminals(ws, &project_id, &*backend, &terminals, cx);
        });
        self.sync_project_columns(cx);
    }

    /// Switch terminal shell - kills old terminal and creates new one with the new shell.
    /// Used when user selects a different shell from the shell selector overlay.
    pub(super) fn switch_terminal_shell(
        &mut self,
        project_id: &str,
        old_terminal_id: &str,
        shell_type: crate::terminal::shell_config::ShellType,
        cx: &mut Context<Self>,
    ) {
        // Get project path and terminal's layout path
        let (project_path, layout_path) = {
            let ws = self.workspace.read(cx);
            let project = match ws.project(project_id) {
                Some(p) => p,
                None => {
                    log::error!("switch_terminal_shell: Project {} not found", project_id);
                    return;
                }
            };
            let layout_path = match project.layout.as_ref().and_then(|l| l.find_terminal_path(old_terminal_id)) {
                Some(p) => p,
                None => {
                    log::error!("switch_terminal_shell: Terminal {} not found in project {}", old_terminal_id, project_id);
                    return;
                }
            };
            (project.path.clone(), layout_path)
        };

        // Get current shell to check if it's actually changing
        let current_shell = self.workspace.read(cx).get_terminal_shell(project_id, &layout_path);
        if current_shell.as_ref() == Some(&shell_type) {
            log::info!("switch_terminal_shell: Shell type unchanged, skipping");
            return;
        }

        // Kill the old terminal
        self.backend.kill(old_terminal_id);
        self.terminals.lock().remove(old_terminal_id);

        // Update shell type in workspace state
        self.workspace.update(cx, |ws, cx| {
            ws.set_terminal_shell(project_id, &layout_path, shell_type.clone(), cx);
        });

        // Determine the actual shell to use (resolve Default to settings)
        let actual_shell = if shell_type == crate::terminal::shell_config::ShellType::Default {
            settings(cx).default_shell.clone()
        } else {
            shell_type
        };

        // Create new terminal with the new shell
        match self.backend.create_terminal(&project_path, Some(&actual_shell)) {
            Ok(new_terminal_id) => {
                log::info!("switch_terminal_shell: Switched to {:?}, new terminal_id: {}", actual_shell, new_terminal_id);

                // Update terminal_id in workspace state
                self.workspace.update(cx, |ws, cx| {
                    ws.set_terminal_id(project_id, &layout_path, new_terminal_id.clone(), cx);
                });

                // Create terminal wrapper and register it
                let size = TerminalSize::default();
                let terminal = Arc::new(Terminal::new(
                    new_terminal_id.clone(),
                    size,
                    self.backend.transport(),
                    project_path.clone(),
                ));
                self.terminals.lock().insert(new_terminal_id, terminal);
            }
            Err(e) => {
                log::error!("switch_terminal_shell: Failed to create terminal with new shell: {}", e);
                ToastManager::error(format!("Failed to create terminal: {}", e), cx);
            }
        }
    }

    /// Create worktree from the focused project
    pub(super) fn create_worktree_from_focus(&mut self, cx: &mut Context<Self>) {
        // Get the focused project ID and info
        let project_info = {
            let ws = self.workspace.read(cx);
            let project_id = ws.focus_manager.focused_terminal_state()
                .map(|f| f.project_id.clone())
                .or_else(|| {
                    // Fallback: use the first visible project
                    ws.visible_projects()
                        .first()
                        .map(|p| p.id.clone())
                });

            project_id.and_then(|id| {
                ws.project(&id).map(|p| {
                    let project_path = p.path.clone();
                    let is_worktree = p.worktree_info.is_some();
                    let is_git = crate::git::get_git_status(std::path::Path::new(&project_path)).is_some();
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
