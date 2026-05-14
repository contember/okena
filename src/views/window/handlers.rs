use crate::action_dispatch::ActionDispatcher;
use crate::settings::settings;
use crate::views::overlay_manager::{OverlayManager, OverlayManagerEvent};
use crate::workspace::requests::{
    FolderOverlay, FolderOverlayKind, OverlayRequest, ProjectOverlay, ProjectOverlayKind,
    SidebarRequest,
};
use crate::workspace::state::{LayoutNode, Workspace};
use gpui::*;

use okena_core::api::ActionRequest;

use super::WindowView;

impl WindowView {
    /// Build an ActionDispatcher for the given project.
    /// Returns Remote variant if the project is a remote project,
    /// otherwise returns Local variant.
    fn dispatcher_for_project(&self, project_id: &str, cx: &Context<Self>) -> ActionDispatcher {
        let backend = Some(self.backend.clone());
        crate::action_dispatch::dispatcher_for_project(
            project_id,
            self.window_id,
            &self.workspace,
            &self.focus_manager,
            &backend,
            &self.terminals,
            &self.service_manager,
            &self.remote_manager,
            cx,
        ).unwrap_or_else(|| ActionDispatcher::Local {
            workspace: self.workspace.clone(),
            focus_manager: self.focus_manager.clone(),
            backend: self.backend.clone(),
            terminals: self.terminals.clone(),
            service_manager: self.service_manager.clone(),
            window_id: self.window_id,
        })
    }

    /// Resolve remote connection parameters for a remote project.
    /// Returns (host, port, token, actual_project_id) or None if unavailable.
    fn remote_params(
        &self,
        project_id: &str,
        connection_id: &str,
        cx: &Context<Self>,
    ) -> Option<(String, u16, String, String)> {
        let rm = self.remote_manager.as_ref()?.read(cx);
        let connections = rm.connections();
        let (config, _, _) = connections.iter().find(|(c, _, _)| c.id == connection_id)?;
        let token = config.saved_token.as_ref()?.clone();
        let actual_id = okena_core::client::strip_prefix(project_id, connection_id);
        Some((config.host.clone(), config.port, token, actual_id))
    }

    /// Build a GitProvider for the given project (local or remote).
    pub(super) fn build_git_provider(
        &self,
        project_id: &str,
        cx: &Context<Self>,
    ) -> Option<std::sync::Arc<dyn crate::views::overlays::diff_viewer::provider::GitProvider>> {
        use crate::views::overlays::diff_viewer::provider::{LocalGitProvider, RemoteGitProvider};
        let ws = self.workspace.read(cx);
        let project = ws.project(project_id)?;
        if project.is_remote {
            let conn_id = project.connection_id.as_ref()?;
            let (host, port, token, actual_id) = self.remote_params(project_id, conn_id, cx)?;
            Some(std::sync::Arc::new(RemoteGitProvider::new(host, port, token, actual_id)))
        } else {
            Some(std::sync::Arc::new(LocalGitProvider::new(project.path.clone())))
        }
    }

    /// Build a ProjectFs provider for the given project (local or remote).
    fn build_project_fs(
        &self,
        project_id: &str,
        cx: &Context<Self>,
    ) -> Option<std::sync::Arc<dyn okena_files::project_fs::ProjectFs>> {
        let ws = self.workspace.read(cx);
        let project = ws.project(project_id)?;
        if project.is_remote {
            let conn_id = project.connection_id.as_ref()?;
            let (host, port, token, actual_id) = self.remote_params(project_id, conn_id, cx)?;
            Some(std::sync::Arc::new(okena_files::project_fs::RemoteProjectFs::new(
                host, port, token, actual_id, project.name.clone(),
            )))
        } else {
            Some(std::sync::Arc::new(okena_files::project_fs::LocalProjectFs::new(
                project.path.clone(),
            )))
        }
    }
}

impl WindowView {
    /// Handle events from the OverlayManager that require WindowView access.
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
                // Collect hook terminal IDs before deleting so we can clean them from the registry
                let hook_tids = self.workspace.read(cx).hook_terminal_ids_for_project(project_id);
                let workspace = self.workspace.clone();
                let pid = project_id.clone();
                self.focus_manager.update(cx, |fm, cx| {
                    workspace.update(cx, |ws, cx| {
                        ws.delete_project(fm, &pid, &settings(cx).hooks, cx);
                    });
                    cx.notify();
                });
                for tid in hook_tids {
                    self.terminals.lock().remove(&tid);
                }
            }
            OverlayManagerEvent::ConfigureHooks { project_id } => {
                self.overlay_manager.update(cx, |om, cx| {
                    om.show_settings_for_project(project_id.clone(), cx);
                });
            }
            OverlayManagerEvent::ReloadServices { project_id } => {
                let dispatcher = self.dispatcher_for_project(project_id, cx);
                dispatcher.dispatch(okena_core::api::ActionRequest::ReloadServices {
                    project_id: project_id.clone(),
                }, cx);
            }
            OverlayManagerEvent::QuickCreateWorktree { project_id } => {
                self.request_broker.update(cx, |broker, cx| {
                    broker.push_sidebar_request(crate::workspace::requests::SidebarRequest::QuickCreateWorktree {
                        project_id: project_id.clone(),
                    }, cx);
                });
            }
            OverlayManagerEvent::ProjectColorChanged { project_id, color } => {
                self.sidebar.update(cx, |sidebar, cx| {
                    sidebar.sync_remote_color(project_id, *color, cx);
                });
            }
            OverlayManagerEvent::FocusParent { project_id } => {
                let parent_id = self.workspace.read(cx)
                    .project(project_id)
                    .and_then(|p| p.worktree_info.as_ref())
                    .map(|wt| wt.parent_project_id.clone());

                if let Some(parent_id) = parent_id {
                    let workspace = self.workspace.clone();
                    self.focus_manager.update(cx, |fm, cx| {
                        workspace.update(cx, |ws, cx| {
                            ws.set_focused_project(fm, Some(parent_id), cx);
                        });
                        cx.notify();
                    });
                }
            }
            OverlayManagerEvent::FocusProject(project_id) => {
                let workspace = self.workspace.clone();
                let pid = project_id.clone();
                self.focus_manager.update(cx, |fm, cx| {
                    workspace.update(cx, |ws, cx| {
                        ws.set_focused_project(fm, Some(pid), cx);
                    });
                    cx.notify();
                });
            }
            OverlayManagerEvent::ToggleProjectVisibility(project_id) => {
                let window_id = self.window_id;
                self.workspace.update(cx, |ws, cx| {
                    ws.toggle_project_overview_visibility(window_id, project_id, cx);
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
                        terminal.send_paste(&text);
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
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| {
                ws.replace_data(fm, data, cx);
            });
            cx.notify();
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
                OverlayRequest::Project(ProjectOverlay { project_id, kind }) => match kind {
                    ProjectOverlayKind::ContextMenu { position } => {
                        if !self.overlay_manager.read(cx).has_context_menu() {
                            self.overlay_manager.update(cx, |om, cx| {
                                om.show_context_menu(
                                    crate::workspace::requests::ContextMenuRequest { project_id, position },
                                    cx,
                                );
                            });
                        }
                    }
                    ProjectOverlayKind::ShellSelector { terminal_id, current_shell } => {
                        self.overlay_manager.update(cx, |om, cx| {
                            om.show_shell_selector(current_shell, project_id, terminal_id, cx);
                        });
                    }
                    ProjectOverlayKind::DiffViewer { file, mode, commit_message, commits, commit_index } => {
                        if let Some(provider) = self.build_git_provider(&project_id, cx) {
                            self.overlay_manager.update(cx, |om, cx| {
                                om.show_diff_viewer(provider, file, mode, commit_message, commits, commit_index, cx);
                            });
                        }
                    }
                    ProjectOverlayKind::TerminalContextMenu { terminal_id, layout_path, position, has_selection, link_url } => {
                        self.overlay_manager.update(cx, |om, cx| {
                            om.show_terminal_context_menu(terminal_id, project_id, layout_path, position, has_selection, link_url, cx);
                        });
                    }
                    ProjectOverlayKind::TabContextMenu { tab_index, num_tabs, layout_path, position } => {
                        self.overlay_manager.update(cx, |om, cx| {
                            om.show_tab_context_menu(tab_index, num_tabs, project_id, layout_path, position, cx);
                        });
                    }
                    ProjectOverlayKind::ShowServiceLog { service_name } => {
                        self.handle_show_service_log(project_id, service_name, cx);
                    }
                    ProjectOverlayKind::ShowHookTerminal { terminal_id } => {
                        if let Some(col) = self.project_columns.get(&project_id).cloned() {
                            col.update(cx, |col, cx| {
                                col.show_hook_terminal(&terminal_id, cx);
                            });
                        }
                    }
                    ProjectOverlayKind::FileSearch => {
                        if let Some(fs) = self.build_project_fs(&project_id, cx) {
                            self.overlay_manager.update(cx, |om, cx| {
                                om.toggle_file_search(fs, cx);
                            });
                        }
                    }
                    ProjectOverlayKind::ContentSearch => {
                        if let Some(fs) = self.build_project_fs(&project_id, cx) {
                            let is_dark = crate::theme::theme(cx).is_dark();
                            self.overlay_manager.update(cx, |om, cx| {
                                om.toggle_content_search(fs, is_dark, cx);
                            });
                        }
                    }
                    ProjectOverlayKind::FileBrowser => {
                        if let Some(fs) = self.build_project_fs(&project_id, cx) {
                            self.overlay_manager.update(cx, |om, cx| {
                                om.show_file_browser(fs, cx);
                            });
                        }
                    }
                    ProjectOverlayKind::ColorPicker { position } => {
                        self.overlay_manager.update(cx, |om, cx| {
                            om.show_color_picker(
                                okena_views_sidebar::ColorPickerTarget::Project { project_id },
                                position,
                                cx,
                            );
                        });
                    }
                    ProjectOverlayKind::WorktreeList { position } => {
                        self.overlay_manager.update(cx, |om, cx| {
                            om.show_worktree_list(project_id, position, cx);
                        });
                    }
                },
                OverlayRequest::Folder(FolderOverlay { folder_id, kind }) => match kind {
                    FolderOverlayKind::ContextMenu { folder_name, position } => {
                        if !self.overlay_manager.read(cx).has_folder_context_menu() {
                            self.overlay_manager.update(cx, |om, cx| {
                                om.show_folder_context_menu(
                                    crate::workspace::requests::FolderContextMenuRequest { folder_id, folder_name, position },
                                    cx,
                                );
                            });
                        }
                    }
                    FolderOverlayKind::ColorPicker { position } => {
                        self.overlay_manager.update(cx, |om, cx| {
                            om.show_color_picker(
                                okena_views_sidebar::ColorPickerTarget::Folder { folder_id },
                                position,
                                cx,
                            );
                        });
                    }
                },
                OverlayRequest::AddProjectDialog => {
                    let rm = self.remote_manager.clone();
                    self.overlay_manager.update(cx, |om, cx| {
                        om.toggle_add_project_dialog(rm, cx);
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
    cx: &Context<WindowView>,
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
