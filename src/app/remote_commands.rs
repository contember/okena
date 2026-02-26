use crate::remote::bridge::{BridgeMessage, BridgeReceiver, CommandResult, RemoteCommand};
use crate::remote::types::{ActionRequest, ApiFolder, ApiFullscreen, ApiProject, ApiServiceInfo, StateResponse};
use crate::services::manager::ServiceStatus;
use crate::terminal::backend::TerminalBackend;
use crate::workspace::actions::execute::{ensure_terminal, execute_action};
use gpui::*;
use std::collections::HashSet;
use std::sync::Arc;

use super::Okena;

impl Okena {
    /// Process commands from the remote API bridge.
    /// Runs on the GPUI main thread via cx.spawn().
    pub(super) fn start_remote_command_loop(
        &mut self,
        bridge_rx: BridgeReceiver,
        backend: Arc<dyn TerminalBackend>,
        cx: &mut Context<Self>,
    ) {
        let workspace = self.workspace.clone();
        let terminals = self.terminals.clone();
        let state_version = self.state_version.clone();
        let git_status_tx = self.git_status_tx.clone();
        let service_manager = self.service_manager.clone();

        cx.spawn(async move |_this: WeakEntity<Okena>, cx| {
            loop {
                let msg: BridgeMessage = match bridge_rx.recv().await {
                    Ok(msg) => msg,
                    Err(_) => break,
                };

                let result = match msg.command {
                    RemoteCommand::Action(action) => {
                        match action {
                            ActionRequest::StartService { project_id, service_name } => {
                                cx.update(|cx| {
                                    service_manager.update(cx, |sm, cx| {
                                        if let Some(path) = sm.project_path(&project_id).cloned() {
                                            sm.start_service(&project_id, &service_name, &path, cx);
                                            CommandResult::Ok(None)
                                        } else {
                                            CommandResult::Err(format!("project not found: {}", project_id))
                                        }
                                    })
                                })
                            }
                            ActionRequest::StopService { project_id, service_name } => {
                                cx.update(|cx| {
                                    service_manager.update(cx, |sm, cx| {
                                        sm.stop_service(&project_id, &service_name, cx);
                                        CommandResult::Ok(None)
                                    })
                                })
                            }
                            ActionRequest::RestartService { project_id, service_name } => {
                                cx.update(|cx| {
                                    service_manager.update(cx, |sm, cx| {
                                        if let Some(path) = sm.project_path(&project_id).cloned() {
                                            sm.restart_service(&project_id, &service_name, &path, cx);
                                            CommandResult::Ok(None)
                                        } else {
                                            CommandResult::Err(format!("project not found: {}", project_id))
                                        }
                                    })
                                })
                            }
                            ActionRequest::StartAllServices { project_id } => {
                                cx.update(|cx| {
                                    service_manager.update(cx, |sm, cx| {
                                        if let Some(path) = sm.project_path(&project_id).cloned() {
                                            sm.start_all(&project_id, &path, cx);
                                            CommandResult::Ok(None)
                                        } else {
                                            CommandResult::Err(format!("project not found: {}", project_id))
                                        }
                                    })
                                })
                            }
                            ActionRequest::StopAllServices { project_id } => {
                                cx.update(|cx| {
                                    service_manager.update(cx, |sm, cx| {
                                        sm.stop_all(&project_id, cx);
                                        CommandResult::Ok(None)
                                    })
                                })
                            }
                            ActionRequest::ReloadServices { project_id } => {
                                cx.update(|cx| {
                                    service_manager.update(cx, |sm, cx| {
                                        if let Some(path) = sm.project_path(&project_id).cloned() {
                                            sm.reload_project_services(&project_id, &path, cx);
                                            CommandResult::Ok(None)
                                        } else {
                                            CommandResult::Err(format!("project not found: {}", project_id))
                                        }
                                    })
                                })
                            }
                            action => {
                                cx.update(|cx| {
                                    workspace.update(cx, |ws, cx| {
                                        execute_action(action, ws, &*backend, &terminals, cx)
                                            .into_command_result()
                                    })
                                })
                            }
                        }
                    }
                    RemoteCommand::GetState => {
                        cx.update(|cx| {
                            let ws = workspace.read(cx);
                            let sm = service_manager.read(cx);
                            let sv = *state_version.borrow();
                            let git_statuses = git_status_tx.borrow().clone();
                            let data = ws.data();

                            // Build a lookup map for projects
                            let project_map: std::collections::HashMap<&str, &crate::workspace::state::ProjectData> =
                                data.projects.iter().map(|p| (p.id.as_str(), p)).collect();

                            // Build ordered projects following project_order + folder expansion
                            let mut projects: Vec<ApiProject> = Vec::new();
                            let mut seen: HashSet<String> = HashSet::new();

                            let build_api_project = |p: &crate::workspace::state::ProjectData| -> ApiProject {
                                let git_status = git_statuses.get(&p.id).cloned();
                                let services: Vec<ApiServiceInfo> = sm.services_for_project(&p.id)
                                    .into_iter()
                                    .map(|inst| {
                                        let (status, exit_code) = match &inst.status {
                                            ServiceStatus::Stopped => ("stopped", None),
                                            ServiceStatus::Starting => ("starting", None),
                                            ServiceStatus::Running => ("running", None),
                                            ServiceStatus::Crashed { exit_code } => ("crashed", *exit_code),
                                            ServiceStatus::Restarting => ("restarting", None),
                                        };
                                        let kind = match &inst.kind {
                                            crate::services::manager::ServiceKind::Okena => "okena",
                                            crate::services::manager::ServiceKind::DockerCompose { .. } => "docker_compose",
                                        };
                                        ApiServiceInfo {
                                            name: inst.definition.name.clone(),
                                            status: status.to_string(),
                                            terminal_id: inst.terminal_id.clone(),
                                            ports: inst.detected_ports.clone(),
                                            exit_code,
                                            kind: kind.to_string(),
                                        }
                                    })
                                    .collect();
                                ApiProject {
                                    id: p.id.clone(),
                                    name: p.name.clone(),
                                    path: p.path.clone(),
                                    is_visible: p.is_visible,
                                    layout: p.layout.as_ref().map(|l| l.to_api()),
                                    terminal_names: p.terminal_names.clone(),
                                    git_status,
                                    folder_color: p.folder_color,
                                    services,
                                }
                            };

                            for id in &data.project_order {
                                if let Some(folder) = data.folders.iter().find(|f| &f.id == id) {
                                    for pid in &folder.project_ids {
                                        if seen.insert(pid.clone()) {
                                            if let Some(p) = project_map.get(pid.as_str()) {
                                                projects.push(build_api_project(p));
                                            }
                                        }
                                    }
                                } else if seen.insert(id.clone()) {
                                    if let Some(p) = project_map.get(id.as_str()) {
                                        projects.push(build_api_project(p));
                                    }
                                }
                            }

                            // Append orphan projects not in any order
                            for p in &data.projects {
                                if seen.insert(p.id.clone()) {
                                    projects.push(build_api_project(p));
                                }
                            }

                            // Build folders for response
                            let folders: Vec<ApiFolder> = data.folders.iter().map(|f| {
                                ApiFolder {
                                    id: f.id.clone(),
                                    name: f.name.clone(),
                                    project_ids: f.project_ids.clone(),
                                    folder_color: f.folder_color,
                                }
                            }).collect();

                            let fullscreen = ws.focus_manager.fullscreen_state().map(|(pid, tid)| {
                                ApiFullscreen {
                                    project_id: pid.to_string(),
                                    terminal_id: tid.to_string(),
                                }
                            });

                            let resp = StateResponse {
                                state_version: sv,
                                projects,
                                focused_project_id: ws.focused_project_id().cloned(),
                                fullscreen_terminal: fullscreen,
                                project_order: data.project_order.clone(),
                                folders,
                            };

                            CommandResult::Ok(Some(serde_json::to_value(resp).expect("BUG: StateResponse must serialize")))
                        })
                    }
                    RemoteCommand::GetTerminalSizes { terminal_ids } => {
                        cx.update(|_cx| {
                            let terms = terminals.lock();
                            let mut sizes = std::collections::HashMap::new();
                            for id in &terminal_ids {
                                if let Some(term) = terms.get(id) {
                                    let s = term.resize_state.lock().size;
                                    sizes.insert(id.clone(), (s.cols, s.rows));
                                }
                            }
                            let val = serde_json::to_value(sizes).expect("BUG: sizes must serialize");
                            CommandResult::Ok(Some(val))
                        })
                    }
                    RemoteCommand::RenderSnapshot { terminal_id } => {
                        cx.update(|cx| {
                            let ws = workspace.read(cx);
                            match ensure_terminal(&terminal_id, &terminals, &*backend, ws) {
                                Some(term) => {
                                    let snapshot = term.render_snapshot();
                                    CommandResult::OkBytes(snapshot)
                                }
                                None => CommandResult::Err(format!("terminal not found: {}", terminal_id)),
                            }
                        })
                    }
                };

                let _ = msg.reply.send(result);
            }
        })
        .detach();
    }
}
