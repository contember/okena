//! GPUI-free toast forwarder: drains the daemon's [`HookMonitor`] pending toasts
//! and broadcasts them to connected clients as [`ApiToast`]s.
//!
//! ## Why this exists
//!
//! In the in-process GUI, the toast overlay polls `HookMonitor::drain_pending_toasts`
//! every ~50ms and shows the results via `ToastManager`. The daemon runs the
//! lifecycle hooks (so hook failures queue toasts in *its* `HookMonitor`) but has
//! no surface to display them — and the thin-client GUI never sees them because
//! the hooks ran remotely. This loop closes that gap: it periodically drains the
//! daemon's `HookMonitor` and pushes each toast onto the toast broadcast that the
//! remote server fans out over the WebSocket (`WsOutbound::Toast`).
//!
//! ## Draining vs. delivery
//!
//! Draining is unconditional: we drain (and thus clear) the pending queue every
//! cycle even when no client is connected, so the queue can never grow unbounded
//! while the daemon runs unattended. `broadcast::Sender::send` returns
//! `Err(SendError)` when there are no receivers — that is expected and ignored
//! (fire-and-forget, mirroring the git-status broadcast). A toast produced while
//! no client is connected is simply dropped; daemon toasts are non-critical
//! notifications, not durable state.

use std::time::Duration;

use okena_core::api::ApiToast;
use okena_hooks::HookMonitor;

/// How often to drain the `HookMonitor` and forward pending toasts. Roughly
/// matches the GUI toast overlay's 50ms poll, but looser since these toasts are
/// non-critical and a small extra latency is unnoticeable.
const TOAST_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Run the daemon toast-forward loop forever (cancelled on daemon shutdown when
/// its `LocalSet`/runtime is torn down).
///
/// Each cycle drains the `HookMonitor`'s pending toasts and broadcasts each one
/// as an [`ApiToast`]; a send with no receivers is ignored. When the daemon has
/// no `HookMonitor` (hooks disabled) there is nothing to forward, so the loop
/// returns immediately rather than spinning.
pub async fn run_toast_poll(
    hook_monitor: Option<HookMonitor>,
    toast_tx: tokio::sync::broadcast::Sender<ApiToast>,
) {
    let Some(hook_monitor) = hook_monitor else {
        // No hook monitor → no toast source. Nothing to do.
        return;
    };

    loop {
        for toast in hook_monitor.drain_pending_toasts() {
            // Ignore the no-receivers error: clients may come and go, and these
            // toasts are fire-and-forget. Draining above already cleared the
            // queue so it cannot grow unbounded regardless of delivery.
            let _ = toast_tx.send(toast.to_api());
        }
        tokio::time::sleep(TOAST_POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_hooks::HookStatus;

    /// A failed hook queues a toast in the monitor; one drain cycle forwards it
    /// to a subscribed receiver as an `ApiToast` carrying the error level.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn forwards_pending_hook_toast_to_receiver() {
        let monitor = HookMonitor::new();
        let id = monitor.record_start("pre_merge", "exit 1", "proj", None);
        monitor.record_finish(
            id,
            HookStatus::Failed {
                duration: Duration::from_millis(1),
                exit_code: 1,
                stderr: "boom".to_string(),
            },
        );

        let (tx, mut rx) = tokio::sync::broadcast::channel::<ApiToast>(8);

        // Manually run a single drain cycle (the public loop sleeps forever).
        for toast in monitor.drain_pending_toasts() {
            let _ = tx.send(toast.to_api());
        }

        let api = rx.try_recv().expect("a toast should have been forwarded");
        assert_eq!(api.level, "error");
        assert!(api.message.contains("pre_merge"));
        // The queue was drained — a second cycle forwards nothing.
        assert!(monitor.drain_pending_toasts().is_empty());
    }

    /// Draining still happens with no receivers: the `send` error is swallowed
    /// and the queue is cleared so it cannot grow unbounded.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn drains_even_without_receivers() {
        let monitor = HookMonitor::new();
        let id = monitor.record_start("on_open", "bad", "proj", None);
        monitor.record_finish(
            id,
            HookStatus::SpawnError {
                message: "not found".to_string(),
            },
        );

        // Sender with no live receiver: `send` errors, but draining proceeds.
        let (tx, _) = tokio::sync::broadcast::channel::<ApiToast>(8);
        for toast in monitor.drain_pending_toasts() {
            let _ = tx.send(toast.to_api());
        }

        assert!(monitor.drain_pending_toasts().is_empty());
    }

    /// No hook monitor → the loop returns immediately (no panic, no spin).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn returns_immediately_without_hook_monitor() {
        let (tx, _) = tokio::sync::broadcast::channel::<ApiToast>(8);
        run_toast_poll(None, tx).await;
    }
}
