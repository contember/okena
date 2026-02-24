mod detached_terminals;
pub mod headless;
mod remote_commands;
mod update_checker;

use crate::git::watcher::GitStatusWatcher;
use crate::remote::auth::AuthStore;
use crate::remote::bridge;
use crate::remote::pty_broadcaster::PtyBroadcaster;
use crate::remote::server::RemoteServer;
use crate::remote::{GlobalRemoteInfo, RemoteInfo};
use crate::remote_client::manager::RemoteConnectionManager;
use crate::services::manager::ServiceManager;
use crate::settings::GlobalSettings;
use crate::views::panels::toast::ToastManager;
use crate::updater::{GlobalUpdateInfo, UpdateInfo};
use crate::terminal::pty_manager::{PtyEvent, PtyManager};
use crate::views::root::{RootView, TerminalsRegistry};
use crate::workspace::persistence;
use crate::workspace::request_broker::RequestBroker;
use crate::workspace::state::{GlobalWorkspace, Workspace, WorkspaceData};
use async_channel::Receiver;
use gpui::*;
use okena_core::api::ApiGitStatus;
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::watch as tokio_watch;

/// Main application state and view
pub struct Okena {
    root_view: Entity<RootView>,
    pub(crate) workspace: Entity<Workspace>,
    #[allow(dead_code)]
    request_broker: Entity<RequestBroker>,
    pub(crate) pty_manager: Arc<PtyManager>,
    pub(crate) terminals: TerminalsRegistry,
    /// Track which detached windows we've already opened
    pub(crate) opened_detached_windows: HashSet<String>,
    /// Flag indicating workspace needs to be saved (for debouncing)
    /// Note: Field is read by spawned tasks, not directly
    #[allow(dead_code)]
    save_pending: Arc<AtomicBool>,
    // ── Git status watcher ────────────────────────────────────────────
    #[allow(dead_code)]
    git_watcher: Entity<GitStatusWatcher>,
    git_status_tx: Arc<tokio_watch::Sender<HashMap<String, ApiGitStatus>>>,
    // ── Remote control fields ───────────────────────────────────────────
    remote_server: Option<RemoteServer>,
    pub auth_store: Arc<AuthStore>,
    pub(crate) pty_broadcaster: Arc<PtyBroadcaster>,
    pub(crate) state_version: Arc<tokio_watch::Sender<u64>>,
    remote_info: RemoteInfo,
    listen_addr: IpAddr,
    /// Whether the listen address was forced via CLI --listen flag
    force_remote: bool,
    /// Service manager for project-scoped background processes
    service_manager: Entity<ServiceManager>,
}

impl Okena {
    pub fn new(
        workspace_data: WorkspaceData,
        pty_manager: Arc<PtyManager>,
        pty_events: Receiver<PtyEvent>,
        listen_addr: Option<IpAddr>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let force_remote = listen_addr.is_some();
        let listen_addr = listen_addr.unwrap_or_else(|| {
            cx.global::<GlobalSettings>().0.read(cx).get()
                .remote_listen_address.parse::<IpAddr>()
                .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
        });
        // Create workspace entity
        let workspace = cx.new(|_cx| Workspace::new(workspace_data));
        cx.set_global(GlobalWorkspace(workspace.clone()));

        // Create request broker entity (decoupled UI request routing)
        let request_broker = cx.new(|_| RequestBroker::new());

        // Shared flag for debounced save
        let save_pending = Arc::new(AtomicBool::new(false));
        // Track last saved data_version to skip saves for UI-only changes
        let last_saved_version = Arc::new(AtomicU64::new(0));

        // Set up debounced auto-save on workspace changes
        let save_pending_for_observer = save_pending.clone();
        let last_saved_version_for_observer = last_saved_version.clone();
        let workspace_for_save = workspace.clone();
        cx.observe(&workspace, move |_this, _workspace, cx| {
            // Check if persistent data actually changed
            let current_version = _workspace.read(cx).data_version();
            if current_version == last_saved_version_for_observer.load(Ordering::Relaxed) {
                return; // UI-only change, skip save
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
                    match persistence::save_workspace(&data) {
                        Ok(()) => {
                            last_saved.store(version, Ordering::Relaxed);
                        }
                        Err(e) => {
                            log::error!("Failed to save workspace: {}", e);
                            let _ = cx.update(|cx| {
                                ToastManager::error(format!("Failed to save workspace: {}", e), cx);
                            });
                            // Don't update last_saved — next mutation will retry the save
                        }
                    }
                }
            }).detach();
        })
        .detach();

        // Create root view (get terminals registry from it)
        let pty_manager_clone = pty_manager.clone();
        let request_broker_clone = request_broker.clone();
        let root_view = cx.new(|cx| {
            RootView::new(workspace.clone(), request_broker_clone, pty_manager_clone, cx)
        });

        // Get terminals registry from root view
        let terminals = root_view.read(cx).terminals().clone();

        // Create service manager for project-scoped background processes
        let local_backend_for_services: Arc<dyn crate::terminal::backend::TerminalBackend> =
            Arc::new(crate::terminal::backend::LocalBackend::new(pty_manager.clone()));
        let service_manager = cx.new(|_cx| {
            ServiceManager::new(local_backend_for_services, terminals.clone())
        });
        root_view.update(cx, |rv, cx| {
            rv.set_service_manager(service_manager.clone(), cx);
        });

        // Create remote connection manager and wire to root view
        let remote_manager = cx.new(|cx| {
            RemoteConnectionManager::new(terminals.clone(), cx)
        });
        root_view.update(cx, |rv, cx| {
            rv.set_remote_manager(remote_manager.clone(), cx);
        });
        // Auto-connect to saved connections with valid tokens
        remote_manager.update(cx, |rm, cx| {
            rm.auto_connect_all(cx);
            rm.start_token_refresh_task(cx);
        });

        // Observe window bounds changes to force re-render
        cx.observe_window_bounds(window, |_this, _window, cx| {
            cx.notify();
        })
        .detach();

        // ── Git status watcher ─────────────────────────────────────────
        let (git_status_tx, _) = tokio_watch::channel(HashMap::new());
        let git_status_tx = Arc::new(git_status_tx);
        let git_watcher = cx.new({
            let workspace = workspace.clone();
            let git_status_tx = git_status_tx.clone();
            |cx| GitStatusWatcher::new(workspace, git_status_tx, cx)
        });

        // Pass git_watcher to root view so ProjectColumns can observe it
        root_view.update(cx, |rv, cx| {
            rv.set_git_watcher(git_watcher.clone(), cx);
        });

        // ── Remote control setup ────────────────────────────────────────
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

        // Create bridge channel and start command loop
        let (bridge_tx, bridge_rx) = bridge::bridge_channel();

        let mut manager = Self {
            root_view,
            workspace: workspace.clone(),
            request_broker,
            pty_manager,
            terminals,
            opened_detached_windows: HashSet::new(),
            save_pending,
            git_watcher,
            git_status_tx: git_status_tx.clone(),
            remote_server: None,
            auth_store: auth_store.clone(),
            pty_broadcaster: pty_broadcaster.clone(),
            state_version: state_version.clone(),
            remote_info: remote_info.clone(),
            listen_addr,
            force_remote,
            service_manager: service_manager.clone(),
        };

        // Start PTY event loop (centralized for all windows)
        manager.start_pty_event_loop(pty_events, cx);

        // Start remote command bridge loop
        let local_backend: Arc<dyn crate::terminal::backend::TerminalBackend> =
            Arc::new(crate::terminal::backend::LocalBackend::new(manager.pty_manager.clone()));
        manager.start_remote_command_loop(bridge_rx, local_backend, cx);

        // Set up observer for detached terminals
        cx.observe(&workspace, move |this, workspace, cx| {
            this.handle_detached_terminals_changed(workspace, cx);
        })
        .detach();

        // Observe workspace to load/unload service configs when projects change
        {
            let service_manager = service_manager.clone();
            let known_project_ids: Arc<parking_lot::Mutex<HashSet<String>>> =
                Arc::new(parking_lot::Mutex::new(HashSet::new()));
            cx.observe(&workspace, move |_this, workspace, cx| {
                // Snapshot project info to avoid borrow conflicts with service_manager.update()
                let local_projects: Vec<(String, String, HashMap<String, String>)> = workspace
                    .read(cx)
                    .data()
                    .projects
                    .iter()
                    .filter(|p| !p.is_remote && p.is_visible)
                    .map(|p| (p.id.clone(), p.path.clone(), p.service_terminals.clone()))
                    .collect();

                let current_ids: HashSet<String> =
                    local_projects.iter().map(|(id, _, _)| id.clone()).collect();

                let mut known = known_project_ids.lock();

                // Load services for new projects
                for (id, path, saved_terminals) in &local_projects {
                    if !known.contains(id) {
                        service_manager.update(cx, |sm, cx| {
                            sm.load_project_services(id, path, saved_terminals, cx);
                        });
                    }
                }

                // Unload services for removed projects
                let removed: Vec<String> = known.difference(&current_ids).cloned().collect();
                for id in removed {
                    service_manager.update(cx, |sm, cx| {
                        sm.unload_project_services(&id, cx);
                    });
                }

                *known = current_ids;
            })
            .detach();
        }

        // Observe service manager to sync terminal IDs back to workspace for persistence
        {
            let workspace_for_svc = workspace.clone();
            cx.observe(&service_manager, move |_this, service_manager, cx| {
                let sm = service_manager.read(cx);
                // Collect project IDs that have services
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

        // Auto-start remote server if enabled in settings or forced via --remote
        let settings = cx.global::<GlobalSettings>().0.clone();
        if settings.read(cx).get().remote_server_enabled || force_remote {
            manager.start_remote_server(bridge_tx.clone());
        }

        // Observe settings changes to start/stop server dynamically
        let bridge_tx_for_observer = bridge_tx.clone();
        cx.observe(&settings, move |this, settings, cx| {
            let s = settings.read(cx).get();
            let enabled = s.remote_server_enabled;
            let running = this.remote_server.is_some();

            if enabled && !running {
                // Update listen_addr from settings if not forced via CLI
                if !this.force_remote {
                    if let Ok(addr) = s.remote_listen_address.parse::<IpAddr>() {
                        this.listen_addr = addr;
                    }
                }
                this.start_remote_server(bridge_tx_for_observer.clone());
            } else if !enabled && running {
                this.stop_remote_server();
            } else if enabled && running && !this.force_remote {
                // Check if address changed while server is running
                if let Ok(new_addr) = s.remote_listen_address.parse::<IpAddr>() {
                    if new_addr != this.listen_addr {
                        this.listen_addr = new_addr;
                        this.stop_remote_server();
                        this.start_remote_server(bridge_tx_for_observer.clone());
                    }
                }
            }
        })
        .detach();

        // ── Self-updater setup ──────────────────────────────────────────
        // Clean up leftover .old binary from a previous update
        crate::updater::installer::cleanup_old_binary();

        let update_info = UpdateInfo::new();
        cx.set_global(GlobalUpdateInfo(update_info.clone()));

        let auto_update_was_enabled = settings.read(cx).get().auto_update_enabled;
        if auto_update_was_enabled {
            Self::start_update_checker(update_info.clone(), cx);
        }

        // Observe settings to start/stop update checker (only on actual change)
        let update_info_for_obs = update_info;
        let mut prev_enabled = auto_update_was_enabled;
        cx.observe(&settings, move |_this, settings, cx| {
            let enabled = settings.read(cx).get().auto_update_enabled;
            if enabled == prev_enabled {
                return;
            }
            prev_enabled = enabled;
            if enabled {
                Self::start_update_checker(update_info_for_obs.clone(), cx);
            } else {
                update_info_for_obs.cancel();
            }
        })
        .detach();

        manager
    }

    /// Start the remote HTTP/WS server.
    fn start_remote_server(&mut self, bridge_tx: bridge::BridgeSender) {
        match RemoteServer::start(
            bridge_tx,
            self.auth_store.clone(),
            self.pty_broadcaster.clone(),
            self.state_version.clone(),
            self.listen_addr,
            self.git_status_tx.clone(),
        ) {
            Ok(server) => {
                let port = server.port();
                self.remote_info.set_active(port, self.auth_store.clone());
                log::info!("Remote server started on port {}", port);

                let code = self.auth_store.get_or_create_code();
                println!("Remote server listening on port {port}");
                println!("Pairing code: {code} (expires in 60s)");
                println!("Run `okena pair` anytime for a fresh code.");

                self.remote_server = Some(server);
            }
            Err(e) => {
                log::error!("Failed to start remote server: {}", e);
            }
        }
    }

    /// Stop the remote server.
    fn stop_remote_server(&mut self) {
        if let Some(mut server) = self.remote_server.take() {
            server.stop();
        }
        self.remote_info.set_inactive();
    }

    /// Centralized PTY event loop - notifies all windows (main and detached)
    fn start_pty_event_loop(
        &mut self,
        pty_events: Receiver<PtyEvent>,
        cx: &mut Context<Self>,
    ) {
        let terminals = self.terminals.clone();
        let broadcaster = self.pty_broadcaster.clone();

        cx.spawn(async move |this: WeakEntity<Okena>, cx| {
            loop {
                let event = match pty_events.recv().await {
                    Ok(event) => event,
                    Err(_) => break,
                };

                // Collect exit events for service manager processing
                let mut exit_events: Vec<(String, Option<u32>)> = Vec::new();

                // Process first event + broadcast to remote subscribers
                match &event {
                    PtyEvent::Data { terminal_id, data } => {
                        let terminals_guard = terminals.lock();
                        if let Some(terminal) = terminals_guard.get(terminal_id) {
                            terminal.process_output(data);
                        }
                        broadcaster.publish(terminal_id.clone(), data.clone());
                    }
                    PtyEvent::Exit { terminal_id, exit_code } => {
                        terminals.lock().remove(terminal_id);
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
                            broadcaster.publish(terminal_id.clone(), data.clone());
                        }
                        PtyEvent::Exit { terminal_id, exit_code } => {
                            terminals.lock().remove(terminal_id);
                            exit_events.push((terminal_id.clone(), *exit_code));
                        }
                    }
                }

                // Notify main window after processing the batch
                let _ = this.update(cx, |this, cx| {
                    // Route exit events to service manager for crash/restart handling
                    if !exit_events.is_empty() {
                        this.service_manager.update(cx, |sm, cx| {
                            for (terminal_id, exit_code) in &exit_events {
                                sm.handle_service_exit(terminal_id, *exit_code, cx);
                            }
                        });
                    }
                    this.root_view.update(cx, |_, cx| cx.notify());
                });
            }
        })
        .detach();
    }
}

impl Render for Okena {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.root_view.clone())
    }
}
