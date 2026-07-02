//! `POST /v1/shutdown` — loopback-only, client-aware daemon shutdown.
//!
//! Replaces the desktop's old SIGKILL-at-quit. The quitting GUI asks the daemon
//! to stop, but the daemon REFUSES while any *other* client is still connected —
//! e.g. a second GUI that attached during a fast restart, or a mobile/web WS.
//! This is the fix for the restart race where the old GUI's quit killed the
//! daemon the new GUI had just attached to. Loopback-gated exactly like
//! `/v1/restart`; refusal is a normal 200 response, not an HTTP error.
//!
//! Self-exclusion: the caller disconnects its OWN loopback WS before calling and
//! the daemon simply counts live WS connections — see `local::request_local_shutdown`.
//!
//! On accept the daemon exits cleanly (unlike restart's hard `process::exit`,
//! since there is no successor to hand the port/lock to): waking the daemon's
//! `run()` loop drops the `RemoteServer` (unlinks the socket + removes
//! remote.json) and releases the instance lock on drop.

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

/// Pure accept/refuse decision: shut down only when no client is connected.
///
/// The caller disconnects its own loopback WS first (see `request_local_shutdown`),
/// so a live count of zero means "only the quitting UI was here".
pub fn should_shut_down(active_clients: u64) -> bool {
    active_clients == 0
}

pub async fn post_shutdown(
    Extension(peer): Extension<PeerInfo>,
    State(state): State<AppState>,
) -> Response {
    if !peer.is_local_trusted() {
        return StatusCode::FORBIDDEN.into_response();
    }

    let active_clients = state.active_connections.load(Ordering::Relaxed);
    if !should_shut_down(active_clients) {
        log::info!("Shutdown refused: {active_clients} other client(s) connected");
        return Json(ShutdownResponse {
            shutting_down: false,
            active_clients,
        })
        .into_response();
    }

    // Accepted. Tear down AFTER the response has been flushed (like restart).
    match state.process_shutdown.clone() {
        Some(notify) => {
            tokio::spawn(async move {
                tokio::time::sleep(EXIT_DELAY).await;
                log::info!("Daemon shutting down (no other clients connected)");
                // Wakes the daemon's `run()` loop → drops the RemoteServer
                // (socket unlink + pid-guarded remote.json removal) and releases
                // the instance lock on drop. A clean teardown, no SIGKILL.
                notify.notify_one();
            });
        }
        None => {
            // Transitional `okena --headless` fallback: no graceful run-loop to
            // signal, so hard-exit like the restart route. Because `process::exit`
            // skips Drop, clean up our own files first (pid-guarded): remove
            // remote.json, unlink the local socket, and drop the instance lock —
            // otherwise the leftover socket/lock race the next daemon start.
            tokio::spawn(async move {
                tokio::time::sleep(EXIT_DELAY).await;
                crate::server::cleanup_on_hard_exit();
                log::info!("Headless daemon exiting for shutdown");
                std::process::exit(0);
            });
        }
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
    fn shuts_down_only_when_no_clients_remain() {
        assert!(should_shut_down(0), "no other clients → accept");
        assert!(!should_shut_down(1), "one other client → refuse");
        assert!(!should_shut_down(5), "many other clients → refuse");
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
