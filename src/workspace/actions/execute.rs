//! Unified action execution layer.
//!
//! Single entry point for all `ActionRequest` actions — used by both
//! the desktop UI and the remote API to eliminate code duplication
//! and ensure consistent behavior.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use crate::remote::bridge::CommandResult;
use crate::remote::types::ActionRequest;
use crate::terminal::backend::TerminalBackend;
use crate::terminal::terminal::{Terminal, TerminalSize};
use crate::views::root::TerminalsRegistry;
use crate::workspace::state::{LayoutNode, Workspace};
use gpui::*;
use std::sync::Arc;

/// Result of executing an action.
pub enum ActionResult {
    /// Success with optional JSON payload.
    Ok(Option<serde_json::Value>),
    /// Error with human-readable message.
    Err(String),
}

impl ActionResult {
    pub fn into_command_result(self) -> CommandResult {
        match self {
            ActionResult::Ok(v) => CommandResult::Ok(v),
            ActionResult::Err(e) => CommandResult::Err(e),
        }
    }
}

/// Execute any `ActionRequest` against the workspace.
///
/// This is the single source of truth for all client-facing actions.
/// Both desktop UI handlers and the remote API delegate here.
pub fn execute_action(
    action: ActionRequest,
    ws: &mut Workspace,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    match action {
        ActionRequest::CreateTerminal { project_id } => {
            ws.start_terminal(&project_id, cx);
            spawn_uninitialized_terminals(ws, &project_id, backend, terminals, cx)
        }
        ActionRequest::SplitTerminal {
            project_id,
            path,
            direction,
        } => {
            ws.split_terminal(&project_id, &path, direction, cx);
            spawn_uninitialized_terminals(ws, &project_id, backend, terminals, cx)
        }
        ActionRequest::CloseTerminal {
            project_id,
            terminal_id,
        } => {
            let path = find_terminal_path(ws, &project_id, &terminal_id);
            match path {
                Some(path) => {
                    backend.kill(&terminal_id);
                    terminals.lock().remove(&terminal_id);
                    ws.close_terminal_and_focus_sibling(&project_id, &path, cx);
                    ActionResult::Ok(None)
                }
                None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
            }
        }
        ActionRequest::FocusTerminal {
            project_id,
            terminal_id,
        } => {
            let path = find_terminal_path(ws, &project_id, &terminal_id);
            match path {
                Some(path) => {
                    ws.set_focused_terminal(project_id, path, cx);
                    ActionResult::Ok(None)
                }
                None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
            }
        }
        ActionRequest::SendText { terminal_id, text } => {
            match ensure_terminal(&terminal_id, terminals, backend, ws) {
                Some(term) => {
                    term.send_input(&text);
                    ActionResult::Ok(None)
                }
                None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
            }
        }
        ActionRequest::RunCommand {
            terminal_id,
            command,
        } => match ensure_terminal(&terminal_id, terminals, backend, ws) {
            Some(term) => {
                term.send_input(&format!("{}\r", command));
                ActionResult::Ok(None)
            }
            None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
        },
        ActionRequest::SendSpecialKey { terminal_id, key } => {
            match ensure_terminal(&terminal_id, terminals, backend, ws) {
                Some(term) => {
                    term.send_bytes(key.to_bytes());
                    ActionResult::Ok(None)
                }
                None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
            }
        }
        ActionRequest::Resize {
            terminal_id,
            cols,
            rows,
        } => match ensure_terminal(&terminal_id, terminals, backend, ws) {
            Some(term) => {
                let size = TerminalSize {
                    cols,
                    rows,
                    cell_width: 8.0,
                    cell_height: 16.0,
                };
                term.resize(size);
                ActionResult::Ok(None)
            }
            None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
        },
        ActionRequest::UpdateSplitSizes {
            project_id,
            path,
            sizes,
        } => {
            ws.update_split_sizes(&project_id, &path, sizes, cx);
            ActionResult::Ok(None)
        }
        ActionRequest::ReadContent { terminal_id } => {
            match ensure_terminal(&terminal_id, terminals, backend, ws) {
                Some(term) => {
                    let content = term.with_content(|term| {
                        let grid = term.grid();
                        let screen_lines = grid.screen_lines();
                        let cols = grid.columns();
                        let mut lines = Vec::with_capacity(screen_lines);

                        for row in 0..screen_lines as i32 {
                            let mut line = String::with_capacity(cols);
                            for col in 0..cols {
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
                    ActionResult::Ok(Some(serde_json::json!({"content": content})))
                }
                None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
            }
        }
    }
}

/// Look up a terminal in the registry. If not found, attempt to spawn it
/// by finding the terminal_id in the workspace layout and creating a PTY.
pub fn ensure_terminal(
    terminal_id: &str,
    terminals: &TerminalsRegistry,
    backend: &dyn TerminalBackend,
    ws: &Workspace,
) -> Option<Arc<Terminal>> {
    // Fast path: already in registry
    if let Some(term) = terminals.lock().get(terminal_id).cloned() {
        return Some(term);
    }

    // Find which project owns this terminal_id and get its path
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

    // Spawn PTY via backend
    match backend.reconnect_terminal(terminal_id, &cwd, None) {
        Ok(_id) => {
            let terminal = Arc::new(Terminal::new(
                terminal_id.to_string(),
                TerminalSize::default(),
                backend.transport(),
                cwd,
            ));
            terminals
                .lock()
                .insert(terminal_id.to_string(), terminal.clone());
            log::info!("Auto-spawned terminal {} for remote client", terminal_id);
            Some(terminal)
        }
        Err(e) => {
            log::error!("Failed to auto-spawn terminal {}: {}", terminal_id, e);
            None
        }
    }
}

/// Spawn PTYs for any uninitialized terminals (`terminal_id: None`) in a project's layout.
///
/// Used after `CreateTerminal` / `SplitTerminal` to eagerly create PTYs for
/// remote clients that don't have a rendering layer to trigger lazy spawning.
pub fn spawn_uninitialized_terminals(
    ws: &mut Workspace,
    project_id: &str,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    let project = match ws.project(project_id) {
        Some(p) => p,
        None => return ActionResult::Err(format!("project not found: {}", project_id)),
    };

    let project_path = project.path.clone();
    let mut uninitialized = Vec::new();
    if let Some(layout) = &project.layout {
        collect_uninitialized_terminals(layout, vec![], &mut uninitialized);
    }

    for path in uninitialized {
        match backend.create_terminal(&project_path, None) {
            Ok(terminal_id) => {
                ws.set_terminal_id(project_id, &path, terminal_id.clone(), cx);
                let terminal = Arc::new(Terminal::new(
                    terminal_id.clone(),
                    TerminalSize::default(),
                    backend.transport(),
                    project_path.clone(),
                ));
                terminals.lock().insert(terminal_id, terminal);
            }
            Err(e) => {
                log::error!(
                    "Failed to spawn terminal for project {}: {}",
                    project_id,
                    e
                );
                return ActionResult::Err(format!("failed to spawn terminal: {}", e));
            }
        }
    }

    ActionResult::Ok(None)
}

/// Find the layout path for a terminal within a project.
pub fn find_terminal_path(
    ws: &Workspace,
    project_id: &str,
    terminal_id: &str,
) -> Option<Vec<usize>> {
    ws.project(project_id)?
        .layout
        .as_ref()?
        .find_terminal_path(terminal_id)
}

/// Recursively collect paths to all Terminal nodes with `terminal_id: None`.
pub fn collect_uninitialized_terminals(
    node: &LayoutNode,
    current_path: Vec<usize>,
    result: &mut Vec<Vec<usize>>,
) {
    match node {
        LayoutNode::Terminal {
            terminal_id: None, ..
        } => {
            result.push(current_path);
        }
        LayoutNode::Terminal { .. } => {}
        LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
            for (i, child) in children.iter().enumerate() {
                let mut child_path = current_path.clone();
                child_path.push(i);
                collect_uninitialized_terminals(child, child_path, result);
            }
        }
    }
}
