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

use super::{ActionResult, ensure_terminal, spawn_uninitialized_terminals};
use crate::workspace::focus::FocusManager;
use crate::workspace::persistence::AppSettings;
use crate::workspace::persistence::{
    delete_session, export_workspace, import_workspace, list_sessions, load_session_with_cleanup,
    rename_session, save_session, session_exists,
};
use crate::workspace::state::{Workspace, WorkspaceData};
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::TerminalBackend;
use okena_workspace::context::WorkspaceCx;
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

/// Kill outgoing-only PTYs, swap the workspace, then restore incoming terminals.
fn replace_workspace_with(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    data: WorkspaceData,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    stale_terminal_ids: &[String],
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    if let Some(project) = ws
        .projects()
        .iter()
        .find(|project| ws.is_creating_project(&project.id))
    {
        return ActionResult::Err(format!(
            "cannot replace workspace while worktree '{}' is being created",
            project.name
        ));
    }
    if let Some(project) = ws
        .projects()
        .iter()
        .find(|project| ws.is_project_closing(&project.id))
    {
        return ActionResult::Err(format!(
            "cannot replace workspace while worktree '{}' is closing",
            project.name
        ));
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
    ids_to_kill.extend(stale_terminal_ids.iter().cloned());
    // Incoming hooks are never reconnected; load clears them and fires fresh
    // project-open lifecycle hooks instead.
    ids_to_kill.extend(
        data.projects
            .iter()
            .flat_map(|project| project.hook_terminals.keys().cloned()),
    );
    ids_to_kill.retain(|id| !incoming_persistent_ids.contains(id));
    let mut ids: Vec<String> = ids_to_kill.into_iter().collect();
    ids.sort();
    if let Some(monitor) = cx.hook_monitor() {
        for id in &outgoing_hook_ids {
            monitor.finish_by_terminal_id(id, None);
            monitor.notify_exit(id, None);
        }
    }
    for id in &ids {
        backend.kill(id);
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

pub(super) fn load_session_action(
    ws: &mut Workspace,
    focus_manager: &mut FocusManager,
    name: String,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    settings: &AppSettings,
    cx: &mut impl WorkspaceCx,
) -> ActionResult {
    let loaded = match load_session_with_cleanup(&name, settings.session_backend) {
        Ok(loaded) => loaded,
        Err(e) => return ActionResult::Err(format!("failed to load session '{name}': {e}")),
    };
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
            &["discarded-stale".to_string()],
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
    fn replacement_finishes_outgoing_hook_monitor_before_dropping_ownership() {
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
        assert!(monitor.history().iter().all(|execution| !matches!(
            execution.status,
            crate::workspace::hook_monitor::HookStatus::Running
        )));
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
}
