use crate::remote::auth::TOKEN_TTL_SECS;
use crate::remote::routes::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;

pub async fn post_refresh(
    State(state): State<AppState>,
    req: axum::http::Request<axum::body::Body>,
) -> impl IntoResponse {
    let token = match req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
    {
        Some(header) if header.starts_with("Bearer ") => &header[7..],
        _ => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "missing or invalid authorization header"})),
            )
                .into_response()
        }
    };

    match state.auth_store.refresh_token(token) {
        Ok(new_token) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "token": new_token,
                "expires_in": TOKEN_TTL_SECS,
            })),
        )
            .into_response(),
        Err(msg) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": msg})),
        )
            .into_response(),
    }
}
