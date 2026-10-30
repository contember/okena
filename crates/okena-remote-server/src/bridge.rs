use crate::types::ActionRequest;
use tokio::sync::oneshot;

/// Result of processing a `RemoteCommand`. Defined in `okena-core` and
/// re-exported here so existing `bridge::CommandResult` paths keep working.
pub use okena_core::api::CommandResult;

/// Commands sent from the axum server to the GPUI main thread.
/// Fire-and-forget commands (SendText, Resize, etc.) set `reply` to `None`
/// to skip the oneshot allocation and avoid blocking the sender.
pub struct BridgeMessage {
    pub command: RemoteCommand,
    pub reply: Option<oneshot::Sender<CommandResult>>,
}

/// All operations the remote API can request.
#[derive(Debug)]
pub enum RemoteCommand {
    /// All client-facing actions (workspace + I/O).
    Action(ActionRequest),
    /// Client-facing action carrying the WebSocket connection that produced it.
    ActionFromConnection {
        action: ActionRequest,
        connection_id: String,
    },
    /// Resize from a WebSocket client. Only the current owner may resize.
    ResizeFromConnection {
        terminal_id: String,
        cols: u16,
        rows: u16,
        connection_id: String,
    },
    /// Get the full workspace state snapshot.
    GetState,
    /// Render a terminal's visible content as ANSI bytes (for snapshots).
    RenderSnapshot { terminal_id: String },
    /// Get current grid sizes (cols, rows) for multiple terminals.
    GetTerminalSizes { terminal_ids: Vec<String> },
    /// Bracketed-paste server-local text into the target terminal.
    PastePath { terminal_id: String, text: String },
    /// Agent session + lifecycle of a pane, answered with a serialized
    /// [`okena_core::claude_remote::ClaudeTerminalInfo`]. `probe_foreground`
    /// also fills `foreground_command` and `shell_idle`, which fork tmux or
    /// scan /proc: too costly for every polled read. `read_screen` fills
    /// `startup_dialog` from the pane's visible text.
    ClaudeTerminalInfo {
        terminal_id: String,
        probe_foreground: bool,
        read_screen: bool,
    },
}

/// Channel types for the bridge.
pub type BridgeSender = async_channel::Sender<BridgeMessage>;
pub type BridgeReceiver = async_channel::Receiver<BridgeMessage>;

/// Create a new bridge channel pair (bounded to prevent memory exhaustion).
pub fn bridge_channel() -> (BridgeSender, BridgeReceiver) {
    async_channel::bounded(256)
}
