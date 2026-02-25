use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gpui::{App, AsyncApp, Global};
use serde_json::Value;

/// Type-erased handle to an app entity.
pub struct AppEntityHandle {
    pub app_kind: String,
    /// Get serialized view state — called from async context.
    pub view_state: Arc<dyn Fn(&mut AsyncApp) -> Option<Value> + Send + Sync>,
    /// Dispatch a serialized action — called from sync App context.
    pub handle_action: Arc<dyn Fn(Value, &mut App) -> Result<(), String> + Send + Sync>,
}

/// Registry of all active app entities, keyed by app_id.
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

    /// Get the serialized view state for an app.
    ///
    /// Acquires the mutex briefly to clone the `Arc`, then calls the closure
    /// without holding the lock — avoids deadlock with the GPUI main thread.
    pub fn get_view_state(&self, app_id: &str, cx: &mut AsyncApp) -> Option<Value> {
        let view_state = {
            let apps = self.apps.lock().unwrap();
            apps.get(app_id).map(|h| h.view_state.clone())
        };
        view_state.and_then(|f| f(cx))
    }

    /// Dispatch a serialized action to an app.
    ///
    /// Acquires the mutex briefly to clone the `Arc`, then calls the closure
    /// without holding the lock — avoids deadlock with the GPUI main thread.
    pub fn handle_action(
        &self,
        app_id: &str,
        action: Value,
        cx: &mut App,
    ) -> Result<(), String> {
        let handle_action = {
            let apps = self.apps.lock().unwrap();
            apps.get(app_id)
                .map(|h| h.handle_action.clone())
                .ok_or_else(|| format!("App not found: {}", app_id))?
        };
        handle_action(action, cx)
    }

    pub fn app_kind(&self, app_id: &str) -> Option<String> {
        self.apps.lock().unwrap().get(app_id).map(|h| h.app_kind.clone())
    }
}

/// GPUI global wrapping `Arc<AppEntityRegistry>` for shared access.
pub struct GlobalAppEntityRegistry(pub Arc<AppEntityRegistry>);

impl Global for GlobalAppEntityRegistry {}
