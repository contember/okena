/// Connection status returned via FFI.
#[derive(Debug, Clone)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Pairing,
    Error { message: String },
}

/// Initialize the app (called once at startup).
#[flutter_rust_bridge::frb(init)]
pub fn init_app() {
    flutter_rust_bridge::setup_default_user_utils();
}

/// Connect to an Okena remote server.
#[flutter_rust_bridge::frb(sync)]
pub fn connect(host: String, port: u16) -> String {
    // TODO: establish WebSocket connection, return connection ID
    let id = uuid::Uuid::new_v4().to_string();
    log::info!("connect stub: {}:{} -> {}", host, port, id);
    id
}

/// Pair with the server using a pairing code.
pub async fn pair(conn_id: String, code: String) -> anyhow::Result<()> {
    // TODO: POST /v1/pair with the code, store bearer token
    log::info!("pair stub: conn={}, code={}", conn_id, code);
    Ok(())
}

/// Disconnect from a server.
#[flutter_rust_bridge::frb(sync)]
pub fn disconnect(conn_id: String) {
    // TODO: close WebSocket, clean up state
    log::info!("disconnect stub: conn={}", conn_id);
}

/// Get current connection status.
#[flutter_rust_bridge::frb(sync)]
pub fn connection_status(_conn_id: String) -> ConnectionStatus {
    // TODO: return actual status from connection manager
    ConnectionStatus::Disconnected
}
