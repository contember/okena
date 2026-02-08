use crate::remote::types::ActionRequest;
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
    /// All client-facing actions (workspace + I/O).
    Action(ActionRequest),
    /// Get the full workspace state snapshot.
    GetState,
    /// Render a terminal's visible content as ANSI bytes (for snapshots).
    RenderSnapshot { terminal_id: String },
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
