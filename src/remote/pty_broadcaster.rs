use okena_terminal::pty_manager::PtyOutputSink;
use tokio::sync::broadcast;

/// A PTY broadcast event for WebSocket subscribers.
#[derive(Clone, Debug)]
pub enum PtyBroadcastEvent {
    /// Terminal output data.
    Output { terminal_id: String, data: Vec<u8> },
    /// Terminal was resized (server-side).
    Resized { terminal_id: String, cols: u16, rows: u16 },
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
        let _ = self.tx.send(PtyBroadcastEvent::Output { terminal_id, data });
    }

    /// Publish a terminal resize event. Non-blocking; drops if no subscribers.
    pub fn publish_resize(&self, terminal_id: String, cols: u16, rows: u16) {
        let _ = self.tx.send(PtyBroadcastEvent::Resized { terminal_id, cols, rows });
    }

    /// Create a new subscriber receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<PtyBroadcastEvent> {
        self.tx.subscribe()
    }
}

impl PtyOutputSink for PtyBroadcaster {
    fn publish(&self, terminal_id: String, data: Vec<u8>) {
        self.publish(terminal_id, data);
    }

    fn publish_resize(&self, terminal_id: String, cols: u16, rows: u16) {
        self.publish_resize(terminal_id, cols, rows);
    }
}
