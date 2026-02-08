use crate::workspace::state::SplitDirection;
use tokio::sync::oneshot;

pub use okena_core::keys::SpecialKey;

/// Commands sent from the axum server to the GPUI main thread.
/// Each command carries a oneshot sender for the reply.
pub struct BridgeMessage {
    pub command: RemoteCommand,
    pub reply: oneshot::Sender<CommandResult>,
}

/// All operations the remote API can request.
#[derive(Debug)]
pub enum RemoteCommand {
    /// Get the full workspace state snapshot.
    GetState,
    /// Send raw text to a terminal (no newline appended).
    SendText {
        terminal_id: String,
        text: String,
    },
    /// Send text + newline to a terminal.
    RunCommand {
        terminal_id: String,
        command: String,
    },
    /// Send a named special key to a terminal.
    SendSpecialKey {
        terminal_id: String,
        key: SpecialKey,
    },
    /// Split a terminal pane.
    SplitTerminal {
        project_id: String,
        path: Vec<usize>,
        direction: SplitDirection,
    },
    /// Close a terminal pane.
    CloseTerminal {
        project_id: String,
        terminal_id: String,
    },
    /// Focus a terminal pane.
    FocusTerminal {
        project_id: String,
        terminal_id: String,
    },
    /// Read visible content of a terminal.
    ReadContent {
        terminal_id: String,
    },
    /// Resize a terminal to new dimensions.
    Resize {
        terminal_id: String,
        cols: u16,
        rows: u16,
    },
    /// Create a terminal for a project that has no layout.
    CreateTerminal {
        project_id: String,
    },
    /// Render a terminal's visible content as ANSI bytes (for snapshots).
    RenderSnapshot {
        terminal_id: String,
    },
}

/// Result of processing a RemoteCommand.
#[derive(Debug)]
pub enum CommandResult {
    /// Success with optional JSON-serializable payload.
    Ok(Option<serde_json::Value>),
    /// Success with raw bytes (e.g., terminal snapshots).
    OkBytes(Vec<u8>),
    /// Error with a human-readable message.
    Err(String),
}

/// Channel types for the bridge.
pub type BridgeSender = async_channel::Sender<BridgeMessage>;
pub type BridgeReceiver = async_channel::Receiver<BridgeMessage>;

/// Create a new bridge channel pair (bounded to prevent memory exhaustion).
pub fn bridge_channel() -> (BridgeSender, BridgeReceiver) {
    async_channel::bounded(256)
}
