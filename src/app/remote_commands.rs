use crate::remote::bridge::{BridgeMessage, BridgeReceiver, CommandResult, RemoteCommand};
use crate::remote::types::{ApiFolder, ApiFullscreen, ApiProject, StateResponse};
use crate::terminal::backend::TerminalBackend;
use crate::workspace::actions::execute::{ensure_terminal, execute_action};
use gpui::*;
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

        cx.spawn(async move |_this: WeakEntity<Okena>, cx| {
            loop {
                let msg: BridgeMessage = match bridge_rx.recv().await {
                    Ok(msg) => msg,
                    Err(_) => break,
                };

                let result = match msg.command {
                    RemoteCommand::Action(action) => {
                        cx.update(|cx| {
                            workspace.update(cx, |ws, cx| {
                                execute_action(action, ws, &*backend, &terminals, cx)
                                    .into_command_result()
                            })
                        })
                    }
                    RemoteCommand::GetState => {
                        cx.update(|cx| {
                            let ws = workspace.read(cx);
                            let sv = *state_version.borrow();

                            // Build terminal size map from the registry
                            let size_map: std::collections::HashMap<String, (u16, u16)> = {
                                let registry = terminals.lock();
                                registry.iter().map(|(id, term)| {
                                    let size = term.resize_state.lock().size;
                                    (id.clone(), (size.cols, size.rows))
                                }).collect()
                            };

                            let data = ws.data();

                            let projects: Vec<ApiProject> = data.projects.iter().map(|p| {
                                ApiProject {
                                    id: p.id.clone(),
                                    name: p.name.clone(),
                                    path: p.path.clone(),
                                    is_visible: p.is_visible,
                                    layout: p.layout.as_ref().map(|l| l.to_api_with_sizes(&size_map)),
                                    terminal_names: p.terminal_names.clone(),
                                    folder_color: p.folder_color,
                                }
                            }).collect();

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
                                folders,
                                project_order: data.project_order.clone(),
                            };

                            CommandResult::Ok(Some(serde_json::to_value(resp).expect("BUG: StateResponse must serialize")))
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
