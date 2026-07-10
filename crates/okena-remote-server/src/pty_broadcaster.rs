use okena_terminal::pty_manager::PtyOutputSink;
use parking_lot::Mutex;
use std::collections::HashMap;
use tokio::sync::broadcast;

/// A PTY broadcast event for WebSocket subscribers.
#[derive(Clone, Debug)]
pub enum PtyBroadcastEvent {
    /// Terminal output data.
    Output {
        terminal_id: String,
        data: Vec<u8>,
        sequence: u64,
    },
    /// Terminal was resized (server-side). `server_owns` is true when the
    /// origin's local user currently holds resize authority.
    Resized {
        terminal_id: String,
        cols: u16,
        rows: u16,
        server_owns: bool,
        owner_connection_id: Option<String>,
    },
}

/// Fan-out PTY events to WebSocket subscribers.
///
/// Uses `tokio::sync::broadcast` with a bounded buffer. When a subscriber
/// falls behind, `recv()` returns `Lagged(n)` and the subscriber should
/// notify the client with a `dropped` message.
pub struct PtyBroadcaster {
    tx: broadcast::Sender<PtyBroadcastEvent>,
    publish_state: Mutex<PublishState>,
}

struct PublishState {
    next_sequence: u64,
    last_published: HashMap<String, u64>,
}

impl Default for PtyBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

impl PtyBroadcaster {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(4096);
        Self {
            tx,
            publish_state: Mutex::new(PublishState {
                next_sequence: 1,
                last_published: HashMap::new(),
            }),
        }
    }

    /// Publish a PTY output event. Non-blocking; drops if no subscribers.
    pub fn publish(&self, terminal_id: String, data: Vec<u8>) -> u64 {
        let mut state = self.publish_state.lock();
        let sequence = state.next_sequence;
        state.next_sequence += 1;
        state.last_published.insert(terminal_id.clone(), sequence);
        let _ = self.tx.send(PtyBroadcastEvent::Output {
            terminal_id,
            data,
            sequence,
        });
        sequence
    }

    /// Publish a terminal resize event. Non-blocking; drops if no subscribers.
    pub fn publish_resize(
        &self,
        terminal_id: String,
        cols: u16,
        rows: u16,
        server_owns: bool,
        owner_connection_id: Option<String>,
    ) {
        let _ = self.tx.send(PtyBroadcastEvent::Resized {
            terminal_id,
            cols,
            rows,
            server_owns,
            owner_connection_id,
        });
    }

    /// Create a new subscriber receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<PtyBroadcastEvent> {
        self.tx.subscribe()
    }

    pub fn last_published_sequence(&self, terminal_id: &str) -> u64 {
        self.publish_state
            .lock()
            .last_published
            .get(terminal_id)
            .copied()
            .unwrap_or(0)
    }
}

impl PtyOutputSink for PtyBroadcaster {
    fn publish(&self, terminal_id: String, data: Vec<u8>) -> u64 {
        PtyBroadcaster::publish(self, terminal_id, data)
    }

    fn publish_resize(
        &self,
        terminal_id: String,
        cols: u16,
        rows: u16,
        server_owns: bool,
        owner_connection_id: Option<String>,
    ) {
        self.publish_resize(terminal_id, cols, rows, server_owns, owner_connection_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn output_sequences_are_monotonic_and_travel_with_events() {
        let broadcaster = PtyBroadcaster::new();
        let mut receiver = broadcaster.subscribe();

        let first = broadcaster.publish("terminal".to_string(), b"a".to_vec());
        let second = broadcaster.publish("terminal".to_string(), b"b".to_vec());
        assert!(second > first);
        assert_eq!(broadcaster.last_published_sequence("terminal"), second);

        match receiver.recv().await.unwrap() {
            PtyBroadcastEvent::Output { sequence, .. } => assert_eq!(sequence, first),
            PtyBroadcastEvent::Resized { .. } => panic!("expected output"),
        }
    }
}
