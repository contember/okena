use crate::git::watcher::GitStatusWatcher;
use crate::remote::auth::AuthStore;
use crate::remote::bridge;
use crate::remote::pty_broadcaster::PtyBroadcaster;
use crate::remote::server::RemoteServer;
use crate::remote::{GlobalRemoteInfo, RemoteInfo};
use crate::terminal::backend::TerminalBackend;
use crate::terminal::pty_manager::{PtyEvent, PtyManager};
use crate::views::root::TerminalsRegistry;
use crate::workspace::persistence;
use crate::workspace::state::{GlobalWorkspace, Workspace, WorkspaceData};
use async_channel::Receiver;
use gpui::*;
use okena_core::api::ApiGitStatus;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::watch as tokio_watch;

use crate::remote::bridge::{BridgeMessage, BridgeReceiver, CommandResult, RemoteCommand};
use crate::remote::types::{ApiFolder, ApiFullscreen, ApiProject, StateResponse};
use crate::terminal::backend::LocalBackend;
use crate::workspace::actions::execute::{ensure_terminal, execute_action};

/// Headless application entity — runs workspace, PTY management, and remote
/// server without any GUI windows. Used when running over SSH or on machines
/// without a display server.
pub struct HeadlessApp {
    workspace: Entity<Workspace>,
    #[allow(dead_code)]
    pty_manager: Arc<PtyManager>,
    terminals: TerminalsRegistry,
    #[allow(dead_code)]
    remote_server: Option<RemoteServer>,
    auth_store: Arc<AuthStore>,
    pty_broadcaster: Arc<PtyBroadcaster>,
    state_version: Arc<tokio_watch::Sender<u64>>,
    git_status_tx: Arc<tokio_watch::Sender<HashMap<String, ApiGitStatus>>>,
    #[allow(dead_code)]
    git_watcher: Entity<GitStatusWatcher>,
    #[allow(dead_code)]
    save_pending: Arc<AtomicBool>,
}

impl HeadlessApp {
    pub fn new(
        workspace_data: WorkspaceData,
        pty_manager: Arc<PtyManager>,
        pty_events: Receiver<PtyEvent>,
        listen_addr: IpAddr,
        cx: &mut Context<Self>,
    ) -> Self {
        // Create workspace entity
        let workspace = cx.new(|_cx| Workspace::new(workspace_data));
        cx.set_global(GlobalWorkspace(workspace.clone()));

        // Shared flag for debounced save
        let save_pending = Arc::new(AtomicBool::new(false));
        let last_saved_version = Arc::new(AtomicU64::new(0));

        // Set up debounced auto-save on workspace changes
        let save_pending_for_observer = save_pending.clone();
        let last_saved_version_for_observer = last_saved_version.clone();
        let workspace_for_save = workspace.clone();
        cx.observe(&workspace, move |_this, _workspace, cx| {
            let current_version = _workspace.read(cx).data_version();
            if current_version == last_saved_version_for_observer.load(Ordering::Relaxed) {
                return;
            }

            save_pending_for_observer.store(true, Ordering::Relaxed);

            let save_pending = save_pending_for_observer.clone();
            let last_saved = last_saved_version_for_observer.clone();
            let workspace = workspace_for_save.clone();
            cx.spawn(async move |_, cx| {
                smol::Timer::after(std::time::Duration::from_millis(500)).await;

                if save_pending.swap(false, Ordering::Relaxed) {
                    let (data, version) = cx.update(|cx| {
                        let ws = workspace.read(cx);
                        (ws.data().clone(), ws.data_version())
                    });
                    if let Err(e) = persistence::save_workspace(&data) {
                        log::error!("Failed to save workspace: {}", e);
                    }
                    last_saved.store(version, Ordering::Relaxed);
                }
            })
            .detach();
        })
        .detach();

        // Shared terminals registry
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(HashMap::new()));

        // Remote control setup
        let auth_store = Arc::new(AuthStore::new());
        let pty_broadcaster = Arc::new(PtyBroadcaster::new());
        let (state_version_tx, _) = tokio_watch::channel(0u64);
        let state_version = Arc::new(state_version_tx);
        let remote_info = RemoteInfo::new();
        cx.set_global(GlobalRemoteInfo(remote_info.clone()));

        // Bump state_version on workspace changes
        let sv = state_version.clone();
        cx.observe(&workspace, move |_this, _workspace, _cx| {
            sv.send_modify(|v| *v += 1);
        })
        .detach();

        // Git status watcher
        let (git_status_tx, _) = tokio_watch::channel(HashMap::new());
        let git_status_tx = Arc::new(git_status_tx);
        let git_watcher = cx.new({
            let workspace = workspace.clone();
            let git_status_tx = git_status_tx.clone();
            |cx| GitStatusWatcher::new(workspace, git_status_tx, cx)
        });

        // Create bridge channel
        let (bridge_tx, bridge_rx) = bridge::bridge_channel();

        let mut app = Self {
            workspace,
            pty_manager: pty_manager.clone(),
            terminals,
            remote_server: None,
            auth_store: auth_store.clone(),
            pty_broadcaster: pty_broadcaster.clone(),
            state_version: state_version.clone(),
            git_status_tx: git_status_tx.clone(),
            git_watcher,
            save_pending,
        };

        // Start PTY event loop
        app.start_pty_event_loop(pty_events, cx);

        // Start remote command bridge loop
        let local_backend: Arc<dyn TerminalBackend> =
            Arc::new(LocalBackend::new(pty_manager));
        app.start_remote_command_loop(bridge_rx, local_backend, cx);

        // Start remote server
        app.start_remote_server(bridge_tx, listen_addr, &remote_info);

        app
    }

    /// Start the remote HTTP/WS server.
    fn start_remote_server(
        &mut self,
        bridge_tx: bridge::BridgeSender,
        listen_addr: IpAddr,
        remote_info: &RemoteInfo,
    ) {
        match RemoteServer::start(
            bridge_tx,
            self.auth_store.clone(),
            self.pty_broadcaster.clone(),
            self.state_version.clone(),
            listen_addr,
            self.git_status_tx.clone(),
        ) {
            Ok(server) => {
                let port = server.port();
                remote_info.set_active(port, self.auth_store.clone());
                log::info!("Remote server started on port {}", port);

                let code = self.auth_store.get_or_create_code();
                println!("Remote server listening on port {port}");
                println!("Pairing code: {code} (expires in 60s)");
                println!("Run `okena pair` anytime for a fresh code.");

                self.remote_server = Some(server);
            }
            Err(e) => {
                log::error!("Failed to start remote server: {}", e);
                eprintln!("Failed to start remote server: {e}");
                std::process::exit(1);
            }
        }
    }

    /// PTY event loop — processes terminal data and broadcasts to web clients.
    /// Unlike the GUI version, this does not notify any root view.
    fn start_pty_event_loop(
        &mut self,
        pty_events: Receiver<PtyEvent>,
        cx: &mut Context<Self>,
    ) {
        let terminals = self.terminals.clone();
        let broadcaster = self.pty_broadcaster.clone();

        cx.spawn(async move |_this: WeakEntity<HeadlessApp>, _cx| {
            loop {
                let event = match pty_events.recv().await {
                    Ok(event) => event,
                    Err(_) => break,
                };

                match &event {
                    PtyEvent::Data { terminal_id, data } => {
                        let terminals_guard = terminals.lock();
                        if let Some(terminal) = terminals_guard.get(terminal_id) {
                            terminal.process_output(data);
                        }
                        broadcaster.publish(terminal_id.clone(), data.clone());
                    }
                    PtyEvent::Exit { terminal_id, .. } => {
                        terminals.lock().remove(terminal_id);
                    }
                }

                // Drain pending events (batch processing)
                while let Ok(event) = pty_events.try_recv() {
                    match &event {
                        PtyEvent::Data { terminal_id, data } => {
                            let terminals_guard = terminals.lock();
                            if let Some(terminal) = terminals_guard.get(terminal_id) {
                                terminal.process_output(data);
                            }
                            broadcaster.publish(terminal_id.clone(), data.clone());
                        }
                        PtyEvent::Exit { terminal_id, .. } => {
                            terminals.lock().remove(terminal_id);
                        }
                    }
                }
            }
        })
        .detach();
    }

    /// Process commands from the remote API bridge.
    fn start_remote_command_loop(
        &mut self,
        bridge_rx: BridgeReceiver,
        backend: Arc<dyn TerminalBackend>,
        cx: &mut Context<Self>,
    ) {
        let workspace = self.workspace.clone();
        let terminals = self.terminals.clone();
        let state_version = self.state_version.clone();
        let git_status_tx = self.git_status_tx.clone();

        cx.spawn(async move |_this: WeakEntity<HeadlessApp>, cx| {
            loop {
                let msg: BridgeMessage = match bridge_rx.recv().await {
                    Ok(msg) => msg,
                    Err(_) => break,
                };

                let result = match msg.command {
                    RemoteCommand::Action(action) => {
                        cx.update(|cx| {
                            workspace.update(cx, |ws, cx| {
                                execute_action(action, ws, &*backend, &terminals, cx)
                                    .into_command_result()
                            })
                        })
                    }
                    RemoteCommand::GetState => {
                        cx.update(|cx| {
                            let ws = workspace.read(cx);
                            let sv = *state_version.borrow();
                            let git_statuses = git_status_tx.borrow().clone();
                            let data = ws.data();

                            // Build a lookup map for projects
                            let project_map: std::collections::HashMap<&str, &crate::workspace::state::ProjectData> =
                                data.projects.iter().map(|p| (p.id.as_str(), p)).collect();

                            // Build ordered projects following project_order + folder expansion
                            let mut projects: Vec<ApiProject> = Vec::new();
                            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

                            let build_api_project = |p: &crate::workspace::state::ProjectData| -> ApiProject {
                                let git_status = git_statuses.get(&p.id).cloned();
                                ApiProject {
                                    id: p.id.clone(),
                                    name: p.name.clone(),
                                    path: p.path.clone(),
                                    is_visible: p.is_visible,
                                    layout: p.layout.as_ref().map(|l| l.to_api()),
                                    terminal_names: p.terminal_names.clone(),
                                    git_status,
                                    folder_color: p.folder_color,
                                }
                            };

                            for id in &data.project_order {
                                if let Some(folder) = data.folders.iter().find(|f| &f.id == id) {
                                    for pid in &folder.project_ids {
                                        if seen.insert(pid.clone()) {
                                            if let Some(p) = project_map.get(pid.as_str()) {
                                                projects.push(build_api_project(p));
                                            }
                                        }
                                    }
                                } else if seen.insert(id.clone()) {
                                    if let Some(p) = project_map.get(id.as_str()) {
                                        projects.push(build_api_project(p));
                                    }
                                }
                            }

                            // Append orphan projects not in any order
                            for p in &data.projects {
                                if seen.insert(p.id.clone()) {
                                    projects.push(build_api_project(p));
                                }
                            }

                            // Build folders for response
                            let folders: Vec<ApiFolder> = data.folders.iter().map(|f| {
                                ApiFolder {
                                    id: f.id.clone(),
                                    name: f.name.clone(),
                                    project_ids: f.project_ids.clone(),
                                    folder_color: f.folder_color,
                                }
                            }).collect();

                            let fullscreen = ws.focus_manager.fullscreen_state().map(|(pid, tid)| {
                                ApiFullscreen {
                                    project_id: pid.to_string(),
                                    terminal_id: tid.to_string(),
                                }
                            });

                            let resp = StateResponse {
                                state_version: sv,
                                projects,
                                focused_project_id: ws.focused_project_id().cloned(),
                                fullscreen_terminal: fullscreen,
                                project_order: data.project_order.clone(),
                                folders,
                            };

                            CommandResult::Ok(Some(serde_json::to_value(resp).expect("BUG: StateResponse must serialize")))
                        })
                    }
                    RemoteCommand::GetTerminalSizes { terminal_ids } => {
                        cx.update(|_cx| {
                            let terms = terminals.lock();
                            let mut sizes = std::collections::HashMap::new();
                            for id in &terminal_ids {
                                if let Some(term) = terms.get(id) {
                                    let s = term.resize_state.lock().size;
                                    sizes.insert(id.clone(), (s.cols, s.rows));
                                }
                            }
                            let val = serde_json::to_value(sizes).expect("BUG: sizes must serialize");
                            CommandResult::Ok(Some(val))
                        })
                    }
                    RemoteCommand::RenderSnapshot { terminal_id } => {
                        cx.update(|cx| {
                            let ws = workspace.read(cx);
                            match ensure_terminal(&terminal_id, &terminals, &*backend, ws) {
                                Some(term) => {
                                    let snapshot = term.render_snapshot();
                                    CommandResult::OkBytes(snapshot)
                                }
                                None => CommandResult::Err(format!("terminal not found: {}", terminal_id)),
                            }
                        })
                    }
                };

                let _ = msg.reply.send(result);
            }
        })
        .detach();
    }
}
