//! Connection status extraction.
//!
//! The lifecycle entry points (`init_app`, `connect`, `pair`, …) are exported
//! directly from `crate::lib` via uniffi; this module only holds the plain
//! `ConnectionStatus` enum (mirrored as a uniffi enum in `crate::types`) and the
//! one accessor `lib.rs` delegates to.

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

impl From<okena_transport::client::ConnectionStatus> for ConnectionStatus {
    fn from(status: okena_transport::client::ConnectionStatus) -> Self {
        match status {
            okena_transport::client::ConnectionStatus::Disconnected => ConnectionStatus::Disconnected,
            okena_transport::client::ConnectionStatus::Connecting => ConnectionStatus::Connecting,
            okena_transport::client::ConnectionStatus::Connected => ConnectionStatus::Connected,
            okena_transport::client::ConnectionStatus::Pairing => ConnectionStatus::Pairing,
            okena_transport::client::ConnectionStatus::Reconnecting { .. } => {
                ConnectionStatus::Connecting
            }
            okena_transport::client::ConnectionStatus::Error(msg) => {
                ConnectionStatus::Error { message: msg }
            }
        }
    }
}

/// Get current connection status.
pub fn connection_status(conn_id: String) -> ConnectionStatus {
    ConnectionManager::get().get_status(&conn_id).into()
}
