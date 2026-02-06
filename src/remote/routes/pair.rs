use crate::remote::auth::{PairError, TOKEN_TTL_SECS};
use crate::remote::routes::AppState;
use crate::remote::types::{PairRequest, PairResponse};
use axum::Json;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use std::net::SocketAddr;

pub async fn post_pair(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<PairRequest>,
) -> impl IntoResponse {
    match state.auth_store.try_pair(&req.code, addr.ip()) {
        Ok(token) => (
            StatusCode::OK,
            Json(serde_json::to_value(PairResponse {
                token,
                expires_in: TOKEN_TTL_SECS,
            })
            .unwrap()),
        )
            .into_response(),
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
