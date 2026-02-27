# Issue 11: Desktop remote client support

**Priority:** medium
**Files:** `src/remote_client/connection.rs`, `src/remote_client/backend.rs`, `src/remote_client/manager.rs`, new file: `src/views/layout/remote_app_pane.rs`

## Description

When a desktop client connects to a remote Okena instance, app panes should render and be interactive. Create a `RemoteAppPane` entity that renders from `KruhViewState` and sends actions via WebSocket.

## Implementation

### 1. `src/views/layout/remote_app_pane.rs` (new file)

A GPUI entity that:
- Stores the latest `KruhViewState` (or `serde_json::Value`) and re-renders when it changes
- Has an action callback that serializes `KruhAction` and sends via WebSocket
- Renders the same UI as KruhPane but driven by the view state snapshot

```rust
pub struct RemoteAppPane {
    app_id: String,
    app_kind: String,
    state: Option<KruhViewState>,
    action_sender: Box<dyn Fn(KruhAction) + Send + Sync>,
    // Local GPUI state for scroll, focus, etc.
    scroll_handle: ScrollHandle,
    focus_handle: FocusHandle,
}

impl RemoteAppPane {
    pub fn update_state(&mut self, state: KruhViewState, cx: &mut Context<Self>) {
        self.state = Some(state);
        cx.notify();
    }
}

impl Render for RemoteAppPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Render based on self.state — mirror KruhPane's render logic
        // Each interactive element calls (self.action_sender)(KruhAction::...)
    }
}
```

### 2. WebSocket subscription in `connection.rs`

When `StateReceived` arrives with app nodes in the layout:
- Diff app IDs using `collect_all_app_ids()` from okena-core
- Send `WsInbound::SubscribeApps` for new apps, `UnsubscribeApps` for removed ones
- Handle `WsOutbound::AppStateChanged` messages: update the corresponding `RemoteAppPane` entity

### 3. Layout rendering for remote apps

In `layout_container.rs`, when rendering a `LayoutNode::App` from a remote project:
- Check if a `RemoteAppPane` entity exists for this app_id
- If yes, render it
- If no, create one with the initial state from `app_state` in the layout node

### 4. Action dispatch

When `RemoteAppPane`'s action callback fires:
- Serialize the `KruhAction` to JSON
- Send via WebSocket as `WsInbound::AppAction { app_id, action }`

## Acceptance Criteria

- Remote desktop client renders KruhPane state from a connected server
- State updates stream in real-time via WebSocket
- Clicking buttons on the remote client sends actions to the server
- Server's KruhPane mutates in response and broadcasts the new state
- `cargo build` succeeds
