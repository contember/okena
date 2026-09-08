pub mod actions;
pub mod auth_reload;
pub mod download;
pub mod health;
pub mod pair;
pub mod paste_image;
pub mod refresh;
pub mod restart;
pub mod shutdown;
pub mod state;
pub mod stream;
pub mod tokens;
pub mod update;

use crate::auth::AuthStore;
use crate::bridge::BridgeSender;
use crate::pty_broadcaster::PtyBroadcaster;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::Response;
use okena_core::api::{ApiGitStatus, ApiTerminalFocusRequest, ApiToast};
use okena_core::git_poll::GitPollTrigger;
use rust_embed::RustEmbed;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, RwLock};
use std::time::Instant;

// The web client is built into `<repo>/web/dist`. This crate's manifest dir is
// `<repo>/crates/okena-remote-server`, so reach the repo root via `../../`.
#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../web/dist"]
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
    /// Broadcast of daemon-originated toasts. Each WS connection subscribes a
    /// receiver and forwards [`WsOutbound::Toast`] frames; events sent with no
    /// receivers are simply dropped (fire-and-forget, like git status).
    pub toast_tx: Arc<tokio::sync::broadcast::Sender<ApiToast>>,
    /// One-shot exact-terminal focus requests produced by successful external
    /// actions and consumed by connected desktop clients.
    pub terminal_focus_tx: Arc<tokio::sync::broadcast::Sender<ApiTerminalFocusRequest>>,
    /// Per-connection set of subscribed terminal IDs (connection_id → terminal_ids).
    /// Puts the owning projects on the git poller's responsive tier, but only for
    /// connections with no entry in `remote_visible_projects`.
    pub remote_subscribed_terminals: Arc<RwLock<HashMap<u64, HashSet<String>>>>,
    /// Per-connection set of project IDs each client currently renders
    /// (connection_id → project_ids), declared via [`WsInbound::SetVisibleProjects`].
    /// The `gh` PR/CI fan-out unions this with the server's own visible set, and
    /// the git poller trusts it over that connection's subscriptions — see
    /// [`WsInbound::SetVisibleProjects`] for why it can't be derived here.
    pub remote_visible_projects: Arc<RwLock<HashMap<u64, HashSet<String>>>>,
    /// Optional wake-up path for the host git poller when a WS client starts
    /// viewing terminals.
    pub git_poll_trigger_tx: Option<tokio::sync::mpsc::UnboundedSender<GitPollTrigger>>,
    pub next_connection_id: Arc<AtomicU64>,
    /// Count of currently-live authenticated WS connections. The stream route
    /// increments it on accept and decrements it on close; `/v1/shutdown` uses
    /// it for UI-owned lifecycle handoff. (`remote_subscribed_terminals`
    /// only tracks connections that have subscribed to a terminal, so it can't
    /// stand in for a live-connection registry.)
    pub active_connections: Arc<AtomicU64>,
    /// UI-owned daemons shut down once the last connected client leaves after
    /// any desktop has requested lifecycle handoff. Standalone daemons ignore
    /// desktop quit requests.
    pub ui_owned: bool,
    pub shutdown_when_idle: Arc<AtomicBool>,
    /// Set true once at least one authenticated client has connected. Gates the
    /// idle-exit monitor (see [`shutdown::run_idle_exit_monitor`]) so a freshly
    /// spawned UI-owned daemon isn't reaped before its GUI makes first contact.
    pub had_client: Arc<AtomicBool>,
    /// Graceful process-shutdown trigger for `/v1/shutdown`. The shared daemon
    /// run loop awaits it and tears down the socket, discovery file, and
    /// instance lock through normal drops.
    pub process_shutdown: Arc<tokio::sync::Notify>,
    /// Whether the daemon actually bound a same-user local endpoint. Management
    /// routes can only demand more than loopback where one exists — see
    /// [`management_middleware`].
    pub local_bootstrap: bool,
    pub update_info: okena_ext_updater::UpdateInfo,
}

#[derive(Clone, Copy, Debug)]
pub enum PeerInfo {
    Tcp(SocketAddr),
    Local,
}

impl PeerInfo {
    pub fn is_local_trusted(self) -> bool {
        match self {
            Self::Local => true,
            Self::Tcp(addr) => match addr.ip() {
                IpAddr::V4(v4) => v4.is_loopback(),
                // Dual-stack binds can surface an IPv4 loopback peer as the
                // mapped form `::ffff:127.0.0.1`.
                IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                    Some(v4) => v4.is_loopback(),
                    None => v6.is_loopback(),
                },
            },
        }
    }
}

fn request_is_unix_socket(req: &Request) -> bool {
    matches!(req.extensions().get::<PeerInfo>(), Some(PeerInfo::Local))
}

/// Build the complete axum router.
// Each param is a distinct piece of shared router state.
#[allow(clippy::too_many_arguments)]
pub fn build_router(
    bridge_tx: BridgeSender,
    auth_store: Arc<AuthStore>,
    broadcaster: Arc<PtyBroadcaster>,
    state_version: Arc<tokio::sync::watch::Sender<u64>>,
    start_time: Instant,
    git_status: Arc<tokio::sync::watch::Sender<HashMap<String, ApiGitStatus>>>,
    toast_tx: Arc<tokio::sync::broadcast::Sender<ApiToast>>,
    terminal_focus_tx: Arc<tokio::sync::broadcast::Sender<ApiTerminalFocusRequest>>,
    remote_subscribed_terminals: Arc<RwLock<HashMap<u64, HashSet<String>>>>,
    remote_visible_projects: Arc<RwLock<HashMap<u64, HashSet<String>>>>,
    git_poll_trigger_tx: Option<tokio::sync::mpsc::UnboundedSender<GitPollTrigger>>,
    next_connection_id: Arc<AtomicU64>,
    active_connections: Arc<AtomicU64>,
    process_shutdown: Arc<tokio::sync::Notify>,
    ui_owned: bool,
    had_client: Arc<AtomicBool>,
    local_bootstrap: bool,
    update_info: okena_ext_updater::UpdateInfo,
) -> Router {
    let state = AppState {
        bridge_tx,
        auth_store,
        broadcaster,
        state_version,
        start_time,
        git_status,
        toast_tx,
        terminal_focus_tx,
        remote_subscribed_terminals,
        remote_visible_projects,
        git_poll_trigger_tx,
        next_connection_id,
        active_connections,
        process_shutdown,
        ui_owned,
        shutdown_when_idle: Arc::new(AtomicBool::new(false)),
        had_client,
        local_bootstrap,
        update_info,
    };

    // Routes that require auth
    let protected = Router::new()
        .route("/v1/state", axum::routing::get(state::get_state))
        .route("/v1/actions", axum::routing::post(actions::post_actions))
        .route(
            "/v1/files/download",
            axum::routing::post(download::post_download),
        )
        .route(
            "/v1/terminals/{terminal_id}/paste-image",
            axum::routing::post(paste_image::post_paste_image)
                .layer(DefaultBodyLimit::max(paste_image::IMAGE_UPLOAD_LIMIT)),
        )
        .route(
            "/v1/terminals/{terminal_id}/paste-file",
            axum::routing::post(paste_image::post_paste_file)
                .layer(DefaultBodyLimit::max(paste_image::FILE_UPLOAD_LIMIT)),
        )
        .route("/v1/refresh", axum::routing::post(refresh::post_refresh))
        .route("/v1/tokens", axum::routing::get(tokens::list_tokens))
        .route(
            "/v1/tokens/{id}",
            axum::routing::delete(tokens::revoke_token),
        )
        .route(
            "/v1/pair-code",
            axum::routing::post(pair::post_pair_code).delete(pair::delete_pair_code),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    // The WebSocket route runs its own handshake auth (query token, first
    // message, or the local socket), so it must stay off the bearer middleware:
    // browsers cannot set an Authorization header on an upgrade request.
    let stream = Router::new().route("/v1/stream", axum::routing::get(stream::ws_handler));

    // Daemon lifecycle and updater routes. Loopback is not an identity — it
    // does not distinguish OS users — so these need the same-user local socket
    // or a bearer token on top of it, wherever such a socket exists.
    // `/v1/auth/reload` is why the local socket is the bootstrap transport: the
    // CLI register flow calls it before the new token is visible in-memory, so
    // requiring a bearer there would deadlock.
    let management = Router::new()
        .route(
            "/v1/auth/reload",
            axum::routing::post(auth_reload::post_reload),
        )
        .route("/v1/restart", axum::routing::post(restart::post_restart))
        .route("/v1/shutdown", axum::routing::post(shutdown::post_shutdown))
        .route("/v1/update/status", axum::routing::get(update::get_status))
        .route(
            "/v1/update/releases",
            axum::routing::get(update::get_releases),
        )
        .route("/v1/update/check", axum::routing::post(update::post_check))
        .route(
            "/v1/update/revert",
            axum::routing::post(update::post_revert),
        )
        .route(
            "/v1/update/install",
            axum::routing::post(update::post_install),
        )
        .route(
            "/v1/update/dismiss",
            axum::routing::post(update::post_dismiss),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            management_middleware,
        ));

    // Public routes: liveness, and the pairing handshake that mints the very
    // first token for an off-host client.
    let public = Router::new()
        .route("/health", axum::routing::get(health::get_health))
        .route("/v1/pair", axum::routing::post(pair::post_pair));

    public
        .merge(protected)
        .merge(stream)
        .merge(management)
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

fn bearer_token(req: &Request) -> Option<&str> {
    req.headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|header| header.strip_prefix("Bearer "))
}

/// Whether the caller has proven it may act as this user: it either arrived on
/// the same-user local socket or presented a valid bearer token.
fn caller_is_authorized(state: &AppState, req: &Request) -> bool {
    request_is_unix_socket(req)
        || bearer_token(req).is_some_and(|token| state.auth_store.validate_token(token))
}

/// Auth middleware: validates the Bearer token on protected routes.
/// Unix socket traffic is already same-user scoped by the local transport.
async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if !caller_is_authorized(&state, &req) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(req).await)
}

/// Management middleware: daemon lifecycle and updater routes stay same-host,
/// and additionally require the local socket or a bearer token — loopback on
/// its own says nothing about which OS user is calling.
async fn management_middleware(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let local_host = req
        .extensions()
        .get::<PeerInfo>()
        .copied()
        .is_some_and(PeerInfo::is_local_trusted);
    if !local_host {
        return Err(StatusCode::FORBIDDEN);
    }

    // Conditional because the desktop's own quit and update calls have no other
    // way to prove identity where this daemon bound no local endpoint (Windows,
    // or a runtime dir we could not make private); refusing them there would
    // strand unsaved workspace state, so loopback stays the only gate.
    if state.local_bootstrap && !caller_is_authorized(&state, &req) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower_service::Service as _;

    #[test]
    fn unix_socket_requests_skip_bearer_auth() {
        let mut req = Request::new(axum::body::Body::empty());
        req.extensions_mut().insert(PeerInfo::Local);

        assert!(request_is_unix_socket(&req));
    }

    #[test]
    fn tcp_loopback_requests_still_require_bearer_auth() {
        let mut req = Request::new(axum::body::Body::empty());
        req.extensions_mut()
            .insert(PeerInfo::Tcp(SocketAddr::from(([127, 0, 0, 1], 19100))));

        assert!(!request_is_unix_socket(&req));
    }

    const LOOPBACK: PeerInfo = PeerInfo::Tcp(SocketAddr::new(
        IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        19100,
    ));
    const OFF_HOST: PeerInfo = PeerInfo::Tcp(SocketAddr::new(
        IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 50)),
        19100,
    ));

    fn test_router(auth_store: Arc<AuthStore>) -> Router {
        router_with_bootstrap(auth_store, true)
    }

    fn router_with_bootstrap(auth_store: Arc<AuthStore>, local_bootstrap: bool) -> Router {
        // Dropping the bridge receiver makes authorized calls fail at the
        // bridge instead of hanging on a reply that never comes.
        let (bridge_tx, _) = crate::bridge::bridge_channel();
        build_router(
            bridge_tx,
            auth_store,
            Arc::new(PtyBroadcaster::new()),
            Arc::new(tokio::sync::watch::channel(0).0),
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
            local_bootstrap,
            okena_ext_updater::UpdateInfo::new("0.0.0-test".to_string()),
        )
    }

    fn paired_token(store: &AuthStore) -> String {
        let code = store.generate_fresh_code();
        store
            .try_pair(&code, IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
            .expect("pairing succeeds")
    }

    struct Call {
        method: &'static str,
        uri: &'static str,
        peer: PeerInfo,
        upgrade: bool,
        token: Option<String>,
    }

    impl Call {
        fn get(uri: &'static str) -> Self {
            Self {
                method: "GET",
                uri,
                peer: LOOPBACK,
                upgrade: false,
                token: None,
            }
        }

        fn post(uri: &'static str) -> Self {
            Self {
                method: "POST",
                uri,
                peer: LOOPBACK,
                upgrade: false,
                token: None,
            }
        }

        fn from_peer(mut self, peer: PeerInfo) -> Self {
            self.peer = peer;
            self
        }

        fn upgrading(mut self) -> Self {
            self.upgrade = true;
            self
        }

        fn with_token(mut self, token: &str) -> Self {
            self.token = Some(token.to_string());
            self
        }

        async fn status(self, router: &mut Router) -> StatusCode {
            let mut builder = axum::http::Request::builder()
                .method(self.method)
                .uri(self.uri)
                .header("content-type", "application/json");
            if self.upgrade {
                builder = builder
                    .header("upgrade", "websocket")
                    .header("connection", "Upgrade");
            }
            if let Some(token) = &self.token {
                builder = builder.header("authorization", format!("Bearer {token}"));
            }
            let mut req = builder.body(Body::from("{}")).expect("build request");
            req.extensions_mut().insert(self.peer);
            router
                .call(req)
                .await
                .expect("the router is infallible")
                .status()
        }
    }

    #[tokio::test]
    async fn an_upgrade_header_does_not_bypass_rest_authentication() {
        let store = Arc::new(AuthStore::with_secret(vec![7u8; 32]));
        let mut router = test_router(store);

        assert_eq!(
            Call::get("/v1/state").upgrading().status(&mut router).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            Call::post("/v1/actions")
                .upgrading()
                .status(&mut router)
                .await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            Call::post("/v1/files/download")
                .upgrading()
                .status(&mut router)
                .await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn rest_routes_still_reject_a_missing_bearer() {
        let store = Arc::new(AuthStore::with_secret(vec![7u8; 32]));
        let mut router = test_router(store);

        assert_eq!(
            Call::get("/v1/state").status(&mut router).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn rest_routes_accept_a_valid_bearer() {
        let store = Arc::new(AuthStore::with_secret(vec![7u8; 32]));
        let token = paired_token(&store);
        let mut router = test_router(store);

        assert_ne!(
            Call::get("/v1/state")
                .with_token(&token)
                .status(&mut router)
                .await,
            StatusCode::UNAUTHORIZED
        );
    }

    /// The WebSocket route authenticates inside its own handshake, so the
    /// bearer middleware must not stand in front of it: a plain GET reaches the
    /// upgrade extractor and is rejected there, not by auth or routing.
    #[tokio::test]
    async fn the_stream_route_reaches_its_own_upgrade_handshake() {
        let store = Arc::new(AuthStore::with_secret(vec![7u8; 32]));
        let mut router = test_router(store);

        assert_eq!(
            Call::get("/v1/stream").status(&mut router).await,
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn tcp_loopback_alone_cannot_reach_management_routes() {
        let store = Arc::new(AuthStore::with_secret(vec![7u8; 32]));
        let mut router = test_router(store);

        // `/v1/restart` and `/v1/shutdown` are deliberately absent: they are
        // safe here only while the middleware rejects first, and a regression
        // would spawn or stop a real daemon from inside the suite.
        for call in [
            Call::post("/v1/auth/reload"),
            Call::get("/v1/update/status"),
            Call::post("/v1/update/check"),
            Call::post("/v1/update/install"),
            Call::post("/v1/update/dismiss"),
        ] {
            let uri = call.uri;
            assert_eq!(
                call.status(&mut router).await,
                StatusCode::UNAUTHORIZED,
                "{uri} must not trust loopback on its own"
            );
        }
    }

    #[tokio::test]
    async fn management_routes_accept_a_bearer_or_the_local_socket() {
        let store = Arc::new(AuthStore::with_secret(vec![7u8; 32]));
        let token = paired_token(&store);
        let mut router = test_router(store);

        assert_eq!(
            Call::get("/v1/update/status")
                .with_token(&token)
                .status(&mut router)
                .await,
            StatusCode::OK
        );
        assert_eq!(
            Call::get("/v1/update/status")
                .from_peer(PeerInfo::Local)
                .status(&mut router)
                .await,
            StatusCode::OK
        );
    }

    /// A daemon with no same-user local endpoint (Windows, or a runtime dir it
    /// could not make private) has no bootstrap transport, so management stays
    /// on the pre-existing loopback-only gate rather than locking the desktop
    /// out of its own quit and update calls.
    #[tokio::test]
    async fn management_falls_back_to_loopback_without_a_local_endpoint() {
        let store = Arc::new(AuthStore::with_secret(vec![7u8; 32]));
        let mut router = router_with_bootstrap(store, false);

        assert_eq!(
            Call::get("/v1/update/status").status(&mut router).await,
            StatusCode::OK
        );
        assert_eq!(
            Call::get("/v1/update/status")
                .from_peer(OFF_HOST)
                .status(&mut router)
                .await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn management_routes_stay_closed_to_off_host_callers() {
        let store = Arc::new(AuthStore::with_secret(vec![7u8; 32]));
        let token = paired_token(&store);
        let mut router = test_router(store);

        assert_eq!(
            Call::get("/v1/update/status")
                .from_peer(OFF_HOST)
                .with_token(&token)
                .status(&mut router)
                .await,
            StatusCode::FORBIDDEN
        );
    }

    type TestFrame = tokio_tungstenite::tungstenite::Message;
    type TestFrameError = tokio_tungstenite::tungstenite::Error;

    async fn spawn_stream_server(auth_store: Arc<AuthStore>) -> SocketAddr {
        let router = test_router(auth_store);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a loopback listener");
        let addr = listener.local_addr().expect("listener address");
        tokio::spawn(async move {
            let _ = crate::serve::serve_plain(listener, router, std::future::pending::<()>()).await;
        });
        addr
    }

    /// The `type` of the next text frame, or `None` if the daemon closed first.
    async fn next_frame_type<S>(socket: &mut S) -> Option<String>
    where
        S: futures::Stream<Item = Result<TestFrame, TestFrameError>> + Unpin,
    {
        use futures::StreamExt as _;

        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(5), socket.next()).await {
                Ok(Some(Ok(TestFrame::Text(text)))) => {
                    let frame: serde_json::Value = serde_json::from_str(&text).ok()?;
                    return frame
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                }
                Ok(Some(Ok(_))) => continue,
                _ => return None,
            }
        }
    }

    async fn send_frame<S>(socket: &mut S, frame: serde_json::Value)
    where
        S: futures::Sink<TestFrame, Error = TestFrameError> + Unpin,
    {
        use futures::SinkExt as _;

        socket
            .send(TestFrame::Text(frame.to_string()))
            .await
            .expect("the daemon accepts the frame");
    }

    async fn connect_stream(addr: SocketAddr) -> impl futures::Stream<Item = Result<TestFrame, TestFrameError>>
    + futures::Sink<TestFrame, Error = TestFrameError>
    + Unpin {
        let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/stream"))
            .await
            .expect("the daemon accepts the upgrade");
        socket
    }

    /// `/v1/stream` sits outside the bearer middleware, so its own handshake is
    /// the only thing standing between a dialer and terminal I/O.
    #[tokio::test]
    async fn the_stream_refuses_a_client_that_presents_no_token() {
        let store = Arc::new(AuthStore::with_secret(vec![7u8; 32]));
        let addr = spawn_stream_server(store).await;
        let mut socket = connect_stream(addr).await;

        send_frame(
            &mut socket,
            serde_json::json!({ "type": "subscribe", "terminal_ids": [] }),
        )
        .await;

        assert_eq!(next_frame_type(&mut socket).await.as_deref(), Some("auth_failed"));
    }

    #[tokio::test]
    async fn the_stream_refuses_an_unknown_token() {
        let store = Arc::new(AuthStore::with_secret(vec![7u8; 32]));
        let addr = spawn_stream_server(store).await;
        let mut socket = connect_stream(addr).await;

        send_frame(
            &mut socket,
            serde_json::json!({ "type": "auth", "token": "not-a-real-token" }),
        )
        .await;

        assert_eq!(next_frame_type(&mut socket).await.as_deref(), Some("auth_failed"));
    }

    #[tokio::test]
    async fn the_stream_accepts_a_valid_token() {
        let store = Arc::new(AuthStore::with_secret(vec![7u8; 32]));
        let token = paired_token(&store);
        let addr = spawn_stream_server(store).await;
        let mut socket = connect_stream(addr).await;

        send_frame(
            &mut socket,
            serde_json::json!({ "type": "auth", "token": token }),
        )
        .await;

        assert_eq!(next_frame_type(&mut socket).await.as_deref(), Some("auth_ok"));
    }

    /// Revocation must end the established stream, not merely refuse the next
    /// handshake — the connection already has terminal read/write.
    #[tokio::test]
    async fn the_stream_ends_when_its_token_is_revoked() {
        let store = Arc::new(AuthStore::with_secret(vec![7u8; 32]));
        let token = paired_token(&store);
        let token_id = store
            .list_tokens()
            .first()
            .map(|info| info.id.clone())
            .expect("the paired token is listed");
        let addr = spawn_stream_server(store.clone()).await;
        let mut socket = connect_stream(addr).await;

        send_frame(
            &mut socket,
            serde_json::json!({ "type": "auth", "token": token }),
        )
        .await;
        assert_eq!(next_frame_type(&mut socket).await.as_deref(), Some("auth_ok"));

        assert!(store.revoke_token(&token_id));

        assert_eq!(next_frame_type(&mut socket).await.as_deref(), Some("auth_failed"));
        assert!(
            next_frame_type(&mut socket).await.is_none(),
            "the daemon must close the revoked stream"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_local_socket_peer_streams_without_a_token() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("daemon.sock");
        let store = Arc::new(AuthStore::with_secret(vec![7u8; 32]));
        let router = test_router(store);
        let listener = crate::serve::bind_unix_socket(&path).expect("bind the local socket");
        tokio::spawn(crate::serve::serve_unix_listener(
            path.clone(),
            listener,
            router,
            std::future::pending::<()>(),
        ));

        let stream = tokio::net::UnixStream::connect(&path)
            .await
            .expect("connect to the local socket");
        let (mut socket, _) = tokio_tungstenite::client_async("ws://okena.local/v1/stream", stream)
            .await
            .expect("the daemon accepts the upgrade");

        assert_eq!(next_frame_type(&mut socket).await.as_deref(), Some("auth_ok"));
    }

    #[tokio::test]
    async fn health_and_pairing_stay_public() {
        let store = Arc::new(AuthStore::with_secret(vec![7u8; 32]));
        let mut router = test_router(store);

        assert_eq!(
            Call::get("/health").status(&mut router).await,
            StatusCode::OK
        );
        assert_ne!(
            Call::post("/v1/pair").status(&mut router).await,
            StatusCode::UNAUTHORIZED
        );
    }
}
