# Issue 08: WebSocket handler for app subscriptions

**Priority:** high
**Files:** `src/remote/routes/stream.rs`

## Description

Extend the WebSocket handler to support app subscriptions. Clients can subscribe to app IDs and receive `AppStateChanged` messages when app state changes. Also handle `AppAction` messages from clients.

## Implementation

### `src/remote/routes/stream.rs`

The existing WS handler has a `tokio::select!` loop with branches for socket recv, PTY broadcast, state changes, and git status. Add:

**1. New state in the handler:**

```rust
let mut subscribed_app_ids: HashSet<String> = HashSet::new();
let mut app_rx = app_broadcaster.subscribe(); // from AppStateBroadcaster
```

**2. New `tokio::select!` branch for app state:**

```rust
event = app_rx.recv() => {
    match event {
        Ok(AppBroadcastEvent { app_id, app_kind, state }) => {
            if subscribed_app_ids.contains(&app_id) {
                let msg = WsOutbound::AppStateChanged { app_id, app_kind, state };
                if let Ok(json) = serde_json::to_string(&msg) {
                    let _ = socket.send(Message::Text(json)).await;
                }
            }
        }
        Err(broadcast::error::RecvError::Lagged(n)) => {
            // Subscriber fell behind — acceptable, they'll get the next state
            log::warn!("App broadcast lagged by {} events", n);
        }
        Err(broadcast::error::RecvError::Closed) => break,
    }
}
```

**3. Handle new `WsInbound` messages:**

```rust
WsInbound::SubscribeApps { app_ids } => {
    subscribed_app_ids.extend(app_ids.iter().cloned());
    // Send current state for each newly subscribed app
    for app_id in &app_ids {
        let result = bridge_tx.send(RemoteCommand::GetAppState { app_id: app_id.clone() }).await;
        if let Ok(CommandResult::Ok(Some(state))) = result {
            if let Some(app_kind) = /* get from registry or state */ {
                let msg = WsOutbound::AppStateChanged {
                    app_id: app_id.clone(),
                    app_kind,
                    state,
                };
                let _ = socket.send(Message::Text(serde_json::to_string(&msg).unwrap())).await;
            }
        }
    }
}

WsInbound::UnsubscribeApps { app_ids } => {
    for id in &app_ids {
        subscribed_app_ids.remove(id);
    }
}

WsInbound::AppAction { app_id, action } => {
    // Route through bridge to GPUI thread
    let _ = bridge_tx.send(RemoteCommand::AppAction {
        project_id: String::new(), // resolved server-side
        app_id,
        action,
    }).await;
}
```

**4. Thread `AppStateBroadcaster` into the handler:**

The `AppStateBroadcaster` (or its `Arc`) needs to be available in the route handler. Pass it through axum state or the handler's function parameters — follow the same pattern used for `PtyBroadcaster` / `bridge_tx`.

## Acceptance Criteria

- Clients can send `SubscribeApps` and receive `AppStateChanged` for subscribed apps
- Initial state is sent on subscribe (so client doesn't start blank)
- `UnsubscribeApps` stops delivery
- `AppAction` messages route through bridge to GPUI thread
- Lagged receivers log a warning but don't crash
- `cargo build` succeeds
