//! Project, folder, and worktree action handlers.

// Handlers take the workspace, focus manager, terminals registry and cx as
// distinct dependencies; bundling them into a context struct would obscure
// more than it clarifies here.
#![allow(clippy::too_many_arguments)]

use super::{ActionResult, find_first_terminal_id, spawn_uninitialized_terminals};
use crate::workspace::focus::FocusManager;
use crate::workspace::persistence::AppSettings;
use crate::workspace::state::{HookTerminalStatus, WindowId, Workspace};
use okena_core::theme::FolderColor;
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::TerminalBackend;
use okena_terminal::terminal::{Terminal, TerminalSize};
use okena_workspace::context::WorkspaceCx;
use okena_workspace::hook_monitor::HookStatus;
use std::sync::Arc;

fn project_not_found(project_id: &str) -> ActionResult {
    ActionResult::Err(format!("project not found: {}", project_id))
}

fn with_existing_project(
    ws: &mut Workspace,
    project_id: &str,
    f: impl FnOnce(&mut Workspace),
) -> ActionResult {
    if ws.project(project_id).is_none() {
        return project_not_found(project_id);
    }
    f(ws);
    ActionResult::Ok(None)
}

fn with_existing_project_result(
    ws: &mut Workspace,
    project_id: &str,
    f: impl FnOnce(&mut Workspace) -> Result<(), String>,
) -> ActionResult {
    if ws.project(project_id).is_none() {
        return project_not_found(project_id);
    }
    match f(ws) {
        Ok(()) => ActionResult::Ok(None),
        Err(e) => ActionResult::Err(e),
    }
}

pub(super) fn add_project(
    ws: &mut Workspace,
    window_id: WindowId,
    name: String,
    path: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let project_id = match ws.add_project(name, path, true, &settings.hooks, window_id, cx) {
        Ok(project_id) => project_id,
        Err(error) => return ActionResult::Err(error),
    };
    // Surface the newly-created project's id alongside the spawned terminal
    // ids so callers (e.g. the CLI `add-project` verb) can address the project
    // they just created without re-fetching state. `spawn_uninitialized_terminals`
    // returns `{ "terminal_ids": [...] }`; we merge `project_id` into that
    // object, leaving its terminal-spawning behavior unchanged.
    match spawn_uninitialized_terminals(ws, &project_id, backend, terminals, settings, None, cx) {
        ActionResult::Ok(Some(serde_json::Value::Object(mut map))) => {
            map.insert(
                "project_id".to_string(),
                serde_json::Value::String(project_id),
            );
            ActionResult::Ok(Some(serde_json::Value::Object(map)))
        }
        ActionResult::Ok(_) => ActionResult::Ok(Some(serde_json::json!({
            "project_id": project_id,
            "terminal_ids": [],
        }))),
        err => err,
    }
}

pub(super) fn reorder_in_folder(
    ws: &mut Workspace,
    folder_id: String,
    project_id: String,
    new_index: usize,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    ws.reorder_project_in_folder(&folder_id, &project_id, new_index, cx);
    ActionResult::Ok(None)
}

pub(super) fn set_project_color(
    ws: &mut Workspace,
    project_id: String,
    color: FolderColor,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    ws.set_folder_color(&project_id, color, cx);
    ActionResult::Ok(None)
}

pub(super) fn set_folder_color(
    ws: &mut Workspace,
    folder_id: String,
    color: FolderColor,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    ws.set_folder_item_color(&folder_id, color, cx);
    ActionResult::Ok(None)
}

pub(super) fn rename_project(
    ws: &mut Workspace,
    project_id: String,
    name: String,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    with_existing_project(ws, &project_id, |ws| {
        ws.rename_project(&project_id, name, cx)
    })
}

pub(super) fn update_project_hooks(
    ws: &mut Workspace,
    project_id: String,
    hooks: okena_core::api::ApiHooksConfig,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let new_hooks = crate::workspace::settings::HooksConfig::from_api(&hooks);
    // Dirty-check inside the closure: clients flush hooks on every panel
    // close/project-switch even when unchanged, so skip the notify (→ state bump
    // → full-snapshot churn to all clients) when the hooks are identical.
    let mut existed = false;
    ws.with_project(&project_id, cx, |p| {
        existed = true;
        if p.hooks == new_hooks {
            false
        } else {
            p.hooks = new_hooks;
            true
        }
    });
    if !existed {
        return ActionResult::Err(format!("project not found: {}", project_id));
    }
    ActionResult::Ok(None)
}

pub(super) fn rename_project_directory(
    ws: &mut Workspace,
    project_id: String,
    new_name: String,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    if let Err(e) = super::validate_leaf_name(&new_name) {
        return ActionResult::Err(e);
    }
    let current_path = match ws.project(&project_id) {
        Some(p) => p.path.clone(),
        None => return ActionResult::Err(format!("project not found: {}", project_id)),
    };
    let old_path = std::path::Path::new(&current_path);
    let parent = match old_path.parent() {
        Some(p) => p,
        None => return ActionResult::Err("cannot determine parent directory".to_string()),
    };
    let new_path = parent.join(&new_name);
    let new_path_str = new_path.to_string_lossy().to_string();
    match ws.rename_project_directory(&project_id, new_path_str, new_name, cx) {
        Ok(()) => ActionResult::Ok(None),
        Err(error) => ActionResult::Err(error),
    }
}

pub(super) fn delete_project(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    if ws.project(&project_id).is_none() {
        return project_not_found(&project_id);
    }
    // Reject while the worktree is still being created: the optimistic create
    // registers the row and returns before its background `git worktree add`
    // finishes, so dropping the row now would race the in-flight checkout and
    // strand an orphaned, git-registered worktree with no workspace entry.
    // Mirrors the `is_creating` guard on the close/removal routes
    // (`close_worktree`, `begin_worktree_removal`). Guarded here, not inside
    // `Workspace::delete_project` — its finalize/rollback callers invoke that
    // AFTER their own guards.
    if ws.is_creating_project(&project_id) {
        return ActionResult::Err("worktree is still being created".to_string());
    }
    if ws.is_project_closing(&project_id) {
        return ActionResult::Err("project is already being closed".to_string());
    }
    let global_hooks = settings.hooks.clone();
    ws.delete_project(focus_manager, &project_id, &global_hooks, cx);
    ActionResult::Ok(None)
}

pub(super) fn set_show_in_overview(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    window_id: WindowId,
    project_id: String,
    show: bool,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    apply_set_project_show_in_overview(ws, focus_manager, window_id, &project_id, show, cx)
}

/// Apply the SetProjectShowInOverview action against the targeted window.
///
/// Reads the project's current per-window visibility from the targeted
/// window's `hidden_project_ids`, then toggles only when the desired and
/// current states differ. `window_id` carries through from `execute_action`
/// so remote-bridge invocations land on whichever window currently has OS
/// focus. For unknown extras (close-race), the read returns `None`; we treat
/// the project as visible and the toggle delegates to the silent-no-op path.
fn apply_set_project_show_in_overview(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    window_id: WindowId,
    project_id: &str,
    show: bool,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    if ws.project(project_id).is_none() {
        return ActionResult::Err(format!("project not found: {}", project_id));
    }
    let current_hidden = ws
        .data()
        .window(window_id)
        .map(|w| w.hidden_project_ids.contains(project_id))
        .unwrap_or(false);
    let current_visible = !current_hidden;
    if current_visible != show {
        ws.toggle_project_overview_visibility(focus_manager, window_id, project_id, cx);
    }
    ActionResult::Ok(None)
}

pub(super) fn remove_worktree_project(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    force: bool,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    if ws.project(&project_id).is_none() {
        return project_not_found(&project_id);
    }
    if ws.is_project_closing(&project_id) {
        return ActionResult::Err("worktree is already closing".to_string());
    }
    let global_hooks = settings.hooks.clone();
    match ws.remove_worktree_project(focus_manager, &project_id, force, &global_hooks, cx) {
        Ok(()) => ActionResult::Ok(None),
        Err(error) => ActionResult::Err(error),
    }
}

pub(super) fn close_worktree(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    project_id: String,
    merge: bool,
    stash: bool,
    fetch: bool,
    push: bool,
    delete_branch: bool,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    with_existing_project_result(ws, &project_id, |ws| {
        ws.close_worktree(
            focus_manager,
            &project_id,
            merge,
            stash,
            fetch,
            push,
            delete_branch,
            &settings.hooks,
            cx,
        )
    })
}

pub(super) fn create_folder(
    ws: &mut Workspace,
    name: String,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let id = ws.create_folder(name, cx);
    ActionResult::Ok(Some(serde_json::json!({ "folder_id": id })))
}

pub(super) fn delete_folder(
    ws: &mut Workspace,
    folder_id: String,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    ws.delete_folder(&folder_id, cx);
    ActionResult::Ok(None)
}

pub(super) fn rename_folder(
    ws: &mut Workspace,
    folder_id: String,
    name: String,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    ws.rename_folder(&folder_id, name, cx);
    ActionResult::Ok(None)
}

pub(super) fn move_to_folder(
    ws: &mut Workspace,
    project_id: String,
    folder_id: String,
    position: Option<usize>,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    with_existing_project(ws, &project_id, |ws| {
        ws.move_project_to_folder(&project_id, &folder_id, position, cx);
    })
}

pub(super) fn move_out_of_folder(
    ws: &mut Workspace,
    project_id: String,
    top_level_index: usize,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    with_existing_project(ws, &project_id, |ws| {
        ws.move_project_out_of_folder(&project_id, top_level_index, cx);
    })
}

pub(super) fn move_project(
    ws: &mut Workspace,
    project_id: String,
    new_index: usize,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    with_existing_project(ws, &project_id, |ws| {
        ws.move_project(&project_id, new_index, cx);
    })
}

pub(super) fn move_item_in_order(
    ws: &mut Workspace,
    item_id: String,
    new_index: usize,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    // `item_id` may be a folder id or a top-level project id; the workspace
    // method is a no-op when the id isn't in `project_order`.
    ws.move_item_in_order(&item_id, new_index, cx);
    ActionResult::Ok(None)
}

pub(super) fn toggle_project_pinned(
    ws: &mut Workspace,
    project_id: String,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    with_existing_project(ws, &project_id, |ws| {
        ws.toggle_project_pinned(&project_id, cx);
    })
}

pub(super) fn reorder_worktree(
    ws: &mut Workspace,
    parent_id: String,
    worktree_id: String,
    new_index: usize,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    with_existing_project(ws, &parent_id, |ws| {
        ws.reorder_worktree(&parent_id, &worktree_id, new_index, cx);
    })
}

pub(super) fn set_worktree_color_override(
    ws: &mut Workspace,
    project_id: String,
    color: Option<FolderColor>,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    with_existing_project(ws, &project_id, |ws| {
        ws.set_worktree_color_override(&project_id, color, cx);
    })
}

pub(super) fn create_worktree(
    ws: &mut Workspace,
    window_id: WindowId,
    project_id: String,
    branch: String,
    create_branch: bool,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let project = match ws.project(&project_id) {
        Some(p) => p,
        None => return ActionResult::Err(format!("project not found: {}", project_id)),
    };
    let project_path = std::path::PathBuf::from(&project.path);
    let (git_root, subdir) = okena_git::resolve_git_root_and_subdir(&project_path);
    let path_template = settings.worktree.path_template.clone();
    let (worktree_path, wt_project_path) =
        okena_git::compute_target_paths(&git_root, &subdir, &path_template, &branch);
    let global_hooks = settings.hooks.clone();

    match ws.create_worktree_project(
        &project_id,
        &branch,
        &git_root,
        &worktree_path,
        &wt_project_path,
        create_branch,
        &global_hooks,
        window_id,
        cx,
    ) {
        Ok(new_project_id) => {
            let result = spawn_uninitialized_terminals(
                ws,
                &new_project_id,
                backend,
                terminals,
                settings,
                None,
                cx,
            );
            let terminal_id = ws
                .project(&new_project_id)
                .and_then(|p| p.layout.as_ref())
                .and_then(find_first_terminal_id);
            match result {
                ActionResult::Ok(_) => ActionResult::Ok(Some(serde_json::json!({
                    "project_id": new_project_id,
                    "terminal_id": terminal_id,
                    "path": wt_project_path,
                }))),
                err => err,
            }
        }
        Err(e) => ActionResult::Err(e),
    }
}

/// Register an already-on-disk git worktree as a tracked project under its
/// parent. The worktree path is discovered client-side from a local git scan
/// (same filesystem for a local daemon) and passed verbatim — the daemon
/// creates the project, links it to the parent's worktree list, and spawns its
/// terminal. Mirrors the old in-process `add_discovered_worktree` +
/// `add_to_worktree_ids` GUI path.
pub(super) fn add_discovered_worktree(
    ws: &mut Workspace,
    window_id: WindowId,
    parent_project_id: String,
    worktree_path: String,
    branch: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    if ws.project(&parent_project_id).is_none() {
        return ActionResult::Err(format!("project not found: {}", parent_project_id));
    }
    let new_id =
        match ws.add_discovered_worktree(&worktree_path, &branch, &parent_project_id, window_id) {
            Ok(id) => id,
            Err(error) => return ActionResult::Err(error),
        };
    ws.add_to_worktree_ids(&parent_project_id, &new_id);
    // `add_discovered_worktree` deliberately doesn't notify (caller's job).
    ws.notify_data(cx);
    let result = spawn_uninitialized_terminals(ws, &new_id, backend, terminals, settings, None, cx);
    let terminal_id = ws
        .project(&new_id)
        .and_then(|p| p.layout.as_ref())
        .and_then(find_first_terminal_id);
    match result {
        ActionResult::Ok(_) => ActionResult::Ok(Some(serde_json::json!({
            "project_id": new_id,
            "terminal_id": terminal_id,
        }))),
        err => err,
    }
}

/// Rerun a lifecycle-hook terminal: kill the old PTY, spawn a fresh shell at the
/// same cwd, swap the id in state (status back to Running), and type the stored
/// command into the new shell. The daemon is authoritative for the hook's
/// command + cwd (read from `hook_terminals`), so the action only carries the
/// project + terminal ids. Mirrors the old in-process `rerun_hook` GUI path.
pub(super) fn rerun_hook(
    ws: &mut Workspace,
    project_id: String,
    terminal_id: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let (command, cwd, hook_type, project_name, status) =
        match ws.project(&project_id).and_then(|p| {
            p.hook_terminals.get(&terminal_id).map(|entry| {
                (
                    entry.command.clone(),
                    entry.cwd.clone(),
                    entry.hook_type.clone(),
                    p.name.clone(),
                    entry.status.clone(),
                )
            })
        }) {
            Some(details) => details,
            None => return ActionResult::Err(format!("hook terminal not found: {}", terminal_id)),
        };
    if status == HookTerminalStatus::Running {
        return ActionResult::Err("hook is still running".to_string());
    }
    let monitor = cx.hook_monitor();
    let shell = okena_workspace::hooks::keep_alive_hook_shell(&command);
    let new_id = match backend.create_terminal(&cwd, Some(&shell)) {
        Ok(id) => id,
        Err(e) => {
            let message = format!("failed to spawn hook terminal: {e}");
            if let Some(monitor) = monitor.as_ref() {
                let execution_id =
                    monitor.record_start_named(&hook_type, &command, &project_name, None);
                monitor.record_finish(
                    execution_id,
                    HookStatus::SpawnError {
                        message: message.clone(),
                    },
                );
            }
            return ActionResult::Err(message);
        }
    };
    backend.kill(&terminal_id);
    if let Some(monitor) = monitor.as_ref() {
        monitor.finish_by_terminal_id(&terminal_id, None);
        monitor.notify_exit(&terminal_id, None);
    }
    let transport = backend.transport();
    let terminal = Arc::new(Terminal::new(
        new_id.clone(),
        TerminalSize::default(),
        transport.clone(),
        cwd.clone(),
    ));
    {
        let mut guard = terminals.lock();
        guard.remove(&terminal_id);
        guard.insert(new_id.clone(), terminal);
    }
    ws.swap_hook_terminal_id(&project_id, &terminal_id, &new_id, cx);
    if let Some(monitor) = monitor {
        monitor.record_start_named(&hook_type, &command, &project_name, Some(new_id.clone()));
    }
    ActionResult::Ok(Some(serde_json::json!({ "terminal_id": new_id })))
}

/// Stop and remove a lifecycle-hook terminal from authoritative daemon state.
pub(super) fn dismiss_hook(
    ws: &mut Workspace,
    project_id: String,
    terminal_id: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let exists = ws
        .project(&project_id)
        .is_some_and(|project| project.hook_terminals.contains_key(&terminal_id));
    if !exists {
        return ActionResult::Err(format!("hook terminal not found: {terminal_id}"));
    }

    backend.kill(&terminal_id);
    if let Some(monitor) = cx.hook_monitor() {
        monitor.finish_by_terminal_id(&terminal_id, None);
        monitor.notify_exit(&terminal_id, None);
    }
    ws.cancel_pending_worktree_close(&terminal_id);
    ws.remove_hook_terminal(&terminal_id, cx);
    terminals.lock().remove(&terminal_id);
    ActionResult::Ok(None)
}

#[cfg(test)]
mod hook_action_tests {
    use super::{ActionResult, delete_project, dismiss_hook, remove_worktree_project, rerun_hook};
    use crate::workspace::focus::FocusManager;
    use crate::workspace::state::{
        HookTerminalEntry, HookTerminalStatus, ProjectData, WindowState, Workspace, WorkspaceData,
    };
    use okena_core::theme::FolderColor;
    use okena_terminal::TerminalsRegistry;
    use okena_terminal::backend::TerminalBackend;
    use okena_terminal::shell_config::ShellType;
    use okena_terminal::terminal::TerminalTransport;
    use okena_workspace::context::WorkspaceCx;
    use okena_workspace::hook_monitor::{HookMonitor, HookStatus};
    use okena_workspace::hooks::HookRunner;
    use okena_workspace::settings::{AppSettings, HooksConfig};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingTransport {
        inputs: Mutex<Vec<(String, Vec<u8>)>>,
    }

    impl TerminalTransport for RecordingTransport {
        fn send_input(&self, terminal_id: &str, data: &[u8]) {
            self.inputs
                .lock()
                .unwrap()
                .push((terminal_id.to_string(), data.to_vec()));
        }

        fn resize(&self, _terminal_id: &str, _cols: u16, _rows: u16) {}

        fn uses_mouse_backend(&self) -> bool {
            false
        }
    }

    #[derive(Default)]
    struct RecordingBackend {
        transport: Arc<RecordingTransport>,
        next_id: AtomicUsize,
        shells: Mutex<Vec<Option<ShellType>>>,
        killed: Mutex<Vec<String>>,
        fail_create: AtomicBool,
    }

    impl TerminalBackend for RecordingBackend {
        fn transport(&self) -> Arc<dyn TerminalTransport> {
            self.transport.clone()
        }

        fn create_terminal(&self, _cwd: &str, shell: Option<&ShellType>) -> anyhow::Result<String> {
            self.shells.lock().unwrap().push(shell.cloned());
            if self.fail_create.load(Ordering::Relaxed) {
                anyhow::bail!("spawn failed");
            }
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            Ok(format!("rerun-{id}"))
        }

        fn reconnect_terminal(
            &self,
            _terminal_id: &str,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            unreachable!("tests do not reconnect terminals")
        }

        fn kill(&self, terminal_id: &str) {
            self.killed.lock().unwrap().push(terminal_id.to_string());
        }

        fn capture_buffer(&self, _terminal_id: &str) -> Option<std::path::PathBuf> {
            None
        }

        fn supports_buffer_capture(&self) -> bool {
            false
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
        monitor: HookMonitor,
    }

    impl WorkspaceCx for TestCx {
        fn notify(&mut self) {}

        fn refresh_views(&mut self) {}

        fn hook_runner(&self) -> Option<HookRunner> {
            None
        }

        fn hook_monitor(&self) -> Option<HookMonitor> {
            Some(self.monitor.clone())
        }
    }

    fn workspace_with_hook_status(terminal_id: &str, status: HookTerminalStatus) -> Workspace {
        let mut hook_terminals = HashMap::new();
        hook_terminals.insert(
            terminal_id.to_string(),
            HookTerminalEntry {
                label: "on_project_open (Project p1)".to_string(),
                status,
                hook_type: "on_project_open".to_string(),
                command: "export OKENA_PROJECT_ID='p1'; echo test".to_string(),
                cwd: "/tmp/test".to_string(),
            },
        );
        let project = ProjectData {
            id: "p1".to_string(),
            name: "Project p1".to_string(),
            path: "/tmp/test".to_string(),
            layout: None,
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            default_shell: None,
            hook_terminals,
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
        };
        Workspace::new(WorkspaceData {
            version: 1,
            projects: vec![project],
            project_order: vec!["p1".to_string()],
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            folders: Vec::new(),
            main_window: WindowState::default(),
            extra_windows: Vec::new(),
        })
    }

    fn workspace_with_hook(terminal_id: &str) -> Workspace {
        workspace_with_hook_status(terminal_id, HookTerminalStatus::Succeeded)
    }

    fn workspace_with_worktree_hook(terminal_id: &str) -> Workspace {
        let workspace = workspace_with_hook(terminal_id);
        let mut data = workspace.data().clone();
        data.projects[0].worktree_info = Some(crate::workspace::state::WorktreeMetadata {
            parent_project_id: "parent".to_string(),
            color_override: None,
            main_repo_path: "/tmp/main".to_string(),
            worktree_path: "/tmp/test".to_string(),
            branch_name: "topic".to_string(),
        });
        Workspace::new(data)
    }

    #[test]
    fn rerun_hook_uses_completion_wrapper_and_records_fresh_execution() {
        let mut workspace = workspace_with_hook("old-hook");
        let backend = RecordingBackend::default();
        let terminals: TerminalsRegistry = Default::default();
        let monitor = HookMonitor::new();
        monitor.record_start(
            "on_project_open",
            "echo test",
            "Project p1",
            Some("old-hook".to_string()),
        );
        let mut cx = TestCx {
            monitor: monitor.clone(),
        };

        let result = rerun_hook(
            &mut workspace,
            "p1".to_string(),
            "old-hook".to_string(),
            &backend,
            &terminals,
            &mut cx,
        );

        assert!(matches!(result, ActionResult::Ok(_)));
        let entry = workspace
            .project("p1")
            .unwrap()
            .hook_terminals
            .get("rerun-0")
            .unwrap();
        assert_eq!(entry.status, HookTerminalStatus::Running);
        assert!(
            !workspace
                .project("p1")
                .unwrap()
                .hook_terminals
                .contains_key("old-hook")
        );
        assert!(backend.transport.inputs.lock().unwrap().is_empty());

        let shells = backend.shells.lock().unwrap();
        let ShellType::Custom { args, .. } = shells[0].as_ref().unwrap() else {
            panic!("rerun should use a completion-wrapped custom shell");
        };
        assert!(args.iter().any(|arg| arg.contains("__okena_hook_exit")));

        let history = monitor.history();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].terminal_id.as_deref(), Some("rerun-0"));
        assert!(matches!(history[0].status, HookStatus::Running));
        assert_eq!(history[1].terminal_id.as_deref(), Some("old-hook"));
        assert!(matches!(history[1].status, HookStatus::Failed { .. }));
    }

    #[test]
    fn rerun_hook_rejects_a_running_authoritative_execution() {
        let mut workspace = workspace_with_hook_status("running-hook", HookTerminalStatus::Running);
        let backend = RecordingBackend::default();
        let terminals: TerminalsRegistry = Default::default();
        let mut cx = TestCx {
            monitor: HookMonitor::new(),
        };

        let result = rerun_hook(
            &mut workspace,
            "p1".to_string(),
            "running-hook".to_string(),
            &backend,
            &terminals,
            &mut cx,
        );

        assert!(matches!(
            result,
            ActionResult::Err(message) if message == "hook is still running"
        ));
        assert!(backend.shells.lock().unwrap().is_empty());
        assert!(backend.killed.lock().unwrap().is_empty());
        assert!(
            workspace
                .project("p1")
                .unwrap()
                .hook_terminals
                .contains_key("running-hook")
        );
    }

    #[test]
    fn dismiss_hook_finishes_running_execution_before_removing_owner() {
        let mut workspace = workspace_with_hook("hook-1");
        let backend = RecordingBackend::default();
        let terminals: TerminalsRegistry = Default::default();
        let monitor = HookMonitor::new();
        monitor.record_start(
            "on_project_open",
            "echo test",
            "Project p1",
            Some("hook-1".to_string()),
        );
        let mut cx = TestCx {
            monitor: monitor.clone(),
        };

        let result = dismiss_hook(
            &mut workspace,
            "p1".to_string(),
            "hook-1".to_string(),
            &backend,
            &terminals,
            &mut cx,
        );

        assert!(matches!(result, ActionResult::Ok(None)));
        assert!(workspace.project("p1").unwrap().hook_terminals.is_empty());
        let history = monitor.history();
        assert!(matches!(history[0].status, HookStatus::Failed { .. }));
        assert_eq!(backend.killed.lock().unwrap().as_slice(), ["hook-1"]);
    }

    #[test]
    fn rerun_spawn_failure_preserves_the_existing_hook_owner() {
        let mut workspace = workspace_with_hook("old-hook");
        let backend = RecordingBackend::default();
        backend.fail_create.store(true, Ordering::Relaxed);
        let terminals: TerminalsRegistry = Default::default();
        let monitor = HookMonitor::new();
        monitor.record_start(
            "on_project_open",
            "echo test",
            "Project p1",
            Some("old-hook".to_string()),
        );
        let mut cx = TestCx {
            monitor: monitor.clone(),
        };

        let result = rerun_hook(
            &mut workspace,
            "p1".to_string(),
            "old-hook".to_string(),
            &backend,
            &terminals,
            &mut cx,
        );

        assert!(matches!(result, ActionResult::Err(message) if message.contains("spawn failed")));
        assert!(
            workspace
                .project("p1")
                .unwrap()
                .hook_terminals
                .contains_key("old-hook")
        );
        assert!(backend.killed.lock().unwrap().is_empty());
        let history = monitor.history();
        assert!(matches!(history[0].status, HookStatus::SpawnError { .. }));
        assert!(matches!(history[1].status, HookStatus::Running));
    }

    #[test]
    fn delete_project_rejects_an_authoritative_close_in_progress() {
        let mut workspace = workspace_with_hook("hook-1");
        workspace.mark_closing_project_authoritative("p1");
        let mut focus_manager = FocusManager::default();
        let monitor = HookMonitor::new();
        let mut cx = TestCx { monitor };

        let result = delete_project(
            &mut workspace,
            &mut focus_manager,
            "p1".to_string(),
            &AppSettings::default(),
            &mut cx,
        );

        assert!(matches!(
            result,
            ActionResult::Err(message) if message == "project is already being closed"
        ));
        assert!(workspace.project("p1").is_some());
    }

    #[test]
    fn remove_worktree_project_rejects_an_authoritative_close_in_progress() {
        let mut workspace = workspace_with_worktree_hook("hook-1");
        workspace.mark_closing_project_authoritative("p1");
        let mut focus_manager = FocusManager::default();
        let mut cx = TestCx {
            monitor: HookMonitor::new(),
        };

        let result = remove_worktree_project(
            &mut workspace,
            &mut focus_manager,
            "p1".to_string(),
            true,
            &AppSettings::default(),
            &mut cx,
        );

        assert!(matches!(
            result,
            ActionResult::Err(message) if message == "worktree is already closing"
        ));
        assert!(workspace.project("p1").is_some());
    }

    #[test]
    fn project_delete_finishes_owned_hook_executions_before_removal() {
        let mut workspace = workspace_with_hook("hook-1");
        let mut focus_manager = FocusManager::default();
        let monitor = HookMonitor::new();
        monitor.record_start(
            "on_project_open",
            "echo test",
            "Project p1",
            Some("hook-1".to_string()),
        );
        let mut cx = TestCx {
            monitor: monitor.clone(),
        };

        workspace.delete_project(&mut focus_manager, "p1", &HooksConfig::default(), &mut cx);

        assert!(workspace.project("p1").is_none());
        assert!(matches!(
            monitor.history()[0].status,
            HookStatus::Failed { .. }
        ));
    }
}

#[cfg(all(test, feature = "gpui"))]
mod set_show_in_overview_tests {
    use super::{ActionResult, apply_set_project_show_in_overview};
    use crate::workspace::settings::HooksConfig;
    use crate::workspace::state::{ProjectData, WindowId, WindowState, Workspace, WorkspaceData};
    use gpui::AppContext as _;
    use okena_core::theme::FolderColor;
    use std::collections::HashMap;

    fn make_workspace_data() -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects: vec![],
            project_order: vec![],
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            folders: vec![],
            main_window: WindowState::default(),
            extra_windows: Vec::new(),
        }
    }

    fn make_project(id: &str) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: format!("Project {}", id),
            path: "/tmp/test".to_string(),
            layout: None,
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            default_shell: None,
            hook_terminals: HashMap::new(),
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
        }
    }

    #[gpui::test]
    fn apply_set_project_show_in_overview_reads_hidden_set(cx: &mut gpui::TestAppContext) {
        // The action's visibility decision must read from the targeted
        // window's hidden_project_ids. This fixture starts with p1 hidden in
        // main; the action says `show: true`, so the helper toggles,
        // clearing main's hidden set.
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1")];
        data.project_order = vec!["p1".to_string()];
        data.main_window.hidden_project_ids.insert("p1".to_string());
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = crate::workspace::focus::FocusManager::new();
            let result =
                apply_set_project_show_in_overview(ws, &mut fm, WindowId::Main, "p1", true, cx);
            assert!(matches!(result, ActionResult::Ok(_)));
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            assert!(
                !ws.data().main_window.hidden_project_ids.contains("p1"),
                "action should have toggled main's hidden set off"
            );
        });
    }

    #[gpui::test]
    fn apply_set_project_show_in_overview_unknown_project_errs(cx: &mut gpui::TestAppContext) {
        let workspace = cx.new(|_cx| Workspace::new(make_workspace_data()));
        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = crate::workspace::focus::FocusManager::new();
            let result = apply_set_project_show_in_overview(
                ws,
                &mut fm,
                WindowId::Main,
                "missing",
                true,
                cx,
            );
            assert!(matches!(result, ActionResult::Err(_)));
        });
    }

    #[gpui::test]
    fn apply_set_project_show_in_overview_targets_extra_when_window_id_extra(
        cx: &mut gpui::TestAppContext,
    ) {
        // PRD user story 27 / slice 05 cri 13: a remote-bridge action issued
        // while an extra window has OS focus must mutate that extra's
        // per-window hidden set, not main's. The extra starts with p1 hidden
        // (mirrors the spawn snapshot semantic where every project is hidden
        // in a fresh extra); the action says `show: true`, so the helper
        // toggles only on the extra. Main's hidden set must remain empty.
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1")];
        data.project_order = vec!["p1".to_string()];
        let mut extra = WindowState::default();
        extra.hidden_project_ids.insert("p1".to_string());
        let extra_id = extra.id;
        data.extra_windows = vec![extra];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = crate::workspace::focus::FocusManager::new();
            let result = apply_set_project_show_in_overview(
                ws,
                &mut fm,
                WindowId::Extra(extra_id),
                "p1",
                true,
                cx,
            );
            assert!(matches!(result, ActionResult::Ok(_)));
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let extra_state = ws
                .data()
                .window(WindowId::Extra(extra_id))
                .expect("extra still tracked");
            assert!(
                !extra_state.hidden_project_ids.contains("p1"),
                "action should have toggled the targeted extra's hidden set off",
            );
            assert!(
                ws.data().main_window.hidden_project_ids.is_empty(),
                "main's hidden set must stay untouched when routing to an extra",
            );
        });
    }

    #[gpui::test]
    fn apply_set_project_show_in_overview_extra_hide_inserts_only_on_extra(
        cx: &mut gpui::TestAppContext,
    ) {
        // The reverse direction: project visible in both main + extra,
        // action says `show: false` against the extra. Extra's hidden set
        // gains p1; main stays unchanged.
        let mut data = make_workspace_data();
        data.projects = vec![make_project("p1")];
        data.project_order = vec!["p1".to_string()];
        let extra = WindowState::default();
        let extra_id = extra.id;
        data.extra_windows = vec![extra];
        let workspace = cx.new(|_cx| Workspace::new(data));

        workspace.update(cx, |ws: &mut Workspace, cx| {
            let mut fm = crate::workspace::focus::FocusManager::new();
            let result = apply_set_project_show_in_overview(
                ws,
                &mut fm,
                WindowId::Extra(extra_id),
                "p1",
                false,
                cx,
            );
            assert!(matches!(result, ActionResult::Ok(_)));
        });

        workspace.read_with(cx, |ws: &Workspace, _cx| {
            let extra_state = ws.data().window(WindowId::Extra(extra_id)).unwrap();
            assert!(extra_state.hidden_project_ids.contains("p1"));
            assert!(ws.data().main_window.hidden_project_ids.is_empty());
        });
    }
}
