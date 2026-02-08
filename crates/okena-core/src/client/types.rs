use crate::api::StateResponse;
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
    /// Send text input to a remote terminal
    SendText { terminal_id: String, text: String },
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
    /// Warning from the remote server (dropped messages, errors)
    ServerWarning {
        connection_id: String,
        message: String,
    },
    /// Token was refreshed — save new token and update timestamp
    TokenRefreshed {
        connection_id: String,
        token: String,
    },
}

/// Token age threshold for refresh (20 hours = 72000 seconds).
pub const TOKEN_REFRESH_AGE_SECS: i64 = 72000;

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
        let msg = WsClientMessage::SendText {
            terminal_id: "t1".to_string(),
            text: "hello".to_string(),
        };
        let debug = format!("{:?}", msg);
        assert!(debug.contains("SendText"));
    }
}
