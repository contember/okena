use crate::remote_client::connection::RemoteConnection;
use crate::terminal::backend::TerminalBackend;
use crate::views::panels::toast::ToastManager;
use crate::views::root::TerminalsRegistry;
use crate::workspace::settings::{load_settings, update_remote_connections};

use okena_core::api::StateResponse;
use okena_core::client::{
    ConnectionEvent, ConnectionStatus, RemoteConnectionConfig,
};
use okena_core::client::connection::try_refresh_token;

use gpui::*;
use std::collections::HashMap;
use std::sync::Arc;

/// GPUI Entity managing all remote connections.
///
/// Observed by the Sidebar for rendering remote projects,
/// and by RootView for focus coordination.
pub struct RemoteConnectionManager {
    connections: HashMap<String, RemoteConnection>,
    terminals: TerminalsRegistry,
    runtime: Arc<tokio::runtime::Runtime>,

    /// Channel for events coming from tokio tasks
    event_tx: async_channel::Sender<ConnectionEvent>,

    /// Currently focused remote project, if any: (connection_id, project_id)
    focused_remote: Option<(String, String)>,
}

impl RemoteConnectionManager {
    pub fn new(terminals: TerminalsRegistry, cx: &mut Context<Self>) -> Self {
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

        Self {
            connections: HashMap::new(),
            terminals,
            runtime,
            event_tx,
            focused_remote: None,
        }
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
        // Clear focused remote if it belonged to this connection
        if let Some((ref cid, _)) = self.focused_remote {
            if cid == connection_id {
                self.focused_remote = None;
            }
        }
        // Remove from saved settings
        let id = connection_id.to_string();
        let _ = update_remote_connections(|conns| conns.retain(|c| c.id != id));
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
                );
                conn.connect();
                self.connections.insert(id, conn);
            }
        }
        cx.notify();
    }

    /// Get currently focused remote project.
    pub fn focused_remote(&self) -> Option<(&str, &str)> {
        self.focused_remote
            .as_ref()
            .map(|(c, p)| (c.as_str(), p.as_str()))
    }

    /// Set the focused remote project.
    pub fn set_focused_remote(
        &mut self,
        focus: Option<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        self.focused_remote = focus;
        cx.notify();
    }

    /// Handle an event from a connection's tokio task.
    fn handle_event(&mut self, event: ConnectionEvent, cx: &mut Context<Self>) {
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
            } => {
                let now = now_unix_timestamp();
                if let Some(conn) = self.connections.get_mut(&connection_id) {
                    conn.config_mut().saved_token = Some(token.clone());
                    conn.config_mut().token_obtained_at = Some(now);
                }
                // Persist token to settings (atomic update)
                let cid = connection_id.clone();
                let tok = token.clone();
                let _ = update_remote_connections(|conns| {
                    if let Some(saved) = conns.iter_mut().find(|c| c.id == cid) {
                        saved.saved_token = Some(tok);
                        saved.token_obtained_at = Some(now);
                    }
                });
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
                }
                let cid = connection_id.clone();
                let tok = token.clone();
                let _ = update_remote_connections(|conns| {
                    if let Some(saved) = conns.iter_mut().find(|c| c.id == cid) {
                        saved.saved_token = Some(tok);
                        saved.token_obtained_at = Some(now);
                    }
                });
            }
        }
    }

    /// Start a periodic token refresh task.
    /// Checks every 10 minutes and refreshes tokens older than 20 hours.
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

fn now_unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use crate::remote_client::manager::RemoteConnectionManager;
    use crate::views::root::TerminalsRegistry;
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
