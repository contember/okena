//! `POST /v1/restart` — loopback-only daemon self-restart.
//!
//! Restarting the daemon ends every PTY (the daemon owns them all), so this is a
//! deliberate, user-confirmed action surfaced by the desktop GUI ("pick up a
//! freshly-built daemon binary"). It is loopback-gated exactly like
//! `/v1/auth/reload`: a same-host client triggers it; off-host callers are
//! refused.
//!
//! Mechanism (a self-restart, so it works whether the GUI spawned the daemon or
//! merely attached to it):
//!
//! 1. Spawn a *replacement* daemon process via
//!    [`spawn_replacement_daemon`](crate::local::spawn_replacement_daemon). It
//!    re-launches the current executable with the same args plus
//!    `--await-pid <this_pid>`, so it waits for THIS process to exit before it
//!    binds a port / acquires the instance lock.
//! 2. Ack the HTTP request immediately (so the client gets a clean response and
//!    can begin polling for the new daemon).
//! 3. Schedule `std::process::exit` on a short timer — *after* the response has
//!    been flushed — which drops this daemon's socket, releases its port, and
//!    lets the lock file go stale (a dead PID), so the replacement takes over.
//!
//! The replacement's port scan picks the first free port in 19100–19200, which
//! may differ from the outgoing daemon's (the old one can linger in TIME_WAIT).
//! The client re-reads `remote.json` after restart to pick up the new port — see
//! the GUI's restart handler.

use crate::routes::AppState;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

/// Grace before the outgoing daemon exits, so the HTTP ack is fully flushed to
/// the client before its connection drops.
const EXIT_DELAY: Duration = Duration::from_millis(300);

pub async fn post_restart(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(_state): State<AppState>,
) -> StatusCode {
    if !is_trusted_restart_peer(addr) {
        return StatusCode::FORBIDDEN;
    }

    // Spawn the replacement BEFORE acking: if we can't even launch it, fail the
    // request and stay alive rather than exit into a daemon-less state.
    match crate::local::spawn_replacement_daemon() {
        Ok(_child) => {
            log::info!("Daemon restart requested; spawned replacement, exiting shortly");
        }
        Err(e) => {
            log::error!("Daemon restart failed to spawn replacement: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    }

    // Exit after the response is flushed. A detached task on the server runtime
    // sleeps briefly, then hard-exits: `std::process::exit` skips destructors,
    // so the lock file + remote.json are left with this (now-dead) PID, which the
    // replacement treats as stale and takes over.
    tokio::spawn(async move {
        tokio::time::sleep(EXIT_DELAY).await;
        log::info!("Daemon exiting for restart");
        std::process::exit(0);
    });

    StatusCode::OK
}

/// Loopback-only gate, identical in shape to `auth_reload::is_trusted_reload_peer`.
fn is_trusted_restart_peer(addr: SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(v4) => v4.is_loopback(),
        // Dual-stack binds can surface an IPv4 loopback peer as the mapped form
        // `::ffff:127.0.0.1`. `Ipv6Addr::is_loopback` only matches `::1`, so
        // unwrap the mapping first and re-check at the v4 layer.
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.is_loopback(),
            None => v6.is_loopback(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv6Addr;

    #[test]
    fn loopback_peers_can_restart() {
        assert!(is_trusted_restart_peer(SocketAddr::from(([127, 0, 0, 1], 19100))));
        assert!(is_trusted_restart_peer(SocketAddr::from((
            [0, 0, 0, 0, 0, 0, 0, 1],
            19100
        ))));
    }

    #[test]
    fn non_loopback_peers_cannot_restart() {
        assert!(!is_trusted_restart_peer(SocketAddr::from(([192, 168, 1, 50], 19100))));
        assert!(!is_trusted_restart_peer(SocketAddr::from(([10, 0, 0, 2], 19100))));
    }

    #[test]
    fn ipv4_mapped_loopback_is_trusted() {
        let mapped = Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x7f00, 0x0001);
        assert!(is_trusted_restart_peer(SocketAddr::new(IpAddr::V6(mapped), 19100)));
    }

    #[test]
    fn ipv4_mapped_non_loopback_is_not_trusted() {
        let mapped = Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0xc0a8, 0x0132);
        assert!(!is_trusted_restart_peer(SocketAddr::new(IpAddr::V6(mapped), 19100)));
    }
}
