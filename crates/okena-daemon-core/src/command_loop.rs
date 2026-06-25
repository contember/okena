//! GPUI-free remote command loop: the headless daemon's faithful port of the
//! GUI's `remote_command_loop` (in `okena-app`'s `app/remote_commands.rs`).
//!
//! The GUI version runs on the GPUI main thread and dispatches each
//! [`RemoteCommand`] into `Entity<Workspace>` / `Entity<ServiceManager>` via
//! `cx.update(|cx| …)` / `entity.read(cx)` / `entity.update(cx, …)`. The daemon
//! has no entity graph: it holds the same state behind
//! `Arc<parking_lot::Mutex<…>>` and drives the identical
//! `okena-app-core` / `okena-services` code paths against the daemon reactor cx
//! types (see [`crate::workspace_cx`] / [`crate::service_cx`]).
//!
//! Each arm reproduces the GUI behavior arm-for-arm:
//!
//! * **Service actions** lock the [`ServiceManager`], mint a
//!   [`DaemonServiceCx`](crate::service_cx::DaemonServiceCx) from the shared
//!   [`ServiceReactorRef`], and call the same method with the same project-path
//!   lookup + "project not found" error as the GUI.
//! * **App-scoped settings/theme** delegate to [`DaemonConfig`] (the GUI's
//!   `remote_config` counterpart).
//! * **Command palette** is unavailable in the daemon (no GUI action registry):
//!   `ListActions` returns an empty list, `InvokeAction` returns an error.
//! * **Workspace-scoped actions** run through
//!   [`execute_action`](okena_app_core::workspace::actions::execute::execute_action)
//!   against [`WindowId::Main`] (the daemon serves a single synthetic main
//!   window, mirroring headless mode).
//! * **`GetState`** builds the [`StateResponse`](okena_core::api::StateResponse)
//!   the same way the GUI does, with the single synthetic `"main"` window.
//!
//! ## Lock discipline
//!
//! Every arm is fully synchronous: it never `.await`s while a state guard is
//! held, so each guard drops at the arm's end before the loop's next
//! `recv().await`. This mirrors the established daemon pattern in
//! [`crate::pty_loop::handle_exits`]. The single `GetState`/service-action arms
//! that touch both the workspace and service-manager locks take the workspace
//! lock first, then the service-manager lock (consistent order), and both drop
//! before looping.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use okena_app_core::workspace::actions::execute::{
    ensure_terminal, execute_action, spawn_uninitialized_terminals,
};
use okena_core::api::{
    ActionRequest, ApiFolder, ApiFullscreen, ApiGitStatus, ApiProject, ApiServiceInfo, ApiWindow,
    ApiWorktreeMetadata, CommandResult, StateResponse,
};
use okena_remote_server::bridge::{BridgeMessage, BridgeReceiver, RemoteCommand};
use okena_services::manager::{ServiceKind, ServiceManager, ServiceStatus};
use okena_terminal::backend::TerminalBackend;
use okena_terminal::TerminalsRegistry;
use okena_workspace::focus::FocusManager;
use okena_workspace::persistence::AppSettings;
use okena_workspace::state::{ProjectData, WindowId, Workspace};
use parking_lot::Mutex;
use tokio::sync::watch;

use crate::daemon_config::{get_settings_schema, DaemonConfig};
use crate::service_cx::ServiceReactorRef;
use crate::workspace_cx::DaemonWorkspaceCx;

/// Parse a wire-format window id into a [`WindowId`].
///
/// GPUI-free copy of the GUI's `remote_commands::parse_window_id`. `"main"`
/// maps to [`WindowId::Main`]; any other string is parsed as a UUID and, on
/// success, wrapped in [`WindowId::Extra`]. A malformed UUID returns `None` so
/// the caller can reject the action with an "invalid window id" error rather
/// than silently routing it to the wrong window.
fn parse_window_id(s: &str) -> Option<WindowId> {
    if s == "main" {
        Some(WindowId::Main)
    } else {
        uuid::Uuid::parse_str(s).ok().map(WindowId::Extra)
    }
}

/// Pure visibility projection for the remote `ApiProject.show_in_overview` wire
/// flag. GPUI-free copy of the GUI's `remote_commands::api_project_visibility`:
/// a project is "shown in overview" iff it is absent from the per-window hidden
/// set (today: `main_window.hidden_project_ids`).
fn api_project_visibility(project_id: &str, hidden: &HashSet<String>) -> bool {
    !hidden.contains(project_id)
}

/// GPUI-free remote command loop for the headless daemon.
///
/// Processes [`RemoteCommand`]s off the [`BridgeReceiver`] until every bridge
/// sender is dropped (server shutdown), replying via each message's `oneshot`
/// when present. The single dormant `FocusManager` is owned by the loop (which
/// is single-threaded), mirroring the GUI's per-window focus-manager but with no
/// view to drive.
// Bridge loop: each param is a distinct channel / shared-state dependency.
#[allow(clippy::too_many_arguments)]
pub async fn daemon_command_loop(
    bridge_rx: BridgeReceiver,
    backend: Arc<dyn TerminalBackend>,
    workspace: Arc<Mutex<Workspace>>,
    workspace_tick: watch::Sender<u64>,
    hook_runner: Option<okena_hooks::HookRunner>,
    hook_monitor: Option<okena_hooks::HookMonitor>,
    terminals: TerminalsRegistry,
    state_version: Arc<watch::Sender<u64>>,
    git_status_tx: Arc<watch::Sender<HashMap<String, ApiGitStatus>>>,
    service_manager: Arc<Mutex<ServiceManager>>,
    service_tick: watch::Sender<u64>,
    runtime: tokio::runtime::Handle,
    settings: Arc<Mutex<AppSettings>>,
    daemon_config: DaemonConfig,
) {
    // Single dormant "main" FocusManager. The loop is single-threaded, so it
    // owns the FM directly instead of resolving a per-window entity like the
    // GUI. Focus state never drives a render here, so it is effectively dormant.
    let mut focus_manager = FocusManager::new();

    // Shared service reactor: built once, `cx()` re-borrowed per service arm.
    // It re-locks `service_manager` internally on reentry, so the loop locks the
    // manager itself only while the cx is alive — never across an await.
    let service_reactor =
        ServiceReactorRef::new(service_manager.clone(), runtime.clone(), service_tick.clone());

    loop {
        let msg: BridgeMessage = match bridge_rx.recv().await {
            Ok(m) => m,
            Err(_) => break,
        };

        let result: CommandResult = match msg.command {
            RemoteCommand::Action(action) => match action {
                // ── Service actions ──────────────────────────────────────────
                ActionRequest::StartService { project_id, service_name } => {
                    let mut sm = service_manager.lock();
                    let mut cx = service_reactor.cx();
                    if let Some(path) = sm.project_path(&project_id).cloned() {
                        sm.start_service(&project_id, &service_name, &path, &mut cx);
                        CommandResult::Ok(None)
                    } else {
                        CommandResult::Err(format!("project not found: {project_id}"))
                    }
                }
                ActionRequest::StopService { project_id, service_name } => {
                    let mut sm = service_manager.lock();
                    let mut cx = service_reactor.cx();
                    sm.stop_service(&project_id, &service_name, &mut cx);
                    CommandResult::Ok(None)
                }
                ActionRequest::RestartService { project_id, service_name } => {
                    let mut sm = service_manager.lock();
                    let mut cx = service_reactor.cx();
                    if let Some(path) = sm.project_path(&project_id).cloned() {
                        sm.restart_service(&project_id, &service_name, &path, &mut cx);
                        CommandResult::Ok(None)
                    } else {
                        CommandResult::Err(format!("project not found: {project_id}"))
                    }
                }
                ActionRequest::StartAllServices { project_id } => {
                    let mut sm = service_manager.lock();
                    let mut cx = service_reactor.cx();
                    if let Some(path) = sm.project_path(&project_id).cloned() {
                        sm.start_all(&project_id, &path, &mut cx);
                        CommandResult::Ok(None)
                    } else {
                        CommandResult::Err(format!("project not found: {project_id}"))
                    }
                }
                ActionRequest::StopAllServices { project_id } => {
                    let mut sm = service_manager.lock();
                    let mut cx = service_reactor.cx();
                    sm.stop_all(&project_id, &mut cx);
                    CommandResult::Ok(None)
                }
                ActionRequest::ReloadServices { project_id } => {
                    let mut sm = service_manager.lock();
                    let mut cx = service_reactor.cx();
                    if let Some(path) = sm.project_path(&project_id).cloned() {
                        sm.reload_project_services(&project_id, &path, &mut cx);
                        CommandResult::Ok(None)
                    } else {
                        CommandResult::Err(format!("project not found: {project_id}"))
                    }
                }

                // ── App-scoped: settings / theme ─────────────────────────────
                ActionRequest::GetSettings => daemon_config.get_settings(),
                ActionRequest::GetSettingsSchema => get_settings_schema(),
                ActionRequest::SetSettings { patch } => daemon_config.set_settings(patch),
                ActionRequest::GetThemes => daemon_config.get_themes(),
                ActionRequest::GetTheme { id } => daemon_config.get_theme(id),
                ActionRequest::SetTheme { id } => daemon_config.set_theme(id),
                ActionRequest::SaveCustomTheme { id, config, activate } => {
                    daemon_config.save_custom_theme(id, config, activate)
                }

                // ── Command palette ──────────────────────────────────────────
                // The daemon has no GUI action registry, so there are no
                // invokable commands to list or dispatch (the agreed parity
                // decision; the GUI's headless mode rejects these too).
                ActionRequest::ListActions => {
                    CommandResult::Ok(Some(serde_json::json!({ "actions": [] })))
                }
                ActionRequest::InvokeAction { .. } => {
                    CommandResult::Err("command palette unavailable in daemon mode".to_string())
                }

                // ── Default: workspace-scoped action ─────────────────────────
                action => {
                    // Resolve the action's explicit target window (if any)
                    // BEFORE moving `action` into `execute_action`. The daemon
                    // serves only the synthetic main window: `None` and
                    // `Some("main")` are accepted; any other (valid) window id is
                    // "not found"; a malformed id is "invalid".
                    let parsed_target = match action.target_window() {
                        None => Ok(None),
                        Some(s) => match parse_window_id(s) {
                            Some(wid) => Ok(Some(wid)),
                            None => Err(s.to_string()),
                        },
                    };
                    match parsed_target {
                        Err(bad) => {
                            // Malformed window id: rejected up front.
                            CommandResult::Err(format!("invalid window id: {bad}"))
                        }
                        Ok(None) | Ok(Some(WindowId::Main)) => {
                            // Snapshot app settings to thread into the gpui-free
                            // `execute_action` (hooks / worktree template /
                            // default shell). Read before locking the workspace.
                            let app_settings = settings.lock().clone();
                            let mut cx = DaemonWorkspaceCx::new(
                                &workspace_tick,
                                &hook_runner,
                                &hook_monitor,
                            );
                            let mut ws = workspace.lock();
                            // The daemon always targets `WindowId::Main`. The
                            // mutators notify via `cx` themselves, so there is no
                            // separate `cx.notify()` like the GUI's view-refresh.
                            execute_action(
                                action,
                                &mut ws,
                                WindowId::Main,
                                &mut focus_manager,
                                &*backend,
                                &terminals,
                                &app_settings,
                                &mut cx,
                            )
                            .into_command_result()
                        }
                        Ok(Some(WindowId::Extra(uuid))) => {
                            // The daemon has only the synthetic main window.
                            CommandResult::Err(format!("window not found: {uuid}"))
                        }
                    }
                }
            },

            // ── GetState: full workspace snapshot ────────────────────────────
            RemoteCommand::GetState => {
                // Lock order: workspace first, then service manager (kept
                // consistent across the loop). The whole arm is synchronous, so
                // both guards drop before the next `recv().await`.
                let ws = workspace.lock();
                let sm = service_manager.lock();
                let sv = *state_version.borrow();
                let git_statuses = git_status_tx.borrow().clone();
                let data = ws.data();

                // Build terminal size map from the registry.
                let size_map: HashMap<String, (u16, u16)> = {
                    let registry = terminals.lock();
                    registry
                        .iter()
                        .map(|(id, term)| {
                            let size = term.resize_state.lock().size;
                            (id.clone(), (size.cols, size.rows))
                        })
                        .collect()
                };

                // Lookup map for projects.
                let project_map: HashMap<&str, &ProjectData> =
                    data.projects.iter().map(|p| (p.id.as_str(), p)).collect();

                // Source of truth for runtime visibility (per-window viewport).
                let hidden_project_ids = &data.main_window.hidden_project_ids;

                // Build ordered projects following project_order + folder
                // expansion.
                let mut projects: Vec<ApiProject> = Vec::new();
                let mut seen: HashSet<String> = HashSet::new();

                let build_api_project = |p: &ProjectData| -> ApiProject {
                    let git_status = git_statuses.get(&p.id).cloned();
                    let services: Vec<ApiServiceInfo> = sm
                        .services_for_project(&p.id)
                        .into_iter()
                        .map(|inst| {
                            let (status, exit_code) = match &inst.status {
                                ServiceStatus::Stopped => ("stopped", None),
                                ServiceStatus::Starting => ("starting", None),
                                ServiceStatus::Running => ("running", None),
                                ServiceStatus::Crashed { exit_code } => ("crashed", *exit_code),
                                ServiceStatus::Restarting => ("restarting", None),
                            };
                            let kind = match &inst.kind {
                                ServiceKind::Okena => "okena",
                                ServiceKind::DockerCompose { .. } => "docker_compose",
                            };
                            ApiServiceInfo {
                                name: inst.definition.name.clone(),
                                status: status.to_string(),
                                terminal_id: inst.terminal_id.clone(),
                                ports: inst.detected_ports.clone(),
                                exit_code,
                                kind: kind.to_string(),
                                is_extra: inst.is_extra,
                            }
                        })
                        .collect();
                    ApiProject {
                        id: p.id.clone(),
                        name: p.name.clone(),
                        path: p.path.clone(),
                        show_in_overview: api_project_visibility(&p.id, hidden_project_ids),
                        layout: p.layout.as_ref().map(|l| l.to_api_with_sizes(&size_map)),
                        terminal_names: p.terminal_names.clone(),
                        git_status,
                        folder_color: p.folder_color,
                        services,
                        worktree_info: p.worktree_info.as_ref().map(|wt| ApiWorktreeMetadata {
                            parent_project_id: wt.parent_project_id.clone(),
                            color_override: wt.color_override,
                        }),
                        worktree_ids: p.worktree_ids.clone(),
                    }
                };

                for id in &data.project_order {
                    if let Some(folder) = data.folders.iter().find(|f| &f.id == id) {
                        for pid in &folder.project_ids {
                            if seen.insert(pid.clone())
                                && let Some(p) = project_map.get(pid.as_str())
                            {
                                projects.push(build_api_project(p));
                            }
                        }
                    } else if seen.insert(id.clone())
                        && let Some(p) = project_map.get(id.as_str())
                    {
                        projects.push(build_api_project(p));
                    }
                }

                // Append orphan projects not in any order.
                for p in &data.projects {
                    if seen.insert(p.id.clone()) {
                        projects.push(build_api_project(p));
                    }
                }

                // Build folders for response.
                let folders: Vec<ApiFolder> = data
                    .folders
                    .iter()
                    .map(|f| ApiFolder {
                        id: f.id.clone(),
                        name: f.name.clone(),
                        project_ids: f.project_ids.clone(),
                        folder_color: f.folder_color,
                    })
                    .collect();

                // The daemon serves a SINGLE synthetic main window (ported from
                // headless.rs's windows_resolver). No GUI, so it's always
                // "active", has no per-window focus/fullscreen/bounds, and no
                // hidden set — every project in `project_order` is visible.
                let visible_project_ids: Vec<String> = ws
                    .visible_projects(WindowId::Main, None, false)
                    .iter()
                    .map(|p| p.id.clone())
                    .collect();
                let windows = vec![ApiWindow {
                    id: "main".into(),
                    kind: "main".into(),
                    active: true,
                    focused_project_id: None,
                    focused_terminal_id: None,
                    fullscreen: None,
                    visible_project_ids,
                    folder_filter: None,
                    bounds: None,
                    sidebar_open: None,
                }];

                // Back-compat flat fields derived from the active window (both
                // None here — the synthetic main window has no focus/fullscreen).
                let focused_project_id: Option<String> = windows
                    .iter()
                    .find(|w| w.active)
                    .and_then(|w| w.focused_project_id.clone());
                let fullscreen: Option<ApiFullscreen> = windows
                    .iter()
                    .find(|w| w.active)
                    .and_then(|w| w.fullscreen.clone());

                let resp = StateResponse {
                    state_version: sv,
                    projects,
                    focused_project_id,
                    fullscreen_terminal: fullscreen,
                    project_order: data.project_order.clone(),
                    folders,
                    windows,
                };

                // `match` (not `.expect`) so the daemon-core crate stays clean
                // under `clippy::expect_used` had it been enabled — the serialize
                // is unreachable-fail for a well-formed DTO.
                match serde_json::to_value(resp) {
                    Ok(v) => CommandResult::Ok(Some(v)),
                    Err(e) => CommandResult::Err(format!("failed to serialize state: {e}")),
                }
            }

            // ── GetTerminalSizes ─────────────────────────────────────────────
            RemoteCommand::GetTerminalSizes { terminal_ids } => {
                let terms = terminals.lock();
                let mut sizes: HashMap<String, (u16, u16)> = HashMap::new();
                for id in &terminal_ids {
                    if let Some(term) = terms.get(id) {
                        let s = term.resize_state.lock().size;
                        sizes.insert(id.clone(), (s.cols, s.rows));
                    }
                }
                match serde_json::to_value(sizes) {
                    Ok(v) => CommandResult::Ok(Some(v)),
                    Err(e) => CommandResult::Err(format!("failed to serialize sizes: {e}")),
                }
            }

            // ── RenderSnapshot ───────────────────────────────────────────────
            RemoteCommand::RenderSnapshot { terminal_id } => {
                let ws = workspace.lock();
                match ensure_terminal(&terminal_id, &terminals, &*backend, &ws) {
                    Some(term) => CommandResult::OkBytes(term.render_snapshot()),
                    None => CommandResult::Err(format!("terminal not found: {terminal_id}")),
                }
            }

            // ── PasteImage ───────────────────────────────────────────────────
            RemoteCommand::PasteImage { terminal_id, path } => {
                let ws = workspace.lock();
                match ensure_terminal(&terminal_id, &terminals, &*backend, &ws) {
                    Some(term) => {
                        // Bracketed paste of the server-local image path — same as
                        // a local image paste, so the focused TUI's own paste
                        // handler attaches it.
                        term.send_paste(&path);
                        CommandResult::Ok(Some(serde_json::json!({ "path": path })))
                    }
                    None => CommandResult::Err(format!("terminal not found: {terminal_id}")),
                }
            }
        };

        if let Some(reply) = msg.reply {
            let _ = reply.send(result);
        }
    }
}

/// Materialize the PTYs for every restored project's uninitialized terminal
/// slots at daemon startup.
///
/// Persisted `workspace.json` layouts carry terminal slots with
/// `terminal_id: None` (the normal saved state). In daemon-client mode nobody
/// ever materializes them: the GUI client cannot self-spawn over a remote
/// backend, and the daemon only calls
/// [`spawn_uninitialized_terminals`](okena_app_core::workspace::actions::execute::spawn_uninitialized_terminals)
/// from the `CreateTerminal` / `SplitTerminal` / `AddProject` action arms — not
/// on boot. A restored slot therefore never gets a PTY and renders blank
/// forever.
///
/// This is the daemon's boot-time analogue of the GUI's
/// `spawn_terminals_for_project` (fired on project creation): it walks EVERY
/// loaded project and assigns ids + creates PTYs for its uninitialized slots,
/// so `/v1/state` serves real ids and the snapshot/live-PTY path works.
///
/// All projects (not just the visible ones): the prior in-process GUI eagerly
/// spawned terminals when a project column was created, regardless of overview
/// visibility, and `hidden_project_ids` is a per-window viewport concern, not a
/// "don't run this project" signal. Spawning everything keeps behavior simple
/// and correct; project counts are small (one column per project), so this is
/// not too heavy.
///
/// Runs on the LocalSet thread (mirroring the command loop's `execute_action`):
/// PTY spawning and hook execution may reach the reactor, and the
/// `WorkspaceCx::notify` bumps the `workspace_tick` whose observer task bumps
/// `state_version`. The freshly-assigned ids bump `data_version`, so the
/// existing autosave observer persists them — this introduces NO second writer.
///
/// Must be invoked AFTER `spawn_observers` (so the tick reaches them) and BEFORE
/// the command loop starts serving clients (so `/v1/state` never exposes the
/// transient null slots).
pub fn materialize_uninitialized_terminals(
    backend: &dyn TerminalBackend,
    workspace: &Arc<Mutex<Workspace>>,
    workspace_tick: &watch::Sender<u64>,
    hook_runner: &Option<okena_hooks::HookRunner>,
    hook_monitor: &Option<okena_hooks::HookMonitor>,
    terminals: &TerminalsRegistry,
    settings: &Arc<Mutex<AppSettings>>,
) {
    // Snapshot the project ids under a short lock, then drop it before spawning
    // (each `spawn_uninitialized_terminals` call re-locks the workspace itself).
    let project_ids: Vec<String> = {
        let ws = workspace.lock();
        ws.data().projects.iter().map(|p| p.id.clone()).collect()
    };

    // Snapshot settings once, mirroring the command loop's `execute_action` arm.
    let app_settings = settings.lock().clone();

    for project_id in project_ids {
        let mut cx = DaemonWorkspaceCx::new(workspace_tick, hook_runner, hook_monitor);
        let mut ws = workspace.lock();
        match spawn_uninitialized_terminals(
            &mut ws,
            &project_id,
            backend,
            terminals,
            &app_settings,
            &mut cx,
        ) {
            okena_app_core::workspace::actions::execute::ActionResult::Err(e) => {
                log::error!(
                    "startup: failed to materialize terminals for project {project_id}: {e}"
                );
            }
            okena_app_core::workspace::actions::execute::ActionResult::Ok(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use okena_remote_server::bridge::bridge_channel;
    use okena_state::WorkspaceData;
    use okena_terminal::backend::TerminalBackend;
    use okena_terminal::shell_config::ShellType;
    use okena_terminal::terminal::TerminalTransport;
    use tokio::sync::oneshot;

    // ── Stub backend (copied from observers.rs tests) ────────────────────────

    /// No-op transport for the test backend.
    struct StubTransport;

    impl TerminalTransport for StubTransport {
        fn send_input(&self, _terminal_id: &str, _data: &[u8]) {}
        fn resize(&self, _terminal_id: &str, _cols: u16, _rows: u16) {}
        fn uses_mouse_backend(&self) -> bool {
            false
        }
    }

    /// Minimal `TerminalBackend` for constructing a `ServiceManager` /
    /// `execute_action` in tests. The exercised paths (no service config, no PTY
    /// spawn) never reach terminal creation, so these are no-ops / errors.
    struct StubBackend;

    impl TerminalBackend for StubBackend {
        fn transport(&self) -> Arc<dyn TerminalTransport> {
            Arc::new(StubTransport)
        }
        fn create_terminal(&self, _cwd: &str, _shell: Option<&ShellType>) -> anyhow::Result<String> {
            anyhow::bail!("stub backend: create_terminal not supported")
        }
        fn reconnect_terminal(
            &self,
            _terminal_id: &str,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            anyhow::bail!("stub backend: reconnect_terminal not supported")
        }
        fn kill(&self, _terminal_id: &str) {}
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

    /// An empty `WorkspaceData` (no `Default` impl on the type itself).
    fn empty_workspace_data() -> WorkspaceData {
        WorkspaceData {
            version: 1,
            projects: Vec::new(),
            project_order: Vec::new(),
            folders: Vec::new(),
            service_panel_heights: Default::default(),
            hook_panel_heights: Default::default(),
            main_window: Default::default(),
            extra_windows: Vec::new(),
        }
    }

    /// Default `AppSettings` (every field has a serde default).
    fn default_settings() -> AppSettings {
        serde_json::from_value::<AppSettings>(serde_json::json!({})).expect("defaults")
    }

    /// Bundle of the shared state + channels the loop needs, so each test can
    /// spawn the loop and keep handles to inspect afterwards.
    struct Harness {
        workspace: Arc<Mutex<Workspace>>,
        backend: Arc<dyn TerminalBackend>,
        workspace_tick: watch::Sender<u64>,
        terminals: TerminalsRegistry,
        state_version: Arc<watch::Sender<u64>>,
        git_status_tx: Arc<watch::Sender<HashMap<String, ApiGitStatus>>>,
        service_manager: Arc<Mutex<ServiceManager>>,
        service_tick: watch::Sender<u64>,
        settings: Arc<Mutex<AppSettings>>,
        daemon_config: DaemonConfig,
    }

    fn harness() -> Harness {
        let workspace = Arc::new(Mutex::new(Workspace::new(empty_workspace_data())));
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
        let backend: Arc<dyn TerminalBackend> = Arc::new(StubBackend);
        let service_manager = Arc::new(Mutex::new(ServiceManager::new(
            backend.clone(),
            terminals.clone(),
        )));
        let settings = Arc::new(Mutex::new(default_settings()));
        let daemon_config = DaemonConfig::new(settings.clone());
        let (workspace_tick, _wtrx) = watch::channel(0u64);
        let (service_tick, _strx) = watch::channel(0u64);
        let (state_version, _svrx) = watch::channel(0u64);
        let (git_status_tx, _gsrx) = watch::channel(HashMap::new());
        Harness {
            workspace,
            backend,
            workspace_tick,
            terminals,
            state_version: Arc::new(state_version),
            git_status_tx: Arc::new(git_status_tx),
            service_manager,
            service_tick,
            settings,
            daemon_config,
        }
    }

    // ── Pure unit tests ──────────────────────────────────────────────────────

    #[test]
    fn parse_window_id_main_maps_to_main() {
        assert_eq!(parse_window_id("main"), Some(WindowId::Main));
    }

    #[test]
    fn parse_window_id_valid_uuid_maps_to_extra() {
        let id = uuid::Uuid::new_v4();
        assert_eq!(parse_window_id(&id.to_string()), Some(WindowId::Extra(id)));
    }

    #[test]
    fn parse_window_id_garbage_returns_none() {
        assert_eq!(parse_window_id("garbage"), None);
        assert_eq!(parse_window_id(""), None);
        // A near-miss UUID (one char short) is still rejected.
        assert_eq!(parse_window_id("550e8400-e29b-41d4-a716-44665544000"), None);
    }

    #[test]
    fn api_project_visibility_reads_from_hidden_set() {
        let hidden: HashSet<String> = ["p1".to_string()].into_iter().collect();
        assert!(!api_project_visibility("p1", &hidden));
        assert!(api_project_visibility("p2", &hidden));
    }

    #[test]
    fn api_project_visibility_empty_hidden_set_is_visible() {
        let hidden: HashSet<String> = HashSet::new();
        assert!(api_project_visibility("p1", &hidden));
    }

    // ── Loop round-trip tests ─────────────────────────────────────────────────

    /// `GetState` returns `Ok(Some(v))` that deserializes into a `StateResponse`
    /// with the single synthetic `"main"` window.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn get_state_round_trip() {
        let h = harness();
        let (bridge_tx, bridge_rx) = bridge_channel();

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                let handle = tokio::task::spawn_local(daemon_command_loop(
                    bridge_rx,
                    h.backend,
                    h.workspace,
                    h.workspace_tick,
                    None,
                    None,
                    h.terminals,
                    h.state_version,
                    h.git_status_tx,
                    h.service_manager,
                    h.service_tick,
                    tokio::runtime::Handle::current(),
                    h.settings,
                    h.daemon_config,
                ));

                let (reply_tx, reply_rx) = oneshot::channel();
                bridge_tx
                    .send(BridgeMessage {
                        command: RemoteCommand::GetState,
                        reply: Some(reply_tx),
                    })
                    .await
                    .expect("send GetState");

                let result = reply_rx.await.expect("GetState reply");
                let value = match result {
                    CommandResult::Ok(Some(v)) => v,
                    other => panic!("expected Ok(Some), got {other:?}"),
                };
                let resp: StateResponse =
                    serde_json::from_value(value).expect("deserialize StateResponse");
                assert_eq!(resp.windows.len(), 1, "single synthetic window");
                assert_eq!(resp.windows[0].id, "main");
                assert_eq!(resp.windows[0].kind, "main");
                assert!(resp.windows[0].active);

                // Drop the sender so `recv` errors and the loop task joins.
                drop(bridge_tx);
                handle.await.expect("loop task joins");
            })
            .await;
    }

    /// App-scoped `GetSettingsSchema` returns `Ok(Some(_))` with settings keys.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn get_settings_schema_round_trip() {
        let h = harness();
        let (bridge_tx, bridge_rx) = bridge_channel();

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                let handle = tokio::task::spawn_local(daemon_command_loop(
                    bridge_rx,
                    h.backend,
                    h.workspace,
                    h.workspace_tick,
                    None,
                    None,
                    h.terminals,
                    h.state_version,
                    h.git_status_tx,
                    h.service_manager,
                    h.service_tick,
                    tokio::runtime::Handle::current(),
                    h.settings,
                    h.daemon_config,
                ));

                let (reply_tx, reply_rx) = oneshot::channel();
                bridge_tx
                    .send(BridgeMessage {
                        command: RemoteCommand::Action(ActionRequest::GetSettingsSchema),
                        reply: Some(reply_tx),
                    })
                    .await
                    .expect("send GetSettingsSchema");

                let result = reply_rx.await.expect("schema reply");
                match result {
                    CommandResult::Ok(Some(v)) => {
                        let obj = v.as_object().expect("schema is an object");
                        assert!(obj.contains_key("font_size"));
                        assert!(obj.contains_key("theme_mode"));
                    }
                    other => panic!("expected Ok(Some), got {other:?}"),
                }

                drop(bridge_tx);
                handle.await.expect("loop task joins");
            })
            .await;
    }

    /// A workspace-scoped action (`CreateFolder`) returns `Ok(_)` and mutates the
    /// shared workspace.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn create_folder_action_mutates_workspace() {
        let h = harness();
        let workspace_for_assert = h.workspace.clone();
        let (bridge_tx, bridge_rx) = bridge_channel();

        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                let handle = tokio::task::spawn_local(daemon_command_loop(
                    bridge_rx,
                    h.backend,
                    h.workspace,
                    h.workspace_tick,
                    None,
                    None,
                    h.terminals,
                    h.state_version,
                    h.git_status_tx,
                    h.service_manager,
                    h.service_tick,
                    tokio::runtime::Handle::current(),
                    h.settings,
                    h.daemon_config,
                ));

                let (reply_tx, reply_rx) = oneshot::channel();
                bridge_tx
                    .send(BridgeMessage {
                        command: RemoteCommand::Action(ActionRequest::CreateFolder {
                            name: "f".into(),
                        }),
                        reply: Some(reply_tx),
                    })
                    .await
                    .expect("send CreateFolder");

                let result = reply_rx.await.expect("CreateFolder reply");
                assert!(
                    matches!(result, CommandResult::Ok(_)),
                    "expected Ok, got {result:?}"
                );

                // The shared workspace now has the folder.
                {
                    let ws = workspace_for_assert.lock();
                    assert_eq!(ws.data().folders.len(), 1, "folder was created");
                    assert_eq!(ws.data().folders[0].name, "f");
                }

                drop(bridge_tx);
                handle.await.expect("loop task joins");
            })
            .await;
    }

    // ── Startup terminal materialization ──────────────────────────────────────

    /// A `WorkspaceData` carrying one project whose layout is a single
    /// uninitialized terminal slot (`terminal_id: None`) — the normal persisted
    /// state for a restored project. `path` is the project cwd the PTY spawns in.
    fn workspace_with_uninitialized_terminal(path: &str) -> WorkspaceData {
        use okena_state::{LayoutNode, ProjectData};
        let project = ProjectData {
            id: "p1".to_string(),
            name: "Project p1".to_string(),
            path: path.to_string(),
            layout: Some(LayoutNode::Terminal {
                terminal_id: None,
                minimized: false,
                detached: false,
                shell_type: ShellType::Default,
                zoom_level: 1.0,
            }),
            terminal_names: Default::default(),
            hidden_terminals: Default::default(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: Default::default(),
            hooks: Default::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: Default::default(),
            default_shell: None,
            hook_terminals: Default::default(),
            pinned: false,
            last_activity_at: None,
        };
        WorkspaceData {
            version: 1,
            projects: vec![project],
            project_order: vec!["p1".to_string()],
            folders: Vec::new(),
            service_panel_heights: Default::default(),
            hook_panel_heights: Default::default(),
            main_window: Default::default(),
            extra_windows: Vec::new(),
        }
    }

    /// `materialize_uninitialized_terminals` assigns a real `terminal_id` to a
    /// restored `terminal_id: None` slot, creates the backing PTY (so it lands
    /// in the registry), bumps `data_version` (so the autosave observer persists
    /// the assigned id) and the `workspace_tick` (so the state-version observer
    /// fires). This is the boot fix for blank restored terminals in
    /// daemon-client mode.
    #[test]
    fn materialize_assigns_ids_and_spawns_ptys_for_restored_projects() {
        use okena_terminal::backend::LocalBackend;
        use okena_terminal::pty_manager::PtyManager;
        use okena_terminal::session_backend::SessionBackend;
        use okena_workspace::state::LayoutNode;

        // A real, existing cwd for the spawned shell.
        let tmp = std::env::temp_dir();
        let tmp_path = tmp.to_str().expect("temp dir is utf-8");

        let (pty_manager, _pty_events) = PtyManager::new(SessionBackend::None);
        let pty_manager = Arc::new(pty_manager);
        let backend: Arc<dyn TerminalBackend> = Arc::new(LocalBackend::new(pty_manager.clone()));
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));

        let workspace = Arc::new(Mutex::new(Workspace::new(
            workspace_with_uninitialized_terminal(tmp_path),
        )));
        let settings = Arc::new(Mutex::new(default_settings()));
        let (workspace_tick, _wtrx) = watch::channel(0u64);

        // Preconditions: slot is uninitialized, registry empty, tick at 0.
        let version_before = workspace.lock().data_version();
        let tick_before = *workspace_tick.borrow();
        assert!(terminals.lock().is_empty(), "registry starts empty");

        materialize_uninitialized_terminals(
            &*backend,
            &workspace,
            &workspace_tick,
            &None,
            &None,
            &terminals,
            &settings,
        );

        // The slot now has an id, the PTY is in the registry, and both the
        // persistent data_version and the notify tick advanced.
        let ws = workspace.lock();
        let project = ws.project("p1").expect("project p1 exists");
        let assigned = match project.layout.as_ref().expect("layout present") {
            LayoutNode::Terminal { terminal_id, .. } => terminal_id.clone(),
            other => panic!("expected a Terminal layout node, got {other:?}"),
        };
        let assigned = assigned.expect("terminal slot got a real id");
        assert!(
            terminals.lock().contains_key(&assigned),
            "spawned PTY is registered under the assigned id"
        );
        assert!(
            ws.data_version() > version_before,
            "data_version advanced so autosave persists the assigned id"
        );
        assert!(
            *workspace_tick.borrow() > tick_before,
            "workspace_tick advanced so the state-version observer fires"
        );
    }

    /// On an empty workspace `materialize_uninitialized_terminals` is a no-op:
    /// no terminals spawned and the data_version is untouched.
    #[test]
    fn materialize_is_noop_for_empty_workspace() {
        let workspace = Arc::new(Mutex::new(Workspace::new(empty_workspace_data())));
        let backend: Arc<dyn TerminalBackend> = Arc::new(StubBackend);
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(Default::default()));
        let settings = Arc::new(Mutex::new(default_settings()));
        let (workspace_tick, _wtrx) = watch::channel(0u64);

        let version_before = workspace.lock().data_version();
        materialize_uninitialized_terminals(
            &*backend,
            &workspace,
            &workspace_tick,
            &None,
            &None,
            &terminals,
            &settings,
        );

        assert!(terminals.lock().is_empty(), "no terminals spawned");
        assert_eq!(
            workspace.lock().data_version(),
            version_before,
            "data_version untouched on empty workspace"
        );
    }
}
