//! Connection status extraction.
//!
//! The lifecycle entry points (`init_app`, `connect`, `pair`, …) are exported
//! directly from `crate::lib` via uniffi; this module only holds the plain
//! `ConnectionStatus` enum (mirrored as a uniffi enum in `crate::types`) and the
//! one accessor `lib.rs` delegates to.

use crate::client::manager::ConnectionManager;

/// Connection status returned via FFI.
///
/// Mirrors okena_transport's ConnectionStatus. `Reconnecting` stays distinct
/// from `Connecting`: the UI keeps a once-connected user in the workspace (with
/// a banner) while the transport re-dials, instead of sending them back to the
/// pairing screen.
#[derive(Debug, Clone)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Reconnecting { attempt: u32 },
    Connected,
    Pairing,
    Error { message: String },
}

impl From<okena_transport::client::ConnectionStatus> for ConnectionStatus {
    fn from(status: okena_transport::client::ConnectionStatus) -> Self {
        match status {
            okena_transport::client::ConnectionStatus::Disconnected => {
                ConnectionStatus::Disconnected
            }
            okena_transport::client::ConnectionStatus::Connecting => ConnectionStatus::Connecting,
            okena_transport::client::ConnectionStatus::Connected => ConnectionStatus::Connected,
            okena_transport::client::ConnectionStatus::Pairing => ConnectionStatus::Pairing,
            okena_transport::client::ConnectionStatus::Reconnecting { attempt } => {
                ConnectionStatus::Reconnecting { attempt }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnecting_is_not_collapsed_into_connecting() {
        let status: ConnectionStatus =
            okena_transport::client::ConnectionStatus::Reconnecting { attempt: 3 }.into();
        assert!(matches!(status, ConnectionStatus::Reconnecting { attempt: 3 }));

        let ffi: crate::types::ConnectionStatus = status.into();
        assert!(matches!(
            ffi,
            crate::types::ConnectionStatus::Reconnecting { attempt: 3 }
        ));
    }
}
