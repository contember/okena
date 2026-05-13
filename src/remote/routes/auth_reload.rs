use crate::remote::routes::AppState;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use std::net::SocketAddr;

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
    addr.ip().is_loopback()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
