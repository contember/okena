//! Terminal action handlers — create / split / close / focus and direct PTY I/O.

// Handlers take the workspace, focus manager, terminals registry and cx as
// distinct dependencies; bundling them into a context struct would obscure
// more than it clarifies here.
#![allow(clippy::too_many_arguments)]

use super::{ActionResult, ensure_terminal, find_terminal_path, spawn_uninitialized_terminals};
use crate::workspace::focus::FocusManager;
use crate::workspace::persistence::AppSettings;
use crate::workspace::state::Workspace;
use okena_core::keys::SpecialKey;
use okena_core::types::SplitDirection;
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::TerminalBackend;
use okena_terminal::shell_config::ShellType;
use okena_terminal::terminal::Terminal;
use okena_terminal::terminal::TerminalSize;
use okena_workspace::context::WorkspaceCx;

fn with_ensured_terminal(
    ws: &Workspace,
    terminal_id: &str,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    f: impl FnOnce(&Terminal) -> ActionResult,
) -> ActionResult {
    match ensure_terminal(terminal_id, terminals, backend, ws, settings) {
        Some(term) => f(&term),
        None => ActionResult::Err(format!("terminal not found: {}", terminal_id)),
    }
}

pub(super) fn interactive_leaf(
    ws: &Workspace,
    project_id: &str,
    cwd: Option<&str>,
    start_page: bool,
) -> Result<okena_workspace::state::LayoutNode, ActionResult> {
    use okena_workspace::state::LayoutNode;
    let project = ws
        .project(project_id)
        .ok_or_else(|| ActionResult::Err(format!("project not found: {project_id}")))?;
    Ok(if start_page {
        LayoutNode::new_start_page(okena_core::api::StartPage {
            id: uuid::Uuid::new_v4().to_string(),
            cwd: cwd.unwrap_or(&project.path).to_owned(),
        })
    } else {
        LayoutNode::new_terminal()
    })
}

/// Turn a Start Page (or an empty project's screen when `page_id` is `None`)
/// into one terminal. A page that is already gone is a no-op, so a repeated
/// click does not open a second terminal.
pub(super) fn launch_start_page(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    page_id: Option<String>,
    shell: ShellType,
    command: Option<String>,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let Some(project) = ws.project(&project_id) else {
        return ActionResult::Err(format!("project not found: {project_id}"));
    };
    let (path, cwd) = match (project.layout.as_ref(), page_id.as_deref()) {
        (None, None) => (Vec::new(), project.path.clone()),
        (Some(layout), Some(id)) => {
            let Some(path) = layout.find_terminal_path(id) else {
                return ActionResult::Ok(None);
            };
            let Some(page) = layout.get_at_path(&path).and_then(|node| node.start_page()) else {
                return ActionResult::Ok(None);
            };
            let cwd = page.cwd.clone();
            (path, cwd)
        }
        // The first click consumed the page, or it was closed.
        _ => return ActionResult::Ok(None),
    };

    // A page deferred from a cloned layout keeps that pane's shell for Start Terminal.
    let shell = if command.is_none() && shell == ShellType::Default {
        ws.get_terminal_shell(&project_id, &path)
            .unwrap_or_default()
    } else {
        shell
    };
    let selection = super::StartPageSelection {
        path: path.clone(),
        shell,
        command,
    };
    let result = super::spawn_terminals(
        ws,
        &project_id,
        backend,
        terminals,
        settings,
        Some(cwd),
        Some(selection),
        cx,
    );
    if matches!(result, ActionResult::Ok(Some(_))) {
        ws.set_focused_terminal(focus_manager, project_id, path, cx);
    }
    result
}

pub(super) fn create(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    start_page: bool,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    // Open in the focused terminal's cwd (when one is focused in this project),
    // else the project path.
    let inherit_cwd = focus_manager
        .focused_terminal_state()
        .filter(|f| f.project_id == project_id)
        .and_then(|f| super::inherited_cwd(ws, terminals, &project_id, &f.layout_path));
    let leaf = match interactive_leaf(ws, &project_id, inherit_cwd.as_deref(), start_page) {
        Ok(leaf) => leaf,
        Err(error) => return error,
    };
    ws.add_terminal_with_leaf(focus_manager, &project_id, leaf, cx);
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

pub(super) fn split(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    path: Vec<usize>,
    direction: SplitDirection,
    start_page: bool,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    // Inherit the split source terminal's live cwd (captured before the layout
    // mutation invalidates `path`) so the new pane opens in the same directory.
    let inherit_cwd = super::inherited_cwd(ws, terminals, &project_id, &path);
    let leaf = match interactive_leaf(ws, &project_id, inherit_cwd.as_deref(), start_page) {
        Ok(leaf) => leaf,
        Err(error) => return error,
    };
    ws.split_terminal_with_leaf(focus_manager, &project_id, &path, direction, leaf, cx);
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

/// Switch a terminal's shell: kill the old PTY, reset the layout node to
/// uninitialized with the requested shell, then respawn it. Reuses
/// `spawn_uninitialized_terminals` so the new PTY goes through the same
/// shell-default resolution + shell-wrapper/on_create hook application as any
/// freshly created terminal — keeping daemon shell-switch behavior identical to
/// the old in-process GUI path.
pub(super) fn switch_shell(
    ws: &mut Workspace,
    project_id: String,
    terminal_id: String,
    shell: ShellType,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let path = match find_terminal_path(ws, &project_id, &terminal_id) {
        Some(p) => p,
        None => return ActionResult::Err(format!("terminal not found: {}", terminal_id)),
    };
    // No-op if the shell is unchanged (mirrors the old GUI guard).
    if ws.get_terminal_shell(&project_id, &path).as_ref() == Some(&shell) {
        return ActionResult::Ok(None);
    }
    if terminals.lock().contains_key(&terminal_id) {
        ws.remember_closing_terminal_owner(&project_id, &terminal_id);
    }
    backend.kill(&terminal_id);
    terminals.lock().remove(&terminal_id);
    // The pane keeps its slot but the process behind it is gone, so the
    // captured agent session no longer describes what runs here.
    ws.forget_agent_session(&terminal_id, cx);
    ws.set_terminal_shell(&project_id, &path, shell, cx);
    ws.clear_terminal_id(&project_id, &path, cx);
    // Shell-switch respawns the pane in place; keep the project path (the old
    // terminal's cwd is gone with its PTY).
    spawn_uninitialized_terminals(ws, &project_id, backend, terminals, settings, None, cx)
}

pub(super) fn close(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    terminal_id: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let path = find_terminal_path(ws, &project_id, &terminal_id);
    match path {
        Some(path) => {
            if ws
                .project(&project_id)
                .and_then(|p| p.layout.as_ref())
                .and_then(|l| l.get_at_path(&path))
                .and_then(|n| n.start_page())
                .is_some()
            {
                ws.close_terminal_and_focus_sibling(focus_manager, &project_id, &path, cx);
                return ActionResult::Ok(None);
            }
            if terminals.lock().contains_key(&terminal_id) {
                ws.remember_closing_terminal_owner(&project_id, &terminal_id);
            }
            backend.kill(&terminal_id);
            terminals.lock().remove(&terminal_id);
            ws.forget_agent_session(&terminal_id, cx);
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
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let mut last_err = None;
    for terminal_id in terminal_ids {
        if let ActionResult::Err(error) = close(
            ws,
            focus_manager,
            project_id.clone(),
            terminal_id,
            backend,
            terminals,
            cx,
        ) {
            last_err = Some(error);
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
    cx: &mut impl WorkspaceCx,
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
    settings: &AppSettings,
) -> ActionResult {
    with_ensured_terminal(ws, &terminal_id, backend, terminals, settings, |term| {
        term.send_input(&text);
        ActionResult::Ok(None)
    })
}

pub(super) fn send_bytes(
    ws: &mut Workspace,
    terminal_id: String,
    data: Vec<u8>,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
) -> ActionResult {
    with_ensured_terminal(ws, &terminal_id, backend, terminals, settings, |term| {
        term.send_bytes(&data);
        ActionResult::Ok(None)
    })
}

pub(super) fn run_command(
    ws: &mut Workspace,
    terminal_id: String,
    command: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
) -> ActionResult {
    with_ensured_terminal(ws, &terminal_id, backend, terminals, settings, |term| {
        term.send_input(&format!("{}\r", command));
        ActionResult::Ok(None)
    })
}

pub(super) fn send_special_key(
    ws: &mut Workspace,
    terminal_id: String,
    key: SpecialKey,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
) -> ActionResult {
    with_ensured_terminal(ws, &terminal_id, backend, terminals, settings, |term| {
        term.send_bytes(&key.to_bytes());
        ActionResult::Ok(None)
    })
}

pub(super) fn resize(
    ws: &mut Workspace,
    terminal_id: String,
    cols: u16,
    rows: u16,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
) -> ActionResult {
    with_ensured_terminal(ws, &terminal_id, backend, terminals, settings, |term| {
        let size = TerminalSize {
            cols,
            rows,
            cell_width: 8.0,
            cell_height: 16.0,
        };
        term.resize(size);
        ActionResult::Ok(None)
    })
}

pub(super) fn update_split_sizes(
    ws: &mut Workspace,
    project_id: String,
    path: Vec<usize>,
    sizes: Vec<f32>,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    ws.update_split_sizes(&project_id, &path, sizes, cx);
    ActionResult::Ok(None)
}

pub(super) fn toggle_minimized(
    ws: &mut Workspace,
    project_id: String,
    terminal_id: String,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    ws.toggle_terminal_minimized_by_id(&project_id, &terminal_id, cx);
    ActionResult::Ok(None)
}

pub(super) fn set_fullscreen(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    terminal_id: Option<String>,
    cx: &mut impl WorkspaceCx,
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
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    ws.rename_terminal(&project_id, &terminal_id, name, cx);
    ActionResult::Ok(None)
}

pub(super) fn read_content(
    ws: &mut Workspace,
    terminal_id: String,
    scrollback: Option<std::num::NonZeroU32>,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
) -> ActionResult {
    with_ensured_terminal(ws, &terminal_id, backend, terminals, settings, |term| {
        let content = match scrollback {
            Some(rows) => term.tail_text(usize::try_from(rows.get()).unwrap_or(usize::MAX)),
            None => term.visible_text(),
        };
        ActionResult::Ok(Some(serde_json::json!({"content": content})))
    })
}

pub(super) fn export_buffer(
    terminal_id: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
) -> ActionResult {
    if let Some(path) = backend.capture_buffer(&terminal_id) {
        // capture_buffer wrote a daemon-side temp file; read it back and
        // drop it — the real export is the client's own copy.
        let bytes = std::fs::read(&path).unwrap_or_default();
        let _ = std::fs::remove_file(&path);
        let content = String::from_utf8_lossy(&bytes).to_string();
        return ActionResult::Ok(Some(serde_json::json!({ "content": content })));
    }
    // No tmux session to capture: export the terminal's own history, archived
    // rows included, straight from memory. A pane that is not running has none.
    let terminal = terminals.lock().get(&terminal_id).cloned();
    match terminal {
        Some(terminal) => ActionResult::Ok(Some(
            serde_json::json!({ "content": terminal.history_text() }),
        )),
        None => ActionResult::Err(format!("terminal not running: {terminal_id}")),
    }
}
