use crate::bridge::{BridgeMessage, BridgeSender, CommandResult, RemoteCommand};
use crate::routes::AppState;
use crate::types::ActionRequest;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use okena_core::api::{ApiFullscreenRequest, ApiProjectVisibilityRequest, ApiTerminalFocusRequest};
use okena_core::ws::ClientPresentationRequest;
use tokio::sync::broadcast;

/// The one-shot presentation request connected desktops must apply after
/// `action` succeeds. These actions change state each desktop window owns, so
/// the daemon's own copy alone would leave the desktop unchanged.
fn presentation_request(action: &ActionRequest) -> Option<ClientPresentationRequest> {
    match action {
        ActionRequest::FocusTerminal {
            project_id,
            terminal_id,
            window,
        } => Some(ClientPresentationRequest::FocusTerminal(
            ApiTerminalFocusRequest {
                project_id: project_id.clone(),
                terminal_id: terminal_id.clone(),
                window: window.clone(),
            },
        )),
        ActionRequest::SetProjectShowInOverview {
            project_id,
            show,
            window,
        } => Some(ClientPresentationRequest::ProjectVisibility(
            ApiProjectVisibilityRequest {
                project_id: project_id.clone(),
                show: *show,
                window: window.clone(),
            },
        )),
        ActionRequest::SetFullscreen {
            project_id,
            terminal_id,
            window,
        } => Some(ClientPresentationRequest::Fullscreen(
            ApiFullscreenRequest {
                project_id: project_id.clone(),
                terminal_id: terminal_id.clone(),
                window: window.clone(),
            },
        )),
        _ => None,
    }
}

enum ActionDispatchError {
    BridgeUnavailable,
    ProcessingFailed,
}

/// Run `action` on the daemon and, only when it succeeds, push its
/// presentation request to every connected client.
async fn run_action(
    bridge_tx: &BridgeSender,
    presentation_tx: &broadcast::Sender<ClientPresentationRequest>,
    action: ActionRequest,
) -> Result<CommandResult, ActionDispatchError> {
    let presentation = presentation_request(&action);
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let msg = BridgeMessage {
        command: RemoteCommand::Action(action),
        reply: Some(reply_tx),
    };
    if bridge_tx.send(msg).await.is_err() {
        return Err(ActionDispatchError::BridgeUnavailable);
    }
    let result = reply_rx
        .await
        .map_err(|_| ActionDispatchError::ProcessingFailed)?;
    if let (CommandResult::Ok(_), Some(request)) = (&result, presentation) {
        // No connected client is not an error: the request is fire-and-forget.
        let _ = presentation_tx.send(request);
    }
    Ok(result)
}

pub async fn post_actions(
    State(state): State<AppState>,
    Json(action): Json<ActionRequest>,
) -> impl IntoResponse {
    match run_action(&state.bridge_tx, &state.presentation_tx, action).await {
        Ok(CommandResult::Ok(payload)) => {
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
        Err(ActionDispatchError::BridgeUnavailable) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "bridge unavailable"})),
        )
            .into_response(),
        Err(ActionDispatchError::ProcessingFailed) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "command processing failed"})),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hide_main(project_id: &str) -> ActionRequest {
        ActionRequest::SetProjectShowInOverview {
            project_id: project_id.into(),
            show: false,
            window: Some("main".into()),
        }
    }

    /// Run `action` against a daemon stub that answers every command with
    /// `reply`, and return what connected clients would receive.
    async fn presentation_after(
        action: ActionRequest,
        reply: CommandResult,
    ) -> Option<ClientPresentationRequest> {
        let (bridge_tx, bridge_rx) = crate::bridge::bridge_channel();
        let (presentation_tx, mut presentation_rx) = broadcast::channel(8);
        let daemon = tokio::spawn(async move {
            let msg = bridge_rx.recv().await.expect("action reaches the daemon");
            let _ = msg.reply.expect("actions expect a reply").send(reply);
        });

        let _ = run_action(&bridge_tx, &presentation_tx, action).await;
        daemon.await.expect("daemon stub finishes");
        presentation_rx.try_recv().ok()
    }

    #[test]
    fn presentation_requests_preserve_exact_targets() {
        assert_eq!(
            presentation_request(&ActionRequest::FocusTerminal {
                project_id: "project-1".into(),
                terminal_id: "terminal-1".into(),
                window: Some("main".into()),
            }),
            Some(ClientPresentationRequest::FocusTerminal(
                ApiTerminalFocusRequest {
                    project_id: "project-1".into(),
                    terminal_id: "terminal-1".into(),
                    window: Some("main".into()),
                }
            ))
        );
        assert_eq!(
            presentation_request(&hide_main("project-1")),
            Some(ClientPresentationRequest::ProjectVisibility(
                ApiProjectVisibilityRequest {
                    project_id: "project-1".into(),
                    show: false,
                    window: Some("main".into()),
                }
            ))
        );
        assert_eq!(
            presentation_request(&ActionRequest::SetFullscreen {
                project_id: "project-1".into(),
                terminal_id: None,
                window: None,
            }),
            Some(ClientPresentationRequest::Fullscreen(
                ApiFullscreenRequest {
                    project_id: "project-1".into(),
                    terminal_id: None,
                    window: None,
                }
            ))
        );
        assert_eq!(
            presentation_request(&ActionRequest::RecordProjectActivity {
                project_id: "project-1".into(),
            }),
            None
        );
    }

    #[tokio::test]
    async fn successful_show_hide_is_pushed_to_clients() {
        assert_eq!(
            presentation_after(hide_main("project-1"), CommandResult::Ok(None)).await,
            presentation_request(&hide_main("project-1"))
        );
    }

    #[tokio::test]
    async fn failed_show_hide_is_not_pushed_to_clients() {
        assert_eq!(
            presentation_after(
                hide_main("missing"),
                CommandResult::Err("project not found: missing".into())
            )
            .await,
            None
        );
    }
}
