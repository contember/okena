//! `POST /v1/shutdown` — loopback-only, client-aware daemon shutdown.
//!
//! A quitting GUI arms its UI-owned daemon to stop after the final authenticated
//! client disconnects. Standalone daemons ignore desktop lifecycle handoff.
//!
//! Self-exclusion: the caller disconnects its OWN loopback WS before calling and
//! the daemon simply counts live WS connections — see `local::request_local_shutdown`.
//!
//! Dedicated daemons wake their graceful run loop; the single-binary fallback
//! retains its pid-guarded hard-exit cleanup.

use crate::routes::{AppState, PeerInfo};
use axum::Json;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
use std::time::Duration;

/// Grace before teardown so the HTTP ack is flushed to the client before the
/// connection drops (mirrors the restart route's `EXIT_DELAY`).
const EXIT_DELAY: Duration = Duration::from_millis(300);

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
        match notify {
            Some(notify) => notify.notify_one(),
            None => {
                crate::server::cleanup_on_hard_exit();
                log::info!("Headless daemon exiting for shutdown");
                std::process::exit(0);
            }
        }
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
}
