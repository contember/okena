//! Session / whole-workspace action handlers (load / save / import / export).
//!
//! The daemon owns session files (under the profile's `sessions/` dir) and the
//! authoritative workspace (local, non-prefixed ids). The thin GUI client must
//! NOT save/load sessions from its read-only mirror — its ids are
//! `remote:<conn>:…` prefixed, which would round-trip into garbage. So these run
//! daemon-side: save/export read the daemon's real data; load/import replace the
//! daemon's state, discard stale hook PTYs, respawn layout terminals, and run
//! the same project-open lifecycle as daemon boot.

// Handlers take the workspace, focus manager, terminals registry and cx as
// distinct dependencies; bundling them into a context struct would obscure
// more than it clarifies here.
#![allow(clippy::too_many_arguments)]

use super::{
    ActionResult, PreparedTerminalLaunch, PublishedTerminalOwners,
    cleanup_stale_prepared_terminal_launches, effective_terminal_launch, ensure_terminal,
    materialize_prepared_terminal_launches, publish_prepared_terminal_launches,
    spawn_uninitialized_terminals,
};
use crate::workspace::focus::FocusManager;
use crate::workspace::persistence::AppSettings;
use crate::workspace::persistence::{
    LoadedWorkspace, delete_session, export_workspace, import_workspace, list_sessions,
    load_session_with_cleanup, load_session_with_cleanup_for_shell, rename_session, save_session,
    session_exists,
};
use crate::workspace::state::{Workspace, WorkspaceData};
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::{TerminalBackend, TerminalLaunchPlan, TerminalSessionTeardown};
use okena_workspace::context::WorkspaceCx;
use okena_workspace::state::{HookTerminalEntry, HookTerminalStatus, LayoutNode};
use std::collections::HashSet;

fn ordinary_terminal_ids(data: &WorkspaceData) -> HashSet<String> {
    data.projects
        .iter()
        .flat_map(|project| {
            project
                .layout
                .as_ref()
                .map_or_else(Vec::new, |layout| layout.collect_terminal_ids())
        })
        .collect()
}

fn project_owned_terminal_ids(data: &WorkspaceData) -> HashSet<String> {
    data.projects
        .iter()
        .flat_map(|project| {
            let mut ids = project
                .layout
                .as_ref()
                .map_or_else(Vec::new, |layout| layout.collect_terminal_ids());
            ids.extend(project.service_terminals.values().cloned());
            ids.extend(project.hook_terminals.keys().cloned());
            ids
        })
        .collect()
}

fn workspace_replacement_conflict(ws: &Workspace) -> Option<String> {
    if let Some(project) = ws
        .projects()
        .iter()
        .find(|project| ws.is_creating_project(&project.id))
    {
        return Some(format!(
            "cannot replace workspace while worktree '{}' is being created",
            project.name
        ));
    }
    ws.projects()
        .iter()
        .find(|project| ws.is_project_closing(&project.id))
        .map(|project| {
            format!(
                "cannot replace workspace while worktree '{}' is closing",
                project.name
            )
        })
}

/// Reject workspace replacement while an in-flight worktree owns its state.
pub fn ensure_workspace_replacement_allowed(ws: &Workspace) -> Result<(), String> {
    match workspace_replacement_conflict(ws) {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Read and validate a named session without mutating the live workspace.
pub fn load_session_data(
    name: &str,
    backend: okena_terminal::session_backend::SessionBackend,
) -> Result<LoadedWorkspace, String> {
    load_session_with_cleanup(name, backend)
        .map_err(|error| format!("failed to load session '{name}': {error}"))
}

/// Read and validate a named session using the daemon's transient global shell.
pub fn load_session_data_for_shell(
    name: &str,
    backend: okena_terminal::session_backend::SessionBackend,
    global_default_shell: &okena_terminal::shell_config::ShellType,
) -> Result<LoadedWorkspace, String> {
    load_session_with_cleanup_for_shell(name, backend, global_default_shell)
        .map_err(|error| format!("failed to load session '{name}': {error}"))
}

/// Read and validate an exported workspace without mutating the live workspace.
pub fn import_workspace_data(path: &str) -> Result<WorkspaceData, String> {
    import_workspace(std::path::Path::new(path))
        .map_err(|error| format!("failed to import '{path}': {error}"))
}

/// Disk-loaded workspace data with every PTY launch resolved and reserved.
pub struct PreparedWorkspaceReplacement {
    data: WorkspaceData,
    ordinary: Vec<PreparedTerminalLaunch>,
    hooks: Vec<okena_hooks::PreparedHookTerminal>,
}

/// Immutable replacement work published behind the workspace transition gate.
#[derive(Clone)]
pub struct WorkspaceReplacementPlan {
    epoch: u64,
    ordinary: Vec<PreparedTerminalLaunch>,
    ordinary_owners: PublishedTerminalOwners,
    hooks: Vec<okena_hooks::PreparedHookTerminal>,
    kill_ids: Vec<String>,
    stale_sessions: Vec<TerminalSessionTeardown>,
    terminals: TerminalsRegistry,
    hook_runner: Option<okena_hooks::HookRunner>,
    hook_monitor: Option<okena_hooks::HookMonitor>,
}

pub struct WorkspaceReplacementCompletion {
    plan: WorkspaceReplacementPlan,
    failed_ordinary: Vec<String>,
    failed_hooks: Vec<String>,
    errors: Vec<String>,
}

impl WorkspaceReplacementCompletion {
    pub fn epoch(&self) -> u64 {
        self.plan.epoch
    }
}

fn prepare_layout_terminals(
    node: &mut LayoutNode,
    project_id: &str,
    cwd: &str,
    project_default_shell: Option<&okena_terminal::shell_config::ShellType>,
    settings: &AppSettings,
    shell_wrapper: Option<&str>,
    on_create: Option<&str>,
    env: &std::collections::HashMap<String, String>,
    path: &mut Vec<usize>,
    launches: &mut Vec<PreparedTerminalLaunch>,
) {
    match node {
        LayoutNode::Terminal {
            terminal_id,
            shell_type,
            ..
        } => {
            let persisted = terminal_id.is_some();
            let id = terminal_id
                .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
                .clone();
            let launch_plan = if persisted {
                TerminalLaunchPlan::for_shell(
                    shell_type
                        .clone()
                        .resolve_default(project_default_shell, &settings.default_shell),
                )
            } else {
                effective_terminal_launch(
                    shell_type.clone(),
                    project_default_shell,
                    &settings.default_shell,
                    shell_wrapper,
                    on_create,
                    env,
                )
            };
            launches.push(PreparedTerminalLaunch::new(
                project_id.to_string(),
                path.clone(),
                id,
                cwd.to_string(),
                launch_plan,
            ));
        }
        LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
            for (index, child) in children.iter_mut().enumerate() {
                path.push(index);
                prepare_layout_terminals(
                    child,
                    project_id,
                    cwd,
                    project_default_shell,
                    settings,
                    shell_wrapper,
                    on_create,
                    env,
                    path,
                    launches,
                );
                path.pop();
            }
        }
    }
}

/// Resolve terminal and project-open hook launches before taking the live lock.
pub fn prepare_workspace_replacement(
    mut data: WorkspaceData,
    settings: &AppSettings,
) -> PreparedWorkspaceReplacement {
    for project in &mut data.projects {
        let stale: HashSet<String> = project.hook_terminals.keys().cloned().collect();
        if !stale.is_empty() {
            let stale_refs: HashSet<&str> = stale.iter().map(String::as_str).collect();
            LayoutNode::remove_terminal_ids(&mut project.layout, &stale_refs);
            project
                .terminal_names
                .retain(|terminal_id, _| !stale.contains(terminal_id));
            project
                .hidden_terminals
                .retain(|terminal_id, _| !stale.contains(terminal_id));
            project.hook_terminals.clear();
        }
    }

    let lookup = Workspace::new(data.clone());
    let mut ordinary = Vec::new();
    let mut prepared_hooks = Vec::new();
    for project in &mut data.projects {
        let parent_hooks = project
            .worktree_info
            .as_ref()
            .and_then(|worktree| lookup.project(&worktree.parent_project_id))
            .map(|parent| parent.hooks.clone());
        let shell_wrapper = crate::workspace::hooks::resolve_shell_wrapper(
            &project.hooks,
            parent_hooks.as_ref(),
            &settings.hooks,
        );
        let on_create = crate::workspace::hooks::resolve_terminal_on_create_simple(
            &project.hooks,
            parent_hooks.as_ref(),
            &settings.hooks,
        );
        let folder = lookup.folder_for_project_or_parent(&project.id);
        let env = crate::workspace::hooks::terminal_hook_env(
            &project.id,
            &project.name,
            &project.path,
            project.worktree_info.is_some(),
            folder.map(|folder| folder.id.as_str()),
            folder.map(|folder| folder.name.as_str()),
        );
        if let Some(layout) = &mut project.layout {
            prepare_layout_terminals(
                layout,
                &project.id,
                &project.path,
                project.default_shell.as_ref(),
                settings,
                shell_wrapper.as_deref(),
                on_create.as_deref(),
                &env,
                &mut Vec::new(),
                &mut ordinary,
            );
        }
        if let Some(prepared) = okena_hooks::prepare_project_open_hook(
            uuid::Uuid::new_v4().to_string(),
            &project.hooks,
            &project.id,
            &project.name,
            &project.path,
            folder.map(|folder| folder.id.as_str()),
            folder.map(|folder| folder.name.as_str()),
            &settings.hooks,
        ) {
            prepared_hooks.push(prepared);
        }
    }
    ordinary.sort_by(|a, b| a.terminal_id.cmp(&b.terminal_id));
    PreparedWorkspaceReplacement {
        data,
        ordinary,
        hooks: prepared_hooks,
    }
}

/// Atomically publish all incoming logical owners without notifying observers.
#[allow(clippy::too_many_arguments)]
pub fn begin_workspace_replacement(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    prepared: PreparedWorkspaceReplacement,
    stale_terminal_ids: Vec<TerminalSessionTeardown>,
    terminals: &TerminalsRegistry,
    backend: &dyn TerminalBackend,
    cx: &mut impl WorkspaceCx,
) -> Result<WorkspaceReplacementPlan, String> {
    ensure_workspace_replacement_allowed(ws)?;
    if ws.terminal_backend_migration_epoch().is_some() {
        return Err("a terminal ownership transition is already in progress".to_string());
    }

    let incoming_ids: HashSet<String> = prepared
        .ordinary
        .iter()
        .map(|terminal| terminal.terminal_id.clone())
        .chain(
            prepared
                .data
                .projects
                .iter()
                .flat_map(|project| project.service_terminals.values().cloned()),
        )
        .collect();
    let outgoing_hook_ids: Vec<String> = ws
        .projects()
        .iter()
        .flat_map(|project| project.hook_terminals.keys().cloned())
        .collect();
    let mut kill_ids: HashSet<String> = terminals.lock().keys().cloned().collect();
    kill_ids.extend(project_owned_terminal_ids(ws.data()));
    kill_ids.retain(|id| !incoming_ids.contains(id));
    kill_ids.retain(|id| {
        !stale_terminal_ids
            .iter()
            .any(|session| session.terminal_id == *id)
    });
    let mut kill_ids: Vec<String> = kill_ids.into_iter().collect();
    kill_ids.sort();

    let hook_runner = cx.hook_runner();
    let hook_monitor = cx.hook_monitor();
    if let Some(monitor) = &hook_monitor {
        for terminal_id in outgoing_hook_ids {
            monitor.cancel_by_terminal_id(&terminal_id);
        }
    }

    terminals.lock().clear();
    let ordinary_owners =
        publish_prepared_terminal_launches(&prepared.ordinary, terminals, backend)?;
    let epoch = match ws.begin_workspace_replacement_transition(focus_manager, prepared.data) {
        Ok(epoch) => epoch,
        Err(error) => {
            ordinary_owners.release_all();
            return Err(error);
        }
    };
    for prepared_hook in &prepared.hooks {
        let result = prepared_hook.result();
        let Some(project) = ws
            .data
            .projects
            .iter_mut()
            .find(|project| project.id == result.project_id)
        else {
            continue;
        };
        project.hook_terminals.insert(
            result.terminal_id.clone(),
            HookTerminalEntry {
                label: result.label.clone(),
                status: HookTerminalStatus::Running,
                hook_type: result.hook_type.to_string(),
                command: result.command.clone(),
                cwd: result.cwd.clone(),
            },
        );
        project
            .terminal_names
            .insert(result.terminal_id.clone(), result.label.clone());
        prepared_hook.publish_monitor(hook_monitor.as_ref());
    }

    if let Some(runner) = &hook_runner {
        for prepared_hook in &prepared.hooks {
            runner.publish_prepared_terminal(prepared_hook);
        }
    }

    Ok(WorkspaceReplacementPlan {
        epoch,
        ordinary: prepared.ordinary,
        ordinary_owners,
        hooks: prepared.hooks,
        kill_ids,
        stale_sessions: stale_terminal_ids,
        terminals: terminals.clone(),
        hook_runner,
        hook_monitor,
    })
}

/// Run all potentially blocking PTY operations without the workspace lock.
pub fn materialize_workspace_replacement(
    plan: WorkspaceReplacementPlan,
    backend: &dyn TerminalBackend,
) -> WorkspaceReplacementCompletion {
    for terminal_id in &plan.kill_ids {
        backend.kill(terminal_id);
    }
    for session in &plan.stale_sessions {
        backend.kill_session(session);
    }
    backend.flush_teardown();

    let ordinary_outcome = materialize_prepared_terminal_launches(&plan.ordinary, backend);
    let failed_ordinary = ordinary_outcome.failed_terminal_ids;
    let mut failed_hooks = Vec::new();
    let mut errors = ordinary_outcome.errors;
    for prepared_hook in &plan.hooks {
        let result = prepared_hook.result();
        let launch = plan
            .hook_runner
            .as_ref()
            .ok_or_else(|| "hook runner unavailable".to_string())
            .and_then(|runner| runner.launch_prepared_terminal(prepared_hook));
        if let Err(error) = launch {
            prepared_hook.finish_failed_monitor(plan.hook_monitor.as_ref());
            failed_hooks.push(result.terminal_id.clone());
            errors.push(format!("{}: {error}", result.project_id));
        }
    }
    WorkspaceReplacementCompletion {
        plan,
        failed_ordinary,
        failed_hooks,
        errors,
    }
}

/// Convert an aborted blocking task into a deterministic all-launches failure.
pub fn fail_workspace_replacement(
    plan: WorkspaceReplacementPlan,
    error: String,
) -> WorkspaceReplacementCompletion {
    WorkspaceReplacementCompletion {
        failed_ordinary: plan
            .ordinary
            .iter()
            .map(|terminal| terminal.terminal_id.clone())
            .collect(),
        failed_hooks: plan
            .hooks
            .iter()
            .map(|hook| hook.result().terminal_id.clone())
            .collect(),
        plan,
        errors: vec![error],
    }
}

/// Tear down launches whose completion lost the workspace epoch.
pub fn cleanup_stale_workspace_replacement(
    completion: WorkspaceReplacementCompletion,
    backend: &dyn TerminalBackend,
) {
    cleanup_stale_prepared_terminal_launches(&completion.plan.ordinary_owners, backend);
    for hook in &completion.plan.hooks {
        backend.kill(&hook.result().terminal_id);
        hook.finish_failed_monitor(completion.plan.hook_monitor.as_ref());
        if let Some(runner) = &completion.plan.hook_runner {
            runner.remove_prepared_terminal(hook);
        }
    }
    backend.flush_teardown();
}

fn clear_layout_terminal_id(node: &mut LayoutNode, target: &str) -> bool {
    match node {
        LayoutNode::Terminal { terminal_id, .. } if terminal_id.as_deref() == Some(target) => {
            *terminal_id = None;
            true
        }
        LayoutNode::Terminal { .. } => false,
        LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => children
            .iter_mut()
            .any(|child| clear_layout_terminal_id(child, target)),
    }
}

/// Commit materialization results if the replacement still owns its epoch.
pub fn finish_workspace_replacement(
    ws: &mut Workspace,
    completion: WorkspaceReplacementCompletion,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let plan = completion.plan;
    if ws.terminal_backend_migration_epoch() != Some(plan.epoch)
        || ws.data_replacement_epoch() != plan.epoch
    {
        plan.ordinary_owners.release_all();
        for hook in &plan.hooks {
            if let Some(runner) = &plan.hook_runner {
                runner.remove_prepared_terminal(hook);
            }
        }
        return ActionResult::Err("stale workspace replacement completion".to_string());
    }

    for terminal_id in &completion.failed_ordinary {
        for project in &mut ws.data.projects {
            if project
                .layout
                .as_mut()
                .is_some_and(|layout| clear_layout_terminal_id(layout, terminal_id))
            {
                break;
            }
        }
        plan.ordinary_owners
            .release(std::slice::from_ref(terminal_id));
    }
    for terminal_id in &completion.failed_hooks {
        for project in &mut ws.data.projects {
            if project.hook_terminals.remove(terminal_id).is_some() {
                project.terminal_names.remove(terminal_id);
                project.hidden_terminals.remove(terminal_id);
                break;
            }
        }
        plan.terminals.lock().remove(terminal_id);
    }
    ws.finish_workspace_replacement_transition(plan.epoch, cx);
    if completion.errors.is_empty() {
        ActionResult::Ok(None)
    } else {
        ActionResult::Err(format!(
            "workspace loaded with terminal materialization errors: {}",
            completion.errors.join("; ")
        ))
    }
}

/// Kill outgoing-only PTYs, swap the workspace, then restore incoming terminals.
fn replace_workspace_with(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    data: WorkspaceData,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    stale_terminal_ids: &[TerminalSessionTeardown],
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    if let Some(error) = workspace_replacement_conflict(ws) {
        return ActionResult::Err(error);
    }

    let incoming_ordinary_ids = ordinary_terminal_ids(&data);
    let mut incoming_persistent_ids = incoming_ordinary_ids.clone();
    incoming_persistent_ids.extend(
        data.projects
            .iter()
            .flat_map(|project| project.service_terminals.values().cloned()),
    );

    let outgoing_hook_ids: HashSet<String> = ws
        .projects()
        .iter()
        .flat_map(|project| project.hook_terminals.keys().cloned())
        .collect();
    let mut ids_to_kill: HashSet<String> = terminals.lock().keys().cloned().collect();
    ids_to_kill.extend(project_owned_terminal_ids(ws.data()));
    // Incoming hooks are never reconnected; load clears them and fires fresh
    // project-open lifecycle hooks instead.
    ids_to_kill.extend(
        data.projects
            .iter()
            .flat_map(|project| project.hook_terminals.keys().cloned()),
    );
    ids_to_kill.retain(|id| !incoming_persistent_ids.contains(id));
    ids_to_kill.retain(|id| {
        !stale_terminal_ids
            .iter()
            .any(|session| session.terminal_id == *id)
    });
    let mut ids: Vec<String> = ids_to_kill.into_iter().collect();
    ids.sort();
    if let Some(monitor) = cx.hook_monitor() {
        for id in &outgoing_hook_ids {
            monitor.cancel_by_terminal_id(id);
        }
    }
    for id in &ids {
        backend.kill(id);
    }
    for session in stale_terminal_ids {
        if !incoming_persistent_ids.contains(&session.terminal_id) {
            backend.kill_session(session);
        }
    }
    terminals.lock().clear();

    ws.replace_data(focus_manager, data, cx);

    let project_ids: Vec<String> = ws.projects().iter().map(|p| p.id.clone()).collect();
    for pid in &project_ids {
        ws.clear_stale_hook_terminals(pid, cx);
    }

    let mut materialization_errors = Vec::new();
    let mut incoming_ordinary_ids: Vec<String> = incoming_ordinary_ids.into_iter().collect();
    incoming_ordinary_ids.sort();
    for terminal_id in incoming_ordinary_ids {
        if ensure_terminal(&terminal_id, terminals, backend, ws, settings).is_none() {
            materialization_errors.push(format!(
                "failed to reconnect persisted terminal {terminal_id}"
            ));
        }
    }
    for pid in &project_ids {
        if let ActionResult::Err(e) =
            spawn_uninitialized_terminals(ws, pid, backend, terminals, settings, None, cx)
        {
            materialization_errors.push(format!("{pid}: {e}"));
        }
    }
    for pid in &project_ids {
        ws.fire_project_open_hooks(pid, &settings.hooks, cx);
    }
    if materialization_errors.is_empty() {
        ActionResult::Ok(None)
    } else {
        ActionResult::Err(format!(
            "workspace loaded with terminal materialization errors: {}",
            materialization_errors.join("; ")
        ))
    }
}

/// Atomically apply data loaded by [`load_session_data`].
#[allow(clippy::too_many_arguments)]
pub fn apply_loaded_session(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    loaded: LoadedWorkspace,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    replace_workspace_with(
        ws,
        focus_manager,
        loaded.data,
        backend,
        terminals,
        settings,
        &loaded.stale_terminal_ids,
        cx,
    )
}

/// Atomically apply data loaded by [`import_workspace_data`].
#[allow(clippy::too_many_arguments)]
pub fn apply_imported_workspace(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    data: WorkspaceData,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    replace_workspace_with(
        ws,
        focus_manager,
        data,
        backend,
        terminals,
        settings,
        &[],
        cx,
    )
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
    load_session_action_with_loader(ws, focus_manager, backend, terminals, settings, cx, || {
        load_session_data_for_shell(&name, settings.session_backend, &settings.default_shell)
    })
}

fn load_session_action_with_loader(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
    loader: impl FnOnce() -> Result<LoadedWorkspace, String>,
) -> ActionResult {
    if let Err(error) = ensure_workspace_replacement_allowed(ws) {
        return ActionResult::Err(error);
    }
    let loaded = match loader() {
        Ok(loaded) => loaded,
        Err(error) => return ActionResult::Err(error),
    };
    apply_loaded_session(ws, focus_manager, loaded, backend, terminals, settings, cx)
}

pub(super) fn list_sessions_action() -> ActionResult {
    match list_sessions() {
        Ok(sessions) => ActionResult::Ok(Some(
            serde_json::to_value(sessions).expect("BUG: SessionInfo must serialize"),
        )),
        Err(e) => ActionResult::Err(format!("failed to list sessions: {e}")),
    }
}

pub(super) fn save_session_action(ws: &Workspace, name: String) -> ActionResult {
    if session_exists(&name) {
        return ActionResult::Err(format!("session '{name}' already exists"));
    }
    match save_session(&name, &ws.data().without_remote_projects()) {
        Ok(()) => ActionResult::Ok(None),
        Err(e) => ActionResult::Err(format!("failed to save session '{name}': {e}")),
    }
}

pub(super) fn rename_session_action(old_name: String, new_name: String) -> ActionResult {
    match rename_session(&old_name, &new_name) {
        Ok(()) => ActionResult::Ok(None),
        Err(e) => ActionResult::Err(format!(
            "failed to rename session '{old_name}' to '{new_name}': {e}"
        )),
    }
}

pub(super) fn delete_session_action(name: String) -> ActionResult {
    match delete_session(&name) {
        Ok(()) => ActionResult::Ok(None),
        Err(e) => ActionResult::Err(format!("failed to delete session '{name}': {e}")),
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
    import_workspace_action_with_loader(ws, focus_manager, backend, terminals, settings, cx, || {
        import_workspace_data(&path)
    })
}

fn import_workspace_action_with_loader(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
    loader: impl FnOnce() -> Result<WorkspaceData, String>,
) -> ActionResult {
    if let Err(error) = ensure_workspace_replacement_allowed(ws) {
        return ActionResult::Err(error);
    }
    let data = match loader() {
        Ok(d) => d,
        Err(error) => return ActionResult::Err(error),
    };
    apply_imported_workspace(ws, focus_manager, data, backend, terminals, settings, cx)
}

pub(super) fn export_workspace_action(ws: &Workspace, path: String) -> ActionResult {
    match export_workspace(
        &ws.data().without_remote_projects(),
        std::path::Path::new(&path),
    ) {
        Ok(()) => ActionResult::Ok(None),
        Err(e) => ActionResult::Err(format!("failed to export to '{path}': {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::hook_monitor::HookMonitor;
    use crate::workspace::hooks::HookRunner;
    use crate::workspace::settings::{HooksConfig, ProjectHooks};
    use crate::workspace::state::{
        HookTerminalEntry, HookTerminalStatus, LayoutNode, ProjectData, WindowState,
    };
    use okena_terminal::shell_config::ShellType;
    use okena_terminal::terminal::{Terminal, TerminalSize, TerminalTransport};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    struct StubTransport;

    impl TerminalTransport for StubTransport {
        fn send_input(&self, _terminal_id: &str, _data: &[u8]) {}
        fn resize(&self, _terminal_id: &str, _cols: u16, _rows: u16) {}
        fn uses_mouse_backend(&self) -> bool {
            false
        }
    }

    struct RecordingBackend {
        next_id: AtomicUsize,
        killed: Mutex<Vec<String>>,
        reconnected: Mutex<Vec<(String, String)>>,
        fail_next_cwds: Mutex<HashSet<String>>,
    }

    struct OwnershipCheckingBackend {
        workspace: Arc<Mutex<Workspace>>,
        launches_with_owner: AtomicUsize,
    }

    impl TerminalBackend for OwnershipCheckingBackend {
        fn transport(&self) -> Arc<dyn TerminalTransport> {
            Arc::new(StubTransport)
        }

        fn create_terminal(
            &self,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            anyhow::bail!("reserved launches must reconnect by id")
        }

        fn reconnect_terminal(
            &self,
            terminal_id: &str,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            let workspace = self.workspace.lock().unwrap();
            if workspace.find_project_for_terminal(terminal_id).is_some()
                || workspace.is_hook_terminal(terminal_id).is_some()
            {
                self.launches_with_owner.fetch_add(1, Ordering::Relaxed);
            }
            Ok(terminal_id.to_string())
        }

        fn kill(&self, _terminal_id: &str) {}
        fn supports_buffer_capture(&self) -> bool {
            false
        }
        fn capture_buffer(&self, _terminal_id: &str) -> Option<std::path::PathBuf> {
            None
        }
        fn is_remote(&self) -> bool {
            false
        }
        fn get_shell_pid(&self, _terminal_id: &str) -> Option<u32> {
            None
        }
        fn get_service_pids(&self, _terminal_id: &str) -> Vec<u32> {
            Vec::new()
        }
    }

    impl TerminalBackend for RecordingBackend {
        fn transport(&self) -> Arc<dyn TerminalTransport> {
            Arc::new(StubTransport)
        }

        fn create_terminal(&self, cwd: &str, _shell: Option<&ShellType>) -> anyhow::Result<String> {
            if self.fail_next_cwds.lock().unwrap().remove(cwd) {
                anyhow::bail!("requested failure for {cwd}");
            }
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            Ok(format!("created-{id}"))
        }

        fn reconnect_terminal(
            &self,
            terminal_id: &str,
            cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            if self.fail_next_cwds.lock().unwrap().remove(cwd) {
                anyhow::bail!("requested failure for {cwd}");
            }
            self.reconnected
                .lock()
                .unwrap()
                .push((terminal_id.to_string(), cwd.to_string()));
            Ok(terminal_id.to_string())
        }

        fn kill(&self, terminal_id: &str) {
            self.killed.lock().unwrap().push(terminal_id.to_string());
        }

        fn supports_buffer_capture(&self) -> bool {
            false
        }

        fn capture_buffer(&self, _terminal_id: &str) -> Option<std::path::PathBuf> {
            None
        }

        fn is_remote(&self) -> bool {
            false
        }

        fn get_shell_pid(&self, _terminal_id: &str) -> Option<u32> {
            None
        }

        fn get_service_pids(&self, _terminal_id: &str) -> Vec<u32> {
            Vec::new()
        }
    }

    struct TestCx {
        runner: HookRunner,
        monitor: HookMonitor,
    }

    impl WorkspaceCx for TestCx {
        fn notify(&mut self) {}
        fn refresh_views(&mut self) {}
        fn hook_runner(&self) -> Option<HookRunner> {
            Some(self.runner.clone())
        }
        fn hook_monitor(&self) -> Option<HookMonitor> {
            Some(self.monitor.clone())
        }
    }

    fn project(id: &str, hook_id: &str, on_open: Option<&str>) -> ProjectData {
        let mut hook_terminals = HashMap::new();
        hook_terminals.insert(
            hook_id.to_string(),
            HookTerminalEntry {
                label: "old hook".to_string(),
                status: HookTerminalStatus::Succeeded,
                hook_type: "on_project_open".to_string(),
                command: "echo old".to_string(),
                cwd: "/tmp".to_string(),
            },
        );
        ProjectData {
            id: id.to_string(),
            name: id.to_string(),
            path: std::env::temp_dir().to_string_lossy().into_owned(),
            layout: None,
            terminal_names: HashMap::from([(hook_id.to_string(), "old hook".to_string())]),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: Default::default(),
            hooks: HooksConfig {
                project: ProjectHooks {
                    on_open: on_open.map(str::to_string),
                    on_close: None,
                },
                ..Default::default()
            },
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            agent_sessions: HashMap::new(),
            default_shell: None,
            hook_terminals,
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
        }
    }

    fn data(project: ProjectData) -> WorkspaceData {
        data_many(vec![project])
    }

    fn data_many(projects: Vec<ProjectData>) -> WorkspaceData {
        let project_order = projects.iter().map(|project| project.id.clone()).collect();
        WorkspaceData {
            version: 1,
            projects,
            project_order,
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            folders: Vec::new(),
            main_window: WindowState::default(),
            extra_windows: Vec::new(),
        }
    }

    #[test]
    fn prepared_replacement_publishes_reserved_ordinary_and_hook_owners_before_launch() {
        let workspace = Arc::new(Mutex::new(Workspace::new(data(project(
            "outgoing",
            "outgoing-hook",
            None,
        )))));
        let backend = Arc::new(OwnershipCheckingBackend {
            workspace: workspace.clone(),
            launches_with_owner: AtomicUsize::new(0),
        });
        let terminals: TerminalsRegistry = Arc::new(Default::default());
        let runner = HookRunner::new(backend.clone(), terminals.clone());
        let monitor = HookMonitor::new();
        monitor.record_start_named(
            "on_project_open",
            "sleep 10",
            "outgoing",
            Some("outgoing-hook".to_string()),
        );
        let mut cx = TestCx {
            runner,
            monitor: monitor.clone(),
        };
        let mut incoming = project("incoming", "stale-hook", Some("echo opened"));
        incoming.layout = Some(LayoutNode::new_terminal());
        let prepared = prepare_workspace_replacement(data(incoming), &AppSettings::default());
        assert_eq!(prepared.ordinary.len(), 1);
        assert_eq!(prepared.hooks.len(), 1);
        let ordinary_id = prepared.ordinary[0].terminal_id.clone();
        let hook_id = prepared.hooks[0].result().terminal_id.clone();
        let mut focus = FocusManager::default();
        let plan = begin_workspace_replacement(
            &mut workspace.lock().unwrap(),
            &mut focus,
            prepared,
            Vec::new(),
            &terminals,
            backend.as_ref(),
            &mut cx,
        )
        .expect("begin replacement");

        {
            let workspace = workspace.lock().unwrap();
            assert!(workspace.terminal_backend_migration_epoch().is_some());
            assert!(workspace.find_project_for_terminal(&ordinary_id).is_some());
            assert!(workspace.is_hook_terminal(&hook_id).is_some());
        }
        assert!(terminals.lock().contains_key(&ordinary_id));
        assert!(terminals.lock().contains_key(&hook_id));
        assert!(
            monitor
                .history()
                .iter()
                .any(|entry| entry.terminal_id.as_deref() == Some(&hook_id))
        );
        assert!(
            monitor
                .history()
                .iter()
                .all(|entry| entry.terminal_id.as_deref() != Some("outgoing-hook"))
        );
        assert!(monitor.drain_pending_toasts().is_empty());

        let completion = materialize_workspace_replacement(plan, backend.as_ref());
        let result =
            finish_workspace_replacement(&mut workspace.lock().unwrap(), completion, &mut cx);
        assert!(matches!(result, ActionResult::Ok(_)));
        assert_eq!(backend.launches_with_owner.load(Ordering::Relaxed), 2);
        assert_eq!(
            workspace.lock().unwrap().terminal_backend_migration_epoch(),
            None
        );
    }

    #[test]
    fn failed_reserved_launch_clears_provisional_owner_and_releases_gate() {
        let failed_cwd = std::env::temp_dir().join("okena-prepared-session-failure");
        let backend = Arc::new(RecordingBackend {
            next_id: AtomicUsize::new(1),
            killed: Mutex::new(Vec::new()),
            reconnected: Mutex::new(Vec::new()),
            fail_next_cwds: Mutex::new(HashSet::from([failed_cwd.to_string_lossy().into_owned()])),
        });
        let terminals: TerminalsRegistry = Arc::new(Default::default());
        let runner = HookRunner::new(backend.clone(), terminals.clone());
        let mut cx = TestCx {
            runner,
            monitor: HookMonitor::new(),
        };
        let mut incoming = project("incoming", "stale-hook", None);
        incoming.path = failed_cwd.to_string_lossy().into_owned();
        incoming.layout = Some(LayoutNode::new_terminal());
        let prepared = prepare_workspace_replacement(data(incoming), &AppSettings::default());
        let reserved_id = prepared.ordinary[0].terminal_id.clone();
        let mut workspace = Workspace::new(WorkspaceData::empty());
        let mut focus = FocusManager::default();
        let plan = begin_workspace_replacement(
            &mut workspace,
            &mut focus,
            prepared,
            Vec::new(),
            &terminals,
            backend.as_ref(),
            &mut cx,
        )
        .expect("begin replacement");

        let completion = materialize_workspace_replacement(plan, backend.as_ref());
        let result = finish_workspace_replacement(&mut workspace, completion, &mut cx);
        assert!(matches!(result, ActionResult::Err(_)));
        assert_eq!(workspace.terminal_backend_migration_epoch(), None);
        assert!(!terminals.lock().contains_key(&reserved_id));
        assert!(workspace.project("incoming").is_some_and(|project| {
            matches!(
                project.layout,
                Some(LayoutNode::Terminal {
                    terminal_id: None,
                    ..
                })
            )
        }));
    }

    #[test]
    fn replacement_kills_stale_hooks_and_runs_project_open_lifecycle() {
        let backend = Arc::new(RecordingBackend {
            next_id: AtomicUsize::new(1),
            killed: Mutex::new(Vec::new()),
            reconnected: Mutex::new(Vec::new()),
            fail_next_cwds: Mutex::new(HashSet::new()),
        });
        let terminals: TerminalsRegistry = Arc::new(Default::default());
        let runner = HookRunner::new(backend.clone(), terminals.clone());
        let monitor = HookMonitor::new();
        let mut cx = TestCx {
            runner,
            monitor: monitor.clone(),
        };
        let mut workspace = Workspace::new(data(project("old", "outgoing-hook", None)));
        let mut focus = FocusManager::default();

        let result = replace_workspace_with(
            &mut workspace,
            &mut focus,
            data(project("new", "incoming-hook", Some("echo opened"))),
            backend.as_ref(),
            &terminals,
            &AppSettings::default(),
            &[],
            &mut cx,
        );

        assert!(matches!(result, ActionResult::Ok(_)));
        let killed = backend.killed.lock().unwrap();
        assert!(killed.contains(&"outgoing-hook".to_string()));
        assert!(killed.contains(&"incoming-hook".to_string()));
        drop(killed);
        let loaded = workspace.project("new").unwrap();
        assert!(!loaded.hook_terminals.contains_key("incoming-hook"));
        assert_eq!(loaded.hook_terminals.len(), 1);
        assert!(loaded.hook_terminals.contains_key("created-1"));
        let history = monitor.history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].hook_type, "on_project_open");
    }

    #[test]
    fn persistent_replacement_preserves_and_reconnects_incoming_terminal_ids() {
        let backend = Arc::new(RecordingBackend {
            next_id: AtomicUsize::new(1),
            killed: Mutex::new(Vec::new()),
            reconnected: Mutex::new(Vec::new()),
            fail_next_cwds: Mutex::new(HashSet::new()),
        });
        let terminals: TerminalsRegistry = Arc::new(Default::default());
        let runner = HookRunner::new(backend.clone(), terminals.clone());
        let monitor = HookMonitor::new();
        let mut cx = TestCx { runner, monitor };
        let terminal_node = |id: &str| LayoutNode::Terminal {
            terminal_id: Some(id.to_string()),
            shell_type: ShellType::Default,
            minimized: false,
            detached: false,
            zoom_level: 1.0,
        };

        let mut outgoing = project("old", "outgoing-hook", None);
        outgoing.layout = Some(LayoutNode::Tabs {
            children: vec![terminal_node("shared"), terminal_node("outgoing-only")],
            active_tab: 0,
        });
        let mut workspace = Workspace::new(data(outgoing));
        let mut focus = FocusManager::default();

        terminals.lock().insert(
            "registry-only".to_string(),
            Arc::new(Terminal::new(
                "registry-only".to_string(),
                TerminalSize::default(),
                backend.transport(),
                "/old".to_string(),
            )),
        );

        let mut incoming = project("new", "incoming-hook", None);
        let incoming_cwd = incoming.path.clone();
        incoming.layout = Some(LayoutNode::Tabs {
            children: vec![terminal_node("shared"), terminal_node("incoming-only")],
            active_tab: 0,
        });
        incoming
            .service_terminals
            .insert("server".to_string(), "incoming-service".to_string());

        let result = replace_workspace_with(
            &mut workspace,
            &mut focus,
            data(incoming),
            backend.as_ref(),
            &terminals,
            &AppSettings::default(),
            &[TerminalSessionTeardown::host("discarded-stale".to_string())],
            &mut cx,
        );

        assert!(matches!(result, ActionResult::Ok(_)));
        let killed = backend.killed.lock().unwrap();
        assert!(killed.contains(&"outgoing-only".to_string()));
        assert!(killed.contains(&"registry-only".to_string()));
        assert!(killed.contains(&"outgoing-hook".to_string()));
        assert!(killed.contains(&"incoming-hook".to_string()));
        assert!(killed.contains(&"discarded-stale".to_string()));
        assert!(!killed.contains(&"shared".to_string()));
        assert!(!killed.contains(&"incoming-only".to_string()));
        assert!(!killed.contains(&"incoming-service".to_string()));
        drop(killed);

        let reconnected = backend.reconnected.lock().unwrap();
        assert_eq!(
            *reconnected,
            vec![
                ("incoming-only".to_string(), incoming_cwd.clone()),
                ("shared".to_string(), incoming_cwd),
            ]
        );
        drop(reconnected);
        let registry = terminals.lock();
        assert_eq!(registry.len(), 2);
        assert!(registry.contains_key("shared"));
        assert!(registry.contains_key("incoming-only"));
        assert!(!registry.contains_key("registry-only"));
        let loaded = workspace.project("new").expect("incoming project loaded");
        assert!(!loaded.hook_terminals.contains_key("incoming-hook"));
    }

    #[test]
    fn replacement_attempts_every_project_and_reports_aggregated_materialization_errors() {
        let failed_cwd = std::env::temp_dir().join("okena-session-failed");
        let successful_cwd = std::env::temp_dir().join("okena-session-successful");
        let backend = Arc::new(RecordingBackend {
            next_id: AtomicUsize::new(1),
            killed: Mutex::new(Vec::new()),
            reconnected: Mutex::new(Vec::new()),
            fail_next_cwds: Mutex::new(HashSet::from([failed_cwd.to_string_lossy().into_owned()])),
        });
        let terminals: TerminalsRegistry = Arc::new(Default::default());
        let runner = HookRunner::new(backend.clone(), terminals.clone());
        let monitor = HookMonitor::new();
        let mut cx = TestCx {
            runner,
            monitor: monitor.clone(),
        };
        let mut workspace = Workspace::new(WorkspaceData::empty());
        let mut focus = FocusManager::default();

        let mut failed = project("failed", "failed-old-hook", Some("echo failed-open"));
        failed.path = failed_cwd.to_string_lossy().into_owned();
        failed.layout = Some(LayoutNode::Terminal {
            terminal_id: None,
            shell_type: ShellType::Default,
            minimized: false,
            detached: false,
            zoom_level: 1.0,
        });
        let mut successful = project(
            "successful",
            "successful-old-hook",
            Some("echo successful-open"),
        );
        successful.path = successful_cwd.to_string_lossy().into_owned();
        successful.layout = Some(LayoutNode::Terminal {
            terminal_id: None,
            shell_type: ShellType::Default,
            minimized: false,
            detached: false,
            zoom_level: 1.0,
        });

        let result = replace_workspace_with(
            &mut workspace,
            &mut focus,
            data_many(vec![failed, successful]),
            backend.as_ref(),
            &terminals,
            &AppSettings::default(),
            &[],
            &mut cx,
        );

        let ActionResult::Err(error) = result else {
            panic!("one project materialization should fail");
        };
        assert!(error.contains("failed: failed to spawn terminal"));
        assert!(workspace.project("successful").is_some());
        assert!(
            workspace
                .project("successful")
                .and_then(|project| project.layout.as_ref())
                .is_some_and(|layout| matches!(
                    layout,
                    LayoutNode::Terminal {
                        terminal_id: Some(_),
                        ..
                    }
                ))
        );
        let history = monitor.history();
        assert_eq!(history.len(), 2, "every project-open lifecycle must run");
        assert!(history.iter().any(|entry| entry.project_name == "failed"));
        assert!(
            history
                .iter()
                .any(|entry| entry.project_name == "successful")
        );
    }

    #[test]
    fn replacement_cancels_outgoing_hook_monitor_before_dropping_ownership() {
        let backend = Arc::new(RecordingBackend {
            next_id: AtomicUsize::new(1),
            killed: Mutex::new(Vec::new()),
            reconnected: Mutex::new(Vec::new()),
            fail_next_cwds: Mutex::new(HashSet::new()),
        });
        let terminals: TerminalsRegistry = Arc::new(Default::default());
        let runner = HookRunner::new(backend.clone(), terminals.clone());
        let monitor = HookMonitor::new();
        monitor.record_start_named(
            "on_project_open",
            "sleep 10",
            "old",
            Some("outgoing-hook".to_string()),
        );
        let mut cx = TestCx {
            runner,
            monitor: monitor.clone(),
        };
        let mut workspace = Workspace::new(data(project("old", "outgoing-hook", None)));
        let mut focus = FocusManager::default();

        let result = replace_workspace_with(
            &mut workspace,
            &mut focus,
            WorkspaceData::empty(),
            backend.as_ref(),
            &terminals,
            &AppSettings::default(),
            &[],
            &mut cx,
        );

        assert!(matches!(result, ActionResult::Ok(_)));
        assert!(monitor.history().is_empty());
        assert!(monitor.drain_pending_toasts().is_empty());
    }

    #[test]
    fn replacement_is_rejected_while_optimistic_worktree_create_is_active() {
        let backend = Arc::new(RecordingBackend {
            next_id: AtomicUsize::new(1),
            killed: Mutex::new(Vec::new()),
            reconnected: Mutex::new(Vec::new()),
            fail_next_cwds: Mutex::new(HashSet::new()),
        });
        let terminals: TerminalsRegistry = Arc::new(Default::default());
        let runner = HookRunner::new(backend.clone(), terminals.clone());
        let monitor = HookMonitor::new();
        let mut cx = TestCx { runner, monitor };
        let mut workspace = Workspace::new(data(project("creating", "old-hook", None)));
        workspace.mark_creating_project("creating");
        let original_epoch = workspace.data_replacement_epoch();
        let mut focus = FocusManager::default();

        let result = replace_workspace_with(
            &mut workspace,
            &mut focus,
            data(project("replacement", "new-hook", None)),
            backend.as_ref(),
            &terminals,
            &AppSettings::default(),
            &[],
            &mut cx,
        );

        assert!(matches!(
            result,
            ActionResult::Err(ref error)
                if error == "cannot replace workspace while worktree 'creating' is being created"
        ));
        assert!(workspace.project("creating").is_some());
        assert!(workspace.project("replacement").is_none());
        assert!(workspace.is_creating_project("creating"));
        assert_eq!(workspace.data_replacement_epoch(), original_epoch);
        assert!(backend.killed.lock().unwrap().is_empty());
        assert!(backend.reconnected.lock().unwrap().is_empty());
    }

    #[test]
    fn replacement_is_rejected_while_worktree_close_or_merge_is_active() {
        let backend = Arc::new(RecordingBackend {
            next_id: AtomicUsize::new(1),
            killed: Mutex::new(Vec::new()),
            reconnected: Mutex::new(Vec::new()),
            fail_next_cwds: Mutex::new(HashSet::new()),
        });
        let terminals: TerminalsRegistry = Arc::new(Default::default());
        let runner = HookRunner::new(backend.clone(), terminals.clone());
        let monitor = HookMonitor::new();
        let mut cx = TestCx { runner, monitor };
        let mut workspace = Workspace::new(data(project("closing", "old-hook", None)));
        // Both background close routes set this authoritative flag before any
        // non-merge removal or merge git work is dispatched.
        workspace.mark_closing_project_authoritative("closing");
        let original_epoch = workspace.data_replacement_epoch();
        let mut focus = FocusManager::default();

        let result = replace_workspace_with(
            &mut workspace,
            &mut focus,
            data(project("replacement", "new-hook", None)),
            backend.as_ref(),
            &terminals,
            &AppSettings::default(),
            &[],
            &mut cx,
        );

        assert!(matches!(
            result,
            ActionResult::Err(ref error)
                if error == "cannot replace workspace while worktree 'closing' is closing"
        ));
        assert!(workspace.project("closing").is_some());
        assert!(workspace.project("replacement").is_none());
        assert!(workspace.is_project_closing("closing"));
        assert_eq!(workspace.data_replacement_epoch(), original_epoch);
        assert!(backend.killed.lock().unwrap().is_empty());
        assert!(backend.reconnected.lock().unwrap().is_empty());
    }

    #[test]
    fn load_session_rejects_active_create_before_loading() {
        let backend = Arc::new(RecordingBackend {
            next_id: AtomicUsize::new(1),
            killed: Mutex::new(Vec::new()),
            reconnected: Mutex::new(Vec::new()),
            fail_next_cwds: Mutex::new(HashSet::new()),
        });
        let terminals: TerminalsRegistry = Arc::new(Default::default());
        let runner = HookRunner::new(backend.clone(), terminals.clone());
        let monitor = HookMonitor::new();
        let mut cx = TestCx { runner, monitor };
        let mut workspace = Workspace::new(data(project("creating", "old-hook", None)));
        workspace.mark_creating_project("creating");
        let mut focus = FocusManager::default();
        let loader_calls = AtomicUsize::new(0);

        let result = load_session_action_with_loader(
            &mut workspace,
            &mut focus,
            backend.as_ref(),
            &terminals,
            &AppSettings::default(),
            &mut cx,
            || {
                loader_calls.fetch_add(1, Ordering::Relaxed);
                Ok(LoadedWorkspace {
                    data: WorkspaceData::empty(),
                    stale_terminal_ids: Vec::new(),
                })
            },
        );

        assert!(matches!(
            result,
            ActionResult::Err(ref error)
                if error == "cannot replace workspace while worktree 'creating' is being created"
        ));
        assert_eq!(loader_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn import_workspace_rejects_active_close_before_loading() {
        let backend = Arc::new(RecordingBackend {
            next_id: AtomicUsize::new(1),
            killed: Mutex::new(Vec::new()),
            reconnected: Mutex::new(Vec::new()),
            fail_next_cwds: Mutex::new(HashSet::new()),
        });
        let terminals: TerminalsRegistry = Arc::new(Default::default());
        let runner = HookRunner::new(backend.clone(), terminals.clone());
        let monitor = HookMonitor::new();
        let mut cx = TestCx { runner, monitor };
        let mut workspace = Workspace::new(data(project("closing", "old-hook", None)));
        workspace.mark_closing_project_authoritative("closing");
        let mut focus = FocusManager::default();
        let loader_calls = AtomicUsize::new(0);

        let result = import_workspace_action_with_loader(
            &mut workspace,
            &mut focus,
            backend.as_ref(),
            &terminals,
            &AppSettings::default(),
            &mut cx,
            || {
                loader_calls.fetch_add(1, Ordering::Relaxed);
                Ok(WorkspaceData::empty())
            },
        );

        assert!(matches!(
            result,
            ActionResult::Err(ref error)
                if error == "cannot replace workspace while worktree 'closing' is closing"
        ));
        assert_eq!(loader_calls.load(Ordering::Relaxed), 0);
    }
}
