use crate::client::manager::ConnectionManager;

/// Connection status returned via FFI.
///
/// Simplified version of okena_core's ConnectionStatus — collapses `Reconnecting { attempt }`
/// into `Connecting` since mobile UI doesn't need the attempt count.
#[derive(Debug, Clone)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Pairing,
    Error { message: String },
}

impl From<okena_core::client::ConnectionStatus> for ConnectionStatus {
    fn from(status: okena_core::client::ConnectionStatus) -> Self {
        match status {
            okena_core::client::ConnectionStatus::Disconnected => ConnectionStatus::Disconnected,
            okena_core::client::ConnectionStatus::Connecting => ConnectionStatus::Connecting,
            okena_core::client::ConnectionStatus::Connected => ConnectionStatus::Connected,
            okena_core::client::ConnectionStatus::Pairing => ConnectionStatus::Pairing,
            okena_core::client::ConnectionStatus::Reconnecting { .. } => {
                ConnectionStatus::Connecting
            }
            okena_core::client::ConnectionStatus::Error(msg) => {
                ConnectionStatus::Error { message: msg }
            }
        }
    }
}

/// Initialize the app (called once at startup).
#[flutter_rust_bridge::frb(init)]
pub fn init_app() {
    flutter_rust_bridge::setup_default_user_utils();
    ConnectionManager::init();
}

/// Connect to an Okena remote server. Returns a connection ID.
/// If a saved token is provided, it will be used to skip pairing.
#[flutter_rust_bridge::frb(sync)]
pub fn connect(host: String, port: u16, saved_token: Option<String>) -> String {
    let mgr = ConnectionManager::get();
    let conn_id = mgr.add_connection(&host, port, saved_token);
    mgr.connect(&conn_id);
    conn_id
}

/// Get the current auth token for a connection (if paired).
#[flutter_rust_bridge::frb(sync)]
pub fn get_token(conn_id: String) -> Option<String> {
    ConnectionManager::get().get_token(&conn_id)
}

/// Pair with the server using a pairing code.
pub async fn pair(conn_id: String, code: String) -> anyhow::Result<()> {
    let mgr = ConnectionManager::get();
    mgr.pair(&conn_id, &code);
    Ok(())
}

/// Disconnect from a server.
#[flutter_rust_bridge::frb(sync)]
pub fn disconnect(conn_id: String) {
    ConnectionManager::get().disconnect(&conn_id);
}

/// Get current connection status.
#[flutter_rust_bridge::frb(sync)]
pub fn connection_status(conn_id: String) -> ConnectionStatus {
    ConnectionManager::get().get_status(&conn_id).into()
}

/// Get seconds since last WS activity (terminal output).
/// Returns a large value if the connection doesn't exist.
#[flutter_rust_bridge::frb(sync)]
pub fn seconds_since_activity(conn_id: String) -> f64 {
    ConnectionManager::get().seconds_since_activity(&conn_id)
}
