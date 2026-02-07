use alacritty_terminal::grid::Dimensions;
use crate::remote::auth::AuthStore;
use crate::remote::bridge::{self, BridgeMessage, BridgeReceiver, CommandResult, RemoteCommand};
use crate::remote::pty_broadcaster::PtyBroadcaster;
use crate::remote::server::RemoteServer;
use crate::remote::types::{ApiFullscreen, ApiLayoutNode, ApiProject, StateResponse};
use crate::remote::{GlobalRemoteInfo, RemoteInfo};
use crate::settings::GlobalSettings;
use crate::updater::{GlobalUpdateInfo, UpdateInfo, UpdateStatus};
use crate::terminal::pty_manager::{PtyEvent, PtyManager};
use crate::views::detached_terminal::DetachedTerminalView;
use crate::views::root::{RootView, TerminalsRegistry};
use crate::workspace::persistence;
use crate::workspace::state::{Workspace, WorkspaceData};
use async_channel::Receiver;
use gpui::*;
#[cfg(not(target_os = "linux"))]
use gpui_component::Root;
#[cfg(target_os = "linux")]
use crate::simple_root::SimpleRoot as Root;
use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Main application state and view
pub struct Okena {
    root_view: Entity<RootView>,
    workspace: Entity<Workspace>,
    pty_manager: Arc<PtyManager>,
    terminals: TerminalsRegistry,
    /// Track which detached windows we've already opened
    opened_detached_windows: HashSet<String>,
    /// Flag indicating workspace needs to be saved (for debouncing)
    /// Note: Field is read by spawned tasks, not directly
    #[allow(dead_code)]
    save_pending: Arc<AtomicBool>,
    // ── Remote control fields ───────────────────────────────────────────
    remote_server: Option<RemoteServer>,
    pub auth_store: Arc<AuthStore>,
    pty_broadcaster: Arc<PtyBroadcaster>,
    state_version: Arc<AtomicU64>,
    remote_info: RemoteInfo,
}

impl Okena {
    pub fn new(
        workspace_data: WorkspaceData,
        pty_manager: Arc<PtyManager>,
        pty_events: Receiver<PtyEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Create workspace entity
        let workspace = cx.new(|_cx| Workspace::new(workspace_data));

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
                        (ws.data.clone(), ws.data_version())
                    });
                    if let Err(e) = persistence::save_workspace(&data) {
                        log::error!("Failed to save workspace: {}", e);
                    }
                    last_saved.store(version, Ordering::Relaxed);
                }
            }).detach();
        })
        .detach();

        // Create root view (get terminals registry from it)
        let pty_manager_clone = pty_manager.clone();
        let root_view = cx.new(|cx| {
            RootView::new(workspace.clone(), pty_manager_clone, cx)
        });

        // Get terminals registry from root view
        let terminals = root_view.read(cx).terminals().clone();

        // Observe window bounds changes to force re-render
        cx.observe_window_bounds(window, |_this, _window, cx| {
            cx.notify();
        })
        .detach();

        // ── Remote control setup ────────────────────────────────────────
        let auth_store = Arc::new(AuthStore::new());
        let pty_broadcaster = Arc::new(PtyBroadcaster::new());
        let state_version = Arc::new(AtomicU64::new(0));
        let remote_info = RemoteInfo::new();
        cx.set_global(GlobalRemoteInfo(remote_info.clone()));

        // Bump state_version on workspace changes
        let sv = state_version.clone();
        cx.observe(&workspace, move |_this, _workspace, _cx| {
            sv.fetch_add(1, Ordering::Relaxed);
        })
        .detach();

        // Create bridge channel and start command loop
        let (bridge_tx, bridge_rx) = bridge::bridge_channel();

        let mut manager = Self {
            root_view,
            workspace: workspace.clone(),
            pty_manager,
            terminals,
            opened_detached_windows: HashSet::new(),
            save_pending,
            remote_server: None,
            auth_store: auth_store.clone(),
            pty_broadcaster: pty_broadcaster.clone(),
            state_version: state_version.clone(),
            remote_info: remote_info.clone(),
        };

        // Start PTY event loop (centralized for all windows)
        manager.start_pty_event_loop(pty_events, cx);

        // Start remote command bridge loop
        manager.start_remote_command_loop(bridge_rx, cx);

        // Set up observer for detached terminals
        cx.observe(&workspace, move |this, workspace, cx| {
            this.handle_detached_terminals_changed(workspace, cx);
        })
        .detach();

        // Auto-start remote server if enabled in settings
        let settings = cx.global::<GlobalSettings>().0.clone();
        if settings.read(cx).get().remote_server_enabled {
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
        ) {
            Ok(server) => {
                let port = server.port();
                self.remote_info.set_active(port, self.auth_store.clone());
                log::info!("Remote server started on port {}", port);
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

    /// Get the remote server port (if running).
    pub fn remote_server_port(&self) -> Option<u16> {
        self.remote_server.as_ref().map(|s| s.port())
    }

    /// Spawn the update checker loop (30s delay, check, optionally download, sleep 24h).
    /// Uses `try_start()` to prevent duplicate loops and respects cancellation via token.
    fn start_update_checker(update_info: UpdateInfo, cx: &mut Context<Self>) {
        let token = match update_info.try_start() {
            Some(t) => t,
            None => return, // A checker loop is already running
        };

        cx.spawn(async move |this: WeakEntity<Okena>, cx| {
            // Initial delay — check cancellation every second
            for _ in 0..30 {
                if update_info.is_cancelled(token) {
                    update_info.mark_stopped(token);
                    return;
                }
                smol::Timer::after(std::time::Duration::from_secs(1)).await;
            }

            loop {
                if update_info.is_cancelled(token) {
                    update_info.mark_stopped(token);
                    return;
                }

                // Pause while a manual check is in progress
                while update_info.is_manual_active() {
                    if update_info.is_cancelled(token) {
                        update_info.mark_stopped(token);
                        return;
                    }
                    smol::Timer::after(std::time::Duration::from_secs(1)).await;
                }

                // If an update was already found (e.g. by a manual check), stop
                match update_info.status() {
                    UpdateStatus::Ready { .. }
                    | UpdateStatus::ReadyToRestart { .. }
                    | UpdateStatus::Installing { .. }
                    | UpdateStatus::BrewUpdate { .. } => {
                        update_info.mark_stopped(token);
                        return;
                    }
                    _ => {}
                }

                update_info.set_status(UpdateStatus::Checking);
                let _ = this.update(cx, |_, cx| cx.notify());

                match crate::updater::checker::check_for_update().await {
                    Ok(Some(release)) => {

                        if update_info.is_homebrew() {
                            update_info.set_status(UpdateStatus::BrewUpdate {
                                version: release.version,
                            });
                            let _ = this.update(cx, |_, cx| cx.notify());
                            update_info.mark_stopped(token);
                            return;
                        }

                        if update_info.is_cancelled(token) {
                            update_info.mark_stopped(token);
                            return;
                        }

                        // Download with retry (up to 3 attempts) and periodic UI refresh
                        let asset_url = release.asset_url;
                        let asset_name = release.asset_name;
                        let version = release.version;
                        let checksum_url = release.checksum_url;

                        update_info.set_status(UpdateStatus::Downloading {
                            version: version.clone(),
                            progress: 0,
                        });
                        let _ = this.update(cx, |_, cx| cx.notify());

                        let mut last_err: Option<anyhow::Error> = None;
                        for attempt in 0..3u32 {
                            if attempt > 0 {
                                // Backoff: 30s, 60s
                                let delay_secs = 30u64 * (1 << (attempt - 1));
                                for _ in 0..delay_secs {
                                    if update_info.is_cancelled(token) {
                                        update_info.mark_stopped(token);
                                        return;
                                    }
                                    smol::Timer::after(std::time::Duration::from_secs(1)).await;
                                }
                                update_info.set_status(UpdateStatus::Downloading {
                                    version: version.clone(),
                                    progress: 0,
                                });
                                let _ = this.update(cx, |_, cx| cx.notify());
                            }

                            let download = crate::updater::downloader::download_asset(
                                asset_url.clone(),
                                asset_name.clone(),
                                version.clone(),
                                update_info.clone(),
                                token,
                                checksum_url.clone(),
                            );
                            let mut download = std::pin::pin!(download);

                            let result = loop {
                                let polled = std::future::poll_fn(|task_cx| {
                                    match download.as_mut().poll(task_cx) {
                                        std::task::Poll::Ready(r) => std::task::Poll::Ready(Some(r)),
                                        std::task::Poll::Pending => std::task::Poll::Ready(None),
                                    }
                                }).await;
                                match polled {
                                    Some(r) => break r,
                                    None => {
                                        smol::Timer::after(std::time::Duration::from_millis(250)).await;
                                        let _ = this.update(cx, |_, cx| cx.notify());
                                    }
                                }
                            };

                            match result {
                                Ok(path) => {
                                    update_info.set_status(UpdateStatus::Ready {
                                        version,
                                        path,
                                    });
                                    let _ = this.update(cx, |_, cx| cx.notify());
                                    update_info.mark_stopped(token);
                                    return;
                                }
                                Err(e) => {
                                    if update_info.is_cancelled(token) {
                                        update_info.mark_stopped(token);
                                        return;
                                    }
                                    log::warn!("Download attempt {}/3 failed: {}", attempt + 1, e);
                                    last_err = Some(e);
                                }
                            }
                        }

                        if let Some(e) = last_err {
                            log::error!("Download failed after 3 attempts: {}", e);
                            update_info.set_status(UpdateStatus::Failed {
                                error: e.to_string(),
                            });
                            let _ = this.update(cx, |_, cx| cx.notify());
                        }
                    }
                    Ok(None) => {
                        update_info.set_status(UpdateStatus::Idle);
                        let _ = this.update(cx, |_, cx| cx.notify());
                    }
                    Err(e) => {
                        log::error!("Update check failed: {}", e);
                        update_info.set_status(UpdateStatus::Failed {
                            error: e.to_string(),
                        });
                        let _ = this.update(cx, |_, cx| cx.notify());
                    }
                }

                // Keep Failed status visible for 60 seconds before clearing
                if matches!(update_info.status(), UpdateStatus::Failed { .. }) {
                    for _ in 0..60 {
                        if update_info.is_cancelled(token) {
                            update_info.mark_stopped(token);
                            return;
                        }
                        smol::Timer::after(std::time::Duration::from_secs(1)).await;
                    }
                    // Only reset if still Failed (a manual check may have changed status)
                    if matches!(update_info.status(), UpdateStatus::Failed { .. }) {
                        update_info.set_status(UpdateStatus::Idle);
                        let _ = this.update(cx, |_, cx| cx.notify());
                    }
                }

                // Wait 24 hours, checking cancellation every minute
                for _ in 0..(24 * 60) {
                    if update_info.is_cancelled(token) {
                        update_info.mark_stopped(token);
                        return;
                    }
                    smol::Timer::after(std::time::Duration::from_secs(60)).await;
                }
            }
        })
        .detach();
    }

    /// Process commands from the remote API bridge.
    /// Runs on the GPUI main thread via cx.spawn().
    fn start_remote_command_loop(
        &mut self,
        bridge_rx: BridgeReceiver,
        cx: &mut Context<Self>,
    ) {
        let workspace = self.workspace.clone();
        let terminals = self.terminals.clone();
        let state_version = self.state_version.clone();

        cx.spawn(async move |_this: WeakEntity<Okena>, cx| {
            loop {
                let msg: BridgeMessage = match bridge_rx.recv().await {
                    Ok(msg) => msg,
                    Err(_) => break,
                };

                let result = match msg.command {
                    RemoteCommand::GetState => {
                        cx.update(|cx| {
                            let ws = workspace.read(cx);
                            let sv = state_version.load(Ordering::Relaxed);
                            let projects: Vec<ApiProject> = ws.data.projects.iter().map(|p| {
                                ApiProject {
                                    id: p.id.clone(),
                                    name: p.name.clone(),
                                    path: p.path.clone(),
                                    is_visible: p.is_visible,
                                    layout: p.layout.as_ref().map(ApiLayoutNode::from_layout),
                                    terminal_names: p.terminal_names.clone(),
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
                            };

                            CommandResult::Ok(Some(serde_json::to_value(resp).unwrap()))
                        })
                    }
                    RemoteCommand::SendText { terminal_id, text } => {
                        let found = {
                            let guard = terminals.lock();
                            guard.get(&terminal_id).cloned()
                        };
                        match found {
                            Some(term) => {
                                term.send_input(&text);
                                CommandResult::Ok(None)
                            }
                            None => CommandResult::Err(format!("terminal not found: {}", terminal_id)),
                        }
                    }
                    RemoteCommand::RunCommand { terminal_id, command } => {
                        let found = {
                            let guard = terminals.lock();
                            guard.get(&terminal_id).cloned()
                        };
                        match found {
                            Some(term) => {
                                term.send_input(&format!("{}\r", command));
                                CommandResult::Ok(None)
                            }
                            None => CommandResult::Err(format!("terminal not found: {}", terminal_id)),
                        }
                    }
                    RemoteCommand::SendSpecialKey { terminal_id, key } => {
                        let found = {
                            let guard = terminals.lock();
                            guard.get(&terminal_id).cloned()
                        };
                        match found {
                            Some(term) => {
                                term.send_bytes(key.to_bytes());
                                CommandResult::Ok(None)
                            }
                            None => CommandResult::Err(format!("terminal not found: {}", terminal_id)),
                        }
                    }
                    RemoteCommand::ReadContent { terminal_id } => {
                        let found = {
                            let guard = terminals.lock();
                            guard.get(&terminal_id).cloned()
                        };
                        match found {
                            Some(term) => {
                                let content = term.with_content(|term| {
                                    let grid = term.grid();
                                    let screen_lines = grid.screen_lines();
                                    let cols = grid.columns();
                                    let mut lines = Vec::with_capacity(screen_lines);

                                    for row in 0..screen_lines as i32 {
                                        let mut line = String::with_capacity(cols);
                                        for col in 0..cols {
                                            use alacritty_terminal::index::{Point, Line, Column};
                                            let cell = &grid[Point::new(Line(row), Column(col))];
                                            line.push(cell.c);
                                        }
                                        // Trim trailing spaces
                                        let trimmed = line.trim_end().to_string();
                                        lines.push(trimmed);
                                    }

                                    // Remove trailing empty lines
                                    while lines.last().map_or(false, |l| l.is_empty()) {
                                        lines.pop();
                                    }

                                    lines.join("\n")
                                });
                                CommandResult::Ok(Some(serde_json::json!({"content": content})))
                            }
                            None => CommandResult::Err(format!("terminal not found: {}", terminal_id)),
                        }
                    }
                    RemoteCommand::SplitTerminal { project_id, path, direction } => {
                        cx.update(|cx| {
                            workspace.update(cx, |ws, cx| {
                                if let Some(project) = ws.project_mut(&project_id) {
                                    if let Some(ref mut layout) = project.layout {
                                        if let Some(node) = layout.get_at_path_mut(&path) {
                                            let existing = node.clone();
                                            let new_terminal = crate::workspace::state::LayoutNode::new_terminal();
                                            *node = crate::workspace::state::LayoutNode::Split {
                                                direction,
                                                sizes: vec![0.5, 0.5],
                                                children: vec![existing, new_terminal],
                                            };
                                            cx.notify();
                                            return CommandResult::Ok(None);
                                        }
                                    }
                                }
                                CommandResult::Err(format!("project or path not found: {}:{:?}", project_id, path))
                            })
                        })
                    }
                    RemoteCommand::CloseTerminal { project_id, terminal_id } => {
                        cx.update(|cx| {
                            workspace.update(cx, |ws, cx| {
                                let path = ws.project(&project_id)
                                    .and_then(|p| p.layout.as_ref())
                                    .and_then(|layout| layout.find_terminal_path(&terminal_id));

                                match path {
                                    Some(path) => {
                                        ws.close_terminal(&project_id, &path, cx);
                                        CommandResult::Ok(None)
                                    }
                                    None => CommandResult::Err(format!("terminal not found: {}", terminal_id)),
                                }
                            })
                        })
                    }
                    RemoteCommand::FocusTerminal { project_id, terminal_id } => {
                        cx.update(|cx| {
                            workspace.update(cx, |ws, cx| {
                                let path = ws.project(&project_id)
                                    .and_then(|p| p.layout.as_ref())
                                    .and_then(|layout| layout.find_terminal_path(&terminal_id));

                                match path {
                                    Some(path) => {
                                        ws.set_focused_terminal(project_id, path, cx);
                                        CommandResult::Ok(None)
                                    }
                                    None => CommandResult::Err(format!("terminal not found: {}", terminal_id)),
                                }
                            })
                        })
                    }
                };

                let _ = msg.reply.send(result);
            }
        })
        .detach();
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

    fn handle_detached_terminals_changed(
        &mut self,
        workspace: Entity<Workspace>,
        cx: &mut Context<Self>,
    ) {
        let ws = workspace.read(cx);
        let current_detached: HashSet<String> = ws
            .detached_terminals
            .iter()
            .map(|d| d.terminal_id.clone())
            .collect();

        let new_detached: Vec<_> = ws
            .detached_terminals
            .iter()
            .filter(|d| !self.opened_detached_windows.contains(&d.terminal_id))
            .cloned()
            .collect();

        let reattached: Vec<_> = self
            .opened_detached_windows
            .iter()
            .filter(|id| !current_detached.contains(*id))
            .cloned()
            .collect();

        self.opened_detached_windows = current_detached;

        for detached in new_detached {
            self.open_detached_window(&detached.terminal_id, cx);
        }

        let _ = reattached;
    }

    fn open_detached_window(&self, terminal_id: &str, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let pty_manager = self.pty_manager.clone();
        let terminals = self.terminals.clone();
        let terminal_id_owned = terminal_id.to_string();

        let terminal_name = {
            let ws = workspace.read(cx);
            let mut name = terminal_id.chars().take(8).collect::<String>();
            for project in ws.projects() {
                if let Some(custom_name) = project.terminal_names.get(terminal_id) {
                    name = custom_name.clone();
                    break;
                }
            }
            name
        };

        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("{} - Detached", terminal_name).into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(800.0), px(600.0)),
                })),
                is_resizable: true,
                window_decorations: Some(WindowDecorations::Server),
                window_min_size: Some(Size {
                    width: px(300.0),
                    height: px(200.0),
                }),
                ..Default::default()
            },
            move |window, cx| {
                let detached_view = cx.new(|cx| {
                    DetachedTerminalView::new(
                        workspace.clone(),
                        terminal_id_owned.clone(),
                        pty_manager.clone(),
                        terminals.clone(),
                        cx,
                    )
                });
                cx.new(|cx| Root::new(detached_view, window, cx))
            },
        )
        .ok();
    }
}

impl Render for Okena {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.root_view.clone())
    }
}
