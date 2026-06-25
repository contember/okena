//! Session / whole-workspace action handlers (load / save / import / export).
//!
//! The daemon owns session files (under the profile's `sessions/` dir) and the
//! authoritative workspace (local, non-prefixed ids). The thin GUI client must
//! NOT save/load sessions from its read-only mirror — its ids are
//! `remote:<conn>:…` prefixed, which would round-trip into garbage. So these run
//! daemon-side: save/export read the daemon's real data; load/import replace the
//! daemon's state and respawn its terminals (the loaded data already had its
//! stale terminal ids cleared by `validate_workspace_data`).

// Handlers take the workspace, focus manager, terminals registry and cx as
// distinct dependencies; bundling them into a context struct would obscure
// more than it clarifies here.
#![allow(clippy::too_many_arguments)]

use super::{ActionResult, spawn_uninitialized_terminals};
use crate::workspace::focus::FocusManager;
use crate::workspace::persistence::{
    export_workspace, import_workspace, load_session, save_session,
};
use crate::workspace::persistence::AppSettings;
use crate::workspace::state::{Workspace, WorkspaceData};
use okena_terminal::backend::TerminalBackend;
use okena_terminal::TerminalsRegistry;
use okena_workspace::context::WorkspaceCx;

/// Kill every live PTY, swap the workspace to `data`, then respawn terminals for
/// every project in the new workspace. Shared by `load_session` + `import`.
fn replace_workspace_with(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    data: WorkspaceData,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    // Kill every live PTY (collect ids first so we don't hold the registry lock
    // across backend.kill).
    let ids: Vec<String> = terminals.lock().keys().cloned().collect();
    for id in &ids {
        backend.kill(id);
    }
    terminals.lock().clear();

    ws.replace_data(focus_manager, data, cx);

    // The loaded data had its stale terminal ids cleared, so every layout node is
    // uninitialized — respawn a PTY for each project.
    let project_ids: Vec<String> = ws.projects().iter().map(|p| p.id.clone()).collect();
    for pid in &project_ids {
        if let ActionResult::Err(e) =
            spawn_uninitialized_terminals(ws, pid, backend, terminals, settings, cx)
        {
            return ActionResult::Err(e);
        }
    }
    ActionResult::Ok(None)
}

pub(super) fn load_session_action(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    name: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let data = match load_session(&name, settings.session_backend) {
        Ok(d) => d,
        Err(e) => return ActionResult::Err(format!("failed to load session '{name}': {e}")),
    };
    replace_workspace_with(ws, focus_manager, data, backend, terminals, settings, cx)
}

pub(super) fn save_session_action(ws: &Workspace, name: String) -> ActionResult {
    match save_session(&name, &ws.data().without_remote_projects()) {
        Ok(()) => ActionResult::Ok(None),
        Err(e) => ActionResult::Err(format!("failed to save session '{name}': {e}")),
    }
}

pub(super) fn import_workspace_action(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    path: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let data = match import_workspace(std::path::Path::new(&path)) {
        Ok(d) => d,
        Err(e) => return ActionResult::Err(format!("failed to import '{path}': {e}")),
    };
    replace_workspace_with(ws, focus_manager, data, backend, terminals, settings, cx)
}

pub(super) fn export_workspace_action(ws: &Workspace, path: String) -> ActionResult {
    match export_workspace(&ws.data().without_remote_projects(), std::path::Path::new(&path)) {
        Ok(()) => ActionResult::Ok(None),
        Err(e) => ActionResult::Err(format!("failed to export to '{path}': {e}")),
    }
}
