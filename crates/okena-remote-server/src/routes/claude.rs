//! `/v1/claude/...`: hook and channel endpoints for processes in panes, and the
//! session, transcript, prompt and dialog endpoints for remote clients.

use crate::claude_remote::ClaudeRemoteError;
use crate::routes::AppState;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use okena_core::claude_remote::{
    ApiClaudeDecision, ApiClaudePromptRequest, ApiClaudePromptResponse, ClaudeChannelPollResponse,
    ClaudeHookRequest,
};
use serde::Deserialize;

/// A `Write` or `Edit` dialog carries the whole file content, which easily
/// exceeds the global 1 MiB body cap.
pub const HOOK_PERMISSION_BODY_LIMIT: usize = 16 * 1024 * 1024;

impl IntoResponse for ClaudeRemoteError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::InvalidHookInput(reason) => (
                StatusCode::BAD_REQUEST,
                format!("invalid hook input: {reason}"),
            ),
            Self::EmptyPrompt => (StatusCode::BAD_REQUEST, "prompt is empty".to_string()),
            Self::PromptTooLong => (StatusCode::BAD_REQUEST, "prompt is too long".to_string()),
            Self::Busy => (
                StatusCode::CONFLICT,
                "Claude is busy and Okena's hooks are not attached; try again when it finishes"
                    .to_string(),
            ),
            Self::NoClaudeSession => (
                StatusCode::CONFLICT,
                "No Claude session detected in this pane".to_string(),
            ),
            Self::QueueFull => (StatusCode::CONFLICT, "too many queued prompts".to_string()),
            Self::TerminalNotFound(message) => (StatusCode::NOT_FOUND, message),
            Self::DialogNotFound => (
                StatusCode::NOT_FOUND,
                "dialog is no longer open".to_string(),
            ),
            Self::NoTranscript => (
                StatusCode::NOT_FOUND,
                "no readable transcript for this terminal".to_string(),
            ),
            Self::Daemon(message) => (StatusCode::BAD_REQUEST, message),
            Self::BridgeUnavailable => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "bridge unavailable".to_string(),
            ),
            Self::Io(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
        };
        (status, message).into_response()
    }
}

pub async fn post_hook_permission(
    State(state): State<AppState>,
    Json(request): Json<ClaudeHookRequest>,
) -> Result<Json<serde_json::Value>, ClaudeRemoteError> {
    state.claude_hub.permission_request(request).await.map(Json)
}

pub async fn post_hook_event(
    State(state): State<AppState>,
    Json(request): Json<ClaudeHookRequest>,
) -> Result<Json<serde_json::Value>, ClaudeRemoteError> {
    state.claude_hub.hook_event(request)?;
    Ok(Json(serde_json::json!({})))
}

#[derive(Deserialize)]
pub struct ChannelPollQuery {
    terminal_id: String,
}

pub async fn get_channel_poll(
    State(state): State<AppState>,
    Query(query): Query<ChannelPollQuery>,
) -> Json<ClaudeChannelPollResponse> {
    let prompts = state.claude_hub.poll_channel(&query.terminal_id).await;
    Json(ClaudeChannelPollResponse { prompts })
}

#[derive(Deserialize)]
pub struct SessionQuery {
    /// Report `foreground_command` of a pane without a connected `claude`.
    #[serde(default)]
    probe_foreground: bool,
}

pub async fn get_session(
    State(state): State<AppState>,
    Path(terminal_id): Path<String>,
    Query(query): Query<SessionQuery>,
) -> Result<impl IntoResponse, ClaudeRemoteError> {
    state
        .claude_hub
        .session(&terminal_id, query.probe_foreground)
        .await
        .map(Json)
}

#[derive(Deserialize)]
pub struct TranscriptQuery {
    #[serde(default)]
    cursor: Option<u64>,
    #[serde(default)]
    session_id: Option<String>,
    /// Read this subagent's transcript instead of the session's.
    #[serde(default)]
    agent_id: Option<String>,
}

pub async fn get_transcript(
    State(state): State<AppState>,
    Path(terminal_id): Path<String>,
    Query(query): Query<TranscriptQuery>,
) -> Result<impl IntoResponse, ClaudeRemoteError> {
    state
        .claude_hub
        .transcript(&terminal_id, query.cursor, query.session_id, query.agent_id)
        .await
        .map(Json)
}

pub async fn get_subagents(
    State(state): State<AppState>,
    Path(terminal_id): Path<String>,
) -> Result<impl IntoResponse, ClaudeRemoteError> {
    state.claude_hub.subagents(&terminal_id).await.map(Json)
}

pub async fn post_prompt(
    State(state): State<AppState>,
    Path(terminal_id): Path<String>,
    Json(request): Json<ApiClaudePromptRequest>,
) -> Result<Json<ApiClaudePromptResponse>, ClaudeRemoteError> {
    let delivery = state
        .claude_hub
        .send_prompt(&terminal_id, request.text)
        .await?;
    Ok(Json(ApiClaudePromptResponse { delivery }))
}

pub async fn post_dialog_decision(
    State(state): State<AppState>,
    Path((terminal_id, dialog_id)): Path<(String, String)>,
    Json(decision): Json<ApiClaudeDecision>,
) -> Result<Json<serde_json::Value>, ClaudeRemoteError> {
    state
        .claude_hub
        .decide(&terminal_id, &dialog_id, &decision)?;
    Ok(Json(serde_json::json!({})))
}

#[cfg(test)]
mod tests {
    use crate::auth::AuthStore;
    use crate::bridge::CommandResult;
    use crate::pty_broadcaster::PtyBroadcaster;
    use crate::routes::{PeerInfo, build_router};
    use axum::Router;
    use axum::body::Body;
    use axum::http::StatusCode;
    use okena_core::claude_remote::ClaudeTerminalInfo;
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::sync::{Arc, RwLock};
    use std::time::Instant;
    use tower_service::Service as _;

    fn router() -> Router {
        // Dropping the receiver makes daemon calls fail instead of hanging.
        let (bridge_tx, _) = crate::bridge::bridge_channel();
        router_over(bridge_tx)
    }

    /// A router whose daemon reports every terminal with no agent status.
    fn router_with_plain_shell() -> Router {
        let (bridge_tx, bridge_rx) = crate::bridge::bridge_channel();
        tokio::spawn(async move {
            while let Ok(message) = bridge_rx.recv().await {
                let info =
                    serde_json::to_value(ClaudeTerminalInfo::default()).expect("serializable info");
                if let Some(reply) = message.reply {
                    let _ = reply.send(CommandResult::Ok(Some(info)));
                }
            }
        });
        router_over(bridge_tx)
    }

    fn router_over(bridge_tx: crate::bridge::BridgeSender) -> Router {
        let state_version = Arc::new(tokio::sync::watch::channel(0).0);
        let claude_hub =
            crate::claude_remote::ClaudeRemoteHub::new(bridge_tx.clone(), state_version.clone());
        build_router(
            bridge_tx,
            claude_hub,
            Arc::new(AuthStore::with_secret(vec![7u8; 32])),
            Arc::new(PtyBroadcaster::new()),
            state_version,
            Instant::now(),
            Arc::new(tokio::sync::watch::channel(HashMap::new()).0),
            Arc::new(tokio::sync::broadcast::channel(8).0),
            Arc::new(tokio::sync::broadcast::channel(8).0),
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(RwLock::new(HashMap::new())),
            None,
            Arc::new(AtomicU64::new(1)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(tokio::sync::Notify::new()),
            false,
            Arc::new(AtomicBool::new(false)),
            true,
            okena_ext_updater::UpdateInfo::new("0.0.0-test".to_string()),
            crate::usage::UsageHub::detached(),
        )
    }

    async fn status(
        router: &mut Router,
        method: &str,
        uri: &str,
        peer: PeerInfo,
        body: &str,
    ) -> StatusCode {
        let mut request = axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("build request");
        request.extensions_mut().insert(peer);
        router
            .call(request)
            .await
            .expect("the router is infallible")
            .status()
    }

    const LOOPBACK: PeerInfo =
        PeerInfo::Tcp(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 19100));
    const OFF_HOST: PeerInfo = PeerInfo::Tcp(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)),
        19100,
    ));
    const STOP_HOOK: &str =
        r#"{"terminal_id":"t1","input":{"hook_event_name":"Stop","session_id":"s1"}}"#;

    #[tokio::test]
    async fn pane_routes_need_a_same_user_local_caller() {
        let mut router = router();
        let event = "/v1/claude/hook/event";
        assert_eq!(
            status(&mut router, "POST", event, PeerInfo::Local, STOP_HOOK).await,
            StatusCode::OK
        );
        assert_eq!(
            status(&mut router, "POST", event, LOOPBACK, STOP_HOOK).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(&mut router, "POST", event, OFF_HOST, STOP_HOOK).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(
                &mut router,
                "POST",
                "/v1/claude/hook/permission",
                OFF_HOST,
                STOP_HOOK
            )
            .await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(
                &mut router,
                "GET",
                "/v1/claude/channel/poll?terminal_id=t1",
                OFF_HOST,
                ""
            )
            .await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn client_routes_need_authentication() {
        let mut router = router();
        let calls = [
            ("GET", "/v1/claude/terminals/t1"),
            ("GET", "/v1/claude/terminals/t1/transcript?cursor=0"),
            ("GET", "/v1/claude/terminals/t1/subagents"),
            ("POST", "/v1/claude/terminals/t1/prompt"),
            ("POST", "/v1/claude/terminals/t1/dialogs/1"),
        ];
        for (method, uri) in calls {
            assert_eq!(
                status(&mut router, method, uri, LOOPBACK, "{}").await,
                StatusCode::UNAUTHORIZED,
                "{method} {uri}"
            );
        }
    }

    #[tokio::test]
    async fn unknown_dialog_and_blank_prompt_are_rejected() {
        let mut router = router();
        assert_eq!(
            status(
                &mut router,
                "POST",
                "/v1/claude/terminals/t1/dialogs/1",
                PeerInfo::Local,
                r#"{"decision":"allow"}"#
            )
            .await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            status(
                &mut router,
                "POST",
                "/v1/claude/terminals/t1/prompt",
                PeerInfo::Local,
                r#"{"text":"  "}"#
            )
            .await,
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn prompt_for_a_plain_shell_is_a_plain_text_conflict() {
        let mut router = router_with_plain_shell();
        let mut request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/claude/terminals/t1/prompt")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"text":"ls"}"#))
            .expect("build request");
        request.extensions_mut().insert(PeerInfo::Local);

        let response = router
            .call(request)
            .await
            .expect("the router is infallible");

        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("read body");
        assert_eq!(&body[..], b"No Claude session detected in this pane");
    }

    #[tokio::test]
    async fn errors_are_plain_text() {
        let mut router = router();
        let calls = [
            (
                "/v1/claude/terminals/t1/dialogs/k3f9a2-7",
                r#"{"decision":"allow"}"#,
                StatusCode::NOT_FOUND,
                "dialog is no longer open",
            ),
            (
                "/v1/claude/terminals/t1/prompt",
                r#"{"text":"  "}"#,
                StatusCode::BAD_REQUEST,
                "prompt is empty",
            ),
        ];
        for (uri, body, expected_status, expected_body) in calls {
            let mut request = axum::http::Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("build request");
            request.extensions_mut().insert(PeerInfo::Local);

            let response = router
                .call(request)
                .await
                .expect("the router is infallible");

            assert_eq!(response.status(), expected_status, "{uri}");
            assert!(
                response
                    .headers()
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| value.starts_with("text/plain")),
                "{uri}"
            );
            let bytes = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .expect("read body");
            assert_eq!(&bytes[..], expected_body.as_bytes(), "{uri}");
        }
    }

    #[tokio::test]
    async fn permission_hook_accepts_bodies_above_the_global_limit() {
        let mut router = router();
        let content = "x".repeat(2 * 1024 * 1024);
        let body = serde_json::json!({
            "terminal_id": "t1",
            "input": {"hook_event_name": "Stop", "tool_input": {"content": content}}
        })
        .to_string();

        // Parsed (and refused as the wrong event) rather than cut off as too large.
        assert_eq!(
            status(
                &mut router,
                "POST",
                "/v1/claude/hook/permission",
                PeerInfo::Local,
                &body
            )
            .await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status(
                &mut router,
                "POST",
                "/v1/claude/hook/event",
                PeerInfo::Local,
                &body
            )
            .await,
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }
}
