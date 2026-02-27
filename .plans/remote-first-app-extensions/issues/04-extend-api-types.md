# Issue 04: Extend API types for app state and actions

**Priority:** high
**Files:** `crates/okena-core/src/api.rs`, `crates/okena-core/src/ws.rs`, `crates/okena-core/src/client/state.rs`

## Description

Extend the shared wire-protocol types so that app state can be transmitted and app actions can be dispatched through the existing API and WebSocket infrastructure.

## Implementation

### 1. `crates/okena-core/src/api.rs`

**Extend `ApiLayoutNode::App`:**

```rust
App {
    app_id: Option<String>,
    app_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    app_state: Option<serde_json::Value>,
}
```

**Add `ActionRequest::AppAction`:**

```rust
AppAction {
    project_id: String,
    app_id: String,
    action: serde_json::Value,
}
```

### 2. `crates/okena-core/src/ws.rs`

**Add to `WsInbound`:**

```rust
SubscribeApps { app_ids: Vec<String> },
UnsubscribeApps { app_ids: Vec<String> },
AppAction { app_id: String, action: serde_json::Value },
```

**Add to `WsOutbound`:**

```rust
AppStateChanged {
    app_id: String,
    app_kind: String,
    state: serde_json::Value,
},
```

### 3. `crates/okena-core/src/client/state.rs`

**Add `collect_all_app_ids()`:**

```rust
pub fn collect_all_app_ids(state: &StateResponse) -> HashSet<String> {
    let mut ids = HashSet::new();
    for project in &state.projects {
        collect_layout_app_ids(&project.layout, &mut ids);
    }
    ids
}

fn collect_layout_app_ids(node: &ApiLayoutNode, ids: &mut HashSet<String>) {
    match node {
        ApiLayoutNode::App { app_id: Some(id), .. } => { ids.insert(id.clone()); }
        ApiLayoutNode::Split { children, .. } => {
            for child in children { collect_layout_app_ids(child, ids); }
        }
        ApiLayoutNode::Tabs { children, .. } => {
            for child in children { collect_layout_app_ids(child, ids); }
        }
        _ => {}
    }
}
```

**Extend `StateDiff`:**

```rust
pub struct StateDiff {
    pub added_terminals: Vec<String>,
    pub removed_terminals: Vec<String>,
    pub added_apps: Vec<String>,
    pub removed_apps: Vec<String>,
    pub changed_projects: Vec<String>,
}
```

**Update `diff_states()`** to compute `added_apps` and `removed_apps` using `collect_all_app_ids()`.

## Acceptance Criteria

- `ApiLayoutNode::App` has optional `app_state` field that is skipped when None
- `ActionRequest::AppAction` deserializes correctly with `project_id`, `app_id`, `action`
- `WsInbound` has `SubscribeApps`, `UnsubscribeApps`, `AppAction` variants
- `WsOutbound` has `AppStateChanged` variant
- `StateDiff` tracks `added_apps` / `removed_apps`
- `collect_all_app_ids()` walks the full layout tree
- All existing tests still pass
- `cargo build` succeeds
