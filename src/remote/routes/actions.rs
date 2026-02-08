use crate::remote::bridge::{BridgeMessage, CommandResult, RemoteCommand};
use crate::remote::routes::AppState;
use crate::remote::types::ActionRequest;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;

pub async fn post_actions(
    State(state): State<AppState>,
    Json(action): Json<ActionRequest>,
) -> impl IntoResponse {
    let command = match action {
        ActionRequest::SendText { terminal_id, text } => {
            RemoteCommand::SendText { terminal_id, text }
        }
        ActionRequest::RunCommand { terminal_id, command } => {
            RemoteCommand::RunCommand { terminal_id, command }
        }
        ActionRequest::SendSpecialKey { terminal_id, key } => {
            RemoteCommand::SendSpecialKey { terminal_id, key }
        }
        ActionRequest::SplitTerminal {
            project_id,
            path,
            direction,
        } => RemoteCommand::SplitTerminal {
            project_id,
            path,
            direction,
        },
        ActionRequest::CloseTerminal {
            project_id,
            terminal_id,
        } => RemoteCommand::CloseTerminal {
            project_id,
            terminal_id,
        },
        ActionRequest::FocusTerminal {
            project_id,
            terminal_id,
        } => RemoteCommand::FocusTerminal {
            project_id,
            terminal_id,
        },
        ActionRequest::ReadContent { terminal_id } => {
            RemoteCommand::ReadContent { terminal_id }
        }
        ActionRequest::Resize { terminal_id, cols, rows } => {
            RemoteCommand::Resize { terminal_id, cols, rows }
        }
        ActionRequest::CreateTerminal { project_id } => {
            RemoteCommand::CreateTerminal { project_id }
        }
    };

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let msg = BridgeMessage {
        command,
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
        Ok(CommandResult::Ok(payload)) => {
            let body = payload.unwrap_or(serde_json::json!({"ok": true}));
            (StatusCode::OK, Json(body)).into_response()
        }
        Ok(CommandResult::OkBytes(_)) => {
            (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response()
        }
        Ok(CommandResult::Err(e)) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e})),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "command processing failed"})),
        )
            .into_response(),
    }
}
