pub mod actions;
pub mod health;
pub mod pair;
pub mod refresh;
pub mod state;
pub mod stream;

use crate::remote::auth::AuthStore;
use crate::remote::bridge::BridgeSender;
use crate::remote::pty_broadcaster::PtyBroadcaster;
use axum::extract::DefaultBodyLimit;
use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::Response;
use std::sync::Arc;
use std::time::Instant;
use tower_http::services::{ServeDir, ServeFile};

/// Shared state available to all route handlers.
#[derive(Clone)]
pub struct AppState {
    pub bridge_tx: BridgeSender,
    pub auth_store: Arc<AuthStore>,
    pub broadcaster: Arc<PtyBroadcaster>,
    pub state_version: Arc<tokio::sync::watch::Sender<u64>>,
    pub start_time: Instant,
}

/// Build the complete axum router.
pub fn build_router(
    bridge_tx: BridgeSender,
    auth_store: Arc<AuthStore>,
    broadcaster: Arc<PtyBroadcaster>,
    state_version: Arc<tokio::sync::watch::Sender<u64>>,
    start_time: Instant,
) -> Router {
    let state = AppState {
        bridge_tx,
        auth_store,
        broadcaster,
        state_version,
        start_time,
    };

    // Routes that require auth
    let protected = Router::new()
        .route("/v1/state", axum::routing::get(state::get_state))
        .route("/v1/actions", axum::routing::post(actions::post_actions))
        .route("/v1/stream", axum::routing::get(stream::ws_handler))
        .route("/v1/refresh", axum::routing::post(refresh::post_refresh))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    // Public routes (no auth required)
    let public = Router::new()
        .route("/health", axum::routing::get(health::get_health))
        .route("/v1/pair", axum::routing::post(pair::post_pair));

    // Serve web client as fallback (SPA with index.html fallback for client-side routing)
    let web_dir = web_client_dir();
    let serve_web = ServeDir::new(&web_dir)
        .append_index_html_on_directories(true)
        .not_found_service(ServeFile::new(web_dir.join("index.html")));

    public
        .merge(protected)
        .layer(DefaultBodyLimit::max(1024 * 1024)) // 1 MB
        .fallback_service(serve_web)
        .with_state(state)
}

/// Find the web client directory. Checks `./web/dist` (dev) then next to executable (production).
fn web_client_dir() -> std::path::PathBuf {
    // Dev: project root web/dist
    let dev_dir = std::path::PathBuf::from("web/dist");
    if dev_dir.is_dir() {
        return dev_dir;
    }

    // Production: next to the executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let prod_dir = parent.join("web");
            if prod_dir.is_dir() {
                return prod_dir;
            }
        }
    }

    // Fallback to dev path (ServeDir will just 404 if it doesn't exist)
    dev_dir
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
