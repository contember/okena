mod detached_overlays;
mod detached_terminals;
pub mod headless;
mod remote_commands;

pub use detached_overlays::open_detached_overlay;

use crate::git::watcher::GitStatusWatcher;
use crate::workspace::worktree_sync::WorktreeSyncWatcher;
use crate::remote::auth::AuthStore;
use crate::remote::bridge;
use crate::remote::pty_broadcaster::PtyBroadcaster;
use crate::remote::server::RemoteServer;
use crate::remote::{GlobalRemoteInfo, RemoteInfo};
use crate::remote_client::manager::RemoteConnectionManager;
use crate::services::manager::ServiceManager;
use crate::settings::{GlobalSettings, settings};
use crate::views::panels::toast::ToastManager;
use crate::terminal::pty_manager::{PtyEvent, PtyManager};
use okena_ext_claude::resolve_claude_dir;
use crate::views::root::{RootView, TerminalsRegistry};
use crate::workspace::persistence;
use crate::workspace::request_broker::RequestBroker;
use crate::workspace::state::{GlobalWorkspace, WindowId, Workspace, WorkspaceData};
use async_channel::Receiver;
use gpui::*;
use okena_core::api::ApiGitStatus;
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::watch as tokio_watch;

/// Push the resolved Claude config directory into the PTY manager as CLAUDE_CONFIG_DIR,
/// so `claude` invocations inside Okena terminals read the same install as the status-bar widget.
/// Only set when the resolved dir differs from the CLI default (~/.claude).
fn sync_claude_pty_env(pty_manager: &Arc<PtyManager>, cx: &App) {
    let claude_dir = resolve_claude_dir(cx);
    let default_dir = dirs::home_dir().map(|h| h.join(".claude")).unwrap_or_default();
    if claude_dir != default_dir {
        pty_manager.set_extra_env(vec![
            ("CLAUDE_CONFIG_DIR".to_string(), claude_dir.to_string_lossy().into_owned()),
        ]);
    } else {
        pty_manager.set_extra_env(vec![]);
    }
}

/// Set up an observer that loads/unloads service configs when projects change.
/// Handles deferred worktrees by skipping projects whose directory doesn't exist yet.
pub(crate) fn observe_project_services<T: 'static>(
    workspace: &Entity<Workspace>,
    service_manager: &Entity<ServiceManager>,
    cx: &mut Context<T>,
) {
    let service_manager = service_manager.clone();
    let known: Arc<parking_lot::Mutex<HashSet<String>>> =
        Arc::new(parking_lot::Mutex::new(HashSet::new()));

    // Initial load
    {
        let data = workspace.read(cx).data().clone();
        sync_services(&data, &mut known.lock(), &service_manager, cx);
    }

    let known_for_observer = known.clone();
    cx.observe(workspace, move |_this, workspace: Entity<Workspace>, cx| {
        let data = workspace.read(cx).data().clone();
        sync_services(&data, &mut known_for_observer.lock(), &service_manager, cx);
    })
    .detach();
}

fn sync_services(
    data: &WorkspaceData,
    known: &mut HashSet<String>,
    service_manager: &Entity<ServiceManager>,
    cx: &mut impl AppContext,
) {
    let current_ids: HashSet<String> = data.projects.iter()
        .filter(|p| !p.is_remote)
        .map(|p| p.id.clone())
        .collect();

    for p in &data.projects {
        if p.is_remote || known.contains(&p.id) {
            continue;
        }
        // Skip projects whose directory doesn't exist yet (deferred worktrees).
        if !std::path::Path::new(&p.path).exists() {
            continue;
        }
        service_manager.update(cx, |sm, cx| {
            sm.load_project_services(&p.id, &p.path, &p.service_terminals, cx);
        });
        known.insert(p.id.clone());
    }

    let removed: Vec<String> = known.difference(&current_ids).cloned().collect();
    for id in &removed {
        service_manager.update(cx, |sm, cx| {
            sm.unload_project_services(id, cx);
        });
        known.remove(id);
    }
}

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
    // ── Worktree sync watcher ────────────────────────────────────────
    #[allow(dead_code)]
    worktree_sync: Entity<WorktreeSyncWatcher>,
    git_status_tx: Arc<tokio_watch::Sender<HashMap<String, ApiGitStatus>>>,
    remote_subscribed_terminals: Arc<std::sync::RwLock<HashMap<u64, HashSet<String>>>>,
    next_remote_connection_id: Arc<AtomicU64>,
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
                        let _slow = okena_core::timing::SlowGuard::new("workspace_save_clone");
                        let ws = workspace.read(cx);
                        (ws.data().clone(), ws.data_version())
                    });
                    // Run blocking fs IO off the GPUI main thread — on Windows
                    // an AV scan or OneDrive sync of workspace.json can stall
                    // for seconds and would otherwise freeze the UI.
                    let save_result = smol::unblock(move || persistence::save_workspace(&data)).await;
                    match save_result {
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
            RootView::new(WindowId::Main, workspace.clone(), request_broker_clone, pty_manager_clone, cx)
        });

        // Get terminals registry from root view
        let terminals = root_view.read(cx).terminals().clone();

        // Create service manager for project-scoped background processes
        let local_backend_for_services: Arc<dyn crate::terminal::backend::TerminalBackend> =
            Arc::new(crate::terminal::backend::LocalBackend::new(pty_manager.clone()));
        let service_manager = cx.new(|_cx| {
            ServiceManager::new(local_backend_for_services.clone(), terminals.clone())
        });
        root_view.update(cx, |rv, cx| {
            rv.set_service_manager(service_manager.clone(), cx);
        });

        // Create HookRunner for PTY-backed hook execution
        cx.set_global(crate::workspace::hooks::HookRunner::new(
            local_backend_for_services.clone(),
            terminals.clone(),
        ));

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
        let remote_subscribed_terminals: Arc<std::sync::RwLock<HashMap<u64, HashSet<String>>>> =
            Arc::new(std::sync::RwLock::new(HashMap::new()));
        let next_remote_connection_id = Arc::new(AtomicU64::new(0));
        let git_watcher = cx.new({
            let workspace = workspace.clone();
            let git_status_tx = git_status_tx.clone();
            let remote_subscribed_terminals = remote_subscribed_terminals.clone();
            |cx| GitStatusWatcher::new(workspace, git_status_tx, remote_subscribed_terminals, cx)
        });

        // ── Worktree sync watcher ─────────────────────────────────────
        let worktree_sync = cx.new({
            let workspace = workspace.clone();
            |cx| WorktreeSyncWatcher::new(workspace, cx)
        });

        // Pass git_watcher to root view so ProjectColumns can observe it
        root_view.update(cx, |rv, cx| {
            rv.set_git_watcher(git_watcher.clone(), cx);
        });

        // ── Remote control setup ────────────────────────────────────────
        let auth_store = Arc::new(AuthStore::new());
        let pty_broadcaster = Arc::new(PtyBroadcaster::new());
        // Publish PTY output directly from reader threads (bypasses GPUI event loop latency)
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
            worktree_sync,
            git_status_tx: git_status_tx.clone(),
            remote_subscribed_terminals: remote_subscribed_terminals.clone(),
            next_remote_connection_id: next_remote_connection_id.clone(),
            remote_server: None,
            auth_store: auth_store.clone(),
            pty_broadcaster: pty_broadcaster.clone(),
            state_version: state_version.clone(),
            remote_info: remote_info.clone(),
            listen_addr,
            force_remote,
            service_manager: service_manager.clone(),
        };

        // Propagate claude config dir to spawned PTYs so `claude` CLI invocations inside
        // Okena terminals pick the same install as the status-bar widget.
        sync_claude_pty_env(&manager.pty_manager, cx);
        let settings_entity = cx.global::<GlobalSettings>().0.clone();
        cx.observe(&settings_entity, move |this, _settings, cx| {
            sync_claude_pty_env(&this.pty_manager, cx);
        })
        .detach();

        // Start PTY event loop (centralized for all windows)
        manager.start_pty_event_loop(pty_events, cx);

        // Start remote command bridge loop
        let local_backend: Arc<dyn crate::terminal::backend::TerminalBackend> =
            Arc::new(crate::terminal::backend::LocalBackend::new(manager.pty_manager.clone()));
        manager.start_remote_command_loop(bridge_rx, local_backend, cx);

        // Kill orphaned terminals when projects are deleted
        cx.observe(&workspace, move |this, workspace, cx| {
            let kills = workspace.update(cx, |ws, _| ws.drain_pending_terminal_kills());
            if !kills.is_empty() {
                let mut reg = this.terminals.lock();
                for tid in &kills {
                    this.pty_manager.kill(tid);
                    reg.remove(tid);
                }
            }
        })
        .detach();

        // Set up observer for detached terminals
        cx.observe(&workspace, move |this, workspace, cx| {
            this.handle_detached_terminals_changed(workspace, cx);
        })
        .detach();

        // Observe workspace to load/unload service configs when projects change
        observe_project_services(&workspace, &service_manager, cx);

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

        // Note: updater is now handled by the okena-ext-updater extension.
        // GlobalUpdateInfo is set in main.rs via okena_ext_updater::init().

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
            self.remote_subscribed_terminals.clone(),
            self.next_remote_connection_id.clone(),
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
        let pty_manager = self.pty_manager.clone();

        cx.spawn(async move |this: WeakEntity<Okena>, cx| {
            loop {
                let event = match pty_events.recv().await {
                    Ok(event) => event,
                    Err(_) => break,
                };

                let _slow = okena_core::timing::SlowGuard::new("Okena::pty_event_batch");

                // Collect exit events and track which terminals received data
                let mut exit_events: Vec<(String, Option<u32>)> = Vec::new();
                let mut dirty_terminal_ids: Vec<String> = Vec::new();

                // Process first event (broadcasting handled by PtyOutputSink in reader threads)
                match &event {
                    PtyEvent::Data { terminal_id, data } => {
                        let terminals_guard = terminals.lock();
                        if let Some(terminal) = terminals_guard.get(terminal_id) {
                            terminal.process_output(data);
                        }
                        dirty_terminal_ids.push(terminal_id.clone());
                    }
                    PtyEvent::Exit { terminal_id, exit_code } => {
                        // Clean up the PtyHandle (reader/writer threads) but don't
                        // remove the UI Terminal yet — service manager may keep it
                        // so users can see crash output.
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
                            dirty_terminal_ids.push(terminal_id.clone());
                        }
                        PtyEvent::Exit { terminal_id, exit_code } => {
                            pty_manager.cleanup_exited(terminal_id);
                            exit_events.push((terminal_id.clone(), *exit_code));
                        }
                    }
                }

                // Notify main window after processing the batch
                let _ = this.update(cx, |this, cx| {
                    if !exit_events.is_empty() {
                        // Two-phase hook exit handling:
                        // Phase 1 (here): notify_exit unblocks any sync hook threads
                        // waiting on a PTY terminal via mpsc::Receiver. This MUST happen
                        // before handle_hook_terminal_exits (phase 2) which updates
                        // workspace status and may trigger project removal.
                        if let Some(monitor) = crate::workspace::hooks::try_monitor(cx) {
                            for (terminal_id, exit_code) in &exit_events {
                                monitor.notify_exit(terminal_id, *exit_code);
                            }
                        }

                        // Let service manager handle service terminals (may keep
                        // their UI Terminal for viewing crash output)
                        let service_tids: std::collections::HashSet<String> =
                            this.service_manager.update(cx, |sm, cx| {
                                let mut handled = std::collections::HashSet::new();
                                for (terminal_id, exit_code) in &exit_events {
                                    if sm.handle_service_exit(terminal_id, *exit_code, cx) {
                                        handled.insert(terminal_id.clone());
                                    }
                                }
                                handled
                            });

                        // Handle hook terminal exits (status updates, pending close, cleanup)
                        let hook_tids = this.handle_hook_terminal_exits(&exit_events, &service_tids, cx);

                        // Fire terminal.on_close hook for user terminals (not service, not hook)
                        let terminal_close_infos: Vec<_> = {
                            let global_on_close = crate::settings::settings(cx).hooks.terminal.on_close.is_some();
                            let ws = this.workspace.read(cx);
                            exit_events.iter()
                                .filter(|(tid, _)| !service_tids.contains(tid) && !hook_tids.contains(tid))
                                .filter_map(|(tid, exit_code)| {
                                    ws.find_project_for_terminal(tid).and_then(|p| {
                                        let parent_on_close = p.worktree_info.as_ref()
                                            .and_then(|wt| ws.project(&wt.parent_project_id))
                                            .and_then(|pp| pp.hooks.terminal.on_close.as_ref())
                                            .is_some();
                                        if global_on_close || p.hooks.terminal.on_close.is_some() || parent_on_close {
                                            let parent_hooks = p.worktree_info.as_ref()
                                                .and_then(|wt| ws.project(&wt.parent_project_id))
                                                .map(|pp| pp.hooks.clone());
                                            let terminal_name = p.terminal_names.get(tid).cloned();
                                            let is_worktree = p.worktree_info.is_some();
                                            let folder = ws.folder_for_project_or_parent(&p.id);
                                            let fid = folder.map(|f| f.id.clone());
                                            let fname = folder.map(|f| f.name.clone());
                                            Some((p.hooks.clone(), parent_hooks, p.id.clone(), p.name.clone(), p.path.clone(), tid.clone(), terminal_name, is_worktree, *exit_code, fid, fname))
                                        } else {
                                            None
                                        }
                                    })
                                })
                                .collect()
                        };
                        for (project_hooks, parent_hooks, project_id, project_name, project_path, terminal_id, terminal_name, is_worktree, exit_code, folder_id, folder_name) in terminal_close_infos {
                            crate::workspace::hooks::fire_terminal_on_close(
                                &project_hooks, parent_hooks.as_ref(), &project_id, &project_name,
                                &project_path, &terminal_id, terminal_name.as_deref(), is_worktree, exit_code,
                                folder_id.as_deref(), folder_name.as_deref(), &crate::settings::settings(cx).hooks, cx,
                            );
                        }

                        // Kill session backends and remove UI Terminals for non-service, non-hook terminals.
                        // This is critical for dtach: the PTY exit only means the client disconnected,
                        // but the dtach daemon keeps running. kill() ensures kill_session() is called
                        // to SIGTERM the daemon and remove the socket file.
                        {
                            let mut reg = this.terminals.lock();
                            for (terminal_id, _) in &exit_events {
                                if !service_tids.contains(terminal_id) && !hook_tids.contains(terminal_id) {
                                    this.pty_manager.kill(terminal_id);
                                    reg.remove(terminal_id);
                                }
                            }
                        }
                    }
                    // Notify dirty terminal content panes directly (batched in one update).
                    // All notifications happen in the same GPUI update → single layout pass.
                    if !dirty_terminal_ids.is_empty() {
                        dirty_terminal_ids.dedup();
                        let registry = crate::views::root::content_pane_registry().lock();
                        let mut any_local_pane = false;
                        for tid in &dirty_terminal_ids {
                            if let Some(weak_content) = registry.get(tid) {
                                let _ = weak_content.update(cx, |_content, cx| {
                                    cx.notify();
                                });
                                any_local_pane = true;
                            }
                        }
                        // Remote-only terminals have no local content pane. Without
                        // cx.notify(), GPUI's draw cycle won't run and the event loop
                        // effectively stalls. Notify root_view to keep GPUI responsive
                        // for bridge commands, state queries, and other remote work.
                        if !any_local_pane {
                            this.root_view.update(cx, |_, cx| cx.notify());
                        }
                    }

                    // Check if any hook terminal reported its exit code via
                    // OSC title (__okena_hook_exit:<code>). This happens when
                    // keep_alive hooks finish their command but the PTY stays
                    // alive as an interactive shell.
                    if !dirty_terminal_ids.is_empty() {
                        let terminals_guard = this.terminals.lock();
                        let ws = this.workspace.read(cx);
                        let mut status_updates: Vec<(String, crate::workspace::state::HookTerminalStatus)> = Vec::new();
                        for tid in &dirty_terminal_ids {
                            if ws.is_hook_terminal(tid).is_none() {
                                continue;
                            }
                            if let Some(terminal) = terminals_guard.get(tid) {
                                if let Some(title) = terminal.title() {
                                    if let Some(code_str) = title.strip_prefix("__okena_hook_exit:") {
                                        let exit_code = code_str.parse::<i32>().unwrap_or(-1);
                                        let status = if exit_code == 0 {
                                            crate::workspace::state::HookTerminalStatus::Succeeded
                                        } else {
                                            crate::workspace::state::HookTerminalStatus::Failed { exit_code }
                                        };
                                        status_updates.push((tid.clone(), status));
                                    }
                                }
                            }
                        }
                        drop(terminals_guard);
                        if !status_updates.is_empty() {
                            this.workspace.update(cx, |ws, cx| {
                                for (tid, status) in status_updates {
                                    ws.update_hook_terminal_status(&tid, status, cx);
                                }
                            });
                        }
                    }

                    if !exit_events.is_empty() {
                        this.root_view.update(cx, |_, cx| cx.notify());
                    }
                });
            }
        })
        .detach();
    }

    // ── Hook terminal exit handling ──────────────────────────────────────

    /// Process hook terminal exit events: update status, resolve pending worktree closes,
    /// and schedule cleanup. Returns the set of terminal IDs that were hook terminals.
    fn handle_hook_terminal_exits(
        &mut self,
        exit_events: &[(String, Option<u32>)],
        service_tids: &std::collections::HashSet<String>,
        cx: &mut Context<Self>,
    ) -> std::collections::HashSet<String> {
        let hook_tids: std::collections::HashSet<String> = {
            let ws = self.workspace.read(cx);
            exit_events.iter()
                .filter(|(tid, _)| !service_tids.contains(tid))
                .filter(|(tid, _)| ws.is_hook_terminal(tid).is_some())
                .map(|(tid, _)| tid.clone())
                .collect()
        };

        for (terminal_id, exit_code) in exit_events {
            if !hook_tids.contains(terminal_id) {
                continue;
            }

            let success = *exit_code == Some(0);
            let tid = terminal_id.clone();

            // Update HookMonitor so the hook log shows correct status
            if let Some(monitor) = crate::workspace::hooks::try_monitor(cx) {
                monitor.finish_by_terminal_id(&tid, *exit_code);
            }

            // Single workspace.update: set hook status, then handle pending close atomically.
            let pending_data = self.workspace.update(cx, |ws, cx| {
                // Update hook terminal status
                let status = if success {
                    crate::workspace::state::HookTerminalStatus::Succeeded
                } else {
                    let code = exit_code.map(|c| c as i32).unwrap_or(-1);
                    crate::workspace::state::HookTerminalStatus::Failed { exit_code: code }
                };
                ws.update_hook_terminal_status(&tid, status, cx);

                // Check for pending worktree close tied to this hook terminal
                let pending = ws.take_pending_worktree_close(&tid)?;
                let folder = ws.folder_for_project_or_parent(&pending.project_id);
                let hook_folder_id = folder.map(|f| f.id.clone());
                let hook_folder_name = folder.map(|f| f.name.clone());
                let (project_path_for_git, hook_info) = ws.project(&pending.project_id)
                    .map(|p| (Some(p.path.clone()), Some((p.hooks.clone(), p.name.clone(), p.path.clone()))))
                    .unwrap_or((None, None));
                if success {
                    ws.remove_hook_terminal(&tid, cx);
                    // Collect remaining hook terminal IDs before deleting the project
                    let remaining_hook_tids = ws.hook_terminal_ids_for_project(&pending.project_id);
                    ws.delete_project(&pending.project_id, &settings(cx).hooks, cx);
                    Some((pending, project_path_for_git, hook_info, remaining_hook_tids, hook_folder_id, hook_folder_name))
                } else {
                    ws.finish_closing_project(&pending.project_id);
                    None
                }
            });

            if let Some((pending, project_path_for_git, hook_info, remaining_hook_tids, folder_id, folder_name)) = pending_data {
                self.handle_pending_close_result(&tid, pending, project_path_for_git, hook_info, remaining_hook_tids, folder_id, folder_name, cx);
            }
            // Hook terminal persists — no auto-cleanup. User can dismiss manually or rerun.
        }

        hook_tids
    }

    /// Handle the result of a pending worktree close after hook exit (success path only).
    fn handle_pending_close_result(
        &mut self,
        tid: &str,
        pending: crate::workspace::state::PendingWorktreeClose,
        project_path_for_git: Option<String>,
        hook_info: Option<(crate::workspace::persistence::HooksConfig, String, String)>,
        remaining_hook_tids: Vec<String>,
        folder_id: Option<String>,
        folder_name: Option<String>,
        cx: &mut Context<Self>,
    ) {
        log::info!("Pending worktree close: hook succeeded, removing project {}", pending.project_id);

        let global_hooks = crate::settings::settings(cx).hooks;
        let monitor = crate::workspace::hooks::try_monitor(cx);
        let runner = crate::workspace::hooks::try_runner(cx);
        // Clean up primary and any other persisted hook terminals in a single lock
        {
            let mut guard = self.terminals.lock();
            guard.remove(tid);
            for hook_tid in &remaining_hook_tids {
                guard.remove(hook_tid);
            }
        }

        // Fire lifecycle hooks
        if let Some((project_hooks, project_name, project_path)) = hook_info {
            crate::workspace::hooks::fire_on_worktree_close(
                &project_hooks,
                &pending.project_id,
                &project_name,
                &project_path,
                &pending.branch,
                folder_id.as_deref(),
                folder_name.as_deref(),
                &global_hooks,
                cx,
            );
            let _ = crate::workspace::hooks::fire_worktree_removed(
                &project_hooks,
                &global_hooks,
                &pending.project_id,
                &project_name,
                &project_path,
                &pending.branch,
                &pending.main_repo_path,
                folder_id.as_deref(),
                folder_name.as_deref(),
                monitor.as_ref(),
                runner.as_ref(),
            );
        }

        // Git worktree remove in the background
        let pending_clone = pending.clone();
        let workspace = self.workspace.clone();
        if let Some(ref path) = project_path_for_git {
            workspace.update(cx, |ws, _| {
                ws.mark_worktree_removing(path);
            });
        }
        cx.spawn(async move |_this, cx| {
            if let Some(path) = project_path_for_git {
                let main_repo = pending_clone.main_repo_path.clone();
                let path_clone = path.clone();
                let result = smol::unblock(move || {
                    crate::git::remove_worktree_fast(
                        &std::path::PathBuf::from(&path_clone),
                        &std::path::PathBuf::from(&main_repo),
                    )
                }).await;
                if let Err(e) = result {
                    log::error!("Background worktree remove failed: {}", e);
                }
                let _ = cx.update(|cx| {
                    workspace.update(cx, |ws, _| {
                        ws.finish_worktree_removing(&path);
                    });
                });
            }
        }).detach();
    }

}

impl Render for Okena {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.root_view.clone())
    }
}

