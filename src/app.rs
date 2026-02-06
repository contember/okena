use alacritty_terminal::grid::Dimensions;
use crate::remote::auth::AuthStore;
use crate::remote::bridge::{self, BridgeMessage, BridgeReceiver, CommandResult, RemoteCommand};
use crate::remote::pty_broadcaster::PtyBroadcaster;
use crate::remote::server::RemoteServer;
use crate::remote::types::{ApiFullscreen, ApiLayoutNode, ApiProject, StateResponse};
use crate::remote::{GlobalRemoteInfo, RemoteInfo};
use crate::settings::GlobalSettings;
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

        // Set up debounced auto-save on workspace changes
        let save_pending_for_observer = save_pending.clone();
        let workspace_for_save = workspace.clone();
        cx.observe(&workspace, move |_this, _workspace, cx| {
            save_pending_for_observer.store(true, Ordering::Relaxed);

            let save_pending = save_pending_for_observer.clone();
            let workspace = workspace_for_save.clone();
            cx.spawn(async move |_, cx| {
                smol::Timer::after(std::time::Duration::from_millis(500)).await;

                if save_pending.swap(false, Ordering::Relaxed) {
                    let data = cx.update(|cx| workspace.read(cx).data.clone());
                    if let Err(e) = persistence::save_workspace(&data) {
                        log::error!("Failed to save workspace: {}", e);
                    }
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

                            let fullscreen = ws.fullscreen_terminal.as_ref().map(|fs| {
                                ApiFullscreen {
                                    project_id: fs.project_id.clone(),
                                    terminal_id: fs.terminal_id.clone(),
                                }
                            });

                            let resp = StateResponse {
                                state_version: sv,
                                projects,
                                focused_project_id: ws.focused_project_id.clone(),
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
