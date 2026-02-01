use crate::remote::bridge::{BridgeMessage, CommandResult, RemoteCommand};
use crate::remote::routes::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;

pub async fn get_state(State(state): State<AppState>) -> impl IntoResponse {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();

    let msg = BridgeMessage {
        command: RemoteCommand::GetState,
        reply: reply_tx,
    };

    if state.bridge_tx.send(msg).await.is_err() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "bridge unavailable"})),
        )
            .into_response();
    }

    match reply_rx.await {
        Ok(CommandResult::Ok(Some(value))) => {
            (StatusCode::OK, Json(value)).into_response()
        }
        Ok(CommandResult::Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e})),
        )
            .into_response(),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "unexpected response"})),
        )
            .into_response(),
    }
}
