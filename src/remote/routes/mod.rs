pub mod actions;
pub mod health;
pub mod pair;
pub mod refresh;
pub mod state;
pub mod stream;
pub mod tokens;

use crate::remote::auth::AuthStore;
use crate::remote::bridge::BridgeSender;
use crate::remote::pty_broadcaster::PtyBroadcaster;
use axum::extract::DefaultBodyLimit;
use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::Response;
use okena_core::api::ApiGitStatus;
use rust_embed::RustEmbed;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

#[derive(RustEmbed)]
#[folder = "web/dist"]
struct WebAssets;

/// Shared state available to all route handlers.
#[derive(Clone)]
pub struct AppState {
    pub bridge_tx: BridgeSender,
    pub auth_store: Arc<AuthStore>,
    pub broadcaster: Arc<PtyBroadcaster>,
    pub state_version: Arc<tokio::sync::watch::Sender<u64>>,
    pub start_time: Instant,
    pub git_status: Arc<tokio::sync::watch::Sender<HashMap<String, ApiGitStatus>>>,
}

/// Build the complete axum router.
pub fn build_router(
    bridge_tx: BridgeSender,
    auth_store: Arc<AuthStore>,
    broadcaster: Arc<PtyBroadcaster>,
    state_version: Arc<tokio::sync::watch::Sender<u64>>,
    start_time: Instant,
    git_status: Arc<tokio::sync::watch::Sender<HashMap<String, ApiGitStatus>>>,
) -> Router {
    let state = AppState {
        bridge_tx,
        auth_store,
        broadcaster,
        state_version,
        start_time,
        git_status,
    };

    // Routes that require auth
    let protected = Router::new()
        .route("/v1/state", axum::routing::get(state::get_state))
        .route("/v1/actions", axum::routing::post(actions::post_actions))
        .route("/v1/stream", axum::routing::get(stream::ws_handler))
        .route("/v1/refresh", axum::routing::post(refresh::post_refresh))
        .route("/v1/tokens", axum::routing::get(tokens::list_tokens))
        .route("/v1/tokens/{id}", axum::routing::delete(tokens::revoke_token))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    // Public routes (no auth required)
    let public = Router::new()
        .route("/health", axum::routing::get(health::get_health))
        .route("/v1/pair", axum::routing::post(pair::post_pair));

    public
        .merge(protected)
        .layer(DefaultBodyLimit::max(1024 * 1024)) // 1 MB
        .fallback(serve_web_asset)
        .with_state(state)
}

/// Serve embedded web client assets (SPA with index.html fallback for client-side routing).
async fn serve_web_asset(uri: axum::http::Uri) -> axum::response::Response {
    use axum::response::IntoResponse;

    let path = uri.path().trim_start_matches('/');
    let file = if path.is_empty() { "index.html" } else { path };

    match WebAssets::get(file) {
        Some(content) => serve_embedded_file(file, content),
        None => {
            // SPA fallback: serve index.html for unmatched routes
            match WebAssets::get("index.html") {
                Some(content) => serve_embedded_file("index.html", content),
                None => (StatusCode::NOT_FOUND, "web client not available").into_response(),
            }
        }
    }
}

fn serve_embedded_file(path: &str, file: rust_embed::EmbeddedFile) -> axum::response::Response {
    use axum::response::IntoResponse;

    let mime = match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("mjs") => "application/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    };

    ([(axum::http::header::CONTENT_TYPE, mime)], file.data).into_response()
}

/// Auth middleware: validates Bearer token on protected routes.
/// Skips validation for WebSocket upgrade requests (WS has its own auth flow).
async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Allow WebSocket upgrades through — they handle auth via query param or first message
    let is_websocket = req
        .headers()
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);
    if is_websocket {
        return Ok(next.run(req).await);
    }

    let auth_header = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    let token = match auth_header {
        Some(header) if header.starts_with("Bearer ") => &header[7..],
        _ => return Err(StatusCode::UNAUTHORIZED),
    };

    if !state.auth_store.validate_token(token) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(req).await)
}
