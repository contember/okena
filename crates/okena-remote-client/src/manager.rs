use crate::connection::RemoteConnection;
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::TerminalBackend;
use okena_terminal::terminal::Terminal;
use okena_workspace::settings::{AppSettings, load_settings, update_remote_connections};
use okena_workspace::toast::{Toast, ToastManager};

use okena_core::api::{ActionRequest, ApiSystemStats, StateResponse};
use okena_core::soft_close::{
    SOFT_CLOSE_KILL_PREFIX, SOFT_CLOSE_UNDO_PREFIX, decode_action, encode_action,
};
use okena_transport::client::connection::try_refresh_token;
use okena_transport::client::{
    ConnectionEvent, ConnectionStatus, LOCAL_DAEMON_CONNECTION_ID, RemoteConnectionConfig,
    is_remote_terminal, make_prefixed_id, strip_prefix,
};

use gpui::*;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

struct QueuedAction {
    config: RemoteConnectionConfig,
    token: String,
    action: ActionRequest,
}

struct PasteUpload {
    endpoint: &'static str,
    content_type: String,
    extension: Option<String>,
    bytes: Vec<u8>,
}

struct ActionQueues {
    runtime: Arc<tokio::runtime::Runtime>,
    event_tx: async_channel::Sender<ConnectionEvent>,
    senders: parking_lot::Mutex<HashMap<String, async_channel::Sender<QueuedAction>>>,
}

impl ActionQueues {
    fn new(
        runtime: Arc<tokio::runtime::Runtime>,
        event_tx: async_channel::Sender<ConnectionEvent>,
    ) -> Self {
        Self {
            runtime,
            event_tx,
            senders: parking_lot::Mutex::new(HashMap::new()),
        }
    }

    fn enqueue(&self, connection_id: &str, action: QueuedAction) {
        let sender = self
            .senders
            .lock()
            .entry(connection_id.to_string())
            .or_insert_with(|| {
                let (tx, rx) = async_channel::unbounded();
                let connection_id = connection_id.to_string();
                let event_tx = self.event_tx.clone();
                self.runtime.spawn(async move {
                    run_action_queue(connection_id, rx, event_tx).await;
                });
                tx
            })
            .clone();

        if sender.try_send(action).is_err() {
            log::error!("action queue unexpectedly closed for {connection_id}");
        }
    }
}

async fn run_action_queue(
    connection_id: String,
    receiver: async_channel::Receiver<QueuedAction>,
    event_tx: async_channel::Sender<ConnectionEvent>,
) {
    let mut pending = None;
    loop {
        let mut action = match pending.take() {
            Some(action) => action,
            None => match receiver.recv().await {
                Ok(action) => action,
                Err(_) => break,
            },
        };
        if matches!(&action.action, ActionRequest::SetSettings { .. }) {
            while let Ok(next) = receiver.try_recv() {
                if matches!(&next.action, ActionRequest::SetSettings { .. }) {
                    action = next;
                } else {
                    pending = Some(next);
                    break;
                }
            }
        }
        send_queued_action(&connection_id, action, &event_tx).await;
    }
}

async fn send_queued_action(
    connection_id: &str,
    queued: QueuedAction,
    event_tx: &async_channel::Sender<ConnectionEvent>,
) {
    let QueuedAction {
        config,
        token,
        action,
    } = queued;
    let name = config.name.clone();
    let result = okena_transport::remote_action::post_action_async(&config, &token, action).await;

    let message = match result {
        Ok(_) => {
            log::debug!("send_action: success for {name}");
            return;
        }
        Err(error) => {
            log::error!("send_action: request error for {name}: {error}");
            format!("Action request failed: {error}")
        }
    };
    let _ = event_tx
        .send(ConnectionEvent::ServerWarning {
            connection_id: connection_id.to_string(),
            message,
        })
        .await;
}

/// Lightweight events emitted by [`RemoteConnectionManager`] that must NOT go
/// through `cx.notify()`.
///
/// `cx.notify()` on the manager fans out to the project-sync observer
/// (`WindowView::sync_remote_projects_into_workspace`), which clones every
/// connection's full `StateResponse` and diffs it into the workspace. That is
/// the right cost for a discrete state change, but ruinous at the cadence of
/// terminal output. These events let high-frequency, repaint-only signals
/// reach just the views that care (the sidebar) without triggering that sync.
pub enum RemoteManagerEvent {
    /// A remote terminal produced output / changed derived state (bell, idle).
    /// Subscribers should repaint indicators but must not re-sync project state.
    ///
    /// Carries the ids of the remote terminals whose `content_generation`
    /// advanced this wake. The sidebar ignores the payload (it re-reads every
    /// terminal's flags), but `Okena` uses it to drain OSC 9/777/99 + bell
    /// notifications for exactly those terminals — the daemon-client equivalent
    /// of the local PTY loop's `process_terminal_notifications` pass. Remote
    /// PTY output never goes through that loop (it arrives over the WS and is
    /// only buffered via `enqueue_output`), so without this the per-terminal
    /// notification queues would be parsed here but never fire an OS bubble.
    TerminalActivity(Vec<String>),

    /// An external API client asked the desktop to focus and raise an exact
    /// remote terminal. IDs are already prefixed for this manager connection.
    TerminalFocusRequested {
        project_id: String,
        terminal_id: String,
        window: Option<String>,
    },

    /// The implicit local-daemon loopback connection reached a terminal failed
    /// state (its own connect/reconnect retries are exhausted against a dead
    /// endpoint). The manager stays generic — it only reports; the app decides
    /// to re-run daemon discovery/ensure and re-point the connection. Emitted
    /// ONLY for `LOCAL_DAEMON_CONNECTION_ID`, never for user-managed remotes.
    LocalConnectionFailed,

    /// The local daemon published a new authoritative settings snapshot.
    SettingsChanged(Box<AppSettings>),
}

/// GPUI Entity managing all remote connections.
///
/// Observed by the Sidebar for rendering remote projects,
/// and by WindowView for focus coordination.
pub struct RemoteConnectionManager {
    connections: HashMap<String, RemoteConnection>,
    terminals: TerminalsRegistry,
    runtime: Arc<tokio::runtime::Runtime>,

    /// Channel for events coming from tokio tasks
    event_tx: async_channel::Sender<ConnectionEvent>,

    /// Per-connection FIFO queues for state-changing HTTP actions.
    action_queues: ActionQueues,

    /// Coalescing doorbell rung by the tokio reader whenever a remote terminal
    /// produces output. Capacity 1: a wake already pending absorbs further
    /// output until the GPUI side drains, so output bursts collapse into a
    /// single repaint pass. Handed to every connection's `ConnectionHandler`.
    activity_tx: async_channel::Sender<()>,
}

impl RemoteConnectionManager {
    pub fn new(terminals: TerminalsRegistry, cx: &mut Context<Self>) -> Self {
        #[allow(
            clippy::expect_used,
            reason = "tokio runtime build only fails on OS resource exhaustion at startup — nothing recoverable"
        )]
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("remote-client")
                .build()
                .expect("Failed to create tokio runtime for remote client"),
        );

        let (event_tx, event_rx) = async_channel::bounded::<ConnectionEvent>(256);

        // Spawn event processing loop
        cx.spawn({
            let event_rx = event_rx.clone();
            async move |this: WeakEntity<Self>, cx| {
                while let Ok(event) = event_rx.recv().await {
                    let should_continue = this
                        .update(cx, |this, cx| {
                            this.handle_event(event, cx);
                        })
                        .is_ok();
                    if !should_continue {
                        break;
                    }
                }
            }
        })
        .detach();

        // Coalescing doorbell for remote terminal output (see field docs).
        let (activity_tx, activity_rx) = async_channel::bounded::<()>(1);

        let action_queues = ActionQueues::new(runtime.clone(), event_tx.clone());
        let manager = Self {
            connections: HashMap::new(),
            terminals,
            runtime,
            event_tx,
            action_queues,
            activity_tx,
        };
        manager.start_terminal_activity_pump(activity_rx, cx);
        manager
    }

    /// Drive sidebar repaints from incoming remote terminal output — reactively,
    /// woken by the `activity_rx` doorbell rather than by polling.
    ///
    /// Remote output arrives on a tokio task that only buffers bytes via
    /// `Terminal::enqueue_output`; it cannot touch GPUI directly. Each enqueue
    /// rings a capacity-1 doorbell (`try_send`, so bursts coalesce). On every
    /// wake this drains and parses pending output for all remote terminals on
    /// the GPUI thread, then watches `content_generation` to identify which
    /// terminals advanced.
    ///
    /// `RemoteManagerEvent::TerminalActivity` carries those terminal ids to
    /// `WindowView`, which directly notifies their registered content panes and
    /// repaints each window's sidebar. This keeps mounted and background bell /
    /// idle state current without a per-pane 8 ms polling task. While idle this
    /// task parks on `recv()`, consuming no CPU.
    fn start_terminal_activity_pump(
        &self,
        activity_rx: async_channel::Receiver<()>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            // Per-terminal `content_generation` snapshots from the previous
            // wake. A terminal whose generation advanced (or that appeared /
            // disappeared) means derived state the sidebar reads may have
            // changed.
            let mut last_generations: HashMap<String, u64> = HashMap::new();

            while activity_rx.recv().await.is_ok() {
                let result = this.update(cx, |this, cx| {
                    let terminals: Vec<(String, Arc<Terminal>)> = {
                        let registry = this.terminals.lock();
                        registry
                            .iter()
                            .filter(|(id, _)| id.starts_with("remote:"))
                            .map(|(id, terminal)| (id.clone(), terminal.clone()))
                            .collect()
                    };

                    let mut next_generations = HashMap::with_capacity(terminals.len());
                    // Terminals whose generation advanced this wake — i.e. ones
                    // that actually parsed new output. `Okena` drains their
                    // notification/bell queues; an OSC alert or bell always
                    // bumps the generation (via `drain_pending_output`), so this
                    // set is a superset of the terminals that have something to
                    // fire.
                    let mut advanced: Vec<String> = Vec::new();
                    for (id, terminal) in &terminals {
                        // Consume the edge-triggered dirty marker before parsing.
                        // Any bytes arriving after this point enqueue another
                        // activity wake, so no per-pane polling is needed.
                        terminal.take_dirty();
                        // Parse on the GPUI thread so bell/idle flags are
                        // current even for terminals with no mounted pane.
                        terminal.process_pending_output();
                        okena_core::latency_probe::client_output_parsed(id);
                        let generation = terminal.content_generation();
                        if last_generations.get(id) != Some(&generation) {
                            advanced.push(id.clone());
                        }
                        next_generations.insert(id.clone(), generation);
                    }
                    let changed = activity_changed(&last_generations, &next_generations);
                    last_generations = next_generations;

                    if changed {
                        for terminal_id in &advanced {
                            okena_core::latency_probe::client_activity_emitted(terminal_id);
                        }
                        // Emit (not notify): repaint the sidebar's bell/idle
                        // indicators without dragging in the heavy project-sync
                        // observer that fires on `cx.notify()`, and let `Okena`
                        // fire OS notifications for the advanced terminals.
                        cx.emit(RemoteManagerEvent::TerminalActivity(advanced));
                    }
                });
                if result.is_err() {
                    break; // Entity dropped
                }
            }
        })
        .detach();
    }

    /// Check if a connection to the given host:port already exists.
    pub fn find_by_host_port(&self, host: &str, port: u16) -> Option<&str> {
        self.connections
            .values()
            .find(|c| c.config().host == host && c.config().port == port)
            .map(|c| c.config().name.as_str())
    }

    /// Add a new connection and start connecting.
    /// Returns Err if a connection to the same host:port already exists.
    pub fn add_connection(
        &mut self,
        config: RemoteConnectionConfig,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if let Some(name) = self.find_by_host_port(&config.host, config.port) {
            return Err(format!(
                "Already connected to {}:{} as '{}'",
                config.host, config.port, name
            ));
        }
        let id = config.id.clone();
        let mut conn = RemoteConnection::new(
            config,
            self.runtime.clone(),
            self.terminals.clone(),
            self.event_tx.clone(),
            self.activity_tx.clone(),
        );
        conn.connect();
        self.connections.insert(id, conn);
        cx.notify();
        Ok(())
    }

    /// Reconnect without discarding the last state or live terminal objects.
    pub fn reconnect(&mut self, connection_id: &str, cx: &mut Context<Self>) {
        if let Some(conn) = self.connections.get_mut(connection_id) {
            conn.reconnect();
            cx.notify();
        }
    }

    /// Re-point an existing connection at a (possibly new) local daemon + token, then
    /// reconnect. Used after a local-daemon restart: the replacement daemon may
    /// bind a DIFFERENT port (the old one can linger in TIME_WAIT), so a plain
    /// `reconnect` — which reuses the old config — could dial a dead endpoint.
    /// The caller re-reads `remote.json` and passes the full fresh config here.
    ///
    /// `connect()` clones the config at call time, so replacing it first and
    /// reconnecting picks up the new endpoint. The token usually survives a
    /// restart (the daemon reloads `remote_tokens.json` at startup), so `token`
    /// is normally the existing one; it is refreshed here for completeness. Does
    /// nothing if the connection id is unknown.
    pub fn redirect_and_reconnect(
        &mut self,
        connection_id: &str,
        next_config: RemoteConnectionConfig,
        token: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(conn) = self.connections.get_mut(connection_id) {
            *conn.config_mut() = next_config;
            if let Some(token) = token {
                conn.config_mut().saved_token = Some(token);
            }
            conn.reconnect();
            cx.notify();
        }
    }

    /// Remove a connection (disconnects first).
    pub fn remove_connection(&mut self, connection_id: &str, cx: &mut Context<Self>) {
        if let Some(mut conn) = self.connections.remove(connection_id) {
            conn.disconnect();
        }
        // Remove from saved settings (off GPUI thread)
        let id = connection_id.to_string();
        cx.background_executor()
            .spawn(async move {
                let _ = update_remote_connections(|conns| conns.retain(|c| c.id != id));
            })
            .detach();
        cx.notify();
    }

    /// Get a handle to the tokio runtime (for running reqwest in dialogs).
    pub fn runtime(&self) -> Arc<tokio::runtime::Runtime> {
        self.runtime.clone()
    }

    /// Pair with a remote server using a code.
    pub fn pair(&mut self, connection_id: &str, code: &str, cx: &mut Context<Self>) {
        if let Some(conn) = self.connections.get_mut(connection_id) {
            conn.pair(code);
            cx.notify();
        }
    }

    /// Flip a saved connection's TLS flag and persist it, clearing any stale pin
    /// so the next pairing captures a fresh one. Does not reconnect/re-pair — the
    /// caller typically follows this by opening the pair dialog.
    pub fn set_connection_tls(&mut self, connection_id: &str, tls: bool, cx: &mut Context<Self>) {
        if let Some(conn) = self.connections.get_mut(connection_id) {
            conn.config_mut().tls = tls;
            conn.config_mut().pinned_cert_sha256 = None;
        }
        let id = connection_id.to_string();
        cx.background_executor()
            .spawn(async move {
                let _ = update_remote_connections(|conns| {
                    if let Some(c) = conns.iter_mut().find(|c| c.id == id) {
                        c.tls = tls;
                        c.pinned_cert_sha256 = None;
                    }
                });
            })
            .detach();
        cx.notify();
    }

    /// Get all connections for sidebar rendering.
    pub fn connections(
        &self,
    ) -> Vec<(
        &RemoteConnectionConfig,
        &ConnectionStatus,
        Option<&StateResponse>,
    )> {
        self.connections
            .values()
            .map(|conn| (conn.config(), conn.status(), conn.remote_state()))
            .collect()
    }

    pub fn connections_with_system_stats(
        &self,
    ) -> Vec<(
        &RemoteConnectionConfig,
        &ConnectionStatus,
        Option<&StateResponse>,
        Option<&ApiSystemStats>,
    )> {
        self.connections
            .values()
            .map(|conn| {
                (
                    conn.config(),
                    conn.status(),
                    conn.remote_state(),
                    conn.system_stats(),
                )
            })
            .collect()
    }

    /// Get the backend for a specific connection.
    pub fn backend_for(&self, connection_id: &str) -> Option<Arc<dyn TerminalBackend>> {
        self.connections
            .get(connection_id)
            .map(|conn| conn.backend())
    }

    /// Get the remote state for a specific connection.
    #[allow(dead_code)]
    pub fn remote_state(&self, connection_id: &str) -> Option<&StateResponse> {
        self.connections
            .get(connection_id)
            .and_then(|conn| conn.remote_state())
    }

    /// Auto-connect to all saved connections with valid tokens.
    pub fn auto_connect_all(&mut self, cx: &mut Context<Self>) {
        let settings = load_settings();
        for config in settings.remote_connections {
            if config.saved_token.is_some()
                && !self.connections.contains_key(&config.id)
                && self.find_by_host_port(&config.host, config.port).is_none()
            {
                let id = config.id.clone();
                let mut conn = RemoteConnection::new(
                    config,
                    self.runtime.clone(),
                    self.terminals.clone(),
                    self.event_tx.clone(),
                    self.activity_tx.clone(),
                );
                conn.connect();
                self.connections.insert(id, conn);
            }
        }
        cx.notify();
    }

    /// Tell every connection which of its projects this client renders.
    ///
    /// `visible_ids` are client-side (prefixed) project ids; each connection
    /// receives only its own, unprefixed. A connection with nothing visible is
    /// told so explicitly, so the server drops it from the `gh` PR/CI scope
    /// instead of keeping a stale viewport. Sorted for a stable no-op check.
    pub fn publish_visible_projects(&self, visible_ids: &HashSet<String>) {
        for (connection_id, connection) in &self.connections {
            let mut project_ids: Vec<String> = visible_ids
                .iter()
                .filter(|id| is_remote_terminal(id, connection_id))
                .map(|id| strip_prefix(id, connection_id))
                .collect();
            project_ids.sort();
            connection.set_visible_projects(project_ids);
        }
    }

    /// Send an action to a remote server via HTTP POST /v1/actions.
    ///
    /// Fire-and-forget from the UI thread, but FIFO within each connection.
    pub fn send_action(&self, connection_id: &str, action: ActionRequest, cx: &mut Context<Self>) {
        let config = match self.connections.get(connection_id) {
            Some(conn) => conn.config().clone(),
            None => {
                log::error!("send_action: connection {} not found", connection_id);
                return;
            }
        };
        let token = match config.effective_auth_token() {
            Some(t) => t,
            None => {
                log::error!(
                    "send_action: no auth token for connection {}",
                    connection_id
                );
                ToastManager::error("No auth token for remote connection".to_string(), cx);
                return;
            }
        };

        self.action_queues.enqueue(
            connection_id,
            QueuedAction {
                config,
                token,
                action,
            },
        );
    }

    /// Upload a pasted clipboard image to the remote server, which writes it to
    /// a temp file on its own filesystem and bracketed-pastes that path into the
    /// terminal (so a server-side TUI like Claude Code can read it).
    ///
    /// `terminal_id` is the server-local id (already stripped of the
    /// `remote:{cid}:` prefix). Mirrors [`send_action`]'s fire-and-forget HTTP
    /// pattern: spawns on the tokio runtime, logs errors and warns on failure.
    pub fn upload_paste_image(
        &self,
        connection_id: &str,
        terminal_id: &str,
        mime: &str,
        bytes: Vec<u8>,
        cx: &mut Context<Self>,
    ) {
        self.upload_pastes(
            connection_id,
            terminal_id,
            "Image paste",
            vec![PasteUpload {
                endpoint: "paste-image",
                content_type: mime.to_string(),
                extension: None,
                bytes,
            }],
            cx,
        );
    }

    /// Upload dropped files sequentially so their pasted paths preserve order.
    pub fn upload_paste_files(
        &self,
        connection_id: &str,
        terminal_id: &str,
        files: Vec<(String, Vec<u8>)>,
        cx: &mut Context<Self>,
    ) {
        let uploads = files
            .into_iter()
            .map(|(extension, bytes)| PasteUpload {
                endpoint: "paste-file",
                content_type: "application/octet-stream".to_string(),
                extension: Some(extension),
                bytes,
            })
            .collect();
        self.upload_pastes(connection_id, terminal_id, "File drop", uploads, cx);
    }

    fn upload_pastes(
        &self,
        connection_id: &str,
        terminal_id: &str,
        label: &'static str,
        uploads: Vec<PasteUpload>,
        cx: &mut Context<Self>,
    ) {
        if uploads.is_empty() {
            return;
        }
        let config = match self.connections.get(connection_id) {
            Some(conn) => conn.config().clone(),
            None => {
                log::error!("paste upload: connection {} not found", connection_id);
                return;
            }
        };
        let token = match config.effective_auth_token() {
            Some(t) => t,
            None => {
                log::error!(
                    "paste upload: no auth token for connection {}",
                    connection_id
                );
                ToastManager::error("No auth token for remote connection".to_string(), cx);
                return;
            }
        };

        let name = config.name.clone();
        let event_tx = self.event_tx.clone();
        let connection_id = connection_id.to_string();
        let terminal_id = terminal_id.to_string();

        self.runtime.spawn(async move {
            let (client, base_url) =
                match okena_transport::remote_http::async_client_and_url(&config, "") {
                    Ok(client_and_url) => client_and_url,
                    Err(error) => {
                        let _ = event_tx.try_send(ConnectionEvent::ServerWarning {
                            connection_id,
                            message: format!("{label} client initialisation failed: {error}"),
                        });
                        return;
                    }
                };
            for upload in uploads {
                let url = format!("{base_url}/v1/terminals/{terminal_id}/{}", upload.endpoint);
                let mut request = client
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", token))
                    .header("Content-Type", upload.content_type)
                    .body(upload.bytes)
                    .timeout(std::time::Duration::from_secs(90));
                if let Some(extension) = upload.extension {
                    request = request.header("X-Okena-File-Extension", extension);
                }
                let result = request.send().await;

                let message = match result {
                    Ok(response) if response.status().is_success() => {
                        log::debug!("paste upload: success for {}", name);
                        continue;
                    }
                    Ok(response) => {
                        let status = response.status();
                        let body = response.text().await.unwrap_or_default();
                        log::error!("paste upload failed ({status}): {body} for {name}");
                        format!("{label} failed ({status}): {body}")
                    }
                    Err(error) => {
                        log::error!("paste upload request error for {name}: {error}");
                        format!("{label} request failed: {error}")
                    }
                };
                let _ = event_tx.try_send(ConnectionEvent::ServerWarning {
                    connection_id: connection_id.clone(),
                    message,
                });
            }
        });
    }

    /// Handle an event from a connection's tokio task.
    fn handle_event(&mut self, event: ConnectionEvent, cx: &mut Context<Self>) {
        let event_label: &'static str = match &event {
            ConnectionEvent::StatusChanged { .. } => "StatusChanged",
            ConnectionEvent::TokenObtained { .. } => "TokenObtained",
            ConnectionEvent::TlsUpgraded { .. } => "TlsUpgraded",
            ConnectionEvent::StateReceived { .. } => "StateReceived",
            ConnectionEvent::SettingsChanged { .. } => "SettingsChanged",
            ConnectionEvent::SubscriptionMappings { .. } => "SubscriptionMappings",
            ConnectionEvent::GitStatusChanged { .. } => "GitStatusChanged",
            ConnectionEvent::SystemStatsChanged { .. } => "SystemStatsChanged",
            ConnectionEvent::Toast { .. } => "Toast",
            ConnectionEvent::TerminalFocusRequested { .. } => "TerminalFocusRequested",
            ConnectionEvent::ServerWarning { .. } => "ServerWarning",
            ConnectionEvent::TokenRefreshed { .. } => "TokenRefreshed",
        };
        let _slow = okena_core::timing::SlowGuard::with_detail(
            "RemoteConnectionManager::handle_event",
            event_label,
        );
        match event {
            ConnectionEvent::StatusChanged {
                connection_id,
                status,
            } => {
                if let Some(conn) = self.connections.get_mut(&connection_id) {
                    let prev = std::mem::replace(conn.status_mut(), status.clone());
                    let name = &conn.config().name;
                    match &status {
                        ConnectionStatus::Error(msg) => {
                            ToastManager::error(format!("{}: {}", name, msg), cx);
                        }
                        ConnectionStatus::Reconnecting { attempt: 1 } => {
                            ToastManager::warning(
                                format!("{}: Connection lost, reconnecting...", name),
                                cx,
                            );
                        }
                        ConnectionStatus::Connected
                            if matches!(prev, ConnectionStatus::Reconnecting { .. }) =>
                        {
                            ToastManager::info(format!("{}: Reconnected", name), cx);
                        }
                        _ => {}
                    }
                }
                // The local daemon backs the whole GUI; when its connection
                // dead-ends, ask the app to self-heal (re-run discovery/ensure).
                if is_local_connection_terminal_failure(&connection_id, &status) {
                    cx.emit(RemoteManagerEvent::LocalConnectionFailed);
                }
                cx.notify();
            }
            ConnectionEvent::TokenObtained {
                connection_id,
                token,
                cert_fingerprint,
            } => {
                let now = now_unix_timestamp();
                if let Some(conn) = self.connections.get_mut(&connection_id) {
                    conn.config_mut().saved_token = Some(token.clone());
                    conn.config_mut().token_obtained_at = Some(now);
                    // Pin the cert on first successful TLS pairing (TOFU).
                    if cert_fingerprint.is_some() {
                        conn.config_mut().pinned_cert_sha256 = cert_fingerprint.clone();
                    }
                }
                // Persist token (+ pinned cert) to settings (off GPUI thread)
                let cid = connection_id.clone();
                let tok = token.clone();
                let fp = cert_fingerprint.clone();
                cx.background_executor()
                    .spawn(async move {
                        let _ = update_remote_connections(|conns| {
                            if let Some(saved) = conns.iter_mut().find(|c| c.id == cid) {
                                saved.saved_token = Some(tok);
                                saved.token_obtained_at = Some(now);
                                if fp.is_some() {
                                    saved.pinned_cert_sha256 = fp;
                                }
                            }
                        });
                    })
                    .detach();
                cx.notify();
            }
            ConnectionEvent::TlsUpgraded {
                connection_id,
                cert_fingerprint,
            } => {
                if let Some(conn) = self.connections.get_mut(&connection_id) {
                    conn.config_mut().tls = true;
                    conn.config_mut().pinned_cert_sha256 = cert_fingerprint.clone();
                }
                let cid = connection_id.clone();
                let fp = cert_fingerprint.clone();
                cx.background_executor()
                    .spawn(async move {
                        let _ = update_remote_connections(|conns| {
                            if let Some(saved) = conns.iter_mut().find(|c| c.id == cid) {
                                saved.tls = true;
                                saved.pinned_cert_sha256 = fp;
                            }
                        });
                    })
                    .detach();
                cx.notify();
            }
            ConnectionEvent::StateReceived {
                connection_id,
                state,
            } => {
                if let Some(conn) = self.connections.get_mut(&connection_id) {
                    conn.set_remote_state(Some(state));
                }
                cx.notify();
            }
            ConnectionEvent::SettingsChanged {
                connection_id,
                settings,
            } => {
                if connection_id == LOCAL_DAEMON_CONNECTION_ID {
                    match serde_json::from_value::<AppSettings>(settings) {
                        Ok(settings) => {
                            cx.emit(RemoteManagerEvent::SettingsChanged(Box::new(settings)))
                        }
                        Err(error) => {
                            log::warn!("Failed to decode daemon settings: {error}");
                        }
                    }
                }
            }
            ConnectionEvent::SubscriptionMappings {
                connection_id,
                mappings,
            } => {
                if let Some(conn) = self.connections.get_mut(&connection_id) {
                    conn.update_stream_mappings(mappings);
                }
            }
            ConnectionEvent::GitStatusChanged {
                connection_id,
                statuses,
            } => {
                if let Some(conn) = self.connections.get_mut(&connection_id)
                    && let Some(state) = conn.remote_state_mut()
                {
                    for project in &mut state.projects {
                        project.git_status = statuses.get(&project.id).cloned();
                    }
                }
                cx.notify();
            }
            ConnectionEvent::SystemStatsChanged {
                connection_id,
                stats,
            } => {
                if let Some(conn) = self.connections.get_mut(&connection_id) {
                    conn.set_system_stats(Some(stats));
                }
            }
            ConnectionEvent::TerminalFocusRequested {
                connection_id,
                request,
            } => {
                cx.emit(RemoteManagerEvent::TerminalFocusRequested {
                    project_id: make_prefixed_id(&connection_id, &request.project_id),
                    terminal_id: make_prefixed_id(&connection_id, &request.terminal_id),
                    window: request.window,
                });
            }
            ConnectionEvent::Toast {
                connection_id,
                mut toast,
            } => {
                // A daemon-originated toast: reconstruct the local `Toast` (fresh
                // `created` timestamp, ttl from `ttl_ms`) and show it the same way
                // local toasts are shown.
                //
                // Daemon toasts carry daemon-side project/terminal ids in their
                // soft-close action ids; prefix them with this connection so the
                // GUI's dispatcher routing + prefix-strip-on-dispatch line up.
                for action in &mut toast.actions {
                    for prefix in [SOFT_CLOSE_UNDO_PREFIX, SOFT_CLOSE_KILL_PREFIX] {
                        if let Some((p, t)) = decode_action(&action.id, prefix) {
                            action.id = encode_action(
                                prefix,
                                &make_prefixed_id(&connection_id, &p),
                                &make_prefixed_id(&connection_id, &t),
                            );
                            break;
                        }
                    }
                }
                ToastManager::post(Toast::from_api(&toast), cx);
            }
            ConnectionEvent::ServerWarning {
                connection_id,
                message,
            } => {
                let name = self
                    .connections
                    .get(&connection_id)
                    .map(|c| c.config().name.as_str())
                    .unwrap_or("Remote");
                ToastManager::warning(format!("{}: {}", name, message), cx);
            }
            ConnectionEvent::TokenRefreshed {
                connection_id,
                token,
            } => {
                let now = now_unix_timestamp();
                if let Some(conn) = self.connections.get_mut(&connection_id) {
                    conn.config_mut().saved_token = Some(token.clone());
                    conn.config_mut().token_obtained_at = Some(now);
                    conn.update_shared_token(&token);
                }
                let cid = connection_id.clone();
                let tok = token.clone();
                cx.background_executor()
                    .spawn(async move {
                        let _ = update_remote_connections(|conns| {
                            if let Some(saved) = conns.iter_mut().find(|c| c.id == cid) {
                                saved.saved_token = Some(tok);
                                saved.token_obtained_at = Some(now);
                            }
                        });
                    })
                    .detach();
            }
        }
    }

    /// Start a periodic token refresh task.
    /// Checks every 10 minutes and refreshes tokens older than 3 days.
    pub fn start_token_refresh_task(&self, cx: &mut Context<Self>) {
        let event_tx = self.event_tx.clone();
        let runtime = self.runtime.clone();

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                // Sleep 10 minutes between checks
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(600))
                    .await;

                // Collect configs of Connected connections
                let configs: Vec<RemoteConnectionConfig> = match this.update(cx, |this, _cx| {
                    this.connections
                        .values()
                        .filter(|c| matches!(c.status(), ConnectionStatus::Connected))
                        .map(|c| c.config().clone())
                        .collect()
                }) {
                    Ok(configs) => configs,
                    Err(_) => break, // Entity dropped
                };

                // Try refresh for each (runs on tokio runtime)
                for config in configs {
                    let event_tx = event_tx.clone();
                    runtime.spawn(async move {
                        try_refresh_token(&config, &event_tx).await;
                    });
                }
            }
        })
        .detach();
    }
}

impl EventEmitter<RemoteManagerEvent> for RemoteConnectionManager {}

/// Decide whether the terminal-activity pump should repaint, given the previous
/// and current per-terminal `content_generation` snapshots.
///
/// Returns true when any terminal's generation advanced (new output parsed) or
/// when a terminal appeared/disappeared. A pure helper so the change-detection
/// branches stay testable without a live GPUI/tokio stack.
fn activity_changed(last: &HashMap<String, u64>, current: &HashMap<String, u64>) -> bool {
    // A pure removal (terminal gone, none added) won't show up when scanning
    // `current`, so compare counts first.
    if last.len() != current.len() {
        return true;
    }
    current
        .iter()
        .any(|(id, generation)| last.get(id) != Some(generation))
}

/// Whether a status change should trigger local-daemon recovery.
///
/// True only for the implicit local-daemon loopback connection reaching the
/// terminal `Error` state — the two dead-end paths in the client engine
/// (initial connect exhausting its attempts, and the WS reconnect loop
/// exhausting its attempts) both land here. User-managed remotes and every
/// non-terminal state (Connecting/Pairing/Reconnecting/Connected/Disconnected)
/// return false, so a normal reconnect — or `remove_connection`, which sets
/// Disconnected without emitting Error — never provokes recovery. Pure so the
/// decision is testable without a live GPUI/tokio stack.
fn is_local_connection_terminal_failure(connection_id: &str, status: &ConnectionStatus) -> bool {
    connection_id == LOCAL_DAEMON_CONNECTION_ID && matches!(status, ConnectionStatus::Error(_))
}

fn now_unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::{
        ActionQueues, QueuedAction, RemoteConnectionManager, activity_changed,
        is_local_connection_terminal_failure,
    };
    use okena_core::api::ActionRequest;
    use okena_transport::client::{ConnectionStatus, LOCAL_DAEMON_CONNECTION_ID};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::{Duration, Instant};

    fn accept_until(listener: &TcpListener, deadline: Instant) -> TcpStream {
        loop {
            match listener.accept() {
                Ok((stream, _)) => return stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "timed out waiting for request");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("failed to accept request: {error}"),
            }
        }
    }

    fn read_request_body(stream: &mut TcpStream) -> String {
        let mut reader = BufReader::new(&mut *stream);
        let mut content_length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            let lowercase = line.to_ascii_lowercase();
            if let Some(value) = lowercase.strip_prefix("content-length:") {
                content_length = value.trim().parse().unwrap();
            }
        }
        let mut body = vec![0; content_length];
        reader.read_exact(&mut body).unwrap();
        String::from_utf8(body).unwrap()
    }

    fn respond_ok(stream: &mut TcpStream) {
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
    }

    #[test]
    fn local_error_status_triggers_recovery() {
        assert!(is_local_connection_terminal_failure(
            LOCAL_DAEMON_CONNECTION_ID,
            &ConnectionStatus::Error("dead socket".into()),
        ));
    }

    #[test]
    fn local_non_error_states_do_not_trigger_recovery() {
        // Reconnecting/Connected/etc. are transient or healthy — not dead-ends.
        // Disconnected is what `remove_connection` (on quit) leaves behind, so
        // it must never look like a failure.
        for status in [
            ConnectionStatus::Disconnected,
            ConnectionStatus::Connecting,
            ConnectionStatus::Pairing,
            ConnectionStatus::Connected,
            ConnectionStatus::Reconnecting { attempt: 3 },
        ] {
            assert!(!is_local_connection_terminal_failure(
                LOCAL_DAEMON_CONNECTION_ID,
                &status
            ));
        }
    }

    #[test]
    fn user_remote_error_does_not_trigger_recovery() {
        // A user-managed remote failing is surfaced as a toast only; recovery is
        // reserved for the daemon the GUI depends on.
        assert!(!is_local_connection_terminal_failure(
            "some-user-remote",
            &ConnectionStatus::Error("gone".into()),
        ));
    }

    #[test]
    fn action_queue_waits_for_each_response_before_sending_the_next_action() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut first = accept_until(&listener, deadline);
            assert!(read_request_body(&mut first).contains("first"));

            std::thread::sleep(Duration::from_millis(100));
            match listener.accept() {
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Ok(_) => panic!("second action started before the first response"),
                Err(error) => panic!("failed to inspect action queue: {error}"),
            }
            respond_ok(&mut first);

            let mut second = accept_until(&listener, deadline);
            assert!(read_request_body(&mut second).contains("second"));
            respond_ok(&mut second);
        });

        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let (event_tx, _event_rx) = async_channel::unbounded();
        let queues = ActionQueues::new(runtime, event_tx);
        let mut config = make_config("127.0.0.1", port);
        config.name = "ordered-test".to_string();
        for terminal_id in ["first", "second"] {
            queues.enqueue(
                "connection",
                QueuedAction {
                    config: config.clone(),
                    token: "token".to_string(),
                    action: ActionRequest::SendText {
                        terminal_id: terminal_id.to_string(),
                        text: "input".to_string(),
                    },
                },
            );
        }

        server.join().unwrap();
    }

    fn gens(pairs: &[(&str, u64)]) -> HashMap<String, u64> {
        pairs.iter().map(|(id, g)| (id.to_string(), *g)).collect()
    }

    #[test]
    fn activity_changed_detects_generation_advance() {
        let last = gens(&[("a", 1), ("b", 5)]);
        let current = gens(&[("a", 1), ("b", 6)]); // b produced output
        assert!(activity_changed(&last, &current));
    }

    #[test]
    fn activity_changed_false_when_idle() {
        let last = gens(&[("a", 1), ("b", 5)]);
        let current = gens(&[("a", 1), ("b", 5)]);
        assert!(!activity_changed(&last, &current));
    }

    #[test]
    fn activity_changed_detects_added_and_removed_terminals() {
        let last = gens(&[("a", 1)]);
        assert!(activity_changed(&last, &gens(&[("a", 1), ("b", 1)]))); // added
        assert!(activity_changed(&last, &gens(&[]))); // removed
    }

    #[test]
    fn activity_changed_detects_swap_at_equal_count() {
        // One terminal removed, another added in the same tick — counts match
        // but the new id isn't in the previous snapshot.
        let last = gens(&[("a", 3)]);
        let current = gens(&[("b", 3)]);
        assert!(activity_changed(&last, &current));
    }
    use gpui::AppContext as _;
    use okena_terminal::TerminalsRegistry;
    use okena_transport::client::RemoteConnectionConfig;
    use parking_lot::Mutex as PMutex;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn make_config(host: &str, port: u16) -> RemoteConnectionConfig {
        RemoteConnectionConfig {
            id: uuid::Uuid::new_v4().to_string(),
            name: format!("{}:{}", host, port),
            host: host.to_string(),
            port,
            saved_token: None,
            token_obtained_at: None,
            tls: false,
            pinned_cert_sha256: None,
            local_endpoint: None,
        }
    }

    fn make_terminals() -> TerminalsRegistry {
        Arc::new(PMutex::new(HashMap::new()))
    }

    #[gpui::test]
    fn test_add_duplicate_connection_returns_err(cx: &mut gpui::TestAppContext) {
        let terminals = make_terminals();
        let manager = cx.new(|cx| RemoteConnectionManager::new(terminals, cx));

        let config1 = make_config("192.168.1.10", 19100);
        let config2 = make_config("192.168.1.10", 19100); // same host:port, different ID

        manager.update(cx, |rm, cx| {
            assert!(rm.add_connection(config1, cx).is_ok());
        });

        manager.update(cx, |rm, cx| {
            let result = rm.add_connection(config2, cx);
            assert!(result.is_err(), "duplicate host:port should be rejected");
            assert!(result.unwrap_err().contains("Already connected"));
        });
    }

    #[gpui::test]
    fn test_add_different_host_port_returns_ok(cx: &mut gpui::TestAppContext) {
        let terminals = make_terminals();
        let manager = cx.new(|cx| RemoteConnectionManager::new(terminals, cx));

        let config1 = make_config("192.168.1.10", 19100);
        let config2 = make_config("192.168.1.11", 19100); // different host
        let config3 = make_config("192.168.1.10", 19101); // different port

        manager.update(cx, |rm, cx| {
            assert!(rm.add_connection(config1, cx).is_ok());
            assert!(rm.add_connection(config2, cx).is_ok());
            assert!(rm.add_connection(config3, cx).is_ok());
        });
    }
}
