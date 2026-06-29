use crate::routes::{AppState, PeerInfo};
use axum::extract::{Extension, State};
use axum::http::StatusCode;

pub async fn post_reload(
    Extension(peer): Extension<PeerInfo>,
    State(state): State<AppState>,
) -> StatusCode {
    if !peer.is_local_trusted() {
        return StatusCode::FORBIDDEN;
    }

    if state.auth_store.reload_tokens() {
        StatusCode::OK
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[cfg(test)]
mod tests {
    use crate::routes::PeerInfo;
    use std::net::{IpAddr, Ipv6Addr, SocketAddr};

    #[test]
    fn loopback_peers_can_reload_tokens() {
        assert!(PeerInfo::Local.is_local_trusted());
        assert!(PeerInfo::Tcp(SocketAddr::from(([127, 0, 0, 1], 19100))).is_local_trusted());
        assert!(PeerInfo::Tcp(SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 19100))).is_local_trusted());
    }

    #[test]
    fn non_loopback_peers_cannot_reload_tokens() {
        assert!(!PeerInfo::Tcp(SocketAddr::from(([192, 168, 1, 50], 19100))).is_local_trusted());
        assert!(!PeerInfo::Tcp(SocketAddr::from(([10, 0, 0, 2], 19100))).is_local_trusted());
    }

    #[test]
    fn ipv4_mapped_loopback_is_trusted() {
        // `::ffff:127.0.0.1` -- IPv4 loopback surfaced through a dual-stack
        // socket. Must be accepted: the CLI register flow uses this path on
        // some hosts and would otherwise hit FORBIDDEN.
        let mapped = Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x7f00, 0x0001);
        assert!(PeerInfo::Tcp(SocketAddr::new(IpAddr::V6(mapped), 19100)).is_local_trusted());
    }

    #[test]
    fn ipv4_mapped_non_loopback_is_not_trusted() {
        // `::ffff:192.168.1.50` -- mapped form of a private LAN peer. Must
        // stay rejected; the unwrap-then-check at the v4 layer should not
        // be a back door for off-host callers.
        let mapped = Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0xc0a8, 0x0132);
        assert!(!PeerInfo::Tcp(SocketAddr::new(IpAddr::V6(mapped), 19100)).is_local_trusted());
    }
}
