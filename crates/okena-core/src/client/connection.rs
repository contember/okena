use crate::api::StateResponse;
use crate::client::config::RemoteConnectionConfig;
use crate::client::id::make_prefixed_id;
use crate::client::state::{collect_all_app_ids, collect_state_terminal_ids, diff_states};
use crate::client::types::{
    ConnectionEvent, ConnectionStatus, SessionError, WsClientMessage, TOKEN_REFRESH_AGE_SECS,
};

use std::collections::HashMap;
use std::sync::Arc;
use tokio_tungstenite::tungstenite;

/// Platform-specific operations that the generic client delegates to.
///
/// Desktop creates `Terminal` objects and inserts into `TerminalsRegistry`.
/// Mobile may create Flutter-side terminal state via FFI callbacks.
pub trait ConnectionHandler: Send + Sync + 'static {
    /// Terminal discovered — create platform terminal object.
    /// `ws_sender` is for constructing a transport that sends WS commands.
    fn create_terminal(
        &self,
        connection_id: &str,
        terminal_id: &str,
        prefixed_id: &str,
        ws_sender: async_channel::Sender<WsClientMessage>,
    );
    /// Binary PTY output arrived — route to the terminal's emulator.
    fn on_terminal_output(&self, prefixed_id: &str, data: &[u8]);
    /// Terminal removed — clean up platform terminal object.
    fn remove_terminal(&self, prefixed_id: &str);
    /// Pre-resize a terminal's grid to match the server's dimensions.
    /// Called before snapshot arrives so the ANSI data renders at the correct size.
    fn resize_terminal(&self, prefixed_id: &str, cols: u16, rows: u16);
    /// Connection is disconnecting — remove ALL terminals for this connection.
    fn remove_all_terminals(&self, connection_id: &str);
}

/// Generic remote client state machine, parameterized by a platform handler.
pub struct RemoteClient<H: ConnectionHandler> {
    config: RemoteConnectionConfig,
    status: ConnectionStatus,
    runtime: Arc<tokio::runtime::Runtime>,
    ws_tx: Option<async_channel::Sender<WsClientMessage>>,
    remote_state: Option<StateResponse>,
    stream_map: HashMap<String, u32>,
    reverse_stream_map: HashMap<u32, String>,
    handler: Arc<H>,
    event_tx: async_channel::Sender<ConnectionEvent>,
    ws_abort_handle: Option<tokio::task::AbortHandle>,
    /// Shared token reference so WS reconnect loop can pick up refreshed tokens.
    shared_token: Arc<std::sync::RwLock<Option<String>>>,
}

impl<H: ConnectionHandler> RemoteClient<H> {
    pub fn new(
        config: RemoteConnectionConfig,
        runtime: Arc<tokio::runtime::Runtime>,
        handler: Arc<H>,
        event_tx: async_channel::Sender<ConnectionEvent>,
    ) -> Self {
        let shared_token = Arc::new(std::sync::RwLock::new(config.saved_token.clone()));
        Self {
            config,
            status: ConnectionStatus::Disconnected,
            runtime,
            ws_tx: None,
            remote_state: None,
            stream_map: HashMap::new(),
            reverse_stream_map: HashMap::new(),
            handler,
            event_tx,
            ws_abort_handle: None,
            shared_token,
        }
    }

    pub fn config(&self) -> &RemoteConnectionConfig {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut RemoteConnectionConfig {
        &mut self.config
    }

    pub fn status(&self) -> &ConnectionStatus {
        &self.status
    }

    pub fn status_mut(&mut self) -> &mut ConnectionStatus {
        &mut self.status
    }

    pub fn set_status(&mut self, status: ConnectionStatus) {
        self.status = status;
    }

    pub fn remote_state(&self) -> Option<&StateResponse> {
        self.remote_state.as_ref()
    }

    pub fn set_remote_state(&mut self, state: Option<StateResponse>) {
        self.remote_state = state;
    }

    /// Update the shared token so WS reconnect loop uses the latest token.
    pub fn update_shared_token(&self, token: &str) {
        if let Ok(mut guard) = self.shared_token.write() {
            *guard = Some(token.to_string());
        }
    }

    pub fn ws_sender(&self) -> Option<&async_channel::Sender<WsClientMessage>> {
        self.ws_tx.as_ref()
    }

    /// Update stream mappings from a subscription response.
    pub fn update_stream_mappings(&mut self, mappings: HashMap<String, u32>) {
        for (terminal_id, stream_id) in &mappings {
            self.stream_map.insert(terminal_id.clone(), *stream_id);
            self.reverse_stream_map
                .insert(*stream_id, terminal_id.clone());
        }
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
        let handler = self.handler.clone();
        let shared_token = self.shared_token.clone();

        // Update shared token from config
        if let Ok(mut guard) = self.shared_token.write() {
            *guard = config.saved_token.clone();
        }

        // Create fresh WS message channel
        let (ws_tx, ws_rx) = async_channel::bounded::<WsClientMessage>(256);
        self.ws_tx = Some(ws_tx.clone());

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
                        Self::run_ws_loop(config, token, event_tx, ws_tx, ws_rx, handler, shared_token).await;
                        return;
                    }
                    Ok(resp) if resp.status() == reqwest::StatusCode::UNAUTHORIZED => {
                        log::info!(
                            "Token expired for {}:{}, need re-pairing",
                            config.host,
                            config.port
                        );
                    }
                    Ok(resp) => {
                        log::warn!(
                            "Token validation got unexpected status: {}",
                            resp.status()
                        );
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
        let handler = self.handler.clone();
        let shared_token = self.shared_token.clone();

        // Create fresh WS message channel
        let (ws_tx, ws_rx) = async_channel::bounded::<WsClientMessage>(256);
        self.ws_tx = Some(ws_tx.clone());

        self.status = ConnectionStatus::Connecting;

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

                            // Update shared token
                            if let Ok(mut guard) = shared_token.write() {
                                *guard = Some(pair_resp.token.clone());
                            }

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
                                handler,
                                shared_token,
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

        // Remove all terminals belonging to this connection
        self.handler.remove_all_terminals(&self.config.id);

        self.stream_map.clear();
        self.reverse_stream_map.clear();
        self.remote_state = None;
        self.status = ConnectionStatus::Disconnected;
    }

    /// Run the main WebSocket loop with reconnection.
    async fn run_ws_loop(
        config: RemoteConnectionConfig,
        token: String,
        event_tx: async_channel::Sender<ConnectionEvent>,
        ws_tx: async_channel::Sender<WsClientMessage>,
        ws_rx: async_channel::Receiver<WsClientMessage>,
        handler: Arc<H>,
        shared_token: Arc<std::sync::RwLock<Option<String>>>,
    ) {
        let mut reconnect_attempt: u32 = 0;
        let max_backoff_secs: u64 = 30;
        let max_reconnect_attempts: u32 = 10;
        let mut current_token = token;

        loop {
            match Self::ws_session(&config, &current_token, &event_tx, &ws_tx, &ws_rx, &handler).await {
                Ok(()) => {
                    // Clean disconnect requested
                    log::info!(
                        "WebSocket cleanly disconnected from {}:{}",
                        config.host,
                        config.port
                    );
                    break;
                }
                Err(SessionError::Auth(msg)) => {
                    log::warn!(
                        "Auth error for {}:{}: {}. Switching to Pairing state.",
                        config.host,
                        config.port,
                        msg
                    );
                    let _ = event_tx
                        .send(ConnectionEvent::StatusChanged {
                            connection_id: config.id.clone(),
                            status: ConnectionStatus::Pairing,
                        })
                        .await;
                    break;
                }
                Err(SessionError::Transient(e)) => {
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
                        1u64.saturating_mul(
                            2u64.saturating_pow(reconnect_attempt.saturating_sub(1)),
                        ),
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

                    // Read the latest token (may have been refreshed since last attempt)
                    if let Ok(guard) = shared_token.read() {
                        if let Some(ref latest) = *guard {
                            current_token = latest.clone();
                        }
                    }
                }
            }
        }
    }

    /// A single WebSocket session. Returns Ok(()) on clean disconnect, Err on failure.
    async fn ws_session(
        config: &RemoteConnectionConfig,
        token: &str,
        event_tx: &async_channel::Sender<ConnectionEvent>,
        ws_tx: &async_channel::Sender<WsClientMessage>,
        ws_rx: &async_channel::Receiver<WsClientMessage>,
        handler: &Arc<H>,
    ) -> Result<(), SessionError> {
        // Shared stream maps: terminal_id -> stream_id (for writer) and reverse (for reader)
        let stream_map: Arc<std::sync::RwLock<HashMap<String, u32>>> =
            Arc::new(std::sync::RwLock::new(HashMap::new()));
        let mut reverse_stream_map: HashMap<u32, String> = HashMap::new();
        let ws_url = format!("ws://{}:{}/v1/stream", config.host, config.port);

        // Connect WebSocket
        let (ws_stream, _response) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .map_err(|e| SessionError::Transient(format!("WebSocket connect failed: {}", e)))?;

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
        .map_err(|e| SessionError::Transient(format!("Failed to send auth: {}", e)))?;

        // Step 2: Wait for AuthOk
        let auth_response = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            futures::StreamExt::next(&mut ws_read),
        )
        .await
        .map_err(|_| SessionError::Transient("Auth response timeout".to_string()))?
        .ok_or_else(|| {
            SessionError::Transient("WebSocket closed before auth response".to_string())
        })?
        .map_err(|e| SessionError::Transient(format!("WebSocket read error: {}", e)))?;

        match &auth_response {
            tungstenite::Message::Text(text) => {
                let parsed: serde_json::Value = serde_json::from_str(text)
                    .map_err(|e| SessionError::Transient(format!("Invalid JSON: {}", e)))?;
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
                    return Err(SessionError::Auth(format!("Auth failed: {}", error)));
                } else {
                    return Err(SessionError::Transient(format!(
                        "Unexpected auth response type: {}",
                        msg_type
                    )));
                }
            }
            _ => {
                return Err(SessionError::Transient(
                    "Expected text message for auth response".to_string(),
                ));
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
            .map_err(|e| SessionError::Transient(format!("Failed to fetch state: {}", e)))?;

        if state_resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(SessionError::Auth(format!(
                "State fetch failed: HTTP {}",
                state_resp.status()
            )));
        }
        if !state_resp.status().is_success() {
            return Err(SessionError::Transient(format!(
                "State fetch failed: HTTP {}",
                state_resp.status()
            )));
        }

        let state: StateResponse = state_resp
            .json()
            .await
            .map_err(|e| SessionError::Transient(format!("Failed to parse state: {}", e)))?;

        // Step 4: Create terminal objects for all remote terminals
        let terminal_ids = collect_state_terminal_ids(&state);

        for tid in &terminal_ids {
            let prefixed = make_prefixed_id(&config.id, tid);
            handler.create_terminal(&config.id, tid, &prefixed, ws_tx.clone());
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
            .map_err(|e| SessionError::Transient(format!("Failed to send subscribe: {}", e)))?;
        }

        // Step 5b: Subscribe to all app state streams
        let initial_app_ids: Vec<String> = collect_all_app_ids(&state).into_iter().collect();
        if !initial_app_ids.is_empty() {
            let subscribe_apps_msg = serde_json::json!({
                "type": "subscribe_apps",
                "app_ids": initial_app_ids,
            });
            futures::SinkExt::send(
                &mut ws_write,
                tungstenite::Message::Text(subscribe_apps_msg.to_string().into()),
            )
            .await
            .map_err(|e| SessionError::Transient(format!("Failed to send subscribe_apps: {}", e)))?;
        }

        // Notify connected
        let _ = event_tx
            .send(ConnectionEvent::StatusChanged {
                connection_id: config.id.clone(),
                status: ConnectionStatus::Connected,
            })
            .await;

        // Step 6: Main loop
        let config_id = config.id.clone();
        let config_host = config.host.clone();
        let config_port = config.port;
        let event_tx_clone = event_tx.clone();
        let handler_clone = handler.clone();
        let ws_tx_clone = ws_tx.clone();

        // Spawn writer task
        let ws_rx_clone = ws_rx.clone();
        let stream_map_for_writer = stream_map.clone();
        let writer_handle = tokio::spawn(async move {
            while let Ok(msg) = ws_rx_clone.recv().await {
                // For SendText, prefer binary frame when stream_id is known
                if let WsClientMessage::SendText { terminal_id, text } = &msg {
                    let stream_id = stream_map_for_writer
                        .read()
                        .ok()
                        .and_then(|m| m.get(terminal_id).copied());
                    if let Some(sid) = stream_id {
                        let frame = crate::ws::build_binary_frame(
                            crate::ws::FRAME_TYPE_INPUT,
                            sid,
                            text.as_bytes(),
                        );
                        if let Err(e) = futures::SinkExt::send(
                            &mut ws_write,
                            tungstenite::Message::Binary(frame.into()),
                        )
                        .await
                        {
                            log::warn!("Failed to send binary input: {}", e);
                            break;
                        }
                        continue;
                    }
                }

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
                    WsClientMessage::SubscribeApps { app_ids } => {
                        serde_json::json!({
                            "type": "subscribe_apps",
                            "app_ids": app_ids,
                        })
                    }
                    WsClientMessage::UnsubscribeApps { app_ids } => {
                        serde_json::json!({
                            "type": "unsubscribe_apps",
                            "app_ids": app_ids,
                        })
                    }
                    WsClientMessage::AppAction { app_id, action } => {
                        serde_json::json!({
                            "type": "app_action",
                            "app_id": app_id,
                            "action": action,
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
                    // Generic binary frame: [proto:1][type:1][stream_id:4 BE][payload...]
                    if let Some((frame_type, stream_id, payload)) =
                        crate::ws::parse_binary_frame(&data)
                    {
                        match frame_type {
                            crate::ws::FRAME_TYPE_PTY | crate::ws::FRAME_TYPE_SNAPSHOT => {
                                // Route PTY output or snapshot to the correct terminal
                                if let Some(remote_tid) = reverse_stream_map.get(&stream_id) {
                                    let prefixed = make_prefixed_id(&config_id, remote_tid);
                                    handler_clone.on_terminal_output(&prefixed, payload);
                                }
                            }
                            _ => {
                                log::debug!("Unknown binary frame type: {}", frame_type);
                            }
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
                                            for (terminal_id, stream_id) in &map {
                                                reverse_stream_map
                                                    .insert(*stream_id, terminal_id.clone());
                                            }
                                            // Update shared stream_map for writer task
                                            if let Ok(mut sm) = stream_map.write() {
                                                for (terminal_id, stream_id) in &map {
                                                    sm.insert(terminal_id.clone(), *stream_id);
                                                }
                                            }
                                            // Pre-resize terminals to server dimensions before snapshots arrive
                                            if let Some(sizes) = value.get("sizes") {
                                                if let Ok(size_map) = serde_json::from_value::<
                                                    HashMap<String, (u16, u16)>,
                                                >(sizes.clone()) {
                                                    for (terminal_id, (cols, rows)) in &size_map {
                                                        let prefixed = make_prefixed_id(&config_id, terminal_id);
                                                        handler_clone.resize_terminal(&prefixed, *cols, *rows);
                                                    }
                                                    log::info!("Pre-resized {} terminals to server dimensions", size_map.len());
                                                }
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

                                                // Add new terminals via handler
                                                for tid in &diff.added_terminals {
                                                    let prefixed =
                                                        make_prefixed_id(&config_id, tid);
                                                    handler_clone.create_terminal(
                                                        &config_id,
                                                        tid,
                                                        &prefixed,
                                                        ws_tx_clone.clone(),
                                                    );
                                                }

                                                // Remove old terminals via handler
                                                for tid in &diff.removed_terminals {
                                                    let prefixed =
                                                        make_prefixed_id(&config_id, tid);
                                                    handler_clone.remove_terminal(&prefixed);
                                                }

                                                // Subscribe to new terminals
                                                if !diff.added_terminals.is_empty() {
                                                    let _ = ws_tx_clone.try_send(
                                                        WsClientMessage::Subscribe {
                                                            terminal_ids: diff
                                                                .added_terminals
                                                                .clone(),
                                                        },
                                                    );
                                                }

                                                // Unsubscribe from removed terminals
                                                if !diff.removed_terminals.is_empty() {
                                                    let _ = ws_tx_clone.try_send(
                                                        WsClientMessage::Unsubscribe {
                                                            terminal_ids: diff
                                                                .removed_terminals
                                                                .clone(),
                                                        },
                                                    );
                                                }

                                                // Subscribe to new app streams
                                                if !diff.added_apps.is_empty() {
                                                    let _ = ws_tx_clone.try_send(
                                                        WsClientMessage::SubscribeApps {
                                                            app_ids: diff.added_apps.clone(),
                                                        },
                                                    );
                                                }

                                                // Unsubscribe from removed app streams
                                                if !diff.removed_apps.is_empty() {
                                                    let _ = ws_tx_clone.try_send(
                                                        WsClientMessage::UnsubscribeApps {
                                                            app_ids: diff.removed_apps.clone(),
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
                                    let count = value
                                        .get("count")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0);
                                    log::warn!(
                                        "Server dropped {} messages for {}:{}",
                                        count,
                                        config_host,
                                        config_port
                                    );
                                    let _ = event_tx_clone
                                        .send(ConnectionEvent::ServerWarning {
                                            connection_id: config_id.clone(),
                                            message: format!(
                                                "Server dropped {} messages",
                                                count
                                            ),
                                        })
                                        .await;
                                }
                                "app_state_changed" => {
                                    if let (Some(app_id), Some(app_kind), Some(state)) = (
                                        value.get("app_id").and_then(|v| v.as_str()),
                                        value.get("app_kind").and_then(|v| v.as_str()),
                                        value.get("state"),
                                    ) {
                                        let _ = event_tx_clone
                                            .send(ConnectionEvent::AppStateChanged {
                                                connection_id: config_id.clone(),
                                                app_id: app_id.to_string(),
                                                app_kind: app_kind.to_string(),
                                                state: state.clone(),
                                            })
                                            .await;
                                    }
                                }
                                "error" => {
                                    let error = value
                                        .get("error")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("unknown");
                                    log::warn!("Server error: {}", error);
                                    let _ = event_tx_clone
                                        .send(ConnectionEvent::ServerWarning {
                                            connection_id: config_id.clone(),
                                            message: format!("Server error: {}", error),
                                        })
                                        .await;
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
                    log::trace!("WS Ping received ({} bytes)", data.len());
                }
                Some(Ok(tungstenite::Message::Pong(_))) => {
                    // Expected keepalive response
                }
                Some(Ok(tungstenite::Message::Close(_))) => {
                    log::info!("Server closed WebSocket connection");
                    writer_handle.abort();
                    return Err(SessionError::Transient(
                        "Server closed connection".to_string(),
                    ));
                }
                Some(Ok(tungstenite::Message::Frame(_))) => {
                    // Raw frame, ignore
                }
                Some(Err(e)) => {
                    writer_handle.abort();
                    return Err(SessionError::Transient(format!("WebSocket error: {}", e)));
                }
                None => {
                    // Stream ended
                    writer_handle.abort();
                    return Err(SessionError::Transient(
                        "WebSocket stream ended".to_string(),
                    ));
                }
            }
        }
    }
}

/// Attempt to refresh a token if it's older than 20 hours.
/// On success, sends a `TokenRefreshed` event. On failure, logs a warning.
pub async fn try_refresh_token(
    config: &RemoteConnectionConfig,
    event_tx: &async_channel::Sender<ConnectionEvent>,
) {
    let token = match &config.saved_token {
        Some(t) => t,
        None => return,
    };

    // Check token age
    if let Some(obtained_at) = config.token_obtained_at {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        if now - obtained_at < TOKEN_REFRESH_AGE_SECS {
            return; // Token is still fresh
        }
    }
    // If token_obtained_at is None, attempt refresh (legacy token without timestamp)

    let base_url = format!("http://{}:{}", config.host, config.port);
    let client = reqwest::Client::new();

    match client
        .post(format!("{}/v1/refresh", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            #[derive(serde::Deserialize)]
            struct RefreshResp {
                token: String,
                #[allow(dead_code)]
                expires_in: u64,
            }
            match resp.json::<RefreshResp>().await {
                Ok(refresh_resp) => {
                    log::info!("Token refreshed for {}:{}", config.host, config.port);
                    let _ = event_tx
                        .send(ConnectionEvent::TokenRefreshed {
                            connection_id: config.id.clone(),
                            token: refresh_resp.token,
                        })
                        .await;
                }
                Err(e) => {
                    log::warn!(
                        "Failed to parse refresh response for {}:{}: {}",
                        config.host,
                        config.port,
                        e
                    );
                }
            }
        }
        Ok(resp) => {
            log::warn!(
                "Token refresh failed for {}:{}: HTTP {} (server may not support refresh)",
                config.host,
                config.port,
                resp.status()
            );
        }
        Err(e) => {
            log::warn!(
                "Token refresh request failed for {}:{}: {}",
                config.host,
                config.port,
                e
            );
        }
    }
}
