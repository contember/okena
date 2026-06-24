//! ToastManager — GPUI Global wrapper around the `Toast` data type from `okena-state`.

pub use okena_state::{Toast, ToastAction, ToastActionStyle, ToastLevel};

#[cfg(feature = "gpui")]
use gpui::{App, Global};
#[cfg(feature = "gpui")]
use parking_lot::Mutex;
#[cfg(feature = "gpui")]
use std::sync::Arc;

// ─── ToastManager (Global) ─────────────────────────────────────────────────

/// Maximum number of visible toasts
#[cfg(any(feature = "gpui", test))]
const MAX_VISIBLE_TOASTS: usize = 5;

/// Trim the queue to `MAX_VISIBLE_TOASTS`, preferring to drop the oldest toast
/// that has **no** actions. Toasts with actions (e.g. the soft-close "Undo"
/// toast) carry a pending operation that only resolves when the toast lives out
/// its TTL — evicting one early would silently strip the user's undo affordance
/// while the underlying close still goes through. Falls back to the oldest toast
/// when every toast has actions, so the hard cap is always honoured.
#[cfg(any(feature = "gpui", test))]
fn trim_to_cap(queue: &mut Vec<Toast>) {
    while queue.len() > MAX_VISIBLE_TOASTS {
        let idx = queue
            .iter()
            .position(|t| t.actions.is_empty())
            .unwrap_or(0);
        queue.remove(idx);
    }
}

#[cfg(feature = "gpui")]
#[derive(Clone)]
pub struct ToastManager(pub Arc<Mutex<Vec<Toast>>>);

#[cfg(feature = "gpui")]
impl Global for ToastManager {}

#[cfg(feature = "gpui")]
impl Default for ToastManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "gpui")]
impl ToastManager {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }

    /// Post a toast, capping the queue at MAX_VISIBLE_TOASTS (oldest dropped).
    pub fn post(toast: Toast, cx: &App) {
        match toast.level {
            ToastLevel::Error => {
                log::error!("[toast] {}", toast.message);
                eprintln!("[ERROR] {}", toast.message);
            }
            ToastLevel::Warning => {
                log::warn!("[toast] {}", toast.message);
                eprintln!("[WARN] {}", toast.message);
            }
            _ => {}
        }
        if let Some(tm) = cx.try_global::<ToastManager>() {
            let mut queue = tm.0.lock();
            queue.push(toast);
            trim_to_cap(&mut queue);
        }
    }

    #[allow(dead_code)]
    pub fn success(message: impl Into<String>, cx: &App) {
        Self::post(Toast::success(message), cx);
    }

    pub fn error(message: impl Into<String>, cx: &App) {
        Self::post(Toast::error(message), cx);
    }

    pub fn warning(message: impl Into<String>, cx: &App) {
        Self::post(Toast::warning(message), cx);
    }

    pub fn info(message: impl Into<String>, cx: &App) {
        Self::post(Toast::info(message), cx);
    }

    /// Post multiple toasts at once, capping the queue at MAX_VISIBLE_TOASTS.
    pub fn post_batch(toasts: Vec<Toast>, cx: &App) {
        if toasts.is_empty() {
            return;
        }
        if let Some(tm) = cx.try_global::<ToastManager>() {
            let mut queue = tm.0.lock();
            for toast in toasts {
                log::debug!("[hook toast] {}", toast.message);
                queue.push(toast);
            }
            trim_to_cap(&mut queue);
        }
    }

    /// Remove a toast by ID.
    pub fn dismiss(id: &str, cx: &App) {
        if let Some(tm) = cx.try_global::<ToastManager>() {
            tm.0.lock().retain(|t| t.id != id);
        }
    }

    /// Return non-expired toasts and prune expired ones from the queue.
    pub fn drain_snapshot(&self) -> Vec<Toast> {
        let mut queue = self.0.lock();
        queue.retain(|t| !t.is_expired());
        queue.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{trim_to_cap, Toast, ToastAction, ToastActionStyle, MAX_VISIBLE_TOASTS};

    fn action_toast(msg: &str) -> Toast {
        Toast::info(msg)
            .with_actions(vec![ToastAction::new("undo", "Undo", ToastActionStyle::Primary)])
    }

    #[test]
    fn trim_keeps_action_toast_and_drops_oldest_plain() {
        // Oldest entry is the undo toast, followed by enough plain toasts to
        // overflow the cap by one.
        let mut q = vec![action_toast("undo")];
        for i in 0..MAX_VISIBLE_TOASTS {
            q.push(Toast::info(format!("plain {i}")));
        }
        trim_to_cap(&mut q);
        assert_eq!(q.len(), MAX_VISIBLE_TOASTS);
        assert_eq!(q[0].message, "undo", "undo toast survives eviction");
        assert!(!q[0].actions.is_empty());
    }

    #[test]
    fn trim_falls_back_to_oldest_when_all_have_actions() {
        let mut q: Vec<Toast> = (0..MAX_VISIBLE_TOASTS + 1)
            .map(|i| action_toast(&format!("a{i}")))
            .collect();
        trim_to_cap(&mut q);
        assert_eq!(q.len(), MAX_VISIBLE_TOASTS);
        assert_eq!(q[0].message, "a1", "oldest action toast dropped as fallback");
    }
}
