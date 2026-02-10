use crate::remote::types::ActionRequest;
use crate::views::overlay_manager::{OverlayManager, OverlayManagerEvent};
use crate::workspace::actions::execute::execute_action;
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
            OverlayManagerEvent::TerminalCopy { terminal_id } => {
                let terminals = self.terminals.lock();
                if let Some(terminal) = terminals.get(terminal_id) {
                    if let Some(text) = terminal.get_selected_text() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                }
            }
            OverlayManagerEvent::TerminalPaste { terminal_id } => {
                let text = cx.read_from_clipboard()
                    .and_then(|item| item.text().map(|t| t.to_string()));
                if let Some(text) = text {
                    let terminals = self.terminals.lock();
                    if let Some(terminal) = terminals.get(terminal_id) {
                        terminal.send_input(&text);
                    }
                }
            }
            OverlayManagerEvent::TerminalClear { terminal_id } => {
                let terminals = self.terminals.lock();
                if let Some(terminal) = terminals.get(terminal_id) {
                    terminal.clear();
                }
            }
            OverlayManagerEvent::TerminalSelectAll { terminal_id } => {
                let terminals = self.terminals.lock();
                if let Some(terminal) = terminals.get(terminal_id) {
                    terminal.select_all();
                }
                cx.notify();
            }
            OverlayManagerEvent::TerminalSplit { project_id, layout_path, direction } => {
                let action = ActionRequest::SplitTerminal {
                    project_id: project_id.clone(),
                    path: layout_path.clone(),
                    direction: *direction,
                };
                let backend = self.backend.clone();
                let terminals = self.terminals.clone();
                self.workspace.update(cx, |ws, cx| {
                    execute_action(action, ws, &*backend, &terminals, cx);
                });
            }
            OverlayManagerEvent::TerminalClose { project_id, terminal_id } => {
                let action = ActionRequest::CloseTerminal {
                    project_id: project_id.clone(),
                    terminal_id: terminal_id.clone(),
                };
                let backend = self.backend.clone();
                let terminals = self.terminals.clone();
                self.workspace.update(cx, |ws, cx| {
                    execute_action(action, ws, &*backend, &terminals, cx);
                });
            }
            OverlayManagerEvent::TabClose { project_id, layout_path, tab_index } => {
                let removed = self.workspace.update(cx, |ws, cx| {
                    ws.close_tab(project_id, layout_path, *tab_index, cx)
                });
                for id in &removed {
                    self.backend.kill(id);
                    self.terminals.lock().remove(id);
                }
            }
            OverlayManagerEvent::TabCloseOthers { project_id, layout_path, tab_index } => {
                let removed = self.workspace.update(cx, |ws, cx| {
                    ws.close_other_tabs(project_id, layout_path, *tab_index, cx)
                });
                for id in &removed {
                    self.backend.kill(id);
                    self.terminals.lock().remove(id);
                }
            }
            OverlayManagerEvent::TabCloseToRight { project_id, layout_path, tab_index } => {
                let removed = self.workspace.update(cx, |ws, cx| {
                    ws.close_tabs_to_right(project_id, layout_path, *tab_index, cx)
                });
                for id in &removed {
                    self.backend.kill(id);
                    self.terminals.lock().remove(id);
                }
            }
            OverlayManagerEvent::RemoteConnected { config } => {
                if let Some(ref rm) = self.remote_manager {
                    let config_clone = config.clone();
                    let result = rm.update(cx, |rm, cx| {
                        rm.add_connection(config.clone(), cx)
                    });
                    if let Err(msg) = result {
                        crate::views::panels::toast::ToastManager::warning(msg, cx);
                        return;
                    }
                    // Save connection config (with token) to settings (atomic update)
                    let _ = crate::workspace::settings::update_remote_connections(|conns| {
                        if !conns.iter().any(|c| c.id == config_clone.id) {
                            conns.push(config_clone);
                        }
                    });
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
                OverlayRequest::TerminalContextMenu { terminal_id, project_id, layout_path, position, has_selection } => {
                    self.overlay_manager.update(cx, |om, cx| {
                        om.show_terminal_context_menu(terminal_id, project_id, layout_path, position, has_selection, cx);
                    });
                }
                OverlayRequest::TabContextMenu { tab_index, num_tabs, project_id, layout_path, position } => {
                    self.overlay_manager.update(cx, |om, cx| {
                        om.show_tab_context_menu(tab_index, num_tabs, project_id, layout_path, position, cx);
                    });
                }
            }
        }
    }
}
