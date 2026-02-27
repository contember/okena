# Issue 06: App state publishing from GPUI thread

**Priority:** high
**Files:** `src/views/layout/kruh_pane/mod.rs`, `src/views/layout/app_registry.rs`, `src/views/layout/layout_container.rs`, `src/views/root.rs` or equivalent, `src/app.rs` or wherever `Okena`/`RootView` is created

## Description

Thread the `AppStateBroadcaster` from the top-level app down through the layout container into KruhPane. When KruhPane's state changes, debounce and publish the serialized view state.

## Implementation

### 1. KruhPane receives broadcaster

In `src/views/layout/kruh_pane/mod.rs`:

- Add field: `app_broadcaster: Option<Arc<AppStateBroadcaster>>`
- Accept it in constructor: `pub fn new(..., app_broadcaster: Option<Arc<AppStateBroadcaster>>, ...) -> Self`
- In the constructor, set up a notification-based publish using `cx.observe_self()` or equivalent:

```rust
// After construction, set up debounced state publishing
if app_broadcaster.is_some() {
    // Use cx.observe_self to watch for changes and publish
    // Debounce at 100ms to prevent flooding during rapid agent output
    cx.observe_self(|this, cx| {
        this.schedule_state_publish(cx);
    });
}
```

- Add `schedule_state_publish` method that uses a debounce timer (100ms):

```rust
fn schedule_state_publish(&mut self, cx: &mut Context<Self>) {
    if let Some(broadcaster) = &self.app_broadcaster {
        let state = self.view_state(cx);
        let app_kind = "kruh".to_string();
        if let Ok(json) = serde_json::to_value(&state) {
            if let Some(app_id) = &self.app_id {
                broadcaster.publish(app_id.clone(), app_kind, json);
            }
        }
    }
}
```

For the 100ms debounce: store a `publish_timer: Option<Task<()>>` field. In `schedule_state_publish`, cancel any pending timer and set a new one that fires after 100ms and calls the actual publish. This prevents flooding when output lines arrive rapidly.

### 2. Thread broadcaster through creation chain

- `src/views/layout/app_registry.rs` — `create_app_pane()` takes `app_broadcaster: Option<Arc<AppStateBroadcaster>>` and passes to `KruhPane::new()`
- `src/views/layout/layout_container.rs` — store `app_broadcaster: Option<Arc<AppStateBroadcaster>>` field, pass to `create_app_pane()`
- Thread from wherever `LayoutContainer` is created (likely `ProjectColumn` or `RootView`) — accept and store the broadcaster

### 3. Create broadcaster at app level

Where the remote server is initialized (likely `src/app.rs` or `Okena::new()`), create the `AppStateBroadcaster` and pass it down:
- If remote is enabled: `Some(Arc::new(AppStateBroadcaster::new()))`
- If remote is disabled: `None` (no overhead)

Also store a reference so the WebSocket handler can access it (Step 8 will consume it).

## Acceptance Criteria

- KruhPane publishes state on every notify (debounced 100ms)
- Broadcaster is `None` when remote server is not running (no overhead)
- Threading: `Okena` → layout layers → `create_app_pane()` → `KruhPane::new()`
- State is published as serialized `KruhViewState` JSON
- `cargo build` succeeds
