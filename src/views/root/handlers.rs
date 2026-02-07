use crate::views::overlay_manager::{OverlayManager, OverlayManagerEvent};
use crate::workspace::requests::OverlayRequest;
use crate::workspace::requests::SidebarRequest;
use gpui::*;

use super::RootView;

impl RootView {
    /// Handle events from the OverlayManager that require RootView access.
    pub(super) fn handle_overlay_manager_event(
        &mut self,
        _: Entity<OverlayManager>,
        event: &OverlayManagerEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            OverlayManagerEvent::SwitchWorkspace(data) => {
                self.handle_switch_workspace(data.clone(), cx);
            }
            OverlayManagerEvent::WorktreeCreated(new_project_id) => {
                self.spawn_terminals_for_project(new_project_id.clone(), cx);
            }
            OverlayManagerEvent::ShellSelected { shell_type, project_id, terminal_id } => {
                self.switch_terminal_shell(project_id, terminal_id, shell_type.clone(), cx);
            }
            OverlayManagerEvent::AddTerminal { project_id } => {
                self.workspace.update(cx, |ws, cx| {
                    ws.add_terminal(project_id, cx);
                });
            }
            OverlayManagerEvent::CreateWorktree { project_id, project_path } => {
                self.overlay_manager.update(cx, |om, cx| {
                    om.show_worktree_dialog(project_id.clone(), project_path.clone(), cx);
                });
            }
            OverlayManagerEvent::RenameProject { project_id, project_name } => {
                self.request_broker.update(cx, |broker, cx| {
                    broker.push_sidebar_request(SidebarRequest::RenameProject {
                        project_id: project_id.clone(),
                        project_name: project_name.clone(),
                    }, cx);
                });
            }
            OverlayManagerEvent::CloseWorktree { project_id } => {
                let result = self.workspace.update(cx, |ws, cx| {
                    ws.remove_worktree_project(project_id, false, cx)
                });
                if let Err(e) = result {
                    log::error!("Failed to close worktree: {}", e);
                }
            }
            OverlayManagerEvent::DeleteProject { project_id } => {
                self.workspace.update(cx, |ws, cx| {
                    ws.delete_project(project_id, cx);
                });
            }
            OverlayManagerEvent::ConfigureHooks { project_id } => {
                self.overlay_manager.update(cx, |om, cx| {
                    om.show_settings_for_project(project_id.clone(), cx);
                });
            }
            OverlayManagerEvent::FocusProject(project_id) => {
                self.workspace.update(cx, |ws, cx| {
                    // Focus the project (like clicking on it in sidebar)
                    ws.set_focused_project(Some(project_id.clone()), cx);
                });
            }
            OverlayManagerEvent::ToggleProjectVisibility(project_id) => {
                self.workspace.update(cx, |ws, cx| {
                    ws.toggle_project_visibility(project_id, cx);
                });
            }
            OverlayManagerEvent::RemoteReconnect { connection_id } => {
                if let Some(ref rm) = self.remote_manager {
                    rm.update(cx, |rm, cx| {
                        rm.reconnect(connection_id, cx);
                    });
                }
            }
            OverlayManagerEvent::RemoteRemoveConnection { connection_id } => {
                if let Some(ref rm) = self.remote_manager {
                    rm.update(cx, |rm, cx| {
                        rm.remove_connection(connection_id, cx);
                    });
                }
            }
            OverlayManagerEvent::RemoteConnected { config, code } => {
                if let Some(ref rm) = self.remote_manager {
                    let connection_id = config.id.clone();
                    let code = code.clone();
                    rm.update(cx, |rm, cx| {
                        rm.add_connection(config.clone(), cx);
                        rm.pair(&connection_id, &code, cx);
                    });
                    // Save connection config to settings
                    let mut settings = crate::workspace::settings::load_settings();
                    if !settings.remote_connections.iter().any(|c| c.id == connection_id) {
                        settings.remote_connections.push(config.clone());
                        let _ = crate::workspace::settings::save_settings(&settings);
                    }
                }
            }
        }
    }

    /// Handle workspace switch from session manager.
    pub(super) fn handle_switch_workspace(&mut self, data: crate::workspace::state::WorkspaceData, cx: &mut Context<Self>) {
        // Kill all existing terminals
        {
            let terminals = self.terminals.lock();
            for terminal in terminals.values() {
                self.backend.kill(&terminal.terminal_id);
            }
        }
        self.terminals.lock().clear();

        // Clear project columns (will be recreated)
        self.project_columns.clear();

        // Update workspace with new data
        self.workspace.update(cx, |ws, cx| {
            ws.replace_data(data, cx);
        });

        // Sync project columns for new data
        self.sync_project_columns(cx);

        cx.notify();
    }

    /// Process pending overlay requests from workspace state.
    ///
    /// Drains the overlay request queue and dispatches each request to the
    /// OverlayManager. Requests for already-open overlays are silently dropped.
    pub(super) fn process_pending_requests(&mut self, cx: &mut Context<Self>) {
        let requests: Vec<_> = self.request_broker.update(cx, |broker, _cx| {
            broker.drain_overlay_requests()
        });

        for request in requests {
            match request {
                OverlayRequest::ContextMenu { project_id, position } => {
                    if !self.overlay_manager.read(cx).has_context_menu() {
                        self.overlay_manager.update(cx, |om, cx| {
                            om.show_context_menu(
                                crate::workspace::requests::ContextMenuRequest { project_id, position },
                                cx,
                            );
                        });
                    }
                }
                OverlayRequest::FolderContextMenu { folder_id, folder_name, position } => {
                    if !self.overlay_manager.read(cx).has_folder_context_menu() {
                        self.overlay_manager.update(cx, |om, cx| {
                            om.show_folder_context_menu(
                                crate::workspace::requests::FolderContextMenuRequest { folder_id, folder_name, position },
                                cx,
                            );
                        });
                    }
                }
                OverlayRequest::ShellSelector { project_id, terminal_id, current_shell } => {
                    self.overlay_manager.update(cx, |om, cx| {
                        om.show_shell_selector(current_shell, project_id, terminal_id, cx);
                    });
                }
                OverlayRequest::AddProjectDialog => {
                    self.overlay_manager.update(cx, |om, cx| {
                        om.toggle_add_project_dialog(cx);
                    });
                }
                OverlayRequest::DiffViewer { path, file } => {
                    self.overlay_manager.update(cx, |om, cx| {
                        om.show_diff_viewer(path, file, cx);
                    });
                }
                OverlayRequest::RemoteConnect => {
                    if let Some(ref rm) = self.remote_manager {
                        let rm = rm.clone();
                        self.overlay_manager.update(cx, |om, cx| {
                            om.toggle_remote_connect(rm, cx);
                        });
                    }
                }
                OverlayRequest::RemoteConnectionContextMenu { connection_id, connection_name, position } => {
                    if !self.overlay_manager.read(cx).has_remote_context_menu() {
                        self.overlay_manager.update(cx, |om, cx| {
                            om.show_remote_context_menu(connection_id, connection_name, position, cx);
                        });
                    }
                }
            }
        }
    }
}
