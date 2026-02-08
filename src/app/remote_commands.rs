use alacritty_terminal::grid::Dimensions;
use crate::remote::bridge::{BridgeMessage, BridgeReceiver, CommandResult, RemoteCommand};
use crate::remote::types::{ApiFullscreen, ApiProject, StateResponse};
use crate::terminal::terminal::{Terminal, TerminalSize};
use crate::terminal::pty_manager::PtyManager;
use crate::views::root::TerminalsRegistry;
use crate::workspace::state::Workspace;
use gpui::*;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::Okena;

/// Look up a terminal in the registry. If not found, attempt to spawn it by
/// finding the terminal_id in the workspace layout and creating a PTY for it.
fn ensure_terminal(
    terminal_id: &str,
    terminals: &TerminalsRegistry,
    pty_manager: &Arc<PtyManager>,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) -> Option<Arc<Terminal>> {
    // Fast path: already in registry
    if let Some(term) = terminals.lock().get(terminal_id).cloned() {
        return Some(term);
    }

    // Find which project owns this terminal_id and get its path
    let ws = workspace.read(cx);
    let mut cwd = None;
    for project in &ws.data().projects {
        if let Some(layout) = &project.layout {
            if layout.find_terminal_path(terminal_id).is_some() {
                cwd = Some(project.path.clone());
                break;
            }
        }
    }
    let cwd = cwd?;

    // Spawn PTY via PtyManager
    match pty_manager.create_or_reconnect_terminal_with_shell(Some(terminal_id), &cwd, None) {
        Ok(_id) => {
            let terminal = Arc::new(Terminal::new(
                terminal_id.to_string(),
                TerminalSize::default(),
                pty_manager.clone(),
                cwd,
            ));
            terminals.lock().insert(terminal_id.to_string(), terminal.clone());
            log::info!("Auto-spawned terminal {} for remote client", terminal_id);
            Some(terminal)
        }
        Err(e) => {
            log::error!("Failed to auto-spawn terminal {}: {}", terminal_id, e);
            None
        }
    }
}

impl Okena {
    /// Process commands from the remote API bridge.
    /// Runs on the GPUI main thread via cx.spawn().
    pub(super) fn start_remote_command_loop(
        &mut self,
        bridge_rx: BridgeReceiver,
        cx: &mut Context<Self>,
    ) {
        let workspace = self.workspace.clone();
        let terminals = self.terminals.clone();
        let state_version = self.state_version.clone();
        let pty_manager = self.pty_manager.clone();

        cx.spawn(async move |_this: WeakEntity<Okena>, cx| {
            loop {
                let msg: BridgeMessage = match bridge_rx.recv().await {
                    Ok(msg) => msg,
                    Err(_) => break,
                };

                let result = match msg.command {
                    RemoteCommand::GetState => {
                        cx.update(|cx| {
                            let ws = workspace.read(cx);
                            let sv = state_version.load(Ordering::Relaxed);
                            let projects: Vec<ApiProject> = ws.data().projects.iter().map(|p| {
                                ApiProject {
                                    id: p.id.clone(),
                                    name: p.name.clone(),
                                    path: p.path.clone(),
                                    is_visible: p.is_visible,
                                    layout: p.layout.as_ref().map(|l| l.to_api()),
                                    terminal_names: p.terminal_names.clone(),
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
                            };

                            CommandResult::Ok(Some(serde_json::to_value(resp).expect("BUG: StateResponse must serialize")))
                        })
                    }
                    RemoteCommand::SendText { terminal_id, text } => {
                        cx.update(|cx| {
                            match ensure_terminal(&terminal_id, &terminals, &pty_manager, &workspace, cx) {
                                Some(term) => {
                                    term.send_input(&text);
                                    CommandResult::Ok(None)
                                }
                                None => CommandResult::Err(format!("terminal not found: {}", terminal_id)),
                            }
                        })
                    }
                    RemoteCommand::RunCommand { terminal_id, command } => {
                        cx.update(|cx| {
                            match ensure_terminal(&terminal_id, &terminals, &pty_manager, &workspace, cx) {
                                Some(term) => {
                                    term.send_input(&format!("{}\r", command));
                                    CommandResult::Ok(None)
                                }
                                None => CommandResult::Err(format!("terminal not found: {}", terminal_id)),
                            }
                        })
                    }
                    RemoteCommand::SendSpecialKey { terminal_id, key } => {
                        cx.update(|cx| {
                            match ensure_terminal(&terminal_id, &terminals, &pty_manager, &workspace, cx) {
                                Some(term) => {
                                    term.send_bytes(key.to_bytes());
                                    CommandResult::Ok(None)
                                }
                                None => CommandResult::Err(format!("terminal not found: {}", terminal_id)),
                            }
                        })
                    }
                    RemoteCommand::ReadContent { terminal_id } => {
                        cx.update(|cx| {
                            match ensure_terminal(&terminal_id, &terminals, &pty_manager, &workspace, cx) {
                                Some(term) => {
                                    let content = term.with_content(|term| {
                                        let grid = term.grid();
                                        let screen_lines = grid.screen_lines();
                                        let cols = grid.columns();
                                        let mut lines = Vec::with_capacity(screen_lines);

                                        for row in 0..screen_lines as i32 {
                                            let mut line = String::with_capacity(cols);
                                            for col in 0..cols {
                                                use alacritty_terminal::index::{Point, Line, Column};
                                                let cell = &grid[Point::new(Line(row), Column(col))];
                                                line.push(cell.c);
                                            }
                                            let trimmed = line.trim_end().to_string();
                                            lines.push(trimmed);
                                        }

                                        while lines.last().map_or(false, |l| l.is_empty()) {
                                            lines.pop();
                                        }

                                        lines.join("\n")
                                    });
                                    CommandResult::Ok(Some(serde_json::json!({"content": content})))
                                }
                                None => CommandResult::Err(format!("terminal not found: {}", terminal_id)),
                            }
                        })
                    }
                    RemoteCommand::Resize { terminal_id, cols, rows } => {
                        cx.update(|cx| {
                            match ensure_terminal(&terminal_id, &terminals, &pty_manager, &workspace, cx) {
                                Some(term) => {
                                    let size = TerminalSize {
                                        cols,
                                        rows,
                                        cell_width: 8.0,
                                        cell_height: 16.0,
                                    };
                                    term.resize(size);
                                    CommandResult::Ok(None)
                                }
                                None => CommandResult::Err(format!("terminal not found: {}", terminal_id)),
                            }
                        })
                    }
                    RemoteCommand::SplitTerminal { project_id, path, direction } => {
                        cx.update(|cx| {
                            workspace.update(cx, |ws, cx| {
                                let ok = ws.with_layout_node(&project_id, &path, cx, |node| {
                                    let existing = node.clone();
                                    let new_terminal = crate::workspace::state::LayoutNode::new_terminal();
                                    *node = crate::workspace::state::LayoutNode::Split {
                                        direction,
                                        sizes: vec![0.5, 0.5],
                                        children: vec![existing, new_terminal],
                                    };
                                    true
                                });
                                if ok {
                                    CommandResult::Ok(None)
                                } else {
                                    CommandResult::Err(format!("project or path not found: {}:{:?}", project_id, path))
                                }
                            })
                        })
                    }
                    RemoteCommand::CloseTerminal { project_id, terminal_id } => {
                        cx.update(|cx| {
                            workspace.update(cx, |ws, cx| {
                                let path = ws.project(&project_id)
                                    .and_then(|p| p.layout.as_ref())
                                    .and_then(|layout| layout.find_terminal_path(&terminal_id));

                                match path {
                                    Some(path) => {
                                        ws.close_terminal(&project_id, &path, cx);
                                        CommandResult::Ok(None)
                                    }
                                    None => CommandResult::Err(format!("terminal not found: {}", terminal_id)),
                                }
                            })
                        })
                    }
                    RemoteCommand::FocusTerminal { project_id, terminal_id } => {
                        cx.update(|cx| {
                            workspace.update(cx, |ws, cx| {
                                let path = ws.project(&project_id)
                                    .and_then(|p| p.layout.as_ref())
                                    .and_then(|layout| layout.find_terminal_path(&terminal_id));

                                match path {
                                    Some(path) => {
                                        ws.set_focused_terminal(project_id, path, cx);
                                        CommandResult::Ok(None)
                                    }
                                    None => CommandResult::Err(format!("terminal not found: {}", terminal_id)),
                                }
                            })
                        })
                    }
                };

                let _ = msg.reply.send(result);
            }
        })
        .detach();
    }
}
