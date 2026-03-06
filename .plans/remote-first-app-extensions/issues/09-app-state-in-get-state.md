# Issue 09: Include app state in GET /v1/state

**Priority:** medium
**Files:** `src/remote/types.rs`, `src/app/remote_commands.rs`

## Description

When a client requests `GET /v1/state`, populate the `app_state` field in `ApiLayoutNode::App` nodes so clients get the full app state on initial load (before subscribing to WebSocket updates).

## Implementation

### 1. `src/remote/types.rs`

Update the `to_api()` conversion for `LayoutNode::App` to include the `app_state` field:

```rust
LayoutNode::App { app_id, app_kind, app_config } => {
    ApiLayoutNode::App {
        app_id: app_id.clone(),
        app_kind: app_kind.clone(),
        app_state: None, // Will be populated by the handler
    }
}
```

### 2. `src/app/remote_commands.rs`

In the `RemoteCommand::GetState` handler, after building the `StateResponse`, walk the layout tree and populate `app_state` from the `AppEntityRegistry`:

```rust
RemoteCommand::GetState => {
    let mut state = build_state_response(...);
    // Populate app states
    for project in &mut state.projects {
        populate_app_states(&mut project.layout, &app_registry, &mut cx);
    }
    CommandResult::Ok(Some(serde_json::to_value(&state).unwrap()))
}

fn populate_app_states(
    node: &mut ApiLayoutNode,
    registry: &AppEntityRegistry,
    cx: &mut AsyncWindowContext,
) {
    match node {
        ApiLayoutNode::App { app_id: Some(id), app_state, .. } => {
            *app_state = registry.get_view_state(id, cx);
        }
        ApiLayoutNode::Split { children, .. } => {
            for child in children {
                populate_app_states(child, registry, cx);
            }
        }
        ApiLayoutNode::Tabs { children, .. } => {
            for child in children {
                populate_app_states(child, registry, cx);
            }
        }
        _ => {}
    }
}
```

## Acceptance Criteria

- `GET /v1/state` response includes `app_state` in `App` layout nodes
- `app_state` is the current serialized `KruhViewState`
- Apps without state (no ID, not registered) have `app_state: null` (omitted in JSON)
- Existing state response structure is preserved — only `App` nodes gain the new field
- `cargo build` succeeds
