use crate::bridge::{BridgeMessage, CommandResult, RemoteCommand};
use crate::routes::AppState;
use crate::types::ActionRequest;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use okena_core::api::ApiTerminalFocusRequest;

fn terminal_focus_request(action: &ActionRequest) -> Option<ApiTerminalFocusRequest> {
    match action {
        ActionRequest::FocusTerminal {
            project_id,
            terminal_id,
            window,
        } => Some(ApiTerminalFocusRequest {
            project_id: project_id.clone(),
            terminal_id: terminal_id.clone(),
            window: window.clone(),
        }),
        _ => None,
    }
}

pub async fn post_actions(
    State(state): State<AppState>,
    Json(action): Json<ActionRequest>,
) -> impl IntoResponse {
    let terminal_focus = terminal_focus_request(&action);
    let command = RemoteCommand::Action(action);

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let msg = BridgeMessage {
        command,
        reply: Some(reply_tx),
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
            if let Some(request) = terminal_focus {
                let _ = state.terminal_focus_tx.send(request);
            }
            let body = payload.unwrap_or(serde_json::json!({"ok": true}));
            (StatusCode::OK, Json(body)).into_response()
        }
        Ok(CommandResult::OkBytes(_) | CommandResult::OkSnapshot { .. }) => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_request_preserves_exact_target() {
        let request = terminal_focus_request(&ActionRequest::FocusTerminal {
            project_id: "project-1".into(),
            terminal_id: "terminal-1".into(),
            window: Some("main".into()),
        })
        .expect("focus action should broadcast");

        assert_eq!(request.project_id, "project-1");
        assert_eq!(request.terminal_id, "terminal-1");
        assert_eq!(request.window.as_deref(), Some("main"));
        assert!(terminal_focus_request(&ActionRequest::RecordProjectActivity {
            project_id: "project-1".into(),
        })
        .is_none());
    }
}
