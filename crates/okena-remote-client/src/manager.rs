use crate::connection::RemoteConnection;
use okena_terminal::backend::TerminalBackend;
use okena_terminal::terminal::Terminal;
use okena_workspace::toast::{Toast, ToastManager};
use okena_terminal::TerminalsRegistry;
use okena_workspace::settings::{load_settings, update_remote_connections, AppSettings};

use okena_core::api::{ActionRequest, ApiSystemStats, StateResponse};
use okena_core::soft_close::{
    decode_action, encode_action, SOFT_CLOSE_KILL_PREFIX, SOFT_CLOSE_UNDO_PREFIX,
};
use okena_transport::client::LocalEndpoint;
use okena_transport::client::{
    make_prefixed_id, ConnectionEvent, ConnectionStatus, RemoteConnectionConfig,
    LOCAL_DAEMON_CONNECTION_ID,
};
use okena_transport::client::connection::try_refresh_token;

use gpui::*;
use std::collections::HashMap;
use std::sync::Arc;

struct QueuedAction {
    config: RemoteConnectionConfig,
    token: String,
    action: ActionRequest,
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
    let (client, url) = http_client_and_url(&config, "/v1/actions");
    let result = client
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .json(&action)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await;

    let message = match result {
        Ok(resp) if resp.status().is_success() => {
            log::debug!("send_action: success for {name}");
            return;
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            log::error!("send_action: failed ({status}): {body} for {name}");
            format!("Action failed ({status}): {body}")
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

fn http_client_and_url(config: &RemoteConnectionConfig, path: &str) -> (reqwest::Client, String) {
    #[cfg(unix)]
    if let Some(LocalEndpoint::UnixSocket { path: socket_path }) = &config.local_endpoint {
        let client = reqwest::Client::builder()
            .unix_socket(socket_path.as_str())
            .build()
            .unwrap_or_else(|e| {
                log::error!("Failed to build Unix socket HTTP client for {socket_path}: {e}");
                reqwest::Client::new()
            });
        return (client, config.http_url(path));
    }

    // A TLS remote uses a self-signed cert pinned by fingerprint (TOFU). A plain
    // `reqwest::Client::new()` validates against the system trust store and
    // rejects it with "error sending request", so every action would fail even
    // though the WS stream (which uses the pinned connector) works. Build the
    // same pinned client here.
    let client = okena_transport::client::tls::build_reqwest_client(
        config.tls,
        config.pinned_cert_sha256.clone(),
        okena_transport::client::tls::new_observed(),
    );
    (client, config.http_url(path))
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
    /// `Terminal::enqueue_output` — it never touches GPUI. The per-pane dirty
    /// loop (`TerminalPane::start_remote_dirty_check_loop`) repaints the
    /// *focused* terminal grid, but two server-driven indicators are left
    /// stale until unrelated local input forces a global repaint (issue #128):
    ///
    /// 1. **Background (unmounted) terminals never get parsed.** A sidebar
    ///    entry whose pane isn't mounted has no per-pane loop, so its pending
    ///    bytes are never drained — `has_bell()` stays false and the bell
    ///    badge never appears.
    /// 2. **The sidebar is never notified.** It reads bell/idle straight from
    ///    the `TerminalsRegistry` (a plain `Arc<Mutex<..>>`, invisible to
    ///    GPUI's automatic per-entity dependency tracking), so nothing tells
    ///    it to re-render when a terminal's derived state changes.
    ///
    /// Each `enqueue_output` rings the capacity-1 doorbell (`try_send`, so
    /// bursts coalesce). On every wake this drains+parses pending output for all
    /// remote terminals on the GPUI thread (fixing #1) and watches
    /// `content_generation` to confirm something actually advanced — regardless
    /// of whether the per-pane loop also drained it. When so it emits
    /// `RemoteManagerEvent::TerminalActivity`, which repaints every window's
    /// sidebar via the subscription in `WindowView::set_remote_manager`
    /// (fixing #2). Idle ⇒ the task simply parks on `recv()`, no CPU.
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
                        // Parse on the GPUI thread so bell/idle flags are
                        // current even for terminals with no mounted pane.
                        // No-op when the pending buffer is empty.
                        terminal.process_pending_output();
                        let generation = terminal.content_generation();
                        if last_generations.get(id) != Some(&generation) {
                            advanced.push(id.clone());
                        }
                        next_generations.insert(id.clone(), generation);
                    }
                    let changed = activity_changed(&last_generations, &next_generations);
                    last_generations = next_generations;

                    if changed {
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

    /// Send an action to a remote server via HTTP POST /v1/actions.
    ///
    /// Fire-and-forget from the UI thread, but FIFO within each connection.
    pub fn send_action(
        &self,
        connection_id: &str,
        action: ActionRequest,
        cx: &mut Context<Self>,
    ) {
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
                log::error!("send_action: no auth token for connection {}", connection_id);
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
        let config = match self.connections.get(connection_id) {
            Some(conn) => conn.config().clone(),
            None => {
                log::error!("upload_paste_image: connection {} not found", connection_id);
                return;
            }
        };
        let token = match config.effective_auth_token() {
            Some(t) => t,
            None => {
                log::error!(
                    "upload_paste_image: no auth token for connection {}",
                    connection_id
                );
                ToastManager::error("No auth token for remote connection".to_string(), cx);
                return;
            }
        };

        let name = config.name.clone();
        let event_tx = self.event_tx.clone();
        let terminal_id = terminal_id.to_string();
        let mime = mime.to_string();

        self.runtime.spawn(async move {
            let (client, url) = http_client_and_url(
                &config,
                &format!("/v1/terminals/{terminal_id}/paste-image"),
            );
            let result = client
                .post(&url)
                .header("Authorization", format!("Bearer {}", token))
                .header("Content-Type", mime)
                .body(bytes)
                .timeout(std::time::Duration::from_secs(15))
                .send()
                .await;

            match result {
                Ok(resp) if resp.status().is_success() => {
                    log::debug!("upload_paste_image: success for {}", name);
                }
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    log::error!(
                        "upload_paste_image: failed ({}): {} for {}",
                        status, body, name
                    );
                    let _ = event_tx.try_send(ConnectionEvent::ServerWarning {
                        connection_id: String::new(),
                        message: format!("Image paste failed ({}): {}", status, body),
                    });
                }
                Err(e) => {
                    log::error!("upload_paste_image: request error for {}: {}", name, e);
                    let _ = event_tx.try_send(ConnectionEvent::ServerWarning {
                        connection_id: String::new(),
                        message: format!("Image paste request failed: {}", e),
                    });
                }
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
                    && let Some(state) = conn.remote_state_mut() {
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
        activity_changed, is_local_connection_terminal_failure, ActionQueues, QueuedAction,
        RemoteConnectionManager,
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
    use okena_terminal::TerminalsRegistry;
    use gpui::AppContext as _;
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
