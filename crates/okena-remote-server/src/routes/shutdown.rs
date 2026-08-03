//! `POST /v1/shutdown` — loopback-only, client-aware daemon shutdown.
//!
//! A quitting GUI arms its UI-owned daemon to stop after the final authenticated
//! client disconnects. Standalone daemons ignore desktop lifecycle handoff.
//!
//! Self-exclusion: the caller disconnects its OWN loopback WS before calling and
//! the daemon simply counts live WS connections — see `local::request_local_shutdown`.
//!
//! Every host wakes the shared graceful daemon run loop.

use crate::routes::{AppState, PeerInfo};
use axum::Json;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Grace before teardown so the HTTP ack is flushed to the client before the
/// connection drops (mirrors the restart route's `EXIT_DELAY`).
const EXIT_DELAY: Duration = Duration::from_millis(300);

/// How often the idle-exit monitor samples the live-client count.
const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(3);
/// How long a UI-owned daemon may sit with zero clients (after having served at
/// least one) before it self-terminates. Long enough to absorb a GUI's normal
/// reconnect blip, short enough that a closed/crashed GUI's daemon doesn't
/// linger holding the instance lock.
const IDLE_EXIT_GRACE: Duration = Duration::from_secs(10);

/// Safety-net that self-terminates a UI-owned daemon once it has been idle (zero
/// authenticated clients) for [`IDLE_EXIT_GRACE`] after serving at least one.
///
/// The explicit `POST /v1/shutdown` handshake only fires on a *clean* GUI quit.
/// A GUI that crashes / is force-quit / SIGKILLed never arms shutdown, yet a
/// UI-owned daemon with no client has no reason to keep running (and blocks the
/// next launch by holding the instance lock). The `had_client` gate ensures we
/// never reap a freshly-spawned daemon before its GUI makes first contact.
/// Standalone (non-UI-owned) daemons never run this — they are meant to outlive
/// clients.
pub(crate) async fn run_idle_exit_monitor(
    active_connections: Arc<AtomicU64>,
    had_client: Arc<AtomicBool>,
    process_shutdown: Arc<tokio::sync::Notify>,
) {
    idle_exit_monitor(
        active_connections,
        had_client,
        process_shutdown,
        IDLE_CHECK_INTERVAL,
        IDLE_EXIT_GRACE,
    )
    .await
}

/// Parameterized core of [`run_idle_exit_monitor`] (timings injectable for tests).
async fn idle_exit_monitor(
    active_connections: Arc<AtomicU64>,
    had_client: Arc<AtomicBool>,
    process_shutdown: Arc<tokio::sync::Notify>,
    check_interval: Duration,
    grace: Duration,
) {
    let mut idle_since: Option<Instant> = None;
    let mut ticker = tokio::time::interval(check_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        let count = active_connections.load(Ordering::SeqCst);
        if had_client.load(Ordering::SeqCst) && count == 0 {
            let since = *idle_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= grace {
                log::info!(
                    "UI-owned daemon idle with no clients for {grace:?} after serving one; exiting"
                );
                process_shutdown.notify_one();
                return;
            }
        } else {
            // A client is (re)connected — reset the idle timer.
            idle_since = None;
        }
    }
}

/// Wire response for `POST /v1/shutdown`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShutdownResponse {
    /// Whether the daemon accepted and is now tearing itself down.
    pub shutting_down: bool,
    /// Live client connections the daemon counted at decision time.
    pub active_clients: u64,
}

/// Only daemons explicitly spawned for a desktop lifecycle accept quit handoff.
pub fn should_shut_down(ui_owned: bool) -> bool {
    ui_owned
}

pub(crate) fn schedule_process_shutdown(state: &AppState) {
    let notify = state.process_shutdown.clone();
    tokio::spawn(async move {
        tokio::time::sleep(EXIT_DELAY).await;
        notify.notify_one();
    });
}

pub async fn post_shutdown(
    Extension(peer): Extension<PeerInfo>,
    State(state): State<AppState>,
) -> Response {
    if !peer.is_local_trusted() {
        return StatusCode::FORBIDDEN.into_response();
    }

    if !should_shut_down(state.ui_owned) {
        let active_clients = state.active_connections.load(Ordering::Relaxed);
        log::info!("Shutdown ignored for standalone daemon");
        return Json(ShutdownResponse {
            shutting_down: false,
            active_clients,
        })
        .into_response();
    }

    // Arm before reading the connection count so a concurrent last disconnect
    // either observes the flag or is observed by the load below.
    state.shutdown_when_idle.store(true, Ordering::SeqCst);
    let active_clients = state.active_connections.load(Ordering::SeqCst);
    if active_clients == 0 && state.shutdown_when_idle.swap(false, Ordering::SeqCst) {
        log::info!("UI-owned daemon shutting down with no clients remaining");
        schedule_process_shutdown(&state);
    } else {
        log::info!(
            "UI-owned daemon shutdown armed until {active_clients} remaining client(s) disconnect"
        );
    }

    Json(ShutdownResponse {
        shutting_down: true,
        active_clients,
    })
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::should_shut_down;
    use crate::routes::PeerInfo;
    use std::net::{IpAddr, Ipv6Addr, SocketAddr};

    #[test]
    fn only_ui_owned_daemons_accept_lifecycle_handoff() {
        assert!(should_shut_down(true));
        assert!(!should_shut_down(false));
    }

    #[test]
    fn loopback_peers_can_shut_down() {
        assert!(PeerInfo::Local.is_local_trusted());
        assert!(PeerInfo::Tcp(SocketAddr::from(([127, 0, 0, 1], 19100))).is_local_trusted());
        assert!(
            PeerInfo::Tcp(SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 19100))).is_local_trusted()
        );
    }

    #[test]
    fn non_loopback_peers_cannot_shut_down() {
        assert!(!PeerInfo::Tcp(SocketAddr::from(([192, 168, 1, 50], 19100))).is_local_trusted());
        assert!(!PeerInfo::Tcp(SocketAddr::from(([10, 0, 0, 2], 19100))).is_local_trusted());
    }

    #[test]
    fn ipv4_mapped_loopback_is_trusted() {
        let mapped = Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x7f00, 0x0001);
        assert!(PeerInfo::Tcp(SocketAddr::new(IpAddr::V6(mapped), 19100)).is_local_trusted());
    }

    use super::idle_exit_monitor;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::Duration;
    use tokio::sync::Notify;

    // Tiny timings so the tests finish in milliseconds.
    const T_INTERVAL: Duration = Duration::from_millis(5);
    const T_GRACE: Duration = Duration::from_millis(20);

    fn spawn_monitor(count: u64, had_client: bool) -> Arc<Notify> {
        let active = Arc::new(AtomicU64::new(count));
        let had = Arc::new(AtomicBool::new(had_client));
        let notify = Arc::new(Notify::new());
        tokio::spawn(idle_exit_monitor(
            active,
            had,
            notify.clone(),
            T_INTERVAL,
            T_GRACE,
        ));
        notify
    }

    /// Served a client, now idle past the grace window → self-exit fires.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn idle_monitor_exits_when_idle_after_serving_a_client() {
        let notify = spawn_monitor(0, true);
        tokio::time::timeout(Duration::from_secs(2), notify.notified())
            .await
            .expect("idle UI-owned daemon must self-exit after the grace window");
    }

    /// Never served a client (freshly spawned) → must NOT exit before its GUI
    /// makes first contact.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn idle_monitor_waits_for_first_client() {
        let notify = spawn_monitor(0, false);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), notify.notified())
                .await
                .is_err(),
            "must not exit before any client has connected"
        );
    }

    /// A client is connected → never exits.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn idle_monitor_stays_while_client_connected() {
        let notify = spawn_monitor(1, true);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), notify.notified())
                .await
                .is_err(),
            "must not exit while a client is connected"
        );
    }

    /// A reconnect within the grace window resets the timer (no premature exit).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn idle_monitor_reconnect_resets_grace() {
        let active = Arc::new(AtomicU64::new(0));
        let had = Arc::new(AtomicBool::new(true));
        let notify = Arc::new(Notify::new());
        tokio::spawn(idle_exit_monitor(
            active.clone(),
            had,
            notify.clone(),
            T_INTERVAL,
            T_GRACE,
        ));
        // Reconnect before the grace elapses, then stay connected.
        tokio::time::sleep(Duration::from_millis(10)).await;
        active.store(1, Ordering::SeqCst);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), notify.notified())
                .await
                .is_err(),
            "a reconnect within grace must cancel the pending exit"
        );
    }
}
