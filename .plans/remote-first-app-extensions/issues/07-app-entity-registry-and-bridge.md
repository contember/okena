# Issue 07: App Entity Registry and Bridge commands

**Priority:** high
**Files:** `src/views/layout/app_entity_registry.rs` (new), `src/views/layout/kruh_pane/mod.rs`, `src/remote/bridge.rs`, `src/app/remote_commands.rs`

## Description

Create a type-erased registry that maps `app_id → (view_state closure, handle_action closure)` so the remote bridge can access any app entity without knowing its concrete type. Add bridge commands for app state and actions.

## Implementation

### 1. `src/views/layout/app_entity_registry.rs` (new file)

```rust
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use serde_json::Value;
use gpui::{Entity, AsyncWindowContext};

/// Type-erased handle to an app entity
pub struct AppEntityHandle {
    pub app_kind: String,
    /// Get serialized view state — called from async context
    pub view_state: Box<dyn Fn(&mut AsyncWindowContext) -> Option<Value> + Send + Sync>,
    /// Dispatch a serialized action — called from async context
    pub handle_action: Box<dyn Fn(Value, &mut AsyncWindowContext) -> Result<(), String> + Send + Sync>,
}

/// Registry of all active app entities, keyed by app_id
pub struct AppEntityRegistry {
    apps: Mutex<HashMap<String, AppEntityHandle>>,
}

impl AppEntityRegistry {
    pub fn new() -> Self {
        Self { apps: Mutex::new(HashMap::new()) }
    }

    pub fn register(&self, app_id: String, handle: AppEntityHandle) {
        self.apps.lock().unwrap().insert(app_id, handle);
    }

    pub fn unregister(&self, app_id: &str) {
        self.apps.lock().unwrap().remove(app_id);
    }

    pub fn get_view_state(&self, app_id: &str, cx: &mut AsyncWindowContext) -> Option<Value> {
        let apps = self.apps.lock().unwrap();
        apps.get(app_id).and_then(|h| (h.view_state)(cx))
    }

    pub fn handle_action(&self, app_id: &str, action: Value, cx: &mut AsyncWindowContext) -> Result<(), String> {
        let apps = self.apps.lock().unwrap();
        match apps.get(app_id) {
            Some(h) => (h.handle_action)(action, cx),
            None => Err(format!("App not found: {}", app_id)),
        }
    }

    pub fn app_kind(&self, app_id: &str) -> Option<String> {
        self.apps.lock().unwrap().get(app_id).map(|h| h.app_kind.clone())
    }
}
```

### 2. KruhPane registers itself

In `src/views/layout/kruh_pane/mod.rs`:

- Accept `Arc<AppEntityRegistry>` in constructor (when remote is enabled)
- In constructor, register with type-erased closures:

```rust
if let Some(ref registry) = app_registry {
    if let Some(ref app_id) = self.app_id {
        let entity = cx.entity().downgrade();
        registry.register(app_id.clone(), AppEntityHandle {
            app_kind: "kruh".to_string(),
            view_state: Box::new(move |cx| {
                entity.update(cx, |pane, cx| {
                    serde_json::to_value(pane.view_state(cx)).ok()
                }).ok().flatten()
            }),
            handle_action: Box::new(move |action, cx| {
                let action: KruhAction = serde_json::from_value(action)
                    .map_err(|e| format!("Invalid action: {}", e))?;
                entity.update(cx, |pane, cx| {
                    pane.handle_action(action, cx);
                }).map_err(|_| "Entity released".to_string())
            }),
        });
    }
}
```

- In Drop or a cleanup method, unregister.

### 3. Bridge commands

In `src/remote/bridge.rs`, add variants:

```rust
pub enum RemoteCommand {
    // ... existing ...
    GetAppState { app_id: String },
    AppAction { project_id: String, app_id: String, action: serde_json::Value },
}
```

### 4. Handle in remote_commands.rs

In `src/app/remote_commands.rs`, handle the new commands:

```rust
RemoteCommand::GetAppState { app_id } => {
    match app_registry.get_view_state(&app_id, &mut cx) {
        Some(state) => CommandResult::Ok(Some(state)),
        None => CommandResult::Err(format!("App not found: {}", app_id)),
    }
}
RemoteCommand::AppAction { project_id, app_id, action } => {
    match app_registry.handle_action(&app_id, action, &mut cx) {
        Ok(()) => CommandResult::Ok(None),
        Err(e) => CommandResult::Err(e),
    }
}
```

Thread `Arc<AppEntityRegistry>` into the command loop (same pattern as existing parameters).

## Acceptance Criteria

- `AppEntityRegistry` allows registering and unregistering apps by ID
- Type-erased closures work for `view_state()` and `handle_action()`
- KruhPane registers on creation, unregisters on drop
- Bridge has `GetAppState` and `AppAction` commands
- `remote_commands.rs` handles both new commands
- `cargo build` succeeds
