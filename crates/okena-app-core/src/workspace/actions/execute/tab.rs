//! Tab and pane-move action handlers.

// Handlers take the workspace, focus manager, terminals registry and cx as
// distinct dependencies; bundling them into a context struct would obscure
// more than it clarifies here.
#![allow(clippy::too_many_arguments)]

use super::{ActionResult, spawn_uninitialized_terminals};
use okena_terminal::backend::TerminalBackend;
use crate::workspace::focus::FocusManager;
use crate::workspace::state::{DropZone, Workspace};
use gpui::*;
use okena_terminal::TerminalsRegistry;

pub(super) fn add_tab(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    path: Vec<usize>,
    in_group: bool,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    if in_group {
        ws.add_tab_to_group(focus_manager, &project_id, &path, cx);
    } else {
        ws.add_tab(focus_manager, &project_id, &path, cx);
    }
    spawn_uninitialized_terminals(ws, &project_id, backend, terminals, cx)
}

pub(super) fn set_active_tab(
    ws: &mut Workspace,
    project_id: String,
    path: Vec<usize>,
    index: usize,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    ws.set_active_tab(&project_id, &path, index, cx);
    ActionResult::Ok(None)
}

pub(super) fn move_tab(
    ws: &mut Workspace,
    project_id: String,
    path: Vec<usize>,
    from_index: usize,
    to_index: usize,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    ws.move_tab(&project_id, &path, from_index, to_index, cx);
    ActionResult::Ok(None)
}

pub(super) fn move_terminal_to_tab_group(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    terminal_id: String,
    target_path: Vec<usize>,
    position: Option<usize>,
    target_project_id: Option<String>,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    let target_pid = target_project_id.as_deref().unwrap_or(&project_id);
    ws.move_terminal_to_tab_group(focus_manager, &project_id, &terminal_id, target_pid, &target_path, position, cx);
    ActionResult::Ok(None)
}

pub(super) fn move_pane_to(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    terminal_id: String,
    target_project_id: String,
    target_terminal_id: String,
    zone: String,
    cx: &mut Context<Workspace>,
) -> ActionResult {
    let drop_zone = match zone.as_str() {
        "top" => DropZone::Top,
        "bottom" => DropZone::Bottom,
        "left" => DropZone::Left,
        "right" => DropZone::Right,
        "center" => DropZone::Center,
        _ => return ActionResult::Err(format!("invalid drop zone: {}", zone)),
    };
    ws.move_pane(focus_manager, &project_id, &terminal_id, &target_project_id, &target_terminal_id, drop_zone, cx);
    ActionResult::Ok(None)
}
