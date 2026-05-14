use crate::remote::routes::AppState;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use std::net::{IpAddr, SocketAddr};

pub async fn post_reload(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> StatusCode {
    if !is_trusted_reload_peer(addr) {
        return StatusCode::FORBIDDEN;
    }

    if state.auth_store.reload_tokens() {
        StatusCode::OK
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

fn is_trusted_reload_peer(addr: SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(v4) => v4.is_loopback(),
        // Dual-stack binds can surface an IPv4 loopback peer as the mapped
        // form `::ffff:127.0.0.1`. `Ipv6Addr::is_loopback` only matches `::1`,
        // so unwrap the mapping first and re-check at the v4 layer.
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
    fn loopback_peers_can_reload_tokens() {
        assert!(is_trusted_reload_peer(SocketAddr::from(([127, 0, 0, 1], 19100))));
        assert!(is_trusted_reload_peer(SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 19100))));
    }

    #[test]
    fn non_loopback_peers_cannot_reload_tokens() {
        assert!(!is_trusted_reload_peer(SocketAddr::from(([192, 168, 1, 50], 19100))));
        assert!(!is_trusted_reload_peer(SocketAddr::from(([10, 0, 0, 2], 19100))));
    }

    #[test]
    fn ipv4_mapped_loopback_is_trusted() {
        // `::ffff:127.0.0.1` -- IPv4 loopback surfaced through a dual-stack
        // socket. Must be accepted: the CLI register flow uses this path on
        // some hosts and would otherwise hit FORBIDDEN.
        let mapped = Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x7f00, 0x0001);
        assert!(is_trusted_reload_peer(SocketAddr::new(IpAddr::V6(mapped), 19100)));
    }

    #[test]
    fn ipv4_mapped_non_loopback_is_not_trusted() {
        // `::ffff:192.168.1.50` -- mapped form of a private LAN peer. Must
        // stay rejected; the unwrap-then-check at the v4 layer should not
        // be a back door for off-host callers.
        let mapped = Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0xc0a8, 0x0132);
        assert!(!is_trusted_reload_peer(SocketAddr::new(IpAddr::V6(mapped), 19100)));
    }
}
