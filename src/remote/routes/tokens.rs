use crate::remote::routes::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;

pub async fn list_tokens(State(state): State<AppState>) -> impl IntoResponse {
    let tokens = state.auth_store.list_tokens();
    Json(serde_json::json!({ "tokens": tokens }))
}

pub async fn revoke_token(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if state.auth_store.revoke_token(&id) {
        (StatusCode::OK, Json(serde_json::json!({ "revoked": true }))).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "token not found" })),
        )
            .into_response()
    }
}
