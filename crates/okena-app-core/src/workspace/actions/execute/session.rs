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

use super::{ActionResult, spawn_uninitialized_terminals};
use crate::workspace::focus::FocusManager;
use crate::workspace::persistence::AppSettings;
use crate::workspace::persistence::{
    delete_session, export_workspace, import_workspace, list_sessions, load_session,
    rename_session, save_session, session_exists,
};
use crate::workspace::state::{Workspace, WorkspaceData};
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::TerminalBackend;
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
    // Include persisted hook ids from both workspaces: a persistent backend can
    // own them even when this process has no matching registry entry.
    let mut ids: Vec<String> = terminals.lock().keys().cloned().collect();
    ids.extend(
        ws.projects()
            .iter()
            .flat_map(|project| project.hook_terminals.keys().cloned()),
    );
    ids.extend(
        data.projects
            .iter()
            .flat_map(|project| project.hook_terminals.keys().cloned()),
    );
    ids.sort();
    ids.dedup();
    for id in &ids {
        backend.kill(id);
    }
    terminals.lock().clear();

    ws.replace_data(focus_manager, data, cx);

    let project_ids: Vec<String> = ws.projects().iter().map(|p| p.id.clone()).collect();
    for pid in &project_ids {
        ws.clear_stale_hook_terminals(pid, cx);
    }

    // Non-persistent loads have uninitialized layout slots; persistent sessions
    // retain ordinary ids and reconnect through the existing spawn path.
    for pid in &project_ids {
        if let ActionResult::Err(e) =
            spawn_uninitialized_terminals(ws, pid, backend, terminals, settings, None, cx)
        {
            return ActionResult::Err(e);
        }
    }
    for pid in &project_ids {
        ws.fire_project_open_hooks(pid, &settings.hooks, cx);
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
    let data = match import_workspace(std::path::Path::new(&path)) {
        Ok(d) => d,
        Err(e) => return ActionResult::Err(format!("failed to import '{path}': {e}")),
    };
    replace_workspace_with(ws, focus_manager, data, backend, terminals, settings, cx)
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
        HookTerminalEntry, HookTerminalStatus, ProjectData, WindowState,
    };
    use okena_terminal::shell_config::ShellType;
    use okena_terminal::terminal::TerminalTransport;
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
    }

    impl TerminalBackend for RecordingBackend {
        fn transport(&self) -> Arc<dyn TerminalTransport> {
            Arc::new(StubTransport)
        }

        fn create_terminal(
            &self,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            Ok(format!("created-{id}"))
        }

        fn reconnect_terminal(
            &self,
            _terminal_id: &str,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            anyhow::bail!("not used")
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
            default_shell: None,
            hook_terminals,
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
        }
    }

    fn data(project: ProjectData) -> WorkspaceData {
        let id = project.id.clone();
        WorkspaceData {
            version: 1,
            projects: vec![project],
            project_order: vec![id],
            service_panel_heights: HashMap::new(),
            hook_panel_heights: HashMap::new(),
            folders: Vec::new(),
            main_window: WindowState::default(),
            extra_windows: Vec::new(),
        }
    }

    #[test]
    fn replacement_kills_stale_hooks_and_runs_project_open_lifecycle() {
        let backend = Arc::new(RecordingBackend {
            next_id: AtomicUsize::new(1),
            killed: Mutex::new(Vec::new()),
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
}
