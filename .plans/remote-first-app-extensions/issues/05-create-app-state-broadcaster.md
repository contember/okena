# Issue 05: Create AppStateBroadcaster

**Priority:** high
**Files:** `src/remote/app_broadcaster.rs` (new), `src/remote/mod.rs`

## Description

Create a tokio broadcast channel for app state changes, analogous to how PTY output is broadcast to WebSocket subscribers. App state is JSON (not binary frames) since it's structured data.

## Implementation

### 1. `src/remote/app_broadcaster.rs` (new file)

```rust
use std::sync::Arc;
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
```

### 2. `src/remote/mod.rs`

Add `pub mod app_broadcaster;`

## Acceptance Criteria

- `AppStateBroadcaster` compiles and is accessible as `crate::remote::app_broadcaster::AppStateBroadcaster`
- `publish()` does not block or panic when there are no subscribers
- `subscribe()` returns a receiver that gets events after subscribing
- Channel buffer is 64 (matches plan decision — app state snapshots are small)
- `cargo build` succeeds
