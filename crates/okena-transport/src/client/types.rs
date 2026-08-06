use okena_core::api::StateResponse;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Status of a remote connection.
#[derive(Clone, Debug, Serialize, Deserialize)]
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

/// Messages sent from the UI thread to the WebSocket writer task.
#[derive(Debug)]
pub enum WsClientMessage {
    /// Send byte-exact input to a remote terminal.
    SendInput { terminal_id: String, data: Vec<u8> },
    /// Resize a remote terminal
    Resize {
        terminal_id: String,
        cols: u16,
        rows: u16,
    },
    /// Close a remote terminal
    CloseTerminal { terminal_id: String },
    /// Subscribe to terminal output streams
    Subscribe { terminal_ids: Vec<String> },
    /// Unsubscribe from terminal output streams
    Unsubscribe { terminal_ids: Vec<String> },
    /// Declare which projects this client renders, as a full replacement set.
    /// Server-side ids (unprefixed) — scopes the server's `gh` PR/CI fan-out.
    SetVisibleProjects { project_ids: Vec<String> },
}

/// Error type distinguishing auth failures from transient network errors.
pub(crate) enum SessionError {
    /// Token expired or invalid — do not retry, go to Pairing state.
    Auth(String),
    /// Network/transient error — retry with backoff.
    Transient(String),
}

/// Event sent from tokio tasks back to the UI thread via async_channel.
pub enum ConnectionEvent {
    /// Connection status changed
    StatusChanged {
        connection_id: String,
        status: ConnectionStatus,
    },
    /// Token obtained from pairing (save to config)
    TokenObtained {
        connection_id: String,
        token: String,
        /// SHA-256 fingerprint (lowercase hex) of the server cert observed during
        /// the (TLS) pairing handshake, to be pinned. `None` for plain-http pairs.
        cert_fingerprint: Option<String>,
    },
    /// A previously plain-http connection auto-detected TLS on connect and
    /// upgraded; persist tls=true and the pinned fingerprint to the config.
    TlsUpgraded {
        connection_id: String,
        cert_fingerprint: Option<String>,
    },
    /// Remote state snapshot received
    StateReceived {
        connection_id: String,
        state: StateResponse,
    },
    /// Daemon-authoritative settings snapshot changed.
    SettingsChanged {
        connection_id: String,
        settings: serde_json::Value,
    },
    /// Stream subscription mappings received
    SubscriptionMappings {
        connection_id: String,
        mappings: HashMap<String, u32>,
    },
    /// Warning from the remote server (dropped messages, errors)
    ServerWarning {
        connection_id: String,
        message: String,
    },
    /// Git status changed for remote projects
    GitStatusChanged {
        connection_id: String,
        statuses: HashMap<String, okena_core::api::ApiGitStatus>,
    },
    /// System metrics changed on the remote host.
    SystemStatsChanged {
        connection_id: String,
        stats: okena_core::api::ApiSystemStats,
    },
    /// A daemon-originated toast to display on this client (e.g. a remote
    /// lifecycle-hook failure). The daemon has no surface, so it forwards these
    /// over the WebSocket and the client renders them via its `ToastManager`.
    Toast {
        connection_id: String,
        toast: okena_core::api::ApiToast,
    },
    /// One-shot request for the desktop client to focus and raise an exact
    /// terminal. IDs are server-local and are prefixed by the manager.
    TerminalFocusRequested {
        connection_id: String,
        request: okena_core::api::ApiTerminalFocusRequest,
    },
    /// Token was refreshed — save new token and update timestamp
    TokenRefreshed {
        connection_id: String,
        token: String,
    },
}

/// Token age threshold for refresh (3 days). Must be well under the 14-day server TTL.
pub const TOKEN_REFRESH_AGE_SECS: i64 = 3 * 24 * 3600;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_status_serde_round_trip() {
        let variants = vec![
            ConnectionStatus::Disconnected,
            ConnectionStatus::Connecting,
            ConnectionStatus::Pairing,
            ConnectionStatus::Connected,
            ConnectionStatus::Reconnecting { attempt: 3 },
            ConnectionStatus::Error("test error".to_string()),
        ];
        for status in variants {
            let json = serde_json::to_string(&status).unwrap();
            let parsed: ConnectionStatus = serde_json::from_str(&json).unwrap();
            // Verify round-trip by re-serializing
            let json2 = serde_json::to_string(&parsed).unwrap();
            assert_eq!(json, json2);
        }
    }

    #[test]
    fn ws_client_message_debug() {
        let msg = WsClientMessage::SendInput {
            terminal_id: "t1".to_string(),
            data: b"hello".to_vec(),
        };
        let debug = format!("{:?}", msg);
        assert!(debug.contains("SendInput"));
    }
}
