//! Terminal action handlers — create / split / close / focus and direct PTY I/O.

// Handlers take the workspace, focus manager, terminals registry and cx as
// distinct dependencies; bundling them into a context struct would obscure
// more than it clarifies here.
#![allow(clippy::too_many_arguments)]

use super::{
    ActionResult, ensure_terminal, find_terminal_path, spawn_uninitialized_terminals,
};
use crate::terminal::backend::TerminalBackend;
use crate::terminal::terminal::TerminalSize;
use crate::workspace::focus::FocusManager;
use crate::workspace::state::Workspace;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use gpui::*;
use okena_core::keys::SpecialKey;
use okena_core::types::SplitDirection;
use okena_terminal::TerminalsRegistry;

pub(super) fn create(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    ws.add_terminal(focus_manager, &project_id, cx);
    spawn_uninitialized_terminals(ws, &project_id, backend, terminals, cx)
}

pub(super) fn split(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    path: Vec<usize>,
    direction: SplitDirection,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    ws.split_terminal(focus_manager, &project_id, &path, direction, cx);
    spawn_uninitialized_terminals(ws, &project_id, backend, terminals, cx)
}

pub(super) fn close(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    terminal_id: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    let path = find_terminal_path(ws, &project_id, &terminal_id);
    match path {
        Some(path) => {
            backend.kill(&terminal_id);
            terminals.lock().remove(&terminal_id);
            ws.close_terminal_and_focus_sibling(focus_manager, &project_id, &path, cx);
            ActionResult::Ok(None)
        }
        None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
    }
}

pub(super) fn close_many(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    terminal_ids: Vec<String>,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    let mut last_err = None;
    for terminal_id in &terminal_ids {
        let path = find_terminal_path(ws, &project_id, terminal_id);
        match path {
            Some(path) => {
                backend.kill(terminal_id);
                terminals.lock().remove(terminal_id);
                ws.close_terminal_and_focus_sibling(focus_manager, &project_id, &path, cx);
            }
            None => {
                last_err = Some(format!("terminal not found: {}", terminal_id));
            }
        }
    }
    match last_err {
        Some(e) => ActionResult::Err(e),
        None => ActionResult::Ok(None),
    }
}

pub(super) fn focus(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    terminal_id: String,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    let path = find_terminal_path(ws, &project_id, &terminal_id);
    match path {
        Some(path) => {
            ws.set_focused_terminal(focus_manager, project_id, path, cx);
            ActionResult::Ok(None)
        }
        None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
    }
}

pub(super) fn send_text(
    ws: &mut Workspace,
    terminal_id: String,
    text: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
) -> ActionResult {
    match ensure_terminal(&terminal_id, terminals, backend, ws) {
        Some(term) => {
            term.claim_resize_remote();
            term.send_input(&text);
            ActionResult::Ok(None)
        }
        None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
    }
}

pub(super) fn run_command(
    ws: &mut Workspace,
    terminal_id: String,
    command: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
) -> ActionResult {
    match ensure_terminal(&terminal_id, terminals, backend, ws) {
        Some(term) => {
            term.claim_resize_remote();
            term.send_input(&format!("{}\r", command));
            ActionResult::Ok(None)
        }
        None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
    }
}

pub(super) fn send_special_key(
    ws: &mut Workspace,
    terminal_id: String,
    key: SpecialKey,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
) -> ActionResult {
    match ensure_terminal(&terminal_id, terminals, backend, ws) {
        Some(term) => {
            term.claim_resize_remote();
            term.send_bytes(&key.to_bytes());
            ActionResult::Ok(None)
        }
        None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
    }
}

pub(super) fn resize(
    ws: &mut Workspace,
    terminal_id: String,
    cols: u16,
    rows: u16,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
) -> ActionResult {
    match ensure_terminal(&terminal_id, terminals, backend, ws) {
        Some(term) => {
            term.claim_resize_remote();
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
    }
}

pub(super) fn update_split_sizes(
    ws: &mut Workspace,
    project_id: String,
    path: Vec<usize>,
    sizes: Vec<f32>,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    ws.update_split_sizes(&project_id, &path, sizes, cx);
    ActionResult::Ok(None)
}

pub(super) fn toggle_minimized(
    ws: &mut Workspace,
    project_id: String,
    terminal_id: String,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    ws.toggle_terminal_minimized_by_id(&project_id, &terminal_id, cx);
    ActionResult::Ok(None)
}

pub(super) fn set_fullscreen(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    terminal_id: Option<String>,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    match terminal_id {
        Some(tid) => ws.set_fullscreen_terminal(focus_manager, project_id, tid, cx),
        None => ws.exit_fullscreen(focus_manager, cx),
    }
    ActionResult::Ok(None)
}

pub(super) fn rename(
    ws: &mut Workspace,
    project_id: String,
    terminal_id: String,
    name: String,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    ws.rename_terminal(&project_id, &terminal_id, name, cx);
    ActionResult::Ok(None)
}

pub(super) fn read_content(
    ws: &mut Workspace,
    terminal_id: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
) -> ActionResult {
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

                while lines.last().is_some_and(|l| l.is_empty()) {
                    lines.pop();
                }

                lines.join("\n")
            });
            ActionResult::Ok(Some(serde_json::json!({"content": content})))
        }
        None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
    }
}
