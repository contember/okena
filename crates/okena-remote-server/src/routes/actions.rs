use crate::bridge::{BridgeMessage, BridgeSender, CommandResult, RemoteCommand};
use crate::routes::AppState;
use crate::types::ActionRequest;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use okena_core::api::{
    ApiFullscreenRequest, ApiProjectLayoutRequest, ApiProjectPlacementRequest,
    ApiProjectVisibilityRequest, ApiTerminalFocusRequest, ApiWindow,
};
use okena_core::ws::ClientPresentationRequest;
use okena_workspace::state::WindowId;
use std::collections::HashMap;
use std::sync::RwLock;
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
        ActionRequest::SetProjectLayout { mode, window } => Some(
            ClientPresentationRequest::ProjectLayout(ApiProjectLayoutRequest {
                mode: *mode,
                window: window.clone(),
            }),
        ),
        _ => None,
    }
}

/// The window an action's newly created project must land in. Built from the
/// daemon's reply, because the daemon assigns the project id.
fn placement_window(action: &ActionRequest) -> Option<String> {
    match action {
        ActionRequest::CreateWorktree { window, .. } => window.clone(),
        _ => None,
    }
}

fn placement_request(
    window: String,
    payload: Option<&serde_json::Value>,
) -> Option<ClientPresentationRequest> {
    let project_id = payload?.get("project_id")?.as_str()?;
    Some(ClientPresentationRequest::ProjectPlacement(
        ApiProjectPlacementRequest {
            project_id: project_id.to_string(),
            window,
        },
    ))
}

/// Why `action` cannot run: it targets an extra window no connected desktop
/// declared, so no client would apply it. `main` always exists (the daemon
/// serves it); a malformed id is left to the daemon to reject.
fn undeclared_window_error(
    action: &ActionRequest,
    declared: &HashMap<u64, Vec<ApiWindow>>,
) -> Option<String> {
    let raw = action
        .target_window()
        .map(str::to_string)
        .or_else(|| placement_window(action))?;
    let target @ WindowId::Extra(_) = WindowId::from_wire(&raw)? else {
        return None;
    };
    let mut windows = declared.values().flatten().peekable();
    if windows.peek().is_none() {
        return Some(format!(
            "window not found: {raw} (no desktop is attached to apply it)"
        ));
    }
    if windows.any(|window| WindowId::from_wire(&window.id) == Some(target)) {
        return None;
    }
    Some(format!(
        "window not found: {raw} (no attached desktop has this window)"
    ))
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
    declared_windows: &RwLock<HashMap<u64, Vec<ApiWindow>>>,
    action: ActionRequest,
) -> Result<CommandResult, ActionDispatchError> {
    // Best-effort: the window can close or its desktop detach while the action runs.
    let undeclared = match declared_windows.read() {
        Ok(declared) => undeclared_window_error(&action, &declared),
        Err(_) => return Err(ActionDispatchError::ProcessingFailed),
    };
    if let Some(error) = undeclared {
        return Ok(CommandResult::Err(error));
    }
    let presentation = presentation_request(&action);
    let placement = placement_window(&action);
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
    if let CommandResult::Ok(payload) = &result {
        let placement = placement.and_then(|window| placement_request(window, payload.as_ref()));
        // No connected client is not an error: the request is fire-and-forget.
        for request in presentation.into_iter().chain(placement) {
            let _ = presentation_tx.send(request);
        }
    }
    Ok(result)
}

pub async fn post_actions(
    State(state): State<AppState>,
    Json(action): Json<ActionRequest>,
) -> impl IntoResponse {
    match run_action(
        &state.bridge_tx,
        &state.presentation_tx,
        &state.remote_windows,
        action,
    )
    .await
    {
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

    const EXTRA: &str = "6f1c3a52-9d4e-4b8a-a0c1-2e7f5b9d3c11";

    fn window(id: &str) -> ApiWindow {
        ApiWindow {
            id: id.into(),
            kind: if id == "main" { "main" } else { "extra" }.into(),
            active: false,
            focused_project_id: None,
            focused_terminal_id: None,
            fullscreen: None,
            visible_project_ids: Vec::new(),
            folder_filter: None,
            bounds: None,
            sidebar_open: None,
        }
    }

    fn hide_in(window: &str) -> ActionRequest {
        ActionRequest::SetProjectShowInOverview {
            project_id: "project-1".into(),
            show: false,
            window: Some(window.into()),
        }
    }

    /// Run `action` against a daemon stub that answers every command with
    /// `reply`, and return what connected clients would receive.
    async fn presentation_after(
        action: ActionRequest,
        reply: CommandResult,
    ) -> Option<ClientPresentationRequest> {
        let declared = RwLock::new(HashMap::from([(1, vec![window("main"), window(EXTRA)])]));
        presentation_with_windows(action, reply, &declared).await.1
    }

    /// Like [`presentation_after`] with the given declared desktop windows;
    /// also returns the action's result and whether it reached the daemon.
    async fn presentation_with_windows(
        action: ActionRequest,
        reply: CommandResult,
        declared: &RwLock<HashMap<u64, Vec<ApiWindow>>>,
    ) -> (CommandResult, Option<ClientPresentationRequest>, bool) {
        let (bridge_tx, bridge_rx) = crate::bridge::bridge_channel();
        let (presentation_tx, mut presentation_rx) = broadcast::channel(8);
        let daemon = tokio::spawn(async move {
            let Ok(msg) = bridge_rx.recv().await else {
                return false;
            };
            let _ = msg.reply.expect("actions expect a reply").send(reply);
            true
        });

        let result = match run_action(&bridge_tx, &presentation_tx, declared, action).await {
            Ok(result) => result,
            Err(_) => panic!("bridge stub answers"),
        };
        drop(bridge_tx);
        let reached_daemon = daemon.await.expect("daemon stub finishes");
        (result, presentation_rx.try_recv().ok(), reached_daemon)
    }

    #[tokio::test]
    async fn show_hide_in_a_declared_extra_window_is_pushed_to_clients() {
        assert_eq!(
            presentation_after(hide_in(EXTRA), CommandResult::Ok(None)).await,
            presentation_request(&hide_in(EXTRA))
        );
    }

    #[tokio::test]
    async fn undeclared_extra_window_is_rejected_before_the_daemon() {
        let other_desktop = RwLock::new(HashMap::from([(1, vec![window("main")])]));
        let no_desktop = RwLock::new(HashMap::new());
        for (declared, reason) in [
            (&other_desktop, "no attached desktop has this window"),
            (&no_desktop, "no desktop is attached"),
        ] {
            for action in [
                hide_in(EXTRA),
                ActionRequest::SetFullscreen {
                    project_id: "project-1".into(),
                    terminal_id: Some("terminal-1".into()),
                    window: Some(EXTRA.into()),
                },
                create_worktree(Some(EXTRA)),
            ] {
                let (result, pushed, reached_daemon) =
                    presentation_with_windows(action, CommandResult::Ok(None), declared).await;
                assert!(
                    matches!(&result, CommandResult::Err(e)
                        if e.starts_with(&format!("window not found: {EXTRA}")) && e.contains(reason)),
                    "unexpected result: {result:?}"
                );
                assert!(!reached_daemon, "rejected action must not run");
                assert_eq!(pushed, None);
            }
        }
    }

    #[tokio::test]
    async fn extra_window_declared_by_another_connection_is_pushed() {
        let declared = RwLock::new(HashMap::from([
            (1, vec![window("main")]),
            (2, vec![window("main"), window(EXTRA)]),
        ]));
        let (result, pushed, reached_daemon) =
            presentation_with_windows(hide_in(EXTRA), CommandResult::Ok(None), &declared).await;
        assert!(matches!(result, CommandResult::Ok(None)));
        assert!(reached_daemon);
        assert_eq!(pushed, presentation_request(&hide_in(EXTRA)));
    }

    #[tokio::test]
    async fn focus_and_layout_in_an_undeclared_extra_window_are_rejected() {
        let declared = RwLock::new(HashMap::from([(1, vec![window("main")])]));
        for action in [
            ActionRequest::FocusTerminal {
                project_id: "project-1".into(),
                terminal_id: "terminal-1".into(),
                window: Some(EXTRA.into()),
            },
            ActionRequest::SetProjectLayout {
                mode: okena_core::types::ProjectLayoutMode::Grid,
                window: Some(EXTRA.into()),
            },
        ] {
            let (result, pushed, reached_daemon) =
                presentation_with_windows(action, CommandResult::Ok(None), &declared).await;
            assert!(
                matches!(&result, CommandResult::Err(e)
                    if e.starts_with(&format!("window not found: {EXTRA}"))),
                "unexpected result: {result:?}"
            );
            assert!(!reached_daemon, "rejected action must not run");
            assert_eq!(pushed, None);
        }
    }

    #[tokio::test]
    async fn malformed_window_id_is_left_to_the_daemon() {
        let no_desktop = RwLock::new(HashMap::new());
        for action in [
            hide_in("not-a-window"),
            create_worktree(Some("not-a-window")),
        ] {
            let daemon_error = CommandResult::Err("invalid window id: not-a-window".into());
            let (result, pushed, reached_daemon) =
                presentation_with_windows(action, daemon_error, &no_desktop).await;
            assert!(reached_daemon, "the gate must not swallow a malformed id");
            assert!(matches!(&result, CommandResult::Err(e)
                if e == "invalid window id: not-a-window"));
            assert_eq!(pushed, None);
        }
    }

    #[tokio::test]
    async fn main_window_runs_without_a_desktop() {
        let no_desktop = RwLock::new(HashMap::new());
        let (result, pushed, reached_daemon) =
            presentation_with_windows(hide_in("main"), CommandResult::Ok(None), &no_desktop).await;
        assert!(matches!(result, CommandResult::Ok(None)));
        assert!(reached_daemon);
        assert_eq!(pushed, presentation_request(&hide_in("main")));
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
            presentation_request(&ActionRequest::SetProjectLayout {
                mode: okena_core::types::ProjectLayoutMode::Grid,
                window: Some("main".into()),
            }),
            Some(ClientPresentationRequest::ProjectLayout(
                ApiProjectLayoutRequest {
                    mode: okena_core::types::ProjectLayoutMode::Grid,
                    window: Some("main".into()),
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

    fn create_worktree(window: Option<&str>) -> ActionRequest {
        ActionRequest::CreateWorktree {
            project_id: "parent".into(),
            branch: "feature".into(),
            create_branch: true,
            window: window.map(str::to_string),
            layout: None,
            terminal_name: None,
            wait: false,
        }
    }

    #[tokio::test]
    async fn worktree_with_window_pushes_placement_of_the_created_project() {
        let reply = CommandResult::Ok(Some(serde_json::json!({
            "project_id": "wt-1",
            "path": "/repo/.worktrees/feature",
            "pending": true,
        })));
        assert_eq!(
            presentation_after(create_worktree(Some("main")), reply).await,
            Some(ClientPresentationRequest::ProjectPlacement(
                ApiProjectPlacementRequest {
                    project_id: "wt-1".into(),
                    window: "main".into(),
                }
            ))
        );
    }

    #[tokio::test]
    async fn worktree_without_window_or_failed_pushes_nothing() {
        let reply = || CommandResult::Ok(Some(serde_json::json!({"project_id": "wt-1"})));
        assert_eq!(
            presentation_after(create_worktree(None), reply()).await,
            None
        );
        assert_eq!(
            presentation_after(
                create_worktree(Some("main")),
                CommandResult::Err("project not found: parent".into())
            )
            .await,
            None
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
