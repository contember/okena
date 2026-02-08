mod detached_terminals;
mod remote_commands;
mod update_checker;

use crate::remote::auth::AuthStore;
use crate::remote::bridge;
use crate::remote::pty_broadcaster::PtyBroadcaster;
use crate::remote::server::RemoteServer;
use crate::remote::{GlobalRemoteInfo, RemoteInfo};
use crate::remote_client::manager::RemoteConnectionManager;
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
use std::collections::HashSet;
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
    // ── Remote control fields ───────────────────────────────────────────
    remote_server: Option<RemoteServer>,
    pub auth_store: Arc<AuthStore>,
    pub(crate) pty_broadcaster: Arc<PtyBroadcaster>,
    pub(crate) state_version: Arc<tokio_watch::Sender<u64>>,
    remote_info: RemoteInfo,
    listen_addr: IpAddr,
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
        let listen_addr = listen_addr.unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
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
                    if let Err(e) = persistence::save_workspace(&data) {
                        log::error!("Failed to save workspace: {}", e);
                        let _ = cx.update(|cx| {
                            ToastManager::error(format!("Failed to save workspace: {}", e), cx);
                        });
                    }
                    last_saved.store(version, Ordering::Relaxed);
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
            remote_server: None,
            auth_store: auth_store.clone(),
            pty_broadcaster: pty_broadcaster.clone(),
            state_version: state_version.clone(),
            remote_info: remote_info.clone(),
            listen_addr,
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

        // Auto-start remote server if enabled in settings or forced via --remote
        let settings = cx.global::<GlobalSettings>().0.clone();
        if settings.read(cx).get().remote_server_enabled || force_remote {
            manager.start_remote_server(bridge_tx.clone());
        }

        // Observe settings changes to start/stop server dynamically
        let bridge_tx_for_observer = bridge_tx.clone();
        cx.observe(&settings, move |this, settings, cx| {
            let enabled = settings.read(cx).get().remote_server_enabled;
            let running = this.remote_server.is_some();

            if enabled && !running {
                this.start_remote_server(bridge_tx_for_observer.clone());
            } else if !enabled && running {
                this.stop_remote_server();
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

                // Process first event + broadcast to remote subscribers
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
                        PtyEvent::Exit { terminal_id, .. } => {
                            terminals.lock().remove(terminal_id);
                        }
                    }
                }

                // Notify main window after processing the batch
                let _ = this.update(cx, |this, cx| {
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
