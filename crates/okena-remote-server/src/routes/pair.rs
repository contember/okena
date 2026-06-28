use crate::auth::{PairError, TOKEN_TTL_SECS};
use crate::routes::AppState;
use crate::types::{PairRequest, PairResponse};
use axum::Json;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use std::net::{IpAddr, SocketAddr};

pub async fn post_pair(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<PairRequest>,
) -> impl IntoResponse {
    match state.auth_store.try_pair(&req.code, addr.ip()) {
        Ok(token) => {
            #[allow(
                clippy::unwrap_used,
                reason = "PairResponse is an internal type — serialization is infallible"
            )]
            let body = serde_json::to_value(PairResponse {
                token,
                expires_in: TOKEN_TTL_SECS,
            })
            .unwrap();
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(PairError::RateLimited) => {
            // 300ms delay after rate-limited attempt
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({"error": "rate limited"})),
            )
                .into_response()
        }
        Err(PairError::InvalidCode) => {
            // 300ms delay after failed attempt
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "invalid or expired code"})),
            )
                .into_response()
        }
    }
}

pub async fn post_pair_code(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    if !is_trusted_pair_code_peer(addr) {
        return StatusCode::FORBIDDEN.into_response();
    }

    let code = state.auth_store.generate_fresh_code();
    Json(serde_json::json!({
        "code": code,
        "expires_in": state.auth_store.code_remaining_secs(),
    }))
    .into_response()
}

pub async fn delete_pair_code(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> StatusCode {
    if !is_trusted_pair_code_peer(addr) {
        return StatusCode::FORBIDDEN;
    }

    state.auth_store.invalidate_code();
    StatusCode::NO_CONTENT
}

fn is_trusted_pair_code_peer(addr: SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(v4) => v4.is_loopback(),
        // Dual-stack binds can surface an IPv4 loopback peer as the mapped
        // form `::ffff:127.0.0.1`.
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.is_loopback(),
            None => v6.is_loopback(),
        },
    }
}
