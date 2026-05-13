use crate::git::watcher::GitStatusWatcher;
use crate::remote::auth::AuthStore;
use crate::remote::bridge;
use crate::remote::pty_broadcaster::PtyBroadcaster;
use crate::remote::server::RemoteServer;
use crate::remote::{GlobalRemoteInfo, RemoteInfo};
use super::observe_project_services;
use crate::services::manager::ServiceManager;
use crate::terminal::backend::TerminalBackend;
use crate::terminal::pty_manager::{PtyEvent, PtyManager};
use crate::views::window::TerminalsRegistry;
use crate::workspace::persistence;
use crate::workspace::state::{GlobalWorkspace, WindowId, Workspace, WorkspaceData};
use async_channel::Receiver;
use gpui::*;
use okena_core::api::ApiGitStatus;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::watch as tokio_watch;

use crate::terminal::backend::LocalBackend;

use super::remote_commands::{remote_command_loop, FocusManagerResolver};

/// Headless application entity — runs workspace, PTY management, and remote
/// server without any GUI windows. Used when running over SSH or on machines
/// without a display server.
pub struct HeadlessApp {
    #[allow(dead_code)]
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
    remote_subscribed_terminals: Arc<std::sync::RwLock<HashMap<u64, HashSet<String>>>>,
    next_remote_connection_id: Arc<AtomicU64>,
    #[allow(dead_code)]
    git_watcher: Entity<GitStatusWatcher>,
    #[allow(dead_code)]
    save_pending: Arc<AtomicBool>,
    #[allow(dead_code)]
    service_manager: Entity<ServiceManager>,
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
                    let save_result =
                        smol::unblock(move || persistence::save_workspace(&data)).await;
                    match save_result {
                        Ok(()) => {
                            last_saved.store(version, Ordering::Relaxed);
                        }
                        Err(e) => {
                            log::error!("Failed to save workspace: {}", e);
                        }
                    }
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
        pty_manager.set_output_sink(pty_broadcaster.clone());
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
        let remote_subscribed_terminals: Arc<std::sync::RwLock<HashMap<u64, HashSet<String>>>> =
            Arc::new(std::sync::RwLock::new(HashMap::new()));
        let next_remote_connection_id = Arc::new(AtomicU64::new(0));
        let git_watcher = cx.new({
            let workspace = workspace.clone();
            let git_status_tx = git_status_tx.clone();
            let remote_subscribed_terminals = remote_subscribed_terminals.clone();
            |cx| GitStatusWatcher::new(workspace, git_status_tx, remote_subscribed_terminals, cx)
        });

        // Create service manager for project-scoped background processes
        let local_backend_for_services: Arc<dyn TerminalBackend> =
            Arc::new(LocalBackend::new(pty_manager.clone()));
        let service_manager = cx.new(|_cx| {
            ServiceManager::new(local_backend_for_services, terminals.clone())
        });

        // Bump state_version on service manager changes
        let sv = state_version.clone();
        cx.observe(&service_manager, move |_this, _sm, _cx| {
            sv.send_modify(|v| *v += 1);
        })
        .detach();

        // Observe workspace to load/unload service configs when projects change
        observe_project_services(&workspace, &service_manager, cx);

        // Observe service manager to sync terminal IDs back to workspace for persistence
        {
            let workspace_for_svc = workspace.clone();
            cx.observe(&service_manager, move |_this, service_manager, cx| {
                let sm = service_manager.read(cx);
                let project_ids: Vec<String> = sm.instances().keys()
                    .map(|(pid, _)| pid.clone())
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .collect();

                let terminal_maps: Vec<(String, HashMap<String, String>)> = project_ids
                    .into_iter()
                    .map(|pid| {
                        let ids = sm.service_terminal_ids(&pid);
                        (pid, ids)
                    })
                    .collect();

                workspace_for_svc.update(cx, |ws, cx| {
                    for (project_id, terminals) in terminal_maps {
                        ws.sync_service_terminals(&project_id, terminals, cx);
                    }
                });
            })
            .detach();
        }

        // Create bridge channel
        let (bridge_tx, bridge_rx) = bridge::bridge_channel();

        let mut app = Self {
            workspace: workspace.clone(),
            pty_manager: pty_manager.clone(),
            terminals: terminals.clone(),
            remote_server: None,
            auth_store: auth_store.clone(),
            pty_broadcaster: pty_broadcaster.clone(),
            state_version: state_version.clone(),
            git_status_tx: git_status_tx.clone(),
            remote_subscribed_terminals: remote_subscribed_terminals.clone(),
            next_remote_connection_id: next_remote_connection_id.clone(),
            git_watcher,
            save_pending,
            service_manager: service_manager.clone(),
        };

        // Start PTY event loop
        app.start_pty_event_loop(pty_events, cx);

        // Start remote command bridge loop (shared with GUI)
        let local_backend: Arc<dyn TerminalBackend> =
            Arc::new(LocalBackend::new(pty_manager));
        // Headless mode has no GUI window. Provide a standalone FocusManager
        // so remote action methods that take `&mut FocusManager` still
        // compile -- in headless the focus state never drives a render so it
        // is effectively dormant. The bridge loop's resolver is constant in
        // headless: there's no focused window to consult, so it always
        // returns the same dormant FocusManager paired with WindowId::Main
        // (per-window data mutations land on the always-present main slot).
        let focus_manager = cx.new(|_| crate::workspace::focus::FocusManager::new());
        let focus_manager_for_resolver = focus_manager.clone();
        let focus_manager_resolver: FocusManagerResolver = Arc::new(move |_cx: &gpui::App| {
            (WindowId::Main, focus_manager_for_resolver.clone())
        });
        cx.spawn({
            let workspace = workspace.clone();
            let terminals = terminals.clone();
            let state_version = state_version.clone();
            let git_status_tx = git_status_tx.clone();
            let service_manager = service_manager.clone();
            async move |_this: WeakEntity<HeadlessApp>, cx: &mut AsyncApp| {
                remote_command_loop(
                    bridge_rx, local_backend, workspace, focus_manager_resolver, terminals,
                    state_version, git_status_tx, service_manager, cx,
                ).await;
            }
        })
        .detach();

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
            self.remote_subscribed_terminals.clone(),
            self.next_remote_connection_id.clone(),
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
    /// Handles service exit events via ServiceManager, matching the GUI version.
    fn start_pty_event_loop(
        &mut self,
        pty_events: Receiver<PtyEvent>,
        cx: &mut Context<Self>,
    ) {
        let terminals = self.terminals.clone();
        let pty_manager = self.pty_manager.clone();
        let service_manager = self.service_manager.clone();
        let state_version = self.state_version.clone();

        cx.spawn(async move |_this: WeakEntity<HeadlessApp>, cx| {
            loop {
                let event = match pty_events.recv().await {
                    Ok(event) => event,
                    Err(_) => break,
                };

                // Collect exit events for service manager processing
                let mut exit_events: Vec<(String, Option<u32>)> = Vec::new();

                // Process first event (broadcasting handled by PtyOutputSink in reader threads)
                match &event {
                    PtyEvent::Data { terminal_id, data } => {
                        let terminals_guard = terminals.lock();
                        if let Some(terminal) = terminals_guard.get(terminal_id) {
                            terminal.process_output(data);
                        }
                    }
                    PtyEvent::Exit { terminal_id, exit_code } => {
                        pty_manager.cleanup_exited(terminal_id);
                        exit_events.push((terminal_id.clone(), *exit_code));
                    }
                }

                // Drain any additional pending events (batch processing)
                while let Ok(event) = pty_events.try_recv() {
                    match &event {
                        PtyEvent::Data { terminal_id, data } => {
                            let terminals_guard = terminals.lock();
                            if let Some(terminal) = terminals_guard.get(terminal_id) {
                                terminal.process_output(data);
                            }
                        }
                        PtyEvent::Exit { terminal_id, exit_code } => {
                            pty_manager.cleanup_exited(terminal_id);
                            exit_events.push((terminal_id.clone(), *exit_code));
                        }
                    }
                }

                if !exit_events.is_empty() {
                    let _ = cx.update(|cx| {
                        // Let service manager handle service terminals
                        let service_tids: HashSet<String> =
                            service_manager.update(cx, |sm, cx| {
                                let mut handled = HashSet::new();
                                for (terminal_id, exit_code) in &exit_events {
                                    if sm.handle_service_exit(terminal_id, *exit_code, cx) {
                                        handled.insert(terminal_id.clone());
                                    }
                                }
                                handled
                            });

                        // Remove UI Terminals for non-service terminals
                        {
                            let mut reg = terminals.lock();
                            for (terminal_id, _) in &exit_events {
                                if !service_tids.contains(terminal_id) {
                                    reg.remove(terminal_id);
                                }
                            }
                        }

                        state_version.send_modify(|v| *v += 1);
                    });
                }
            }
        })
        .detach();
    }
}
