//! `GET /v1/usage` and `POST /v1/usage/refresh` — agent usage (Claude Code /
//! Codex / Grok rate limits, and the AI proxy's account pool) measured on the
//! daemon's machine.

use crate::routes::AppState;
use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use okena_core::api::{ApiUsage, ApiUsageProvider, UsageRefreshRequest};

/// Current usage snapshot.
pub async fn get_usage(State(state): State<AppState>) -> Json<ApiUsage> {
    Json(state.usage.snapshot())
}

/// Ask the daemon to refetch one provider (`{"provider": "claude"}`) or all
/// enabled ones (empty body / `{}`), then return the snapshot once the poller
/// has applied the request: a provider whose `fetching` is `true` there is
/// being refetched, otherwise the request was throttled. The fetch result
/// arrives as a `usage_changed` WS message (or on the next `GET /v1/usage`).
/// The daemon ignores the request for a provider fetched less than 60 s ago,
/// currently fetching, rate limited (`rate_limited_until`), deep in an error
/// backoff, or disabled.
pub async fn post_refresh(State(state): State<AppState>, body: Bytes) -> Response {
    let request = if body.iter().all(u8::is_ascii_whitespace) {
        UsageRefreshRequest::default()
    } else {
        match serde_json::from_slice::<UsageRefreshRequest>(&body) {
            Ok(request) => request,
            Err(error) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": format!("invalid request: {error}") })),
                )
                    .into_response();
            }
        }
    };
    if request.provider == Some(ApiUsageProvider::Unknown) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "unknown provider" })),
        )
            .into_response();
    }
    Json(state.usage.refresh(request.provider).await).into_response()
}
