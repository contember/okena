use crate::auth::{PairError, TOKEN_TTL_SECS};
use crate::routes::{AppState, PeerInfo};
use crate::types::{PairRequest, PairResponse};
use axum::Json;
use axum::extract::{ConnectInfo, Extension, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use std::net::SocketAddr;

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
    Extension(peer): Extension<PeerInfo>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    if !peer.is_local_trusted() {
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
    Extension(peer): Extension<PeerInfo>,
    State(state): State<AppState>,
) -> StatusCode {
    if !peer.is_local_trusted() {
        return StatusCode::FORBIDDEN;
    }

    state.auth_store.invalidate_code();
    StatusCode::NO_CONTENT
}
