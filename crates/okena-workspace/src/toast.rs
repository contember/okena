//! ToastManager — GPUI Global wrapper around the `Toast` data type from `okena-state`.

pub use okena_state::{Toast, ToastLevel};

use gpui::{App, Global};
use parking_lot::Mutex;
use std::sync::Arc;

// ─── ToastManager (Global) ─────────────────────────────────────────────────

/// Maximum number of visible toasts
const MAX_VISIBLE_TOASTS: usize = 5;

#[derive(Clone)]
pub struct ToastManager(pub Arc<Mutex<Vec<Toast>>>);

impl Global for ToastManager {}

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
            // Drop oldest if over cap
            while queue.len() > MAX_VISIBLE_TOASTS {
                queue.remove(0);
            }
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
            while queue.len() > MAX_VISIBLE_TOASTS {
                queue.remove(0);
            }
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
