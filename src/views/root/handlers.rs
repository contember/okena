use crate::action_dispatch::ActionDispatcher;
use crate::views::overlay_manager::{OverlayManager, OverlayManagerEvent};
use crate::workspace::requests::OverlayRequest;
use crate::workspace::requests::SidebarRequest;
use crate::workspace::state::{LayoutNode, Workspace};
use gpui::*;

use okena_core::api::ActionRequest;

use super::RootView;

impl RootView {
    /// Build an ActionDispatcher for the given project.
    /// Returns Remote variant if the project is a remote project,
    /// otherwise returns Local variant.
    fn dispatcher_for_project(&self, project_id: &str, cx: &Context<Self>) -> ActionDispatcher {
        let backend = Some(self.backend.clone());
        crate::action_dispatch::dispatcher_for_project(
            project_id,
            &self.workspace,
            &backend,
            &self.terminals,
            &self.service_manager,
            &self.remote_manager,
            cx,
        ).unwrap_or_else(|| ActionDispatcher::Local {
            workspace: self.workspace.clone(),
            backend: self.backend.clone(),
            terminals: self.terminals.clone(),
            service_manager: self.service_manager.clone(),
        })
    }
}

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
                let dispatcher = self.dispatcher_for_project(project_id, cx);
                dispatcher.dispatch(ActionRequest::CreateTerminal {
                    project_id: project_id.clone(),
                }, cx);
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
            OverlayManagerEvent::RenameDirectory { project_id, project_path } => {
                self.overlay_manager.update(cx, |om, cx| {
                    om.show_rename_directory_dialog(project_id.clone(), project_path.clone(), cx);
                });
            }
            OverlayManagerEvent::CloseWorktree { project_id } => {
                self.overlay_manager.update(cx, |om, cx| {
                    om.show_close_worktree_dialog(project_id.clone(), cx);
                });
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
            OverlayManagerEvent::CloseAllWorktrees { project_id } => {
                // Collect all worktree project IDs for this parent
                let worktree_ids: Vec<String> = self.workspace.read(cx)
                    .data().projects.iter()
                    .filter(|p| p.worktree_info.as_ref()
                        .map_or(false, |wt| wt.parent_project_id == *project_id))
                    .map(|p| p.id.clone())
                    .collect();

                // Remove each one (non-force, skip dirty ones)
                let mut errors = Vec::new();
                for wt_id in &worktree_ids {
                    let result = self.workspace.update(cx, |ws, cx| {
                        ws.remove_worktree_project(wt_id, false, cx)
                    });
                    if let Err(e) = result {
                        errors.push(e);
                    }
                }
                if !errors.is_empty() {
                    log::warn!("Some worktrees could not be closed: {:?}", errors);
                }
            }
            OverlayManagerEvent::FocusParent { project_id } => {
                let parent_id = self.workspace.read(cx)
                    .project(project_id)
                    .and_then(|p| p.worktree_info.as_ref())
                    .map(|wt| wt.parent_project_id.clone());

                if let Some(parent_id) = parent_id {
                    self.workspace.update(cx, |ws, cx| {
                        ws.set_focused_project(Some(parent_id), cx);
                    });
                }
            }
            OverlayManagerEvent::FocusProject(project_id) => {
                self.workspace.update(cx, |ws, cx| {
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
            OverlayManagerEvent::RemotePair { connection_id, connection_name } => {
                self.overlay_manager.update(cx, |om, cx| {
                    om.show_remote_pair_dialog(connection_id.clone(), connection_name.clone(), cx);
                });
            }
            OverlayManagerEvent::RemotePaired { connection_id, code } => {
                if let Some(ref rm) = self.remote_manager {
                    rm.update(cx, |rm, cx| {
                        rm.pair(connection_id, code, cx);
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
                let dispatcher = self.dispatcher_for_project(project_id, cx);
                dispatcher.dispatch(ActionRequest::SplitTerminal {
                    project_id: project_id.clone(),
                    path: layout_path.clone(),
                    direction: *direction,
                }, cx);
            }
            OverlayManagerEvent::TerminalClose { project_id, terminal_id } => {
                let dispatcher = self.dispatcher_for_project(project_id, cx);
                dispatcher.dispatch(ActionRequest::CloseTerminal {
                    project_id: project_id.clone(),
                    terminal_id: terminal_id.clone(),
                }, cx);
            }
            OverlayManagerEvent::TabClose { project_id, layout_path, tab_index } => {
                let terminal_ids = collect_tab_terminal_ids(&self.workspace, project_id, layout_path, cx);
                if let Some(tid) = terminal_ids.get(*tab_index).cloned() {
                    let dispatcher = self.dispatcher_for_project(project_id, cx);
                    dispatcher.dispatch(ActionRequest::CloseTerminal {
                        project_id: project_id.clone(),
                        terminal_id: tid,
                    }, cx);
                }
            }
            OverlayManagerEvent::TabCloseOthers { project_id, layout_path, tab_index } => {
                let terminal_ids = collect_tab_terminal_ids(&self.workspace, project_id, layout_path, cx);
                let to_close: Vec<String> = terminal_ids.into_iter().enumerate()
                    .filter(|(i, _)| *i != *tab_index)
                    .map(|(_, id)| id)
                    .collect();
                if !to_close.is_empty() {
                    let dispatcher = self.dispatcher_for_project(project_id, cx);
                    dispatcher.dispatch(ActionRequest::CloseTerminals {
                        project_id: project_id.clone(),
                        terminal_ids: to_close,
                    }, cx);
                }
            }
            OverlayManagerEvent::TabCloseToRight { project_id, layout_path, tab_index } => {
                let terminal_ids = collect_tab_terminal_ids(&self.workspace, project_id, layout_path, cx);
                let to_close: Vec<String> = terminal_ids.into_iter().skip(tab_index + 1).collect();
                if !to_close.is_empty() {
                    let dispatcher = self.dispatcher_for_project(project_id, cx);
                    dispatcher.dispatch(ActionRequest::CloseTerminals {
                        project_id: project_id.clone(),
                        terminal_ids: to_close,
                    }, cx);
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
                    let rm = self.remote_manager.clone();
                    self.overlay_manager.update(cx, |om, cx| {
                        om.toggle_add_project_dialog(rm, cx);
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
                OverlayRequest::RemoteConnectionContextMenu { connection_id, connection_name, is_pairing, position } => {
                    if !self.overlay_manager.read(cx).has_remote_context_menu() {
                        self.overlay_manager.update(cx, |om, cx| {
                            om.show_remote_context_menu(connection_id, connection_name, is_pairing, position, cx);
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
                OverlayRequest::ShowServiceLog { project_id, service_name } => {
                    self.handle_show_service_log(project_id, service_name, cx);
                }
            }
        }
    }

    /// Handle a ShowServiceLog request: delegate to the correct ProjectColumn.
    fn handle_show_service_log(
        &mut self,
        project_id: String,
        service_name: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(col) = self.project_columns.get(&project_id).cloned() {
            col.update(cx, |col, cx| {
                col.show_service(&service_name, cx);
            });
        }
    }
}

/// Collect terminal IDs from children of a Tabs node at the given layout path.
///
/// Each child subtree is traversed with `collect_terminal_ids()`, so nested
/// splits/tabs within a tab are handled correctly. Returns one entry per child.
fn collect_tab_terminal_ids(
    workspace: &Entity<Workspace>,
    project_id: &str,
    layout_path: &[usize],
    cx: &Context<RootView>,
) -> Vec<String> {
    let ws = workspace.read(cx);
    let Some(project) = ws.project(project_id) else {
        return Vec::new();
    };
    let Some(ref layout) = project.layout else {
        return Vec::new();
    };
    let Some(node) = layout.get_at_path(layout_path) else {
        return Vec::new();
    };
    match node {
        LayoutNode::Tabs { children, .. } => {
            children.iter().filter_map(|child| {
                // For simple Terminal children, get the ID directly.
                // For nested structures, get the first terminal ID.
                child.collect_terminal_ids().into_iter().next()
            }).collect()
        }
        LayoutNode::Terminal { terminal_id, .. } => {
            terminal_id.iter().cloned().collect()
        }
        _ => Vec::new(),
    }
}
