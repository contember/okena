use crate::workspace::state::SplitDirection;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

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
}

/// Named special keys the remote API supports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SpecialKey {
    Enter,
    Escape,
    CtrlC,
    CtrlD,
    CtrlZ,
    Tab,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
}

impl SpecialKey {
    /// Convert to the byte sequence sent to the PTY.
    pub fn to_bytes(&self) -> &[u8] {
        match self {
            SpecialKey::Enter => b"\r",
            SpecialKey::Escape => b"\x1b",
            SpecialKey::CtrlC => b"\x03",
            SpecialKey::CtrlD => b"\x04",
            SpecialKey::CtrlZ => b"\x1a",
            SpecialKey::Tab => b"\t",
            SpecialKey::ArrowUp => b"\x1b[A",
            SpecialKey::ArrowDown => b"\x1b[B",
            SpecialKey::ArrowRight => b"\x1b[C",
            SpecialKey::ArrowLeft => b"\x1b[D",
            SpecialKey::Home => b"\x1b[H",
            SpecialKey::End => b"\x1b[F",
            SpecialKey::PageUp => b"\x1b[5~",
            SpecialKey::PageDown => b"\x1b[6~",
        }
    }
}

/// Result of processing a RemoteCommand.
#[derive(Debug)]
pub enum CommandResult {
    /// Success with optional JSON-serializable payload.
    Ok(Option<serde_json::Value>),
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
