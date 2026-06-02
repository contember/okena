use crate::connection::RemoteConnection;
use okena_terminal::backend::TerminalBackend;
use okena_terminal::terminal::Terminal;
use okena_workspace::toast::ToastManager;
use okena_terminal::TerminalsRegistry;
use okena_workspace::settings::{load_settings, update_remote_connections};

use okena_core::api::{ActionRequest, StateResponse};
use okena_core::client::{
    ConnectionEvent, ConnectionStatus, RemoteConnectionConfig,
};
use okena_core::client::connection::try_refresh_token;

use gpui::*;
use std::collections::HashMap;
use std::sync::Arc;

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
    TerminalActivity,
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

        let manager = Self {
            connections: HashMap::new(),
            terminals,
            runtime,
            event_tx,
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
                    for (id, terminal) in &terminals {
                        // Parse on the GPUI thread so bell/idle flags are
                        // current even for terminals with no mounted pane.
                        // No-op when the pending buffer is empty.
                        terminal.process_pending_output();
                        next_generations.insert(id.clone(), terminal.content_generation());
                    }
                    let changed = activity_changed(&last_generations, &next_generations);
                    last_generations = next_generations;

                    if changed {
                        // Emit (not notify): repaint the sidebar's bell/idle
                        // indicators without dragging in the heavy project-sync
                        // observer that fires on `cx.notify()`.
                        cx.emit(RemoteManagerEvent::TerminalActivity);
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

    /// Reconnect an existing connection (disconnect then connect again).
    pub fn reconnect(&mut self, connection_id: &str, cx: &mut Context<Self>) {
        if let Some(conn) = self.connections.get_mut(connection_id) {
            conn.disconnect();
            conn.connect();
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
            if config.saved_token.is_some() && !self.connections.contains_key(&config.id) {
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
    /// Fire-and-forget: spawns on the tokio runtime, logs errors and shows toast on failure.
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
        let token = match config.saved_token {
            Some(ref t) => t.clone(),
            None => {
                log::error!("send_action: no auth token for connection {}", connection_id);
                ToastManager::error("No auth token for remote connection".to_string(), cx);
                return;
            }
        };

        let host = config.host.clone();
        let port = config.port;
        let name = config.name.clone();
        let event_tx = self.event_tx.clone();

        self.runtime.spawn(async move {
            let url = format!("http://{}:{}/v1/actions", host, port);
            let client = reqwest::Client::new();
            let result = client
                .post(&url)
                .header("Authorization", format!("Bearer {}", token))
                .json(&action)
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await;

            match result {
                Ok(resp) if resp.status().is_success() => {
                    log::debug!("send_action: success for {}", name);
                }
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    log::error!("send_action: failed ({}): {} for {}", status, body, name);
                    // Send a warning event back to the GPUI thread
                    let _ = event_tx.try_send(ConnectionEvent::ServerWarning {
                        connection_id: String::new(),
                        message: format!("Action failed ({}): {}", status, body),
                    });
                }
                Err(e) => {
                    log::error!("send_action: request error for {}: {}", name, e);
                    let _ = event_tx.try_send(ConnectionEvent::ServerWarning {
                        connection_id: String::new(),
                        message: format!("Action request failed: {}", e),
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
            ConnectionEvent::SubscriptionMappings { .. } => "SubscriptionMappings",
            ConnectionEvent::GitStatusChanged { .. } => "GitStatusChanged",
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

fn now_unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::{activity_changed, RemoteConnectionManager};

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
    use okena_core::client::RemoteConnectionConfig;
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
