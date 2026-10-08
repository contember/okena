use crate::bridge::{BridgeMessage, CommandResult, RemoteCommand};
use crate::routes::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use okena_core::mission::MissionContextRequest;

pub async fn post_context(
    State(state): State<AppState>,
    Json(request): Json<MissionContextRequest>,
) -> impl IntoResponse {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let message = BridgeMessage {
        command: RemoteCommand::GetMissionContext(request),
        reply: Some(reply_tx),
    };
    if state.bridge_tx.send(message).await.is_err() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "bridge unavailable"})),
        )
            .into_response();
    }
    match reply_rx.await {
        Ok(CommandResult::Ok(Some(context))) => (StatusCode::OK, Json(context)).into_response(),
        Ok(CommandResult::Err(error)) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": error})),
        )
            .into_response(),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "context query failed"})),
        )
            .into_response(),
    }
}
