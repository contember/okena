use tokio::sync::broadcast;

/// A PTY output event for broadcast to WebSocket subscribers.
#[derive(Clone, Debug)]
pub struct PtyBroadcastEvent {
    pub terminal_id: String,
    pub data: Vec<u8>,
}

/// Fan-out PTY events to WebSocket subscribers.
///
/// Uses `tokio::sync::broadcast` with a bounded buffer. When a subscriber
/// falls behind, `recv()` returns `Lagged(n)` and the subscriber should
/// notify the client with a `dropped` message.
pub struct PtyBroadcaster {
    tx: broadcast::Sender<PtyBroadcastEvent>,
}

impl PtyBroadcaster {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(4096);
        Self { tx }
    }

    /// Publish a PTY output event. Non-blocking; drops if no subscribers.
    pub fn publish(&self, terminal_id: String, data: Vec<u8>) {
        // Ignore send error (means no active subscribers)
        let _ = self.tx.send(PtyBroadcastEvent { terminal_id, data });
    }

    /// Create a new subscriber receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<PtyBroadcastEvent> {
        self.tx.subscribe()
    }
}
