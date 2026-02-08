use crate::remote::routes::AppState;
use axum::Json;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use std::net::SocketAddr;

/// GET /v1/local/pair-code — localhost-only, unauthenticated.
/// Returns a fresh pairing code for use by `okena pair` CLI.
pub async fn get_local_pair_code(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    // Defense-in-depth: server already binds to 127.0.0.1, but reject non-loopback anyway
    if !addr.ip().is_loopback() {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": "localhost only"})),
        )
            .into_response();
    }

    let code = state.auth_store.get_or_create_code();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "code": code,
            "expires_in": 60,
        })),
    )
        .into_response()
}
