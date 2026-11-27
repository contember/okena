//! Tab and pane-move action handlers.

// Handlers take the workspace, focus manager, terminals registry and cx as
// distinct dependencies; bundling them into a context struct would obscure
// more than it clarifies here.
#![allow(clippy::too_many_arguments)]

use super::{
    ActionResult, ProjectTerminalLaunch, register_spawned_terminal, spawn_uninitialized_terminals,
};
use crate::workspace::focus::FocusManager;
use crate::workspace::persistence::AppSettings;
use crate::workspace::state::{DropZone, LayoutNode, WindowId, Workspace};
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::TerminalBackend;
use okena_terminal::shell_config::ShellType;
use okena_workspace::context::WorkspaceCx;

pub(super) fn add_tab(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    path: Vec<usize>,
    in_group: bool,
    start_page: bool,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    // Inherit the source group/terminal's live cwd (captured before the layout
    // mutation) so the new tab opens in the same directory.
    let inherit_cwd = super::inherited_cwd(ws, terminals, &project_id, &path);
    let leaf = match super::terminal::interactive_leaf(
        ws,
        &project_id,
        inherit_cwd.as_deref(),
        start_page,
    ) {
        Ok(leaf) => leaf,
        Err(error) => return error,
    };
    if in_group {
        ws.add_tab_to_group_with_leaf(focus_manager, &project_id, &path, leaf, cx);
    } else {
        ws.add_tab_with_leaf(focus_manager, &project_id, &path, leaf, cx);
    }
    if start_page {
        return ActionResult::Ok(None);
    }
    spawn_uninitialized_terminals(
        ws,
        &project_id,
        backend,
        terminals,
        settings,
        inherit_cwd,
        cx,
    )
}

pub(super) fn add_command_tab(
    ws: &mut Workspace,
    window_id: WindowId,
    focus_manager: &mut FocusManager,
    project_id: &str,
    source_id: &str,
    shell: ShellType,
    command: &str,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    if command.trim().is_empty() {
        return ActionResult::Err("command must not be empty".to_owned());
    }
    let Some(project) = ws.project(project_id) else {
        return ActionResult::Err(format!("project not found: {project_id}"));
    };
    let source = project.layout.as_ref().and_then(|layout| {
        let path = layout.find_terminal_path(source_id)?;
        match layout.get_at_path(&path)? {
            LayoutNode::Terminal {
                terminal_id: Some(_),
                start_page: None,
                ..
            } => Some(path),
            _ => None,
        }
    });
    let Some(path) = source.filter(|_| {
        terminals.lock().contains_key(source_id) && backend.has_live_terminal(source_id)
    }) else {
        return ActionResult::Err(format!(
            "source terminal is not running in this project: {source_id}"
        ));
    };
    let cwd = super::inherited_cwd(ws, terminals, project_id, &path)
        .unwrap_or_else(|| project.path.clone());
    let launch = ProjectTerminalLaunch::new(ws, project, settings);
    let plan = launch.plan(shell.clone(), &settings.default_shell, Some(command));

    // Publish the new tab only after its command shell has started.
    let terminal_id = match backend.create_terminal_with_plan(&cwd, &plan) {
        Ok(terminal_id) => terminal_id,
        Err(error) => {
            log::error!("Failed to spawn command tab for project {project_id}: {error}");
            return ActionResult::Err(format!("failed to spawn terminal: {error}"));
        }
    };
    register_spawned_terminal(terminals, backend, &terminal_id, &cwd);
    let leaf = LayoutNode::Terminal {
        terminal_id: Some(terminal_id.clone()),
        start_page: None,
        pending_agent_resume: None,
        minimized: false,
        detached: false,
        shell_type: shell,
        zoom_level: 1.0,
    };
    ws.add_tab_with_leaf(focus_manager, project_id, &path, leaf, cx);
    ws.focus_terminal_by_id(focus_manager, window_id, project_id, &terminal_id, cx);
    ActionResult::Ok(Some(serde_json::json!({ "terminal_ids": [terminal_id] })))
}

pub(super) fn set_active_tab(
    ws: &mut Workspace,
    project_id: String,
    path: Vec<usize>,
    index: usize,
    cx: &mut impl WorkspaceCx,
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
    cx: &mut impl WorkspaceCx,
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
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let target_pid = target_project_id.as_deref().unwrap_or(&project_id);
    ws.move_terminal_to_tab_group(
        focus_manager,
        &project_id,
        &terminal_id,
        target_pid,
        &target_path,
        position,
        cx,
    );
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
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let drop_zone = match zone.as_str() {
        "top" => DropZone::Top,
        "bottom" => DropZone::Bottom,
        "left" => DropZone::Left,
        "right" => DropZone::Right,
        "center" => DropZone::Center,
        _ => return ActionResult::Err(format!("invalid drop zone: {}", zone)),
    };
    ws.move_pane(
        focus_manager,
        &project_id,
        &terminal_id,
        &target_project_id,
        &target_terminal_id,
        drop_zone,
        cx,
    );
    ActionResult::Ok(None)
}
