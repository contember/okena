use tokio::sync::broadcast;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct AppBroadcastEvent {
    pub app_id: String,
    pub app_kind: String,
    pub state: Value,
}

pub struct AppStateBroadcaster {
    tx: broadcast::Sender<AppBroadcastEvent>,
}

impl AppStateBroadcaster {
    pub fn new() -> Self {
        // Buffer 64 events — if a subscriber falls behind, they skip intermediate states
        // (which is fine — they'll get the latest state on next broadcast)
        let (tx, _) = broadcast::channel(64);
        Self { tx }
    }

    /// Publish a new state snapshot for an app. Non-blocking.
    pub fn publish(&self, app_id: String, app_kind: String, state: Value) {
        // Ignore send errors (no subscribers)
        let _ = self.tx.send(AppBroadcastEvent { app_id, app_kind, state });
    }

    /// Subscribe to app state changes
    pub fn subscribe(&self) -> broadcast::Receiver<AppBroadcastEvent> {
        self.tx.subscribe()
    }
}
