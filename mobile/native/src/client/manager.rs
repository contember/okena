use crate::client::handler::MobileConnectionHandler;
use crate::client::terminal_holder::TerminalHolder;

use okena_core::api::StateResponse;
use okena_core::client::{
    make_prefixed_id, ConnectionEvent, ConnectionStatus, RemoteClient, RemoteConnectionConfig,
    WsClientMessage,
};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

static MANAGER: OnceLock<ConnectionManager> = OnceLock::new();

pub struct ConnectionManager {
    runtime: Arc<tokio::runtime::Runtime>,
    connections: RwLock<HashMap<String, MobileConnection>>,
}

struct MobileConnection {
    client: RwLock<RemoteClient<MobileConnectionHandler>>,
    handler: Arc<MobileConnectionHandler>,
    status: RwLock<ConnectionStatus>,
    state_cache: RwLock<Option<StateResponse>>,
    _event_task: Option<tokio::task::JoinHandle<()>>,
}

impl ConnectionManager {
    /// Initialize the global singleton. Call once at app startup.
    pub fn init() {
        MANAGER.get_or_init(|| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime");

            ConnectionManager {
                runtime: Arc::new(runtime),
                connections: RwLock::new(HashMap::new()),
            }
        });
    }

    /// Get the global singleton. Panics if `init()` hasn't been called.
    pub fn get() -> &'static ConnectionManager {
        MANAGER.get().expect("ConnectionManager not initialized")
    }

    /// Create a new connection and return its ID.
    pub fn add_connection(&self, host: &str, port: u16) -> String {
        let config = RemoteConnectionConfig {
            id: uuid::Uuid::new_v4().to_string(),
            name: format!("{}:{}", host, port),
            host: host.to_string(),
            port,
            saved_token: None,
            token_obtained_at: None,
        };
        let conn_id = config.id.clone();

        let terminals: Arc<RwLock<HashMap<String, TerminalHolder>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let handler = Arc::new(MobileConnectionHandler::new(terminals));

        let (event_tx, event_rx) = async_channel::bounded::<ConnectionEvent>(256);

        let client = RemoteClient::new(
            config,
            self.runtime.clone(),
            handler.clone(),
            event_tx,
        );

        // Spawn event processor task
        let conn_id_clone = conn_id.clone();
        let status = RwLock::new(ConnectionStatus::Disconnected);
        let state_cache = RwLock::new(None);

        let connection = MobileConnection {
            client: RwLock::new(client),
            handler,
            status,
            state_cache,
            _event_task: None,
        };

        self.connections.write().insert(conn_id.clone(), connection);

        // Spawn event processor
        let event_task = self.runtime.spawn(Self::process_events(
            conn_id_clone.clone(),
            event_rx,
        ));

        // Store the task handle
        if let Some(conn) = self.connections.write().get_mut(&conn_id) {
            conn._event_task = Some(event_task);
        }

        conn_id
    }

    /// Start connecting to the remote server.
    pub fn connect(&self, conn_id: &str) {
        let connections = self.connections.read();
        if let Some(conn) = connections.get(conn_id) {
            conn.client.write().connect();
        }
    }

    /// Pair with the remote server using a pairing code.
    pub fn pair(&self, conn_id: &str, code: &str) {
        let connections = self.connections.read();
        if let Some(conn) = connections.get(conn_id) {
            conn.client.write().pair(code);
        }
    }

    /// Disconnect from the remote server.
    pub fn disconnect(&self, conn_id: &str) {
        let connections = self.connections.read();
        if let Some(conn) = connections.get(conn_id) {
            conn.client.write().disconnect();
            *conn.status.write() = ConnectionStatus::Disconnected;
            *conn.state_cache.write() = None;
        }
    }

    /// Get the current connection status.
    pub fn get_status(&self, conn_id: &str) -> ConnectionStatus {
        let connections = self.connections.read();
        if let Some(conn) = connections.get(conn_id) {
            conn.status.read().clone()
        } else {
            ConnectionStatus::Disconnected
        }
    }

    /// Get the cached remote state.
    pub fn get_state(&self, conn_id: &str) -> Option<StateResponse> {
        let connections = self.connections.read();
        connections
            .get(conn_id)
            .and_then(|conn| conn.state_cache.read().clone())
    }

    /// Access a terminal holder for reading cells / cursor.
    /// The callback receives the TerminalHolder if found.
    pub fn with_terminal<F, R>(&self, conn_id: &str, terminal_id: &str, f: F) -> Option<R>
    where
        F: FnOnce(&TerminalHolder) -> R,
    {
        let connections = self.connections.read();
        let conn = connections.get(conn_id)?;
        let prefixed_id = make_prefixed_id(conn_id, terminal_id);
        let terminals = conn.handler.terminals().read();
        let holder = terminals.get(&prefixed_id)?;
        Some(f(holder))
    }

    /// Send a WebSocket message for a connection.
    pub fn send_ws_message(&self, conn_id: &str, msg: WsClientMessage) {
        let connections = self.connections.read();
        if let Some(conn) = connections.get(conn_id) {
            let client = conn.client.read();
            if let Some(sender) = client.ws_sender() {
                let _ = sender.try_send(msg);
            }
        }
    }

    /// Execute an action on the remote server via HTTP POST /v1/actions.
    /// Returns the response body as a string.
    pub async fn execute_action(
        &self,
        conn_id: &str,
        action: okena_core::api::ActionRequest,
    ) -> anyhow::Result<String> {
        let (host, port, token) = {
            let connections = self.connections.read();
            let conn = connections
                .get(conn_id)
                .ok_or_else(|| anyhow::anyhow!("Connection not found: {}", conn_id))?;
            let client = conn.client.read();
            let config = client.config();
            let token = config
                .saved_token
                .clone()
                .ok_or_else(|| anyhow::anyhow!("No auth token for connection {}", conn_id))?;
            (config.host.clone(), config.port, token)
        };

        let url = format!("http://{}:{}/v1/actions", host, port);
        let client = reqwest::Client::new();
        let resp = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&action)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Action failed: HTTP {} - {}", status, body);
        }

        Ok(resp.text().await.unwrap_or_default())
    }

    /// Spawn an async task on the connection manager's runtime.
    pub fn spawn<F>(&self, future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.runtime.spawn(future)
    }

    /// Resize a terminal holder and send the resize message to the server.
    pub fn resize_terminal(&self, conn_id: &str, terminal_id: &str, cols: u16, rows: u16) {
        let connections = self.connections.read();
        if let Some(conn) = connections.get(conn_id) {
            let prefixed_id = make_prefixed_id(conn_id, terminal_id);
            let terminals = conn.handler.terminals().read();
            if let Some(holder) = terminals.get(&prefixed_id) {
                holder.resize(cols, rows);
            }
        }
        // Also send WS resize message
        self.send_ws_message(
            conn_id,
            WsClientMessage::Resize {
                terminal_id: terminal_id.to_string(),
                cols,
                rows,
            },
        );
    }

    /// Background task that drains the event channel and updates connection state.
    async fn process_events(
        conn_id: String,
        event_rx: async_channel::Receiver<ConnectionEvent>,
    ) {
        while let Ok(event) = event_rx.recv().await {
            let mgr = match MANAGER.get() {
                Some(m) => m,
                None => break,
            };
            let connections = mgr.connections.read();
            let conn = match connections.get(&conn_id) {
                Some(c) => c,
                None => break,
            };

            match event {
                ConnectionEvent::StatusChanged { status, .. } => {
                    *conn.status.write() = status;
                }
                ConnectionEvent::TokenObtained { token, .. } => {
                    conn.client.write().config_mut().saved_token = Some(token.clone());
                    conn.client.write().config_mut().token_obtained_at = Some(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as i64,
                    );
                }
                ConnectionEvent::TokenRefreshed { token, .. } => {
                    conn.client.write().config_mut().saved_token = Some(token.clone());
                    conn.client.write().config_mut().token_obtained_at = Some(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as i64,
                    );
                }
                ConnectionEvent::StateReceived { state, .. } => {
                    *conn.state_cache.write() = Some(state);
                }
                ConnectionEvent::SubscriptionMappings { mappings, .. } => {
                    conn.client.write().update_stream_mappings(mappings);
                }
                ConnectionEvent::ServerWarning { message, .. } => {
                    log::warn!("Server warning for {}: {}", conn_id, message);
                }
            }
        }
    }
}
