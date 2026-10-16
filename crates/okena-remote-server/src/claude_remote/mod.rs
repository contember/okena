//! Claude remote: dialogs, prompt delivery and transcript reading for `claude`
//! sessions in panes, served to remote clients under `/v1/claude/...`.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::watch;

use crate::bridge::BridgeSender;

/// Shared state between the `/v1/claude` routes and the daemon: open dialogs
/// (each held by a long-polling hook process), queued prompts and channel
/// attachments, keyed by terminal id.
pub struct ClaudeRemoteHub {
    bridge_tx: BridgeSender,
    state_version: Arc<watch::Sender<u64>>,
}

impl ClaudeRemoteHub {
    /// `bridge_tx` reaches the daemon (terminal info, PTY writes);
    /// `state_version` is bumped whenever the open-dialog counts change.
    pub fn new(bridge_tx: BridgeSender, state_version: Arc<watch::Sender<u64>>) -> Arc<Self> {
        Arc::new(Self {
            bridge_tx,
            state_version,
        })
    }

    /// Open dialogs per terminal id, for [`okena_core::api::StateResponse::claude_dialogs`].
    pub fn dialog_counts(&self) -> HashMap<String, u32> {
        let _ = (&self.bridge_tx, &self.state_version);
        todo!("U1: count open dialogs per terminal")
    }
}
