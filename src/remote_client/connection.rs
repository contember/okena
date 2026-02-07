use crate::remote::types::StateResponse;
use crate::remote_client::backend::{
    is_remote_terminal, make_prefixed_id, RemoteBackend, RemoteTransport, WsClientMessage,
};
use crate::remote_client::config::RemoteConnectionConfig;
use crate::remote_client::state::diff_states;
use crate::terminal::backend::TerminalBackend;
use crate::terminal::terminal::{Terminal, TerminalSize};
use crate::views::root::TerminalsRegistry;

use std::collections::HashMap;
use std::sync::Arc;
use tokio_tungstenite::tungstenite;

/// Status of a remote connection.
#[derive(Clone, Debug)]
pub enum ConnectionStatus {
    /// Not connected
    Disconnected,
    /// Attempting to connect (health check / token validation)
    Connecting,
    /// Waiting for user to enter pairing code
    Pairing,
    /// Fully connected with active WebSocket
    Connected,
    /// Lost connection, attempting to reconnect
    Reconnecting { attempt: u32 },
    /// Unrecoverable error
    Error(String),
}

/// Event sent from tokio tasks back to the GPUI thread via async_channel.
/// The manager reads these from a cx.spawn() loop and applies state changes.
pub(crate) enum ConnectionEvent {
    /// Connection status changed
    StatusChanged {
        connection_id: String,
        status: ConnectionStatus,
    },
    /// Token obtained from pairing (save to config)
    TokenObtained {
        connection_id: String,
        token: String,
    },
    /// Remote state snapshot received
    StateReceived {
        connection_id: String,
        state: StateResponse,
    },
    /// Stream subscription mappings received
    SubscriptionMappings {
        connection_id: String,
        mappings: HashMap<String, u32>,
    },
}

/// Manages a single WebSocket connection to a remote Okena server.
pub struct RemoteConnection {
    pub(crate) config: RemoteConnectionConfig,
    pub(crate) status: ConnectionStatus,

    /// Tokio runtime (shared across all connections)
    runtime: Arc<tokio::runtime::Runtime>,

    /// Sender for outbound WS messages (to the WS writer task)
    ws_tx: Option<async_channel::Sender<WsClientMessage>>,

    /// Current remote state snapshot (projects, layouts)
    pub(crate) remote_state: Option<StateResponse>,

    /// Mapping: remote terminal_id -> local stream_id (from WS Subscribe response)
    stream_map: HashMap<String, u32>,
    /// Reverse: stream_id -> remote terminal_id
    reverse_stream_map: HashMap<u32, String>,

    /// Reference to shared TerminalsRegistry for inserting remote terminals
    terminals: TerminalsRegistry,

    /// Transport instance shared with all remote Terminal objects for this connection
    pub(crate) transport: Arc<RemoteTransport>,

    /// Channel to send events back to the GPUI manager
    event_tx: async_channel::Sender<ConnectionEvent>,

    /// Handle to abort the WS task on disconnect
    ws_abort_handle: Option<tokio::task::AbortHandle>,
}

impl RemoteConnection {
    pub fn new(
        config: RemoteConnectionConfig,
        runtime: Arc<tokio::runtime::Runtime>,
        terminals: TerminalsRegistry,
        event_tx: async_channel::Sender<ConnectionEvent>,
    ) -> Self {
        // Create the WS message channel (outbound commands)
        let (ws_tx, _ws_rx) = async_channel::bounded::<WsClientMessage>(256);

        let transport = Arc::new(RemoteTransport {
            ws_tx: ws_tx.clone(),
            connection_id: config.id.clone(),
        });

        Self {
            config,
            status: ConnectionStatus::Disconnected,
            runtime,
            ws_tx: Some(ws_tx),
            remote_state: None,
            stream_map: HashMap::new(),
            reverse_stream_map: HashMap::new(),
            terminals,
            transport,
            event_tx,
            ws_abort_handle: None,
        }
    }

    /// Get a TerminalBackend for this connection.
    pub fn backend(&self) -> Arc<dyn TerminalBackend> {
        Arc::new(RemoteBackend::new(
            self.transport.clone(),
            self.config.id.clone(),
        ))
    }

    /// Start the connection process.
    ///
    /// 1. GET /health to verify server is alive
    /// 2. If saved_token: GET /v1/state to validate token
    ///    - 200: token valid, proceed to start_ws()
    ///    - 401: token expired, set Pairing status
    /// 3. No saved_token: set Pairing status
    pub fn connect(&mut self) {
        self.status = ConnectionStatus::Connecting;

        let config = self.config.clone();
        let event_tx = self.event_tx.clone();
        let terminals = self.terminals.clone();
        let transport = self.transport.clone();

        // Create fresh WS message channel
        let (ws_tx, ws_rx) = async_channel::bounded::<WsClientMessage>(256);
        self.ws_tx = Some(ws_tx.clone());

        // Update transport's sender
        self.transport = Arc::new(RemoteTransport {
            ws_tx: ws_tx.clone(),
            connection_id: config.id.clone(),
        });

        let runtime = self.runtime.clone();

        let task = self.runtime.spawn(async move {
            let base_url = format!("http://{}:{}", config.host, config.port);

            // Step 1: Health check
            let client = reqwest::Client::new();
            match client
                .get(format!("{}/health", base_url))
                .timeout(std::time::Duration::from_secs(5))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    log::info!(
                        "Remote server {}:{} is healthy",
                        config.host,
                        config.port
                    );
                }
                Ok(resp) => {
                    let msg = format!("Health check failed: HTTP {}", resp.status());
                    log::warn!("{}", msg);
                    let _ = event_tx
                        .send(ConnectionEvent::StatusChanged {
                            connection_id: config.id.clone(),
                            status: ConnectionStatus::Error(msg),
                        })
                        .await;
                    return;
                }
                Err(e) => {
                    let msg = format!("Cannot reach server: {}", e);
                    log::warn!("{}", msg);
                    let _ = event_tx
                        .send(ConnectionEvent::StatusChanged {
                            connection_id: config.id.clone(),
                            status: ConnectionStatus::Error(msg),
                        })
                        .await;
                    return;
                }
            }

            // Step 2: Validate saved token (if any)
            if let Some(token) = config.saved_token.clone() {
                match client
                    .get(format!("{}/v1/state", base_url))
                    .header("Authorization", format!("Bearer {}", token))
                    .timeout(std::time::Duration::from_secs(5))
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        log::info!("Token valid for {}:{}", config.host, config.port);
                        // Token is valid - start WebSocket
                        Self::run_ws_loop(
                            config,
                            token,
                            event_tx,
                            ws_tx,
                            ws_rx,
                            terminals,
                            transport,
                            runtime,
                        )
                        .await;
                        return;
                    }
                    Ok(resp) if resp.status() == reqwest::StatusCode::UNAUTHORIZED => {
                        log::info!("Token expired for {}:{}, need re-pairing", config.host, config.port);
                    }
                    Ok(resp) => {
                        log::warn!("Token validation got unexpected status: {}", resp.status());
                    }
                    Err(e) => {
                        log::warn!("Token validation failed: {}", e);
                    }
                }
            }

            // Step 3: Need pairing
            let _ = event_tx
                .send(ConnectionEvent::StatusChanged {
                    connection_id: config.id.clone(),
                    status: ConnectionStatus::Pairing,
                })
                .await;
        });

        self.ws_abort_handle = Some(task.abort_handle());
    }

    /// Pair with the remote server using a 6-digit code.
    /// On success, saves the token and starts the WebSocket connection.
    pub fn pair(&mut self, code: &str) {
        let config = self.config.clone();
        let code = code.to_string();
        let event_tx = self.event_tx.clone();
        let terminals = self.terminals.clone();
        let transport = self.transport.clone();

        // Create fresh WS message channel
        let (ws_tx, ws_rx) = async_channel::bounded::<WsClientMessage>(256);
        self.ws_tx = Some(ws_tx.clone());

        self.transport = Arc::new(RemoteTransport {
            ws_tx: ws_tx.clone(),
            connection_id: config.id.clone(),
        });

        self.status = ConnectionStatus::Connecting;

        let runtime = self.runtime.clone();

        let task = self.runtime.spawn(async move {
            let base_url = format!("http://{}:{}", config.host, config.port);
            let client = reqwest::Client::new();

            // POST /v1/pair with the code
            let pair_body = serde_json::json!({ "code": code });
            match client
                .post(format!("{}/v1/pair", base_url))
                .json(&pair_body)
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    #[derive(serde::Deserialize)]
                    struct PairResp {
                        token: String,
                        #[allow(dead_code)]
                        expires_in: u64,
                    }
                    match resp.json::<PairResp>().await {
                        Ok(pair_resp) => {
                            log::info!("Paired with {}:{}", config.host, config.port);

                            // Notify manager to save the token
                            let _ = event_tx
                                .send(ConnectionEvent::TokenObtained {
                                    connection_id: config.id.clone(),
                                    token: pair_resp.token.clone(),
                                })
                                .await;

                            // Start WebSocket
                            Self::run_ws_loop(
                                config,
                                pair_resp.token,
                                event_tx,
                                ws_tx,
                                ws_rx,
                                terminals,
                                transport,
                                runtime,
                            )
                            .await;
                        }
                        Err(e) => {
                            let msg = format!("Failed to parse pair response: {}", e);
                            log::error!("{}", msg);
                            let _ = event_tx
                                .send(ConnectionEvent::StatusChanged {
                                    connection_id: config.id.clone(),
                                    status: ConnectionStatus::Error(msg),
                                })
                                .await;
                        }
                    }
                }
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    let msg = format!("Pairing failed: HTTP {} - {}", status, body);
                    log::warn!("{}", msg);
                    let _ = event_tx
                        .send(ConnectionEvent::StatusChanged {
                            connection_id: config.id.clone(),
                            status: ConnectionStatus::Error(msg),
                        })
                        .await;
                }
                Err(e) => {
                    let msg = format!("Pairing request failed: {}", e);
                    log::warn!("{}", msg);
                    let _ = event_tx
                        .send(ConnectionEvent::StatusChanged {
                            connection_id: config.id.clone(),
                            status: ConnectionStatus::Error(msg),
                        })
                        .await;
                }
            }
        });

        self.ws_abort_handle = Some(task.abort_handle());
    }

    /// Run the main WebSocket loop.
    ///
    /// This is the core of the connection: connects WS, authenticates, fetches
    /// state, subscribes to terminals, and enters the main read/write loop.
    /// On disconnection, attempts reconnection with exponential backoff.
    async fn run_ws_loop(
        config: RemoteConnectionConfig,
        token: String,
        event_tx: async_channel::Sender<ConnectionEvent>,
        ws_tx: async_channel::Sender<WsClientMessage>,
        ws_rx: async_channel::Receiver<WsClientMessage>,
        terminals: TerminalsRegistry,
        transport: Arc<RemoteTransport>,
        _runtime: Arc<tokio::runtime::Runtime>,
    ) {
        let mut reconnect_attempt: u32 = 0;
        let max_backoff_secs: u64 = 30;
        let max_reconnect_attempts: u32 = 10;

        loop {
            match Self::ws_session(
                &config,
                &token,
                &event_tx,
                &ws_tx,
                &ws_rx,
                &terminals,
                &transport,
            )
            .await
            {
                Ok(()) => {
                    // Clean disconnect requested
                    log::info!(
                        "WebSocket cleanly disconnected from {}:{}",
                        config.host,
                        config.port
                    );
                    break;
                }
                Err(e) => {
                    reconnect_attempt += 1;

                    if reconnect_attempt > max_reconnect_attempts {
                        let msg = format!(
                            "Connection lost after {} attempts (last error: {})",
                            max_reconnect_attempts, e
                        );
                        log::error!("{}", msg);
                        let _ = event_tx
                            .send(ConnectionEvent::StatusChanged {
                                connection_id: config.id.clone(),
                                status: ConnectionStatus::Error(msg),
                            })
                            .await;
                        break;
                    }

                    let backoff = std::cmp::min(
                        1u64.saturating_mul(2u64.saturating_pow(reconnect_attempt.saturating_sub(1))),
                        max_backoff_secs,
                    );

                    log::warn!(
                        "WebSocket connection to {}:{} lost: {}. Reconnecting in {}s (attempt {}/{})",
                        config.host,
                        config.port,
                        e,
                        backoff,
                        reconnect_attempt,
                        max_reconnect_attempts
                    );

                    let _ = event_tx
                        .send(ConnectionEvent::StatusChanged {
                            connection_id: config.id.clone(),
                            status: ConnectionStatus::Reconnecting {
                                attempt: reconnect_attempt,
                            },
                        })
                        .await;

                    tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                }
            }
        }
    }

    /// A single WebSocket session. Returns Ok(()) on clean disconnect, Err on failure.
    async fn ws_session(
        config: &RemoteConnectionConfig,
        token: &str,
        event_tx: &async_channel::Sender<ConnectionEvent>,
        _ws_tx: &async_channel::Sender<WsClientMessage>,
        ws_rx: &async_channel::Receiver<WsClientMessage>,
        terminals: &TerminalsRegistry,
        transport: &Arc<RemoteTransport>,
    ) -> Result<(), String> {
        // Local reverse stream map: stream_id -> remote terminal_id
        // Built from "subscribed" responses, used for binary frame routing.
        let mut reverse_stream_map: HashMap<u32, String> = HashMap::new();
        let ws_url = format!("ws://{}:{}/v1/stream", config.host, config.port);

        // Connect WebSocket (pass string directly as IntoClientRequest)
        let (ws_stream, _response) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .map_err(|e| format!("WebSocket connect failed: {}", e))?;

        let (mut ws_write, mut ws_read) = futures::StreamExt::split(ws_stream);

        // Step 1: Send Auth
        let auth_msg = serde_json::json!({
            "type": "auth",
            "token": token,
        });
        futures::SinkExt::send(
            &mut ws_write,
            tungstenite::Message::Text(auth_msg.to_string().into()),
        )
        .await
        .map_err(|e| format!("Failed to send auth: {}", e))?;

        // Step 2: Wait for AuthOk
        let auth_response = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            futures::StreamExt::next(&mut ws_read),
        )
        .await
        .map_err(|_| "Auth response timeout".to_string())?
        .ok_or_else(|| "WebSocket closed before auth response".to_string())?
        .map_err(|e| format!("WebSocket read error: {}", e))?;

        match &auth_response {
            tungstenite::Message::Text(text) => {
                let parsed: serde_json::Value =
                    serde_json::from_str(text).map_err(|e| format!("Invalid JSON: {}", e))?;
                let msg_type = parsed
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if msg_type == "auth_ok" {
                    log::info!("Authenticated with {}:{}", config.host, config.port);
                } else if msg_type == "auth_failed" {
                    let error = parsed
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let _ = event_tx
                        .send(ConnectionEvent::StatusChanged {
                            connection_id: config.id.clone(),
                            status: ConnectionStatus::Error(format!("Auth failed: {}", error)),
                        })
                        .await;
                    return Err(format!("Auth failed: {}", error));
                } else {
                    return Err(format!("Unexpected auth response type: {}", msg_type));
                }
            }
            _ => {
                return Err("Expected text message for auth response".to_string());
            }
        }

        // Step 3: Fetch state via HTTP
        let base_url = format!("http://{}:{}", config.host, config.port);
        let client = reqwest::Client::new();
        let state_resp = client
            .get(format!("{}/v1/state", base_url))
            .header("Authorization", format!("Bearer {}", token))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| format!("Failed to fetch state: {}", e))?;

        if !state_resp.status().is_success() {
            return Err(format!(
                "State fetch failed: HTTP {}",
                state_resp.status()
            ));
        }

        let state: StateResponse = state_resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse state: {}", e))?;

        // Step 4: Create Terminal objects for all remote terminals
        let terminal_ids = collect_state_terminal_ids(&state);

        for tid in &terminal_ids {
            let prefixed = make_prefixed_id(&config.id, tid);
            let terminal = Arc::new(Terminal::new(
                prefixed.clone(),
                TerminalSize::default(),
                transport.clone(),
                String::new(), // No local cwd for remote terminals
            ));
            terminals.lock().insert(prefixed, terminal);
        }

        // Notify state received
        let _ = event_tx
            .send(ConnectionEvent::StateReceived {
                connection_id: config.id.clone(),
                state: state.clone(),
            })
            .await;

        // Step 5: Subscribe to all terminal streams
        if !terminal_ids.is_empty() {
            let subscribe_msg = serde_json::json!({
                "type": "subscribe",
                "terminal_ids": terminal_ids,
            });
            futures::SinkExt::send(
                &mut ws_write,
                tungstenite::Message::Text(subscribe_msg.to_string().into()),
            )
            .await
            .map_err(|e| format!("Failed to send subscribe: {}", e))?;
        }

        // Notify connected
        let _ = event_tx
            .send(ConnectionEvent::StatusChanged {
                connection_id: config.id.clone(),
                status: ConnectionStatus::Connected,
            })
            .await;

        // Step 6: Main loop - read WS messages and handle outbound commands
        let config_id = config.id.clone();
        let config_host = config.host.clone();
        let config_port = config.port;
        let event_tx_clone = event_tx.clone();
        let terminals_clone = terminals.clone();
        let transport_clone = transport.clone();

        // Spawn writer task
        let ws_rx_clone = ws_rx.clone();
        let writer_handle = tokio::spawn(async move {
            while let Ok(msg) = ws_rx_clone.recv().await {
                let json = match &msg {
                    WsClientMessage::SendText { terminal_id, text } => {
                        serde_json::json!({
                            "type": "send_text",
                            "terminal_id": terminal_id,
                            "text": text,
                        })
                    }
                    WsClientMessage::Resize {
                        terminal_id,
                        cols,
                        rows,
                    } => {
                        serde_json::json!({
                            "type": "resize",
                            "terminal_id": terminal_id,
                            "cols": cols,
                            "rows": rows,
                        })
                    }
                    WsClientMessage::CloseTerminal { terminal_id } => {
                        serde_json::json!({
                            "type": "close_terminal",
                            "terminal_id": terminal_id,
                        })
                    }
                    WsClientMessage::Subscribe { terminal_ids } => {
                        serde_json::json!({
                            "type": "subscribe",
                            "terminal_ids": terminal_ids,
                        })
                    }
                    WsClientMessage::Unsubscribe { terminal_ids } => {
                        serde_json::json!({
                            "type": "unsubscribe",
                            "terminal_ids": terminal_ids,
                        })
                    }
                };
                if let Err(e) = futures::SinkExt::send(
                    &mut ws_write,
                    tungstenite::Message::Text(json.to_string().into()),
                )
                .await
                {
                    log::warn!("Failed to send WS message: {}", e);
                    break;
                }
            }
        });

        // Reader loop
        let mut cached_state = state;
        loop {
            match futures::StreamExt::next(&mut ws_read).await {
                Some(Ok(tungstenite::Message::Binary(data))) => {
                    // Binary frame: [proto:1][type:1][stream_id:4 BE][data...]
                    if data.len() < 6 {
                        continue;
                    }
                    let _proto = data[0];
                    let _msg_type = data[1];
                    let stream_id = u32::from_be_bytes([data[2], data[3], data[4], data[5]]);
                    let payload = &data[6..];

                    // Route binary data to the correct terminal
                    if let Some(remote_tid) = reverse_stream_map.get(&stream_id) {
                        let prefixed = make_prefixed_id(&config_id, remote_tid);
                        let terminals_guard = terminals_clone.lock();
                        if let Some(terminal) = terminals_guard.get(&prefixed) {
                            terminal.process_output(payload);
                        }
                    }
                }
                Some(Ok(tungstenite::Message::Text(text))) => {
                    // JSON message
                    match serde_json::from_str::<serde_json::Value>(&text) {
                        Ok(value) => {
                            let msg_type = value
                                .get("type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            match msg_type {
                                "subscribed" => {
                                    // Parse mappings: { terminal_id: stream_id }
                                    if let Some(mappings) = value.get("mappings") {
                                        if let Ok(map) = serde_json::from_value::<
                                            HashMap<String, u32>,
                                        >(
                                            mappings.clone()
                                        ) {
                                            log::info!(
                                                "Subscribed to {} terminal streams",
                                                map.len()
                                            );
                                            // Update local reverse map for binary frame routing
                                            for (terminal_id, stream_id) in &map {
                                                reverse_stream_map.insert(*stream_id, terminal_id.clone());
                                            }
                                            let _ = event_tx_clone
                                                .send(ConnectionEvent::SubscriptionMappings {
                                                    connection_id: config_id.clone(),
                                                    mappings: map,
                                                })
                                                .await;
                                        }
                                    }
                                }
                                "state_changed" => {
                                    log::info!("State changed on remote server");
                                    // Re-fetch state
                                    let client = reqwest::Client::new();
                                    match client
                                        .get(format!("{}/v1/state", base_url))
                                        .header(
                                            "Authorization",
                                            format!("Bearer {}", token),
                                        )
                                        .timeout(std::time::Duration::from_secs(10))
                                        .send()
                                        .await
                                    {
                                        Ok(resp) if resp.status().is_success() => {
                                            if let Ok(new_state) =
                                                resp.json::<StateResponse>().await
                                            {
                                                let diff =
                                                    diff_states(&cached_state, &new_state);

                                                // Add new terminals
                                                for tid in &diff.added_terminals {
                                                    let prefixed = make_prefixed_id(
                                                        &config_id, tid,
                                                    );
                                                    let terminal = Arc::new(Terminal::new(
                                                        prefixed.clone(),
                                                        TerminalSize::default(),
                                                        transport_clone.clone(),
                                                        String::new(),
                                                    ));
                                                    terminals_clone
                                                        .lock()
                                                        .insert(prefixed, terminal);
                                                }

                                                // Remove old terminals
                                                for tid in &diff.removed_terminals {
                                                    let prefixed = make_prefixed_id(
                                                        &config_id, tid,
                                                    );
                                                    terminals_clone.lock().remove(&prefixed);
                                                }

                                                // Subscribe to new terminals
                                                if !diff.added_terminals.is_empty() {
                                                    let _ = transport_clone
                                                        .ws_tx
                                                        .try_send(WsClientMessage::Subscribe {
                                                            terminal_ids: diff
                                                                .added_terminals
                                                                .clone(),
                                                        });
                                                }

                                                // Unsubscribe from removed terminals
                                                if !diff.removed_terminals.is_empty() {
                                                    let _ = transport_clone.ws_tx.try_send(
                                                        WsClientMessage::Unsubscribe {
                                                            terminal_ids: diff
                                                                .removed_terminals
                                                                .clone(),
                                                        },
                                                    );
                                                }

                                                cached_state = new_state.clone();

                                                let _ = event_tx_clone
                                                    .send(ConnectionEvent::StateReceived {
                                                        connection_id: config_id.clone(),
                                                        state: new_state,
                                                    })
                                                    .await;
                                            }
                                        }
                                        Ok(resp) => {
                                            log::warn!(
                                                "State re-fetch failed: HTTP {}",
                                                resp.status()
                                            );
                                        }
                                        Err(e) => {
                                            log::warn!("State re-fetch failed: {}", e);
                                        }
                                    }
                                }
                                "pong" => {
                                    // Keep-alive response, ignore
                                }
                                "dropped" => {
                                    let count =
                                        value.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
                                    log::warn!(
                                        "Server dropped {} messages for {}:{}",
                                        count,
                                        config_host,
                                        config_port
                                    );
                                }
                                "error" => {
                                    let error = value
                                        .get("error")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("unknown");
                                    log::warn!("Server error: {}", error);
                                }
                                _ => {
                                    log::debug!("Unknown WS message type: {}", msg_type);
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("Failed to parse WS JSON: {}", e);
                        }
                    }
                }
                Some(Ok(tungstenite::Message::Ping(data))) => {
                    // tungstenite handles Pong automatically in most cases
                    log::trace!("WS Ping received ({} bytes)", data.len());
                }
                Some(Ok(tungstenite::Message::Pong(_))) => {
                    // Expected keepalive response
                }
                Some(Ok(tungstenite::Message::Close(_))) => {
                    log::info!("Server closed WebSocket connection");
                    writer_handle.abort();
                    return Err("Server closed connection".to_string());
                }
                Some(Ok(tungstenite::Message::Frame(_))) => {
                    // Raw frame, ignore
                }
                Some(Err(e)) => {
                    writer_handle.abort();
                    return Err(format!("WebSocket error: {}", e));
                }
                None => {
                    // Stream ended
                    writer_handle.abort();
                    return Err("WebSocket stream ended".to_string());
                }
            }
        }
    }

    /// Update stream mappings from a subscription response.
    pub(crate) fn update_stream_mappings(&mut self, mappings: HashMap<String, u32>) {
        for (terminal_id, stream_id) in &mappings {
            self.stream_map
                .insert(terminal_id.clone(), *stream_id);
            self.reverse_stream_map
                .insert(*stream_id, terminal_id.clone());
        }
    }

    /// Disconnect and clean up all remote terminals.
    pub fn disconnect(&mut self) {
        // Abort the WS task
        if let Some(handle) = self.ws_abort_handle.take() {
            handle.abort();
        }

        // Close the WS sender
        if let Some(tx) = self.ws_tx.take() {
            tx.close();
        }

        // Remove all terminals belonging to this connection from the registry
        let mut terminals = self.terminals.lock();
        let to_remove: Vec<String> = terminals
            .keys()
            .filter(|k| is_remote_terminal(k, &self.config.id))
            .cloned()
            .collect();
        for key in to_remove {
            terminals.remove(&key);
        }

        self.stream_map.clear();
        self.reverse_stream_map.clear();
        self.remote_state = None;
        self.status = ConnectionStatus::Disconnected;
    }
}

/// Collect all terminal IDs from a StateResponse.
fn collect_state_terminal_ids(state: &StateResponse) -> Vec<String> {
    let mut ids = Vec::new();
    for project in &state.projects {
        if let Some(ref layout) = project.layout {
            collect_layout_ids(layout, &mut ids);
        }
    }
    ids
}

fn collect_layout_ids(node: &crate::remote::types::ApiLayoutNode, ids: &mut Vec<String>) {
    match node {
        crate::remote::types::ApiLayoutNode::Terminal { terminal_id, .. } => {
            if let Some(id) = terminal_id {
                ids.push(id.clone());
            }
        }
        crate::remote::types::ApiLayoutNode::Split { children, .. }
        | crate::remote::types::ApiLayoutNode::Tabs { children, .. } => {
            for child in children {
                collect_layout_ids(child, ids);
            }
        }
    }
}
