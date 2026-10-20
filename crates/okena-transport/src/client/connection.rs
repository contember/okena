use crate::client::config::{LOCAL_DAEMON_CONNECTION_ID, LocalEndpoint, RemoteConnectionConfig};
use crate::client::id::make_prefixed_id;
use crate::client::state::{
    collect_all_terminal_ids, collect_state_terminal_ids, collect_terminal_sizes, diff_states,
};
use crate::client::types::{
    ConnectionEvent, ConnectionStatus, SessionError, TOKEN_REFRESH_AGE_SECS, WsClientMessage,
};
use okena_core::api::{ActionRequest, ApiSystemStats, StateResponse};
use okena_core::ws::ClientPresentationRequest;

use futures::{Sink, Stream};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite;

type TcpWsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
#[cfg(unix)]
type UnixWsStream = tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>;

enum AnyWsStream {
    Tcp(Box<TcpWsStream>),
    #[cfg(unix)]
    Unix(Box<UnixWsStream>),
}

impl Stream for AnyWsStream {
    type Item = Result<tungstenite::Message, tungstenite::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match &mut *self {
            AnyWsStream::Tcp(stream) => Pin::new(stream.as_mut()).poll_next(cx),
            #[cfg(unix)]
            AnyWsStream::Unix(stream) => Pin::new(stream.as_mut()).poll_next(cx),
        }
    }
}

impl Sink<tungstenite::Message> for AnyWsStream {
    type Error = tungstenite::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match &mut *self {
            AnyWsStream::Tcp(stream) => Pin::new(stream.as_mut()).poll_ready(cx),
            #[cfg(unix)]
            AnyWsStream::Unix(stream) => Pin::new(stream.as_mut()).poll_ready(cx),
        }
    }

    fn start_send(mut self: Pin<&mut Self>, item: tungstenite::Message) -> Result<(), Self::Error> {
        match &mut *self {
            AnyWsStream::Tcp(stream) => Pin::new(stream.as_mut()).start_send(item),
            #[cfg(unix)]
            AnyWsStream::Unix(stream) => Pin::new(stream.as_mut()).start_send(item),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match &mut *self {
            AnyWsStream::Tcp(stream) => Pin::new(stream.as_mut()).poll_flush(cx),
            #[cfg(unix)]
            AnyWsStream::Unix(stream) => Pin::new(stream.as_mut()).poll_flush(cx),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match &mut *self {
            AnyWsStream::Tcp(stream) => Pin::new(stream.as_mut()).poll_close(cx),
            #[cfg(unix)]
            AnyWsStream::Unix(stream) => Pin::new(stream.as_mut()).poll_close(cx),
        }
    }
}

#[cfg(unix)]
fn unix_http_client(path: &str) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .unix_socket(path)
        .build()
        .map_err(|error| format!("Cannot initialise Unix socket HTTP client: {error}"))
}

#[cfg(not(unix))]
fn unix_http_client(_path: &str) -> Result<reqwest::Client, String> {
    Err("Unix socket HTTP transport is not supported on this platform".to_string())
}

fn local_unix_path(config: &RemoteConnectionConfig) -> Option<&str> {
    #[cfg(unix)]
    {
        match &config.local_endpoint {
            Some(LocalEndpoint::UnixSocket { path }) => Some(path.as_str()),
            _ => None,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = config;
        None
    }
}

fn initial_connect_attempts(config: &RemoteConnectionConfig) -> u32 {
    if config.id == LOCAL_DAEMON_CONNECTION_ID {
        // Fail fast: ensure_local_daemon() verified reachability right before
        // this dial, and the app-layer self-heal re-runs it once Error fires.
        5
    } else if config.local_endpoint.is_some() {
        30
    } else {
        1
    }
}

fn initial_connect_retry_delay(attempt: u32) -> std::time::Duration {
    let millis = match attempt {
        1 => 100,
        2 => 200,
        3 => 400,
        4 => 800,
        _ => 1_200,
    };
    std::time::Duration::from_millis(millis)
}

/// Whether this task must settle the server's certificate identity before it
/// sends the bearer token: a TLS handshake reached the server and the config
/// carries no pin, so the verifier accepted whatever certificate answered.
fn needs_certificate_adoption(config: &RemoteConnectionConfig, detected_tls: bool) -> bool {
    detected_tls && local_unix_path(config).is_none() && config.pinned_cert_sha256.is_none()
}

/// Pin the certificate this task's TLS handshake observed, so its HTTP client,
/// WS connector and reconnects enforce it instead of re-running TOFU. No-op
/// without TLS: the slot may hold a probe that was abandoned for plain http.
fn adopt_observed_pin(
    config: &mut RemoteConnectionConfig,
    observed: &crate::client::tls::ObservedFingerprint,
) -> Option<String> {
    if !config.tls {
        return None;
    }
    let fingerprint = observed.lock().ok().and_then(|slot| slot.clone())?;
    config.pinned_cert_sha256 = Some(fingerprint.clone());
    Some(fingerprint)
}

/// A config whose TLS identity is settled. [`RemoteClient::run_ws_loop`] takes
/// nothing else, so no entry path can start a session — nor the reconnects that
/// reuse this one config — still willing to trust any certificate.
struct SessionConfig(RemoteConnectionConfig);

impl SessionConfig {
    /// The only constructor: pins whatever the handshake that got us here
    /// observed. Also returns that fingerprint, so a caller reporting it to the
    /// manager reports exactly what the session enforces.
    fn adopt(
        mut config: RemoteConnectionConfig,
        observed: &crate::client::tls::ObservedFingerprint,
    ) -> (Self, Option<String>) {
        let fingerprint = adopt_observed_pin(&mut config, observed);
        (Self(config), fingerprint)
    }

    fn get(&self) -> &RemoteConnectionConfig {
        &self.0
    }

    fn into_inner(self) -> RemoteConnectionConfig {
        self.0
    }
}

/// Parse the payload of a one-shot presentation push. The `WsOutbound`
/// variants are internally tagged, so the payload fields sit next to `type`.
fn parse_presentation_request<T: serde::de::DeserializeOwned>(
    value: &serde_json::Value,
    wrap: fn(T) -> ClientPresentationRequest,
) -> Option<ClientPresentationRequest> {
    match serde_json::from_value::<T>(value.clone()) {
        Ok(payload) => Some(wrap(payload)),
        Err(e) => {
            log::warn!("Failed to parse presentation request: {}", e);
            None
        }
    }
}

fn ws_message_channel() -> (
    async_channel::Sender<WsClientMessage>,
    async_channel::Receiver<WsClientMessage>,
) {
    async_channel::unbounded()
}

async fn fetch_remote_settings(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
) -> Result<serde_json::Value, String> {
    crate::remote_action::post_action_async_with_client(
        client,
        base_url,
        token,
        ActionRequest::GetSettings,
    )
    .await
    .map_err(|error| format!("Failed to fetch settings: {error}"))?
    .ok_or_else(|| "Settings fetch returned no payload".to_string())
}

/// Reconnect budget after an established WS drops. The local daemon connection
/// dead-ends within a few seconds so the app-layer self-heal (re-running
/// ensure_local_daemon, which can respawn the daemon) takes over quickly;
/// user-managed remotes keep the patient schedule for flaky networks.
fn ws_reconnect_max_attempts(config: &RemoteConnectionConfig) -> u32 {
    if config.id == LOCAL_DAEMON_CONNECTION_ID {
        3
    } else {
        8
    }
}

/// Longest sleep between two reconnect attempts to a remote. Short enough that
/// a server coming back is picked up within seconds, so a user staring at a
/// "Reconnecting" banner never waits on a long backoff.
const WS_RECONNECT_BACKOFF_CAP_SECS: u64 = 10;

/// Sleep before reconnect `attempt` (1-based). Local daemon: flat 1s (see
/// [`ws_reconnect_max_attempts`]); remotes: exponential 1,2,4,8 then capped at
/// [`WS_RECONNECT_BACKOFF_CAP_SECS`] — about a minute of backoff in total
/// before the connection settles in Error.
fn ws_reconnect_backoff_secs(config: &RemoteConnectionConfig, attempt: u32) -> u64 {
    if config.id == LOCAL_DAEMON_CONNECTION_ID {
        1
    } else {
        std::cmp::min(
            2u64.saturating_pow(attempt.saturating_sub(1)),
            WS_RECONNECT_BACKOFF_CAP_SECS,
        )
    }
}

/// Bound on dialing the WebSocket: TCP connect (all resolved addresses), TLS
/// handshake and HTTP upgrade together. Without it a SYN that is silently
/// dropped (Android Doze, a dead Wi-Fi after wake) hangs for the kernel's full
/// SYN-retry schedule, about two minutes.
const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How often an established session sends `{"type":"ping"}`. The server answers
/// with `pong` (supported since 2026-02; older daemons answer `error`, which is
/// inbound traffic too), so a live connection never goes quiet.
const WS_PING_INTERVAL: Duration = Duration::from_secs(15);
/// No inbound message for this long → probe the connection.
const WS_IDLE_TIMEOUT: Duration = Duration::from_secs(45);
/// How long a probe ping may stay unanswered before the session is declared
/// dead and reconnected.
const WS_PROBE_GRACE: Duration = Duration::from_secs(5);
/// A foreground probe only pings when nothing arrived for this long; a session
/// that just delivered output is evidently alive.
const WS_PROBE_MIN_QUIET: Duration = Duration::from_secs(2);
/// Happy Eyeballs (RFC 8305) delay before racing the next resolved address.
const HAPPY_EYEBALLS_STAGGER: Duration = Duration::from_millis(250);

/// Reconnect attempt counter for one [`RemoteClient::run_ws_loop`].
#[derive(Debug)]
struct ReconnectBudget {
    attempt: u32,
    max: u32,
}

impl ReconnectBudget {
    fn new(max: u32) -> Self {
        Self { attempt: 0, max }
    }

    /// Count a failed attempt. `None` once the budget is spent.
    fn next_attempt(&mut self) -> Option<u32> {
        self.attempt += 1;
        (self.attempt <= self.max).then_some(self.attempt)
    }

    /// Start over: the app came to the foreground or the user asked to retry.
    fn reset(&mut self) {
        self.attempt = 0;
    }
}

/// What the reader must do when its liveness deadline passes.
#[derive(Debug, PartialEq, Eq)]
enum LivenessAction {
    /// Nothing heard for the idle timeout: send a ping and wait the grace.
    SendProbe,
    /// The probe went unanswered: the connection is dead.
    Dead,
}

/// Dead-connection detection for an established session. Pure (time is passed
/// in) so the deadline logic is unit-testable.
#[derive(Debug)]
struct Liveness {
    idle: Duration,
    grace: Duration,
    last_inbound: Instant,
    deadline: Instant,
    probe_pending: bool,
}

impl Liveness {
    fn new(now: Instant, idle: Duration, grace: Duration) -> Self {
        Self {
            idle,
            grace,
            last_inbound: now,
            deadline: now + idle,
            probe_pending: false,
        }
    }

    fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Any inbound frame proves the connection alive.
    fn on_inbound(&mut self, now: Instant) {
        self.last_inbound = now;
        self.deadline = now + self.idle;
        self.probe_pending = false;
    }

    fn on_deadline(&mut self, now: Instant) -> LivenessAction {
        if self.probe_pending {
            LivenessAction::Dead
        } else {
            self.probe_pending = true;
            self.deadline = now + self.grace;
            LivenessAction::SendProbe
        }
    }

    /// The app asked to verify the connection (e.g. back from background).
    /// Returns whether a probe ping must be sent.
    fn on_probe_request(&mut self, now: Instant, min_quiet: Duration) -> bool {
        if now.duration_since(self.last_inbound) < min_quiet {
            return false;
        }
        if !self.probe_pending {
            self.probe_pending = true;
        }
        self.deadline = std::cmp::min(self.deadline, now + self.grace);
        true
    }
}

/// Out-of-band control of a client's background connection task, shared by
/// every task the client spawns. Mobile drives it from app lifecycle events;
/// desktop never touches it and keeps the default (not paused) behaviour.
///
/// `watch` channels carry generation counters, so a signal sent while the task
/// is busy elsewhere is still seen at its next `changed()`.
#[derive(Clone)]
struct SessionControl {
    /// Abort the in-flight reconnect attempt or backoff sleep and retry now,
    /// with a fresh attempt budget.
    kick: Arc<tokio::sync::watch::Sender<u64>>,
    /// Verify an established session quickly (short deadline ping).
    probe: Arc<tokio::sync::watch::Sender<u64>>,
    /// While true, the reconnect loop dials nothing (the app is in the
    /// background, where Android drops packets and attempts only burn budget).
    paused: Arc<tokio::sync::watch::Sender<bool>>,
}

impl SessionControl {
    fn new() -> Self {
        Self {
            kick: Arc::new(tokio::sync::watch::channel(0).0),
            probe: Arc::new(tokio::sync::watch::channel(0).0),
            paused: Arc::new(tokio::sync::watch::channel(false).0),
        }
    }

    fn kick(&self) {
        self.kick.send_modify(|generation| *generation += 1);
    }

    fn probe(&self) {
        self.probe.send_modify(|generation| *generation += 1);
    }

    fn set_paused(&self, paused: bool) {
        self.paused.send_replace(paused);
    }
}

/// Wait until the control is not paused. Returns immediately when it isn't.
async fn wait_until_resumed(paused: &mut tokio::sync::watch::Receiver<bool>) {
    if !*paused.borrow_and_update() {
        return;
    }
    log::info!("Reconnect paused while the app is in the background");
    while *paused.borrow_and_update() {
        if paused.changed().await.is_err() {
            return;
        }
    }
    log::info!("Reconnect resumed");
}

/// Order resolved addresses for Happy Eyeballs: alternate address families,
/// starting with the family the resolver listed first (RFC 8305 §4).
fn interleave_address_families(addrs: Vec<std::net::SocketAddr>) -> Vec<std::net::SocketAddr> {
    let Some(first) = addrs.first() else {
        return addrs;
    };
    let first_v6 = first.is_ipv6();
    let (preferred, other): (Vec<_>, Vec<_>) = addrs
        .into_iter()
        .partition(|addr| addr.is_ipv6() == first_v6);
    let mut ordered = Vec::with_capacity(preferred.len() + other.len());
    let mut preferred = preferred.into_iter();
    let mut other = other.into_iter();
    loop {
        match (preferred.next(), other.next()) {
            (None, None) => break,
            (a, b) => ordered.extend(a.into_iter().chain(b)),
        }
    }
    ordered
}

/// Race TCP connects to `addrs` in order, starting the next one every
/// `stagger` (or as soon as all running attempts failed). First success wins.
async fn happy_eyeballs_connect(
    addrs: Vec<std::net::SocketAddr>,
    stagger: Duration,
) -> std::io::Result<tokio::net::TcpStream> {
    use futures::stream::{FuturesUnordered, StreamExt};

    let mut pending = addrs.into_iter();
    let mut attempts = FuturesUnordered::new();
    let mut last_error: Option<std::io::Error> = None;
    let dial = |addr: std::net::SocketAddr| async move {
        tokio::net::TcpStream::connect(addr)
            .await
            .map_err(|e| std::io::Error::new(e.kind(), format!("{addr}: {e}")))
    };
    match pending.next() {
        Some(addr) => attempts.push(dial(addr)),
        None => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "host resolved to no addresses",
            ));
        }
    }
    loop {
        tokio::select! {
            Some(result) = attempts.next() => match result {
                Ok(stream) => return Ok(stream),
                Err(error) => {
                    last_error = Some(error);
                    if attempts.is_empty() {
                        match pending.next() {
                            Some(addr) => attempts.push(dial(addr)),
                            None => break,
                        }
                    }
                }
            },
            _ = tokio::time::sleep(stagger), if pending.len() > 0 => {
                if let Some(addr) = pending.next() {
                    attempts.push(dial(addr));
                }
            }
            else => break,
        }
    }
    Err(last_error.unwrap_or_else(|| std::io::Error::other("all connection attempts failed")))
}

/// TCP-connect to `host:port`. An IP literal is dialed directly; a name is
/// resolved and its addresses raced (Happy Eyeballs) so one unreachable
/// address family cannot stall the connection — tungstenite alone would try
/// the addresses one after another.
async fn connect_tcp(host: &str, port: u16) -> std::io::Result<tokio::net::TcpStream> {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
        return tokio::net::TcpStream::connect((ip, port)).await;
    }
    let addrs: Vec<_> = tokio::net::lookup_host((bare, port)).await?.collect();
    happy_eyeballs_connect(interleave_address_families(addrs), HAPPY_EYEBALLS_STAGGER).await
}

/// Dial the WebSocket of a TCP (non-Unix) config: TCP, then TLS (with the
/// pinned connector) and the HTTP upgrade. Unbounded — callers wrap it in
/// [`WS_CONNECT_TIMEOUT`].
async fn dial_tcp_ws(
    config: &RemoteConnectionConfig,
    observed: &crate::client::tls::ObservedFingerprint,
) -> Result<(TcpWsStream, std::net::SocketAddr), SessionError> {
    let stream = connect_tcp(&config.host, config.port)
        .await
        .map_err(|e| SessionError::Transient(format!("TCP connect failed: {e}")))?;
    let peer = stream
        .peer_addr()
        .unwrap_or_else(|_| std::net::SocketAddr::from(([0, 0, 0, 0], config.port)));
    let connector = crate::client::tls::ws_connector(
        config.tls,
        config.pinned_cert_sha256.clone(),
        observed.clone(),
    );
    let (ws, _response) =
        tokio_tungstenite::client_async_tls_with_config(config.ws_url(), stream, None, connector)
            .await
            .map_err(|e| SessionError::Transient(format!("WebSocket connect failed: {e}")))?;
    Ok((ws, peer))
}

/// [`dial_tcp_ws`] bounded by `limit`; a timeout is transient like any other
/// network failure, so the reconnect loop retries it.
async fn dial_tcp_ws_within(
    config: &RemoteConnectionConfig,
    observed: &crate::client::tls::ObservedFingerprint,
    limit: Duration,
) -> Result<(TcpWsStream, std::net::SocketAddr), SessionError> {
    tokio::time::timeout(limit, dial_tcp_ws(config, observed))
        .await
        .map_err(|_| {
            SessionError::Transient(format!(
                "WebSocket connect to {} timed out after {}s",
                config.display_endpoint(),
                limit.as_secs_f32()
            ))
        })?
}

/// Platform-specific operations that the generic client delegates to.
///
/// Desktop creates `Terminal` objects and inserts into `TerminalsRegistry`.
/// Mobile creates `TerminalHolder` state via the FFI binding crate.
pub trait ConnectionHandler: Send + Sync + 'static {
    /// Terminal discovered — create platform terminal object.
    /// `ws_sender` is for constructing a transport that sends WS commands.
    /// `cols`/`rows` are the server's current terminal dimensions (0 if unknown).
    fn create_terminal(
        &self,
        connection_id: &str,
        terminal_id: &str,
        prefixed_id: &str,
        ws_sender: async_channel::Sender<WsClientMessage>,
        cols: u16,
        rows: u16,
    );
    /// Binary PTY output arrived — route to the terminal's emulator.
    fn on_terminal_output(&self, prefixed_id: &str, data: &[u8]);
    /// A snapshot arrived: the server's rendered grid, replayed on every
    /// (re)subscribe. Defaults to [`Self::on_terminal_output`]; a handler that
    /// derives activity from live output overrides it so replayed state does
    /// not look like new output.
    fn on_terminal_snapshot(&self, prefixed_id: &str, data: &[u8]) {
        self.on_terminal_output(prefixed_id, data);
    }
    /// Terminal removed — clean up platform terminal object.
    fn remove_terminal(&self, prefixed_id: &str);
    /// Resize a terminal's grid to match the server's dimensions.
    ///
    /// Called both for the initial pre-resize (before the snapshot arrives, so
    /// ANSI data renders at the correct size) and for live server-side resize
    /// broadcasts. `server_owns` is true only when the origin's local user
    /// currently holds resize authority; the handler should then mark the
    /// remote side as resize owner locally so this client stops re-asserting
    /// its own window size and the origin's reclaim sticks. Pre-resize passes
    /// `false`, leaving the client free to enforce its own size on connect.
    fn resize_terminal(&self, prefixed_id: &str, cols: u16, rows: u16, server_owns: bool);
    /// Connection is disconnecting — remove ALL terminals for this connection.
    fn remove_all_terminals(&self, connection_id: &str);
    /// Remove terminals for this connection that are NOT in the given set of
    /// (unprefixed) terminal IDs.  Called on reconnect to clean up terminals
    /// that disappeared on the server while the client was offline.
    fn remove_terminals_except(
        &self,
        connection_id: &str,
        keep_ids: &std::collections::HashSet<String>,
    );
}

/// Generic remote client state machine, parameterized by a platform handler.
pub struct RemoteClient<H: ConnectionHandler> {
    config: RemoteConnectionConfig,
    status: ConnectionStatus,
    runtime: Arc<tokio::runtime::Runtime>,
    ws_tx: Option<async_channel::Sender<WsClientMessage>>,
    remote_state: Option<StateResponse>,
    system_stats: Option<ApiSystemStats>,
    stream_map: HashMap<String, u32>,
    reverse_stream_map: HashMap<u32, String>,
    handler: Arc<H>,
    event_tx: async_channel::Sender<ConnectionEvent>,
    ws_abort_handle: Option<tokio::task::AbortHandle>,
    /// Shared token reference so WS reconnect loop can pick up refreshed tokens.
    shared_token: Arc<std::sync::RwLock<Option<String>>>,
    /// Last declared viewport (unprefixed project ids). Held here rather than
    /// only pushed down the channel so every reconnect re-declares it — the
    /// server drops a connection's entry on close, and a client that never
    /// re-sends would silently fall out of the `gh` PR/CI scope.
    visible_projects: Arc<std::sync::RwLock<Vec<String>>>,
    /// Lifecycle signals into the background task (retry now, probe, pause).
    control: SessionControl,
}

impl<H: ConnectionHandler> RemoteClient<H> {
    pub fn new(
        config: RemoteConnectionConfig,
        runtime: Arc<tokio::runtime::Runtime>,
        handler: Arc<H>,
        event_tx: async_channel::Sender<ConnectionEvent>,
    ) -> Self {
        let shared_token = Arc::new(std::sync::RwLock::new(config.effective_auth_token()));
        Self {
            config,
            status: ConnectionStatus::Disconnected,
            runtime,
            ws_tx: None,
            remote_state: None,
            system_stats: None,
            stream_map: HashMap::new(),
            reverse_stream_map: HashMap::new(),
            handler,
            event_tx,
            ws_abort_handle: None,
            shared_token,
            visible_projects: Arc::new(std::sync::RwLock::new(Vec::new())),
            control: SessionControl::new(),
        }
    }

    /// Retry right away: abort the in-flight reconnect attempt or backoff
    /// sleep and start over with a fresh attempt budget. Only the reconnect
    /// loop listens, so call it while the status is `Reconnecting`.
    pub fn reconnect_now(&self) {
        self.control.kick();
    }

    /// Verify an established session: if nothing arrived for a moment, send a
    /// ping and reconnect unless something comes back within a few seconds.
    /// For when the app returns from the background, where the socket may have
    /// died silently.
    pub fn probe_connection(&self) {
        self.control.probe();
    }

    /// Pause (or resume) reconnect attempts. A paused loop dials nothing and
    /// spends no attempt budget; an established session is left alone.
    pub fn set_reconnect_paused(&self, paused: bool) {
        self.control.set_paused(paused);
    }

    /// Declare which of this connection's projects the client currently renders.
    ///
    /// Full replacement set of unprefixed ids. Stored for reconnects and pushed
    /// to the server immediately when a session is up.
    pub fn set_visible_projects(&self, project_ids: Vec<String>) {
        if let Ok(mut stored) = self.visible_projects.write() {
            if *stored == project_ids {
                return;
            }
            stored.clone_from(&project_ids);
        }
        if let Some(tx) = self.ws_tx.as_ref() {
            let _ = tx.try_send(WsClientMessage::SetVisibleProjects { project_ids });
        }
    }

    pub fn config(&self) -> &RemoteConnectionConfig {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut RemoteConnectionConfig {
        &mut self.config
    }

    pub fn status(&self) -> &ConnectionStatus {
        &self.status
    }

    pub fn status_mut(&mut self) -> &mut ConnectionStatus {
        &mut self.status
    }

    pub fn set_status(&mut self, status: ConnectionStatus) {
        self.status = status;
    }

    pub fn remote_state(&self) -> Option<&StateResponse> {
        self.remote_state.as_ref()
    }

    pub fn remote_state_mut(&mut self) -> Option<&mut StateResponse> {
        self.remote_state.as_mut()
    }

    pub fn set_remote_state(&mut self, state: Option<StateResponse>) {
        self.remote_state = state;
    }

    pub fn system_stats(&self) -> Option<&ApiSystemStats> {
        self.system_stats.as_ref()
    }

    pub fn set_system_stats(&mut self, stats: Option<ApiSystemStats>) {
        self.system_stats = stats;
    }

    /// Update the shared token so WS reconnect loop uses the latest token.
    pub fn update_shared_token(&self, token: &str) {
        if let Ok(mut guard) = self.shared_token.write() {
            *guard = Some(token.to_string());
        }
    }

    pub fn ws_sender(&self) -> Option<&async_channel::Sender<WsClientMessage>> {
        self.ws_tx.as_ref()
    }

    /// Update stream mappings from a subscription response.
    pub fn update_stream_mappings(&mut self, mappings: HashMap<String, u32>) {
        for (terminal_id, stream_id) in &mappings {
            self.stream_map.insert(terminal_id.clone(), *stream_id);
            self.reverse_stream_map
                .insert(*stream_id, terminal_id.clone());
        }
    }

    /// Start the connection process.
    ///
    /// 1. GET /health to verify server is alive
    /// 2. If auth token or trusted local transport: GET /v1/state to validate/reach state
    ///    - 200: token valid, proceed to start_ws()
    ///    - 401: token expired, set Pairing status
    /// 3. No auth token: set Pairing status
    pub fn connect(&mut self) {
        // Tear down any prior connection so we don't orphan its WS task.
        self.abort_ws_task();
        self.status = ConnectionStatus::Connecting;

        let config = self.config.clone();
        let event_tx = self.event_tx.clone();
        let handler = self.handler.clone();
        let shared_token = self.shared_token.clone();

        // Update shared token from config
        if let Ok(mut guard) = self.shared_token.write() {
            *guard = config.effective_auth_token();
        }

        // Create fresh WS message channel
        let (ws_tx, ws_rx) = ws_message_channel();
        self.ws_tx = Some(ws_tx.clone());
        let visible_projects = self.visible_projects.clone();
        let control = self.control.clone();

        let task = self.runtime.spawn(async move {
            let mut config = config;
            let observed = crate::client::tls::new_observed();

            // Step 1: detect scheme + health check. A pinned/TLS connection only
            // tries TLS (never downgrade). A legacy plain connection prefers TLS
            // (auto-upgrade) but falls back to plain http so it keeps working
            // against a server that hasn't enabled TLS.
            let local_unix = local_unix_path(&config).map(str::to_string);
            let schemes: &[bool] = if local_unix.is_some() {
                &[false]
            } else if config.tls {
                &[true]
            } else {
                &[true, false]
            };
            let mut chosen: Option<(bool, reqwest::Client, String)> = None;
            let attempts = initial_connect_attempts(&config);
            let started = Instant::now();
            let mut last_connect_failure: Option<String> = None;
            for attempt in 1..=attempts {
                if attempt > 1 {
                    let delay = initial_connect_retry_delay(attempt - 1);
                    let detail = last_connect_failure
                        .as_deref()
                        .map(|failure| format!(": {failure}"))
                        .unwrap_or_default();
                    log::warn!(
                        "Initial connection to {} failed{}. Retrying in {}ms (attempt {}/{})",
                        config.display_endpoint(),
                        detail,
                        delay.as_millis(),
                        attempt,
                        attempts
                    );
                    tokio::time::sleep(delay).await;
                }

                for &tls in schemes {
                    let client_and_url = if let Some(path) = local_unix.as_deref() {
                        unix_http_client(path).map(|client| (client, config.http_origin()))
                    } else {
                        crate::client::tls::build_reqwest_client(
                            tls,
                            config.pinned_cert_sha256.clone(),
                            observed.clone(),
                        )
                        .map(|client| {
                            let scheme = if tls { "https" } else { "http" };
                            (
                                client,
                                format!("{}://{}:{}", scheme, config.host, config.port),
                            )
                        })
                    };
                    let (client, base_url) = match client_and_url {
                        Ok(client_and_url) => client_and_url,
                        Err(error) => {
                            last_connect_failure = Some(error);
                            continue;
                        }
                    };
                    log::info!(
                        "Health check {} starting ({}) at +{}ms",
                        config.display_endpoint(),
                        if tls { "https" } else { "http" },
                        started.elapsed().as_millis()
                    );
                    let health_result = client
                        .get(format!("{}/health", base_url))
                        .timeout(Duration::from_secs(5))
                        .send()
                        .await;
                    let ok = matches!(
                        health_result.as_ref(),
                        Ok(resp) if resp.status().is_success()
                    );
                    last_connect_failure = match health_result {
                        Ok(resp) if resp.status().is_success() => None,
                        Ok(resp) => Some(format!("health returned HTTP {}", resp.status())),
                        Err(e) => Some(e.to_string()),
                    };
                    log::info!(
                        "Health check {} {} at +{}ms",
                        config.display_endpoint(),
                        if ok { "ok" } else { "failed" },
                        started.elapsed().as_millis()
                    );
                    if ok {
                        chosen = Some((tls, client, base_url));
                        break;
                    }
                }
                if chosen.is_some() {
                    break;
                }
            }

            let (detected_tls, mut client, base_url) = match chosen {
                Some(v) => v,
                None => {
                    let msg = match last_connect_failure {
                        Some(failure) => format!(
                            "Cannot reach server {} (last error: {})",
                            config.display_endpoint(),
                            failure
                        ),
                        None => format!("Cannot reach server {}", config.display_endpoint()),
                    };
                    log::warn!("{}", msg);
                    let _ = event_tx
                        .send(ConnectionEvent::StatusChanged {
                            connection_id: config.id.clone(),
                            status: ConnectionStatus::Error(msg),
                        })
                        .await;
                    return;
                }
            };
            log::info!(
                "Remote server {} is healthy ({})",
                config.display_endpoint(),
                if detected_tls { "TLS" } else { "plain http" }
            );

            // TOFU: the handshake that reached the server settles the identity
            // for this task. Without this a config carrying `tls` but no pin —
            // what desktop UpgradeToTls persists before pairing — would hand its
            // bearer token to whatever certificate answers, on every reconnect.
            if needs_certificate_adoption(&config, detected_tls) {
                let upgraded_from_plain = !config.tls;
                config.tls = true;
                let fp = adopt_observed_pin(&mut config, &observed);
                // A previously-plain connection asks the manager to persist both,
                // so the sidebar reflects the upgrade and the pin is enforced next
                // time.
                if upgraded_from_plain {
                    log::info!("Auto-upgraded {}:{} to TLS", config.host, config.port);
                    let _ = event_tx
                        .send(ConnectionEvent::TlsUpgraded {
                            connection_id: config.id.clone(),
                            cert_fingerprint: fp,
                        })
                        .await;
                }
                // The client above was built for an unpinned handshake; the
                // requests below carry the bearer token.
                match crate::client::tls::build_reqwest_client(
                    true,
                    config.pinned_cert_sha256.clone(),
                    observed.clone(),
                ) {
                    Ok(pinned) => client = pinned,
                    Err(error) => {
                        let msg = format!("Cannot pin {}: {error}", config.display_endpoint());
                        log::warn!("{}", msg);
                        let _ = event_tx
                            .send(ConnectionEvent::StatusChanged {
                                connection_id: config.id.clone(),
                                status: ConnectionStatus::Error(msg),
                            })
                            .await;
                        return;
                    }
                }
            }

            // Step 2: Validate saved token, or trust same-user Unix socket transport.
            if let Some(token) = config.effective_auth_token() {
                let trusted_local_transport =
                    config.saved_token.is_none() && config.is_trusted_local_transport();
                log::info!(
                    "Token check (GET /v1/state) starting at +{}ms",
                    started.elapsed().as_millis()
                );
                let state_result = client
                    .get(format!("{}/v1/state", base_url))
                    .header("Authorization", format!("Bearer {}", token))
                    .timeout(Duration::from_secs(5))
                    .send()
                    .await;
                log::info!(
                    "Token check finished ({}) at +{}ms",
                    match &state_result {
                        Ok(resp) => resp.status().to_string(),
                        Err(_) => "network error".to_string(),
                    },
                    started.elapsed().as_millis()
                );
                match state_result {
                    Ok(resp) if resp.status().is_success() => {
                        if trusted_local_transport {
                            log::info!(
                                "Trusted local transport accepted for {}",
                                config.display_endpoint()
                            );
                        } else {
                            log::info!("Token valid for {}", config.display_endpoint());
                        }
                        // Token is valid - start WebSocket
                        let (session_config, _) = SessionConfig::adopt(config, &observed);
                        Self::run_ws_loop(
                            session_config,
                            token,
                            event_tx,
                            ws_tx,
                            ws_rx,
                            handler,
                            shared_token,
                            visible_projects,
                            control,
                        )
                        .await;
                        return;
                    }
                    Ok(resp) if resp.status() == reqwest::StatusCode::UNAUTHORIZED => {
                        log::info!(
                            "Token expired for {}, need re-pairing",
                            config.display_endpoint()
                        );
                        // Only 401 means the token is actually invalid → need pairing
                        let _ = event_tx
                            .send(ConnectionEvent::StatusChanged {
                                connection_id: config.id.clone(),
                                status: ConnectionStatus::Pairing,
                            })
                            .await;
                        return;
                    }
                    Ok(resp) => {
                        // Transient server error (e.g. 500 during startup) —
                        // token may still be valid, don't discard it.
                        let msg = format!("Token validation: unexpected HTTP {}", resp.status());
                        log::warn!("{}", msg);
                        let _ = event_tx
                            .send(ConnectionEvent::StatusChanged {
                                connection_id: config.id.clone(),
                                status: ConnectionStatus::Error(msg),
                            })
                            .await;
                        return;
                    }
                    Err(e) => {
                        // Network error — token may still be valid, don't discard it.
                        let msg = format!("Token validation failed: {}", e);
                        log::warn!("{}", msg);
                        let _ = event_tx
                            .send(ConnectionEvent::StatusChanged {
                                connection_id: config.id.clone(),
                                status: ConnectionStatus::Error(msg),
                            })
                            .await;
                        return;
                    }
                }
            }

            // No saved token → need pairing
            let _ = event_tx
                .send(ConnectionEvent::StatusChanged {
                    connection_id: config.id.clone(),
                    status: ConnectionStatus::Pairing,
                })
                .await;
        });

        self.ws_abort_handle = Some(task.abort_handle());
    }

    /// Pair with the remote server using a 6-digit code.
    /// On success, saves the token and starts the WebSocket connection.
    pub fn pair(&mut self, code: &str) {
        // Tear down any prior connection so we don't orphan its WS task.
        self.abort_ws_task();
        let config = self.config.clone();
        let code = code.to_string();
        let event_tx = self.event_tx.clone();
        let handler = self.handler.clone();
        let shared_token = self.shared_token.clone();

        // Create fresh WS message channel
        let (ws_tx, ws_rx) = ws_message_channel();
        self.ws_tx = Some(ws_tx.clone());
        let visible_projects = self.visible_projects.clone();
        let control = self.control.clone();

        self.status = ConnectionStatus::Connecting;

        let task = self.runtime.spawn(async move {
            let local_unix = local_unix_path(&config).map(str::to_string);
            let base_url = config.http_origin();
            let observed = crate::client::tls::new_observed();
            let client = if let Some(path) = local_unix.as_deref() {
                unix_http_client(path)
            } else {
                crate::client::tls::build_reqwest_client(
                    config.tls,
                    config.pinned_cert_sha256.clone(),
                    observed.clone(),
                )
            };
            let client = match client {
                Ok(client) => client,
                Err(error) => {
                    let msg = format!("Pairing client initialisation failed: {error}");
                    let _ = event_tx
                        .send(ConnectionEvent::StatusChanged {
                            connection_id: config.id.clone(),
                            status: ConnectionStatus::Error(msg),
                        })
                        .await;
                    return;
                }
            };

            // POST /v1/pair with the code
            let pair_body = serde_json::json!({ "code": code });
            match client
                .post(format!("{}/v1/pair", base_url))
                .json(&pair_body)
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    #[derive(serde::Deserialize)]
                    struct PairResp {
                        token: String,
                        #[allow(dead_code)]
                        expires_in: u64,
                    }
                    match resp.json::<PairResp>().await {
                        Ok(pair_resp) => {
                            log::info!("Paired with {}:{}", config.host, config.port);

                            // Update shared token
                            if let Ok(mut guard) = shared_token.write() {
                                *guard = Some(pair_resp.token.clone());
                            }

                            // The session runs on the certificate the pairing
                            // handshake presented; the manager persists the same
                            // value.
                            let (session_config, cert_fingerprint) =
                                SessionConfig::adopt(config, &observed);

                            // Notify manager to save the token (+ pin the cert)
                            let _ = event_tx
                                .send(ConnectionEvent::TokenObtained {
                                    connection_id: session_config.get().id.clone(),
                                    token: pair_resp.token.clone(),
                                    cert_fingerprint,
                                })
                                .await;

                            // Start WebSocket
                            Self::run_ws_loop(
                                session_config,
                                pair_resp.token,
                                event_tx,
                                ws_tx,
                                ws_rx,
                                handler,
                                shared_token,
                                visible_projects,
                                control,
                            )
                            .await;
                        }
                        Err(e) => {
                            let msg = format!("Failed to parse pair response: {}", e);
                            log::error!("{}", msg);
                            let _ = event_tx
                                .send(ConnectionEvent::StatusChanged {
                                    connection_id: config.id.clone(),
                                    status: ConnectionStatus::Error(msg),
                                })
                                .await;
                        }
                    }
                }
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    let msg = format!("Pairing failed: HTTP {} - {}", status, body);
                    log::warn!("{}", msg);
                    let _ = event_tx
                        .send(ConnectionEvent::StatusChanged {
                            connection_id: config.id.clone(),
                            status: ConnectionStatus::Error(msg),
                        })
                        .await;
                }
                Err(e) => {
                    let msg = format!("Pairing request failed: {}", e);
                    log::warn!("{}", msg);
                    let _ = event_tx
                        .send(ConnectionEvent::StatusChanged {
                            connection_id: config.id.clone(),
                            status: ConnectionStatus::Error(msg),
                        })
                        .await;
                }
            }
        });

        self.ws_abort_handle = Some(task.abort_handle());
    }

    /// Abort any in-flight WS task and close its message channel. Called before
    /// starting a fresh connection so a reconnect doesn't orphan the prior task,
    /// and on disconnect/drop for cleanup.
    fn abort_ws_task(&mut self) {
        if let Some(handle) = self.ws_abort_handle.take() {
            handle.abort();
        }
        if let Some(tx) = self.ws_tx.take() {
            tx.close();
        }
    }

    /// Disconnect and clean up all remote terminals.
    pub fn disconnect(&mut self) {
        self.abort_ws_task();

        // Remove all terminals belonging to this connection
        self.handler.remove_all_terminals(&self.config.id);

        self.stream_map.clear();
        self.reverse_stream_map.clear();
        self.remote_state = None;
        self.system_stats = None;
        self.status = ConnectionStatus::Disconnected;
    }

    /// Restart the transport while retaining the last state and terminal
    /// objects. The fresh state sync reconciles deletions after reconnect.
    pub fn reconnect(&mut self) {
        self.stream_map.clear();
        self.reverse_stream_map.clear();
        self.connect();
    }

    /// Run the main WebSocket loop with reconnection.
    // Each param is a distinct piece of per-connection state the session needs.
    #[allow(clippy::too_many_arguments)]
    async fn run_ws_loop(
        session_config: SessionConfig,
        token: String,
        event_tx: async_channel::Sender<ConnectionEvent>,
        ws_tx: async_channel::Sender<WsClientMessage>,
        ws_rx: async_channel::Receiver<WsClientMessage>,
        handler: Arc<H>,
        shared_token: Arc<std::sync::RwLock<Option<String>>>,
        visible_projects: Arc<std::sync::RwLock<Vec<String>>>,
        control: SessionControl,
    ) {
        let config = session_config.into_inner();
        let mut budget = ReconnectBudget::new(ws_reconnect_max_attempts(&config));
        let mut current_token = token;
        let mut kick_rx = control.kick.subscribe();
        let mut paused_rx = control.paused.subscribe();

        loop {
            // Never dial from the background; the attempt would only hang in
            // Doze and burn budget. Foreground resumes us (and resets budget).
            wait_until_resumed(&mut paused_rx).await;
            // A kick only aborts work that was already running when it came.
            kick_rx.borrow_and_update();
            // Use the latest token (it may have been refreshed meanwhile).
            if let Ok(guard) = shared_token.read()
                && let Some(ref latest) = *guard
            {
                current_token = latest.clone();
            }

            // Set by the session once it reports Connected: a drop after that
            // starts a fresh budget instead of continuing the old count.
            let mut reached_connected = false;
            let session = Self::ws_session(
                &mut reached_connected,
                &config,
                &current_token,
                &event_tx,
                &ws_tx,
                &ws_rx,
                &handler,
                &visible_projects,
                &control,
            );
            let outcome = tokio::select! {
                result = session => Some(result),
                Ok(()) = kick_rx.changed() => None,
            };

            match outcome {
                None => {
                    log::info!(
                        "Reconnect to {} requested; abandoning the current attempt",
                        config.display_endpoint()
                    );
                    budget.reset();
                }
                Some(Ok(())) => {
                    // Clean disconnect requested
                    log::info!(
                        "WebSocket cleanly disconnected from {}:{}",
                        config.host,
                        config.port
                    );
                    break;
                }
                Some(Err(SessionError::Auth(msg))) => {
                    log::warn!(
                        "Auth error for {}:{}: {}. Switching to Pairing state.",
                        config.host,
                        config.port,
                        msg
                    );
                    let _ = event_tx
                        .send(ConnectionEvent::StatusChanged {
                            connection_id: config.id.clone(),
                            status: ConnectionStatus::Pairing,
                        })
                        .await;
                    break;
                }
                Some(Err(SessionError::Transient(e))) => {
                    if reached_connected {
                        budget.reset();
                    }
                    let Some(attempt) = budget.next_attempt() else {
                        let msg = format!(
                            "Connection lost after {} attempts (last error: {})",
                            budget.max, e
                        );
                        log::error!("{}", msg);
                        let _ = event_tx
                            .send(ConnectionEvent::StatusChanged {
                                connection_id: config.id.clone(),
                                status: ConnectionStatus::Error(msg),
                            })
                            .await;
                        break;
                    };

                    let backoff = ws_reconnect_backoff_secs(&config, attempt);

                    log::warn!(
                        "WebSocket connection to {}:{} lost: {}. Reconnecting in {}s (attempt {}/{})",
                        config.host,
                        config.port,
                        e,
                        backoff,
                        attempt,
                        budget.max
                    );

                    let _ = event_tx
                        .send(ConnectionEvent::StatusChanged {
                            connection_id: config.id.clone(),
                            status: ConnectionStatus::Reconnecting { attempt },
                        })
                        .await;

                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
                        Ok(()) = kick_rx.changed() => {
                            log::info!(
                                "Reconnect to {} requested; skipping the {}s backoff",
                                config.display_endpoint(),
                                backoff
                            );
                            budget.reset();
                        }
                    }
                }
            }
        }
    }

    /// A single WebSocket session. Returns Ok(()) on clean disconnect, Err on failure.
    // Same per-connection state as run_ws_loop, borrowed.
    #[allow(clippy::too_many_arguments)]
    async fn ws_session(
        reached_connected: &mut bool,
        config: &RemoteConnectionConfig,
        token: &str,
        event_tx: &async_channel::Sender<ConnectionEvent>,
        ws_tx: &async_channel::Sender<WsClientMessage>,
        ws_rx: &async_channel::Receiver<WsClientMessage>,
        handler: &Arc<H>,
        visible_projects: &Arc<std::sync::RwLock<Vec<String>>>,
        control: &SessionControl,
    ) -> Result<(), SessionError> {
        // Shared stream maps: terminal_id -> stream_id (for writer) and reverse (for reader)
        let stream_map: Arc<std::sync::RwLock<HashMap<String, u32>>> =
            Arc::new(std::sync::RwLock::new(HashMap::new()));
        let mut reverse_stream_map: HashMap<u32, String> = HashMap::new();
        let observed = crate::client::tls::new_observed();

        let started = Instant::now();
        let elapsed_ms = || started.elapsed().as_millis();

        // Connect WebSocket (TCP + TLS + upgrade), bounded so a silently
        // dropped SYN can't stall the session; a timeout is transient.
        log::info!(
            "WS connect to {} starting ({})",
            config.display_endpoint(),
            if config.tls { "wss" } else { "ws" }
        );
        let ws_stream = if let Some(path) = local_unix_path(config) {
            #[cfg(unix)]
            {
                let dial = async {
                    let stream = tokio::net::UnixStream::connect(path).await.map_err(|e| {
                        SessionError::Transient(format!("Unix socket connect failed: {}", e))
                    })?;
                    tokio_tungstenite::client_async("ws://okena.local/v1/stream", stream)
                        .await
                        .map_err(|e| {
                            SessionError::Transient(format!("WebSocket connect failed: {}", e))
                        })
                };
                let (ws, _response) = tokio::time::timeout(WS_CONNECT_TIMEOUT, dial)
                    .await
                    .map_err(|_| {
                        SessionError::Transient(format!(
                            "WebSocket connect to {} timed out",
                            config.display_endpoint()
                        ))
                    })??;
                log::info!(
                    "WS connected to {} in {}ms",
                    config.display_endpoint(),
                    elapsed_ms()
                );
                AnyWsStream::Unix(Box::new(ws))
            }
            #[cfg(not(unix))]
            {
                let _ = path;
                return Err(SessionError::Transient(
                    "Unix socket transport is not supported on this platform".to_string(),
                ));
            }
        } else {
            let result = dial_tcp_ws_within(config, &observed, WS_CONNECT_TIMEOUT).await;
            match result {
                Ok((ws, peer)) => {
                    log::info!(
                        "WS connected to {} via {} in {}ms",
                        config.display_endpoint(),
                        peer,
                        elapsed_ms()
                    );
                    AnyWsStream::Tcp(Box::new(ws))
                }
                Err(error) => {
                    if let SessionError::Transient(msg) = &error {
                        log::warn!("WS connect failed after {}ms: {}", elapsed_ms(), msg);
                    }
                    return Err(error);
                }
            }
        };

        let (mut ws_write, mut ws_read) = futures::StreamExt::split(ws_stream);

        // Step 1: Send Auth
        let auth_msg = serde_json::json!({
            "type": "auth",
            "token": token,
        });
        futures::SinkExt::send(
            &mut ws_write,
            tungstenite::Message::Text(auth_msg.to_string()),
        )
        .await
        .map_err(|e| SessionError::Transient(format!("Failed to send auth: {}", e)))?;

        // Step 2: Wait for AuthOk
        let auth_response = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            futures::StreamExt::next(&mut ws_read),
        )
        .await
        .map_err(|_| SessionError::Transient("Auth response timeout".to_string()))?
        .ok_or_else(|| {
            SessionError::Transient("WebSocket closed before auth response".to_string())
        })?
        .map_err(|e| SessionError::Transient(format!("WebSocket read error: {}", e)))?;

        match &auth_response {
            tungstenite::Message::Text(text) => {
                let parsed: serde_json::Value = serde_json::from_str(text)
                    .map_err(|e| SessionError::Transient(format!("Invalid JSON: {}", e)))?;
                let msg_type = parsed.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if msg_type == "auth_ok" {
                    log::info!(
                        "Authenticated with {}:{} at +{}ms",
                        config.host,
                        config.port,
                        elapsed_ms()
                    );
                } else if msg_type == "auth_failed" {
                    let error = parsed
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    return Err(SessionError::Auth(format!("Auth failed: {}", error)));
                } else {
                    return Err(SessionError::Transient(format!(
                        "Unexpected auth response type: {}",
                        msg_type
                    )));
                }
            }
            _ => {
                return Err(SessionError::Transient(
                    "Expected text message for auth response".to_string(),
                ));
            }
        }

        // Step 3: Fetch state via HTTP
        let base_url = config.http_origin();
        let client = if let Some(path) = local_unix_path(config) {
            unix_http_client(path)
        } else {
            crate::client::tls::build_reqwest_client(
                config.tls,
                config.pinned_cert_sha256.clone(),
                observed.clone(),
            )
        }
        .map_err(SessionError::Transient)?;
        log::info!("State fetch starting at +{}ms", elapsed_ms());
        let state_resp = client
            .get(format!("{}/v1/state", base_url))
            .header("Authorization", format!("Bearer {}", token))
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| SessionError::Transient(format!("Failed to fetch state: {}", e)))?;

        if state_resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(SessionError::Auth(format!(
                "State fetch failed: HTTP {}",
                state_resp.status()
            )));
        }
        if !state_resp.status().is_success() {
            return Err(SessionError::Transient(format!(
                "State fetch failed: HTTP {}",
                state_resp.status()
            )));
        }

        let state: StateResponse = state_resp
            .json()
            .await
            .map_err(|e| SessionError::Transient(format!("Failed to parse state: {}", e)))?;
        log::info!("State fetched at +{}ms", elapsed_ms());

        // Step 4: Sync terminal objects.
        // On reconnect, remove terminals that no longer exist on the server,
        // and create terminals that are new.  Existing terminals are kept
        // (create_terminal is idempotent) so that views holding Arc<Terminal>
        // continue to work without going stale.
        let current_ids = collect_all_terminal_ids(&state);
        handler.remove_terminals_except(&config.id, &current_ids);

        let terminal_ids = collect_state_terminal_ids(&state);
        let size_map = collect_terminal_sizes(&state);
        for tid in &terminal_ids {
            let prefixed = make_prefixed_id(&config.id, tid);
            let (cols, rows) = size_map.get(tid).copied().unwrap_or((0, 0));
            handler.create_terminal(&config.id, tid, &prefixed, ws_tx.clone(), cols, rows);
        }

        // Notify state received
        let _ = event_tx
            .send(ConnectionEvent::StateReceived {
                connection_id: config.id.clone(),
                state: state.clone(),
            })
            .await;
        match fetch_remote_settings(&client, &base_url, token).await {
            Ok(settings) => {
                let _ = event_tx
                    .send(ConnectionEvent::SettingsChanged {
                        connection_id: config.id.clone(),
                        settings,
                    })
                    .await;
            }
            Err(error) => log::warn!("{error}"),
        }

        // Step 5: Subscribe to all terminal streams
        if !terminal_ids.is_empty() {
            let subscribe_msg = serde_json::json!({
                "type": "subscribe",
                "terminal_ids": terminal_ids,
            });
            futures::SinkExt::send(
                &mut ws_write,
                tungstenite::Message::Text(subscribe_msg.to_string()),
            )
            .await
            .map_err(|e| SessionError::Transient(format!("Failed to send subscribe: {}", e)))?;
        }

        // Re-declare the viewport: the server drops a connection's entry when
        // the socket closes, so a reconnect that stayed quiet would leave this
        // client's projects outside the server's `gh` PR/CI scope.
        let declared = visible_projects
            .read()
            .map(|ids| ids.clone())
            .unwrap_or_default();
        if !declared.is_empty() {
            let visible_msg = serde_json::json!({
                "type": "set_visible_projects",
                "project_ids": declared,
            });
            futures::SinkExt::send(
                &mut ws_write,
                tungstenite::Message::Text(visible_msg.to_string()),
            )
            .await
            .map_err(|e| {
                SessionError::Transient(format!("Failed to send visible projects: {}", e))
            })?;
        }

        // Notify connected
        *reached_connected = true;
        log::info!(
            "Connected to {} in {}ms",
            config.display_endpoint(),
            elapsed_ms()
        );
        let _ = event_tx
            .send(ConnectionEvent::StatusChanged {
                connection_id: config.id.clone(),
                status: ConnectionStatus::Connected,
            })
            .await;

        // Step 6: Main loop
        let config_id = config.id.clone();
        let config_host = config.host.clone();
        let config_port = config.port;
        let event_tx_clone = event_tx.clone();
        let handler_clone = handler.clone();
        let ws_tx_clone = ws_tx.clone();

        // Spawn writer task
        let ws_rx_clone = ws_rx.clone();
        let stream_map_for_writer = stream_map.clone();
        let mut writer_handle = tokio::spawn(async move {
            while let Ok(msg) = ws_rx_clone.recv().await {
                // Prefer compact binary input when the subscription mapping is known.
                if let WsClientMessage::SendInput { terminal_id, data } = &msg {
                    let stream_id = stream_map_for_writer
                        .read()
                        .ok()
                        .and_then(|m| m.get(terminal_id).copied());
                    if let Some(sid) = stream_id {
                        let frame = okena_core::ws::build_binary_frame(
                            okena_core::ws::FRAME_TYPE_INPUT,
                            sid,
                            data,
                        );
                        if let Err(e) = futures::SinkExt::send(
                            &mut ws_write,
                            tungstenite::Message::Binary(frame),
                        )
                        .await
                        {
                            log::warn!("Failed to send binary input: {}", e);
                            break;
                        }
                        continue;
                    }
                }

                let json = match &msg {
                    WsClientMessage::SendInput { terminal_id, data } => {
                        serde_json::json!({
                            "type": "send_bytes",
                            "terminal_id": terminal_id,
                            "data": data,
                        })
                    }
                    WsClientMessage::Resize {
                        terminal_id,
                        cols,
                        rows,
                    } => {
                        serde_json::json!({
                            "type": "resize",
                            "terminal_id": terminal_id,
                            "cols": cols,
                            "rows": rows,
                        })
                    }
                    WsClientMessage::CloseTerminal { terminal_id } => {
                        serde_json::json!({
                            "type": "close_terminal",
                            "terminal_id": terminal_id,
                        })
                    }
                    WsClientMessage::Subscribe { terminal_ids } => {
                        serde_json::json!({
                            "type": "subscribe",
                            "terminal_ids": terminal_ids,
                        })
                    }
                    WsClientMessage::Unsubscribe { terminal_ids } => {
                        serde_json::json!({
                            "type": "unsubscribe",
                            "terminal_ids": terminal_ids,
                        })
                    }
                    WsClientMessage::SetVisibleProjects { project_ids } => {
                        serde_json::json!({
                            "type": "set_visible_projects",
                            "project_ids": project_ids,
                        })
                    }
                    WsClientMessage::Ping => serde_json::json!({ "type": "ping" }),
                };
                if let Err(e) = futures::SinkExt::send(
                    &mut ws_write,
                    tungstenite::Message::Text(json.to_string()),
                )
                .await
                {
                    log::warn!("Failed to send WS message: {}", e);
                    break;
                }
            }
        });

        // Reader loop. Besides inbound frames it watches the liveness
        // deadline, sends the periodic ping, answers probe requests and ends
        // the session if the writer task dies (a failed send means the socket
        // is gone even when no read error surfaced yet).
        let mut cached_state = state;
        let mut liveness = Liveness::new(Instant::now(), WS_IDLE_TIMEOUT, WS_PROBE_GRACE);
        let mut ping_timer = tokio::time::interval_at(
            tokio::time::Instant::now() + WS_PING_INTERVAL,
            WS_PING_INTERVAL,
        );
        ping_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut probe_rx = control.probe.subscribe();
        loop {
            let frame = tokio::select! {
                frame = futures::StreamExt::next(&mut ws_read) => frame,
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(liveness.deadline())) => {
                    match liveness.on_deadline(Instant::now()) {
                        LivenessAction::SendProbe => {
                            log::info!(
                                "No data from {}:{} for {}s; probing with a ping",
                                config_host,
                                config_port,
                                WS_IDLE_TIMEOUT.as_secs()
                            );
                            let _ = ws_tx_clone.try_send(WsClientMessage::Ping);
                        }
                        LivenessAction::Dead => {
                            log::warn!(
                                "Ping timeout: {}:{} did not answer within {}s",
                                config_host,
                                config_port,
                                WS_PROBE_GRACE.as_secs()
                            );
                            writer_handle.abort();
                            return Err(SessionError::Transient(
                                "Server stopped responding (ping timeout)".to_string(),
                            ));
                        }
                    }
                    continue;
                }
                _ = ping_timer.tick() => {
                    let _ = ws_tx_clone.try_send(WsClientMessage::Ping);
                    continue;
                }
                Ok(()) = probe_rx.changed() => {
                    if liveness.on_probe_request(Instant::now(), WS_PROBE_MIN_QUIET) {
                        log::info!(
                            "Probing {}:{} (connection check requested)",
                            config_host,
                            config_port
                        );
                        let _ = ws_tx_clone.try_send(WsClientMessage::Ping);
                    }
                    continue;
                }
                _ = &mut writer_handle => {
                    log::warn!("WebSocket writer to {}:{} stopped", config_host, config_port);
                    return Err(SessionError::Transient(
                        "WebSocket writer stopped".to_string(),
                    ));
                }
            };
            if matches!(frame, Some(Ok(_))) {
                liveness.on_inbound(Instant::now());
            }
            match frame {
                Some(Ok(tungstenite::Message::Binary(data))) => {
                    // Generic binary frame: [proto:1][type:1][stream_id:4 BE][payload...]
                    if let Some((frame_type, stream_id, payload)) =
                        okena_core::ws::parse_binary_frame(&data)
                    {
                        match frame_type {
                            okena_core::ws::FRAME_TYPE_PTY
                            | okena_core::ws::FRAME_TYPE_SNAPSHOT => {
                                // Route PTY output or snapshot to the correct terminal
                                if let Some(remote_tid) = reverse_stream_map.get(&stream_id) {
                                    let prefixed = make_prefixed_id(&config_id, remote_tid);
                                    if frame_type == okena_core::ws::FRAME_TYPE_SNAPSHOT {
                                        handler_clone.on_terminal_snapshot(&prefixed, payload);
                                    } else {
                                        handler_clone.on_terminal_output(&prefixed, payload);
                                    }
                                }
                            }
                            _ => {
                                log::debug!("Unknown binary frame type: {}", frame_type);
                            }
                        }
                    }
                }
                Some(Ok(tungstenite::Message::Text(text))) => {
                    // JSON message
                    match serde_json::from_str::<serde_json::Value>(&text) {
                        Ok(value) => {
                            let msg_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
                            match msg_type {
                                "subscribed" => {
                                    if let Some(mappings) = value.get("mappings")
                                        && let Ok(map) =
                                            serde_json::from_value::<HashMap<String, u32>>(
                                                mappings.clone(),
                                            )
                                    {
                                        log::info!("Subscribed to {} terminal streams", map.len());
                                        for (terminal_id, stream_id) in &map {
                                            reverse_stream_map
                                                .insert(*stream_id, terminal_id.clone());
                                        }
                                        // Update shared stream_map for writer task
                                        if let Ok(mut sm) = stream_map.write() {
                                            for (terminal_id, stream_id) in &map {
                                                sm.insert(terminal_id.clone(), *stream_id);
                                            }
                                        }
                                        // Pre-resize terminals to server dimensions before snapshots arrive
                                        if let Some(sizes) = value.get("sizes")
                                            && let Ok(size_map) = serde_json::from_value::<
                                                HashMap<String, (u16, u16)>,
                                            >(
                                                sizes.clone()
                                            )
                                        {
                                            for (terminal_id, (cols, rows)) in &size_map {
                                                let prefixed =
                                                    make_prefixed_id(&config_id, terminal_id);
                                                // Pre-resize only sizes the grid for the snapshot;
                                                // it must not claim authority, so the client can
                                                // still enforce its own window size after connect.
                                                handler_clone.resize_terminal(
                                                    &prefixed, *cols, *rows, false,
                                                );
                                            }
                                            log::info!(
                                                "Pre-resized {} terminals to server dimensions",
                                                size_map.len()
                                            );
                                        }

                                        let _ = event_tx_clone
                                            .send(ConnectionEvent::SubscriptionMappings {
                                                connection_id: config_id.clone(),
                                                mappings: map,
                                            })
                                            .await;
                                    }
                                }
                                "state_changed" => {
                                    log::info!("State changed on remote server");
                                    // Reuse the session HTTP client (built once at
                                    // Step 3) so connection pooling / keep-alive is
                                    // preserved across state_changed events instead
                                    // of rebuilding a client per event.
                                    match client
                                        .get(format!("{}/v1/state", base_url))
                                        .header("Authorization", format!("Bearer {}", token))
                                        .timeout(std::time::Duration::from_secs(10))
                                        .send()
                                        .await
                                    {
                                        Ok(resp) if resp.status().is_success() => {
                                            if let Ok(new_state) =
                                                resp.json::<StateResponse>().await
                                            {
                                                let diff = diff_states(&cached_state, &new_state);
                                                let new_size_map =
                                                    collect_terminal_sizes(&new_state);

                                                // Add new terminals via handler
                                                for tid in &diff.added_terminals {
                                                    let prefixed =
                                                        make_prefixed_id(&config_id, tid);
                                                    let (cols, rows) = new_size_map
                                                        .get(tid)
                                                        .copied()
                                                        .unwrap_or((0, 0));
                                                    handler_clone.create_terminal(
                                                        &config_id,
                                                        tid,
                                                        &prefixed,
                                                        ws_tx_clone.clone(),
                                                        cols,
                                                        rows,
                                                    );
                                                }

                                                // Remove old terminals via handler
                                                for tid in &diff.removed_terminals {
                                                    let prefixed =
                                                        make_prefixed_id(&config_id, tid);
                                                    handler_clone.remove_terminal(&prefixed);
                                                }

                                                // Subscribe to new terminals. Use a blocking
                                                // send (not try_send): dropping this on a full
                                                // channel would leave the new terminals silently
                                                // never streaming output.
                                                if !diff.added_terminals.is_empty()
                                                    && let Err(e) = ws_tx_clone
                                                        .send(WsClientMessage::Subscribe {
                                                            terminal_ids: diff
                                                                .added_terminals
                                                                .clone(),
                                                        })
                                                        .await
                                                {
                                                    log::warn!(
                                                        "failed to send Subscribe for {} terminals: {}",
                                                        diff.added_terminals.len(),
                                                        e
                                                    );
                                                }

                                                // Unsubscribe from removed terminals. Likewise
                                                // blocking — a dropped Unsubscribe leaks a stream
                                                // for an already-gone terminal.
                                                if !diff.removed_terminals.is_empty()
                                                    && let Err(e) = ws_tx_clone
                                                        .send(WsClientMessage::Unsubscribe {
                                                            terminal_ids: diff
                                                                .removed_terminals
                                                                .clone(),
                                                        })
                                                        .await
                                                {
                                                    log::warn!(
                                                        "failed to send Unsubscribe for {} terminals: {}",
                                                        diff.removed_terminals.len(),
                                                        e
                                                    );
                                                }

                                                cached_state = new_state.clone();

                                                let _ = event_tx_clone
                                                    .send(ConnectionEvent::StateReceived {
                                                        connection_id: config_id.clone(),
                                                        state: new_state,
                                                    })
                                                    .await;
                                            }
                                        }
                                        Ok(resp) => {
                                            log::warn!(
                                                "State re-fetch failed: HTTP {}",
                                                resp.status()
                                            );
                                        }
                                        Err(e) => {
                                            log::warn!("State re-fetch failed: {}", e);
                                        }
                                    }
                                    match fetch_remote_settings(&client, &base_url, token).await {
                                        Ok(settings) => {
                                            let _ = event_tx_clone
                                                .send(ConnectionEvent::SettingsChanged {
                                                    connection_id: config_id.clone(),
                                                    settings,
                                                })
                                                .await;
                                        }
                                        Err(error) => log::warn!("{error}"),
                                    }
                                }
                                "pong" => {
                                    // Keep-alive response, ignore
                                }
                                "dropped" => {
                                    let count =
                                        value.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
                                    log::warn!(
                                        "Server dropped {} messages for {}:{}",
                                        count,
                                        config_host,
                                        config_port
                                    );
                                    let _ = event_tx_clone
                                        .send(ConnectionEvent::ServerWarning {
                                            connection_id: config_id.clone(),
                                            message: format!("Server dropped {} messages", count),
                                        })
                                        .await;
                                }
                                "error" => {
                                    let error = value
                                        .get("error")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("unknown");
                                    log::warn!("Server error: {}", error);
                                    let _ = event_tx_clone
                                        .send(ConnectionEvent::ServerWarning {
                                            connection_id: config_id.clone(),
                                            message: format!("Server error: {}", error),
                                        })
                                        .await;
                                }
                                "terminal_resized" => {
                                    if let (Some(terminal_id), Some(cols), Some(rows)) = (
                                        value.get("terminal_id").and_then(|v| v.as_str()),
                                        value.get("cols").and_then(|v| v.as_u64()),
                                        value.get("rows").and_then(|v| v.as_u64()),
                                    ) {
                                        // Missing field → false (older servers that
                                        // never reclaim, preserving prior behavior).
                                        let server_owns = value
                                            .get("server_owns")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false);
                                        let prefixed = make_prefixed_id(&config_id, terminal_id);
                                        handler_clone.resize_terminal(
                                            &prefixed,
                                            cols as u16,
                                            rows as u16,
                                            server_owns,
                                        );
                                    }
                                }
                                "git_status_changed" => {
                                    if let Some(projects) = value.get("projects")
                                        && let Ok(statuses) = serde_json::from_value::<
                                            HashMap<String, okena_core::api::ApiGitStatus>,
                                        >(
                                            projects.clone()
                                        )
                                    {
                                        let _ = event_tx_clone
                                            .send(ConnectionEvent::GitStatusChanged {
                                                connection_id: config_id.clone(),
                                                statuses,
                                            })
                                            .await;
                                    }
                                }
                                "system_stats_changed" => {
                                    if let Some(stats) = value.get("stats")
                                        && let Ok(stats) = serde_json::from_value::<
                                            okena_core::api::ApiSystemStats,
                                        >(
                                            stats.clone()
                                        )
                                    {
                                        let _ = event_tx_clone
                                            .send(ConnectionEvent::SystemStatsChanged {
                                                connection_id: config_id.clone(),
                                                stats,
                                            })
                                            .await;
                                    }
                                }
                                "terminal_focus_requested" => {
                                    if let Some(request) = parse_presentation_request(
                                        &value,
                                        ClientPresentationRequest::FocusTerminal,
                                    ) {
                                        let _ = event_tx_clone
                                            .send(ConnectionEvent::PresentationRequested {
                                                connection_id: config_id.clone(),
                                                request,
                                            })
                                            .await;
                                    }
                                }
                                "project_visibility_requested" => {
                                    if let Some(request) = parse_presentation_request(
                                        &value,
                                        ClientPresentationRequest::ProjectVisibility,
                                    ) {
                                        let _ = event_tx_clone
                                            .send(ConnectionEvent::PresentationRequested {
                                                connection_id: config_id.clone(),
                                                request,
                                            })
                                            .await;
                                    }
                                }
                                "fullscreen_requested" => {
                                    if let Some(request) = parse_presentation_request(
                                        &value,
                                        ClientPresentationRequest::Fullscreen,
                                    ) {
                                        let _ = event_tx_clone
                                            .send(ConnectionEvent::PresentationRequested {
                                                connection_id: config_id.clone(),
                                                request,
                                            })
                                            .await;
                                    }
                                }
                                "toast" => {
                                    // `WsOutbound::Toast` is internally tagged, so
                                    // the ApiToast fields sit at the top level of
                                    // `value` alongside `"type":"toast"`.
                                    match serde_json::from_value::<okena_core::api::ApiToast>(
                                        value.clone(),
                                    ) {
                                        Ok(toast) => {
                                            let _ = event_tx_clone
                                                .send(ConnectionEvent::Toast {
                                                    connection_id: config_id.clone(),
                                                    toast,
                                                })
                                                .await;
                                        }
                                        Err(e) => {
                                            log::warn!("Failed to parse toast message: {}", e);
                                        }
                                    }
                                }
                                _ => {
                                    log::debug!("Unknown WS message type: {}", msg_type);
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("Failed to parse WS JSON: {}", e);
                        }
                    }
                }
                Some(Ok(tungstenite::Message::Ping(data))) => {
                    log::trace!("WS Ping received ({} bytes)", data.len());
                }
                Some(Ok(tungstenite::Message::Pong(_))) => {
                    // Expected keepalive response
                }
                Some(Ok(tungstenite::Message::Close(_))) => {
                    log::info!("Server closed WebSocket connection");
                    writer_handle.abort();
                    return Err(SessionError::Transient(
                        "Server closed connection".to_string(),
                    ));
                }
                Some(Ok(tungstenite::Message::Frame(_))) => {
                    // Raw frame, ignore
                }
                Some(Err(e)) => {
                    writer_handle.abort();
                    return Err(SessionError::Transient(format!("WebSocket error: {}", e)));
                }
                None => {
                    // Stream ended
                    writer_handle.abort();
                    return Err(SessionError::Transient(
                        "WebSocket stream ended".to_string(),
                    ));
                }
            }
        }
    }
}

impl<H: ConnectionHandler> Drop for RemoteClient<H> {
    fn drop(&mut self) {
        // Ensure the background WS task is aborted and its channel closed even if
        // disconnect() was never called, so the task doesn't outlive the client.
        self.abort_ws_task();
    }
}

/// Attempt to refresh a token if it's older than 20 hours.
/// On success, sends a `TokenRefreshed` event. On failure, logs a warning.
pub async fn try_refresh_token(
    config: &RemoteConnectionConfig,
    event_tx: &async_channel::Sender<ConnectionEvent>,
) {
    let token = match &config.saved_token {
        Some(t) => t,
        None => return,
    };

    // Check token age
    if let Some(obtained_at) = config.token_obtained_at {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        if now - obtained_at < TOKEN_REFRESH_AGE_SECS {
            return; // Token is still fresh
        }
    }
    // If token_obtained_at is None, attempt refresh (legacy token without timestamp)

    let local_unix = local_unix_path(config);
    let base_url = config.http_origin();
    let client = if let Some(path) = local_unix {
        unix_http_client(path)
    } else {
        crate::client::tls::build_reqwest_client(
            config.tls,
            config.pinned_cert_sha256.clone(),
            crate::client::tls::new_observed(),
        )
    };
    let client = match client {
        Ok(client) => client,
        Err(error) => {
            log::warn!("Token refresh client initialisation failed: {error}");
            return;
        }
    };

    match client
        .post(format!("{}/v1/refresh", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            #[derive(serde::Deserialize)]
            struct RefreshResp {
                token: String,
                #[allow(dead_code)]
                expires_in: u64,
            }
            match resp.json::<RefreshResp>().await {
                Ok(refresh_resp) => {
                    log::info!("Token refreshed for {}", config.display_endpoint());
                    let _ = event_tx
                        .send(ConnectionEvent::TokenRefreshed {
                            connection_id: config.id.clone(),
                            token: refresh_resp.token,
                        })
                        .await;
                }
                Err(e) => {
                    log::warn!(
                        "Failed to parse refresh response for {}:{}: {}",
                        config.host,
                        config.port,
                        e
                    );
                }
            }
        }
        Ok(resp) => {
            log::warn!(
                "Token refresh failed for {}:{}: HTTP {} (server may not support refresh)",
                config.host,
                config.port,
                resp.status()
            );
        }
        Err(e) => {
            log::warn!(
                "Token refresh request failed for {}:{}: {}",
                config.host,
                config.port,
                e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct TestHandler {
        remove_all_calls: AtomicUsize,
    }

    impl ConnectionHandler for TestHandler {
        fn create_terminal(
            &self,
            _connection_id: &str,
            _terminal_id: &str,
            _prefixed_id: &str,
            _ws_sender: async_channel::Sender<WsClientMessage>,
            _cols: u16,
            _rows: u16,
        ) {
        }

        fn on_terminal_output(&self, _prefixed_id: &str, _data: &[u8]) {}

        fn remove_terminal(&self, _prefixed_id: &str) {}

        fn resize_terminal(&self, _prefixed_id: &str, _cols: u16, _rows: u16, _server_owns: bool) {}

        fn remove_all_terminals(&self, _connection_id: &str) {
            self.remove_all_calls.fetch_add(1, Ordering::Relaxed);
        }

        fn remove_terminals_except(
            &self,
            _connection_id: &str,
            _keep_ids: &std::collections::HashSet<String>,
        ) {
        }
    }

    fn config(id: &str, local_endpoint: Option<LocalEndpoint>) -> RemoteConnectionConfig {
        RemoteConnectionConfig {
            id: id.to_string(),
            name: "test".to_string(),
            host: "127.0.0.1".to_string(),
            port: 19100,
            saved_token: None,
            token_obtained_at: None,
            tls: false,
            pinned_cert_sha256: None,
            local_endpoint,
        }
    }

    fn unix_endpoint() -> Option<LocalEndpoint> {
        Some(LocalEndpoint::UnixSocket {
            path: "/tmp/okena-test.sock".to_string(),
        })
    }

    #[test]
    fn initial_attempts_local_daemon_fails_fast() {
        // Small budget: the app-layer self-heal owns recovery once Error fires.
        let cfg = config(LOCAL_DAEMON_CONNECTION_ID, unix_endpoint());
        assert_eq!(initial_connect_attempts(&cfg), 5);
    }

    #[test]
    fn initial_attempts_other_local_endpoint_keeps_patient_budget() {
        // Only the implicit local-daemon id fails fast — a local_endpoint alone
        // does not opt a connection into the small budget.
        let cfg = config("some-other-connection", unix_endpoint());
        assert_eq!(initial_connect_attempts(&cfg), 30);
    }

    #[test]
    fn initial_attempts_user_remote_single_try() {
        assert_eq!(initial_connect_attempts(&config("user-remote", None)), 1);
    }

    #[test]
    fn ws_reconnect_budget_local_vs_remote() {
        let local = config(LOCAL_DAEMON_CONNECTION_ID, unix_endpoint());
        let remote = config("user-remote", None);
        assert_eq!(ws_reconnect_max_attempts(&local), 3);
        assert_eq!(ws_reconnect_max_attempts(&remote), 8);
    }

    #[test]
    fn ws_reconnect_backoff_local_is_flat_and_short() {
        // ~3s total to Error, so recovery kicks in within seconds of a WS drop.
        let local = config(LOCAL_DAEMON_CONNECTION_ID, unix_endpoint());
        let total: u64 = (1..=ws_reconnect_max_attempts(&local))
            .map(|attempt| ws_reconnect_backoff_secs(&local, attempt))
            .sum();
        assert_eq!(total, 3);
    }

    #[test]
    fn ws_reconnect_backoff_remote_is_exponential_capped() {
        let remote = config("user-remote", None);
        assert_eq!(ws_reconnect_backoff_secs(&remote, 1), 1);
        assert_eq!(ws_reconnect_backoff_secs(&remote, 2), 2);
        assert_eq!(ws_reconnect_backoff_secs(&remote, 3), 4);
        assert_eq!(ws_reconnect_backoff_secs(&remote, 4), 8);
        assert_eq!(ws_reconnect_backoff_secs(&remote, 5), 10);
        assert_eq!(ws_reconnect_backoff_secs(&remote, 10), 10, "capped at 10s");
    }

    #[test]
    fn ws_reconnect_total_backoff_stays_around_a_minute() {
        let remote = config("user-remote", None);
        let total: u64 = (1..=ws_reconnect_max_attempts(&remote))
            .map(|attempt| ws_reconnect_backoff_secs(&remote, attempt))
            .sum();
        assert_eq!(total, 55);
    }

    #[test]
    fn reconnect_budget_runs_out_and_reset_restores_it() {
        let mut budget = ReconnectBudget::new(2);
        assert_eq!(budget.next_attempt(), Some(1));
        assert_eq!(budget.next_attempt(), Some(2));
        assert_eq!(budget.next_attempt(), None, "budget spent");

        budget.reset();
        assert_eq!(budget.next_attempt(), Some(1), "fresh budget after reset");
    }

    #[test]
    fn liveness_probes_after_idle_then_declares_dead() {
        let t0 = Instant::now();
        let idle = Duration::from_secs(45);
        let grace = Duration::from_secs(5);
        let mut live = Liveness::new(t0, idle, grace);
        assert_eq!(live.deadline(), t0 + idle);

        let t1 = t0 + idle;
        assert_eq!(live.on_deadline(t1), LivenessAction::SendProbe);
        assert_eq!(live.deadline(), t1 + grace, "probe gets the short grace");
        assert_eq!(live.on_deadline(t1 + grace), LivenessAction::Dead);
    }

    #[test]
    fn liveness_inbound_answers_the_probe() {
        let t0 = Instant::now();
        let idle = Duration::from_secs(45);
        let grace = Duration::from_secs(5);
        let mut live = Liveness::new(t0, idle, grace);

        let t1 = t0 + idle;
        assert_eq!(live.on_deadline(t1), LivenessAction::SendProbe);
        let t2 = t1 + Duration::from_secs(1);
        live.on_inbound(t2);
        assert_eq!(live.deadline(), t2 + idle);
        assert_eq!(
            live.on_deadline(t2 + idle),
            LivenessAction::SendProbe,
            "a fresh idle period probes again instead of dying"
        );
    }

    #[test]
    fn liveness_probe_request_shortens_the_deadline_only_when_quiet() {
        let t0 = Instant::now();
        let idle = Duration::from_secs(45);
        let grace = Duration::from_secs(5);
        let quiet = Duration::from_secs(2);
        let mut live = Liveness::new(t0, idle, grace);

        // Output just arrived: no probe needed.
        assert!(!live.on_probe_request(t0 + Duration::from_secs(1), quiet));
        assert_eq!(live.deadline(), t0 + idle);

        // Back from background after 20s of silence: ping with a short deadline.
        let t1 = t0 + Duration::from_secs(20);
        assert!(live.on_probe_request(t1, quiet));
        assert_eq!(live.deadline(), t1 + grace);
        assert_eq!(live.on_deadline(t1 + grace), LivenessAction::Dead);
    }

    #[test]
    fn session_control_signals_are_seen_by_a_busy_task() {
        let control = SessionControl::new();
        let mut kick = control.kick.subscribe();
        let mut paused = control.paused.subscribe();
        assert!(!kick.has_changed().unwrap());

        // Sent while nobody is awaiting: still observed afterwards.
        control.kick();
        assert!(kick.has_changed().unwrap());
        kick.borrow_and_update();
        assert!(!kick.has_changed().unwrap());

        control.set_paused(true);
        assert!(*paused.borrow_and_update());
        control.set_paused(false);
        assert!(!*paused.borrow_and_update());
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
    }

    /// A server that accepts TCP but never answers the HTTP upgrade — the
    /// shape of a half-dead NAT mapping or a stuck proxy. The dial must give
    /// up at the deadline instead of hanging.
    #[test]
    fn ws_dial_times_out_when_the_upgrade_never_answers() {
        rt().block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let _hold = tokio::spawn(async move {
                let mut held = Vec::new();
                while let Ok((socket, _)) = listener.accept().await {
                    held.push(socket);
                }
            });
            let mut cfg = config("blackhole", None);
            cfg.port = port;

            let started = Instant::now();
            let result = dial_tcp_ws_within(
                &cfg,
                &crate::client::tls::new_observed(),
                Duration::from_millis(300),
            )
            .await;

            match result {
                Err(SessionError::Transient(msg)) => assert!(msg.contains("timed out"), "{msg}"),
                Err(SessionError::Auth(msg)) => {
                    panic!("timeout must be transient, got auth: {msg}")
                }
                Ok(_) => panic!("blackhole must not connect"),
            }
            assert!(started.elapsed() < Duration::from_secs(3));
        });
    }

    /// A non-routable address: SYNs go nowhere. Either the deadline fires or
    /// the OS rejects the route immediately — never a two-minute hang.
    #[test]
    fn ws_dial_to_a_non_routable_address_is_bounded() {
        rt().block_on(async {
            let mut cfg = config("non-routable", None);
            cfg.host = "10.255.255.1".to_string();
            cfg.port = 19199;
            let started = Instant::now();
            let result = dial_tcp_ws_within(
                &cfg,
                &crate::client::tls::new_observed(),
                Duration::from_millis(500),
            )
            .await;
            assert!(matches!(result, Err(SessionError::Transient(_))));
            assert!(started.elapsed() < Duration::from_secs(3));
        });
    }

    #[test]
    fn address_families_are_interleaved_starting_with_the_first() {
        let v6a: std::net::SocketAddr = "[2001:db8::1]:1".parse().unwrap();
        let v6b: std::net::SocketAddr = "[2001:db8::2]:1".parse().unwrap();
        let v4a: std::net::SocketAddr = "192.0.2.1:1".parse().unwrap();
        let v4b: std::net::SocketAddr = "192.0.2.2:1".parse().unwrap();
        assert_eq!(
            interleave_address_families(vec![v6a, v6b, v4a, v4b]),
            vec![v6a, v4a, v6b, v4b]
        );
        assert_eq!(
            interleave_address_families(vec![v4a, v4b, v6a]),
            vec![v4a, v6a, v4b]
        );
        assert!(interleave_address_families(Vec::new()).is_empty());
    }

    /// The first address hangs (non-routable); the racer must still connect
    /// through the second one shortly after the stagger.
    #[test]
    fn happy_eyeballs_skips_a_hanging_address() {
        rt().block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let good = listener.local_addr().unwrap();
            let _accept = tokio::spawn(async move {
                let _ = listener.accept().await;
            });
            let hanging: std::net::SocketAddr = "10.255.255.1:19199".parse().unwrap();

            let started = Instant::now();
            let stream = tokio::time::timeout(
                Duration::from_secs(5),
                happy_eyeballs_connect(vec![hanging, good], Duration::from_millis(50)),
            )
            .await
            .expect("racer must not wait for the hanging address")
            .expect("second address connects");
            assert_eq!(stream.peer_addr().unwrap(), good);
            assert!(started.elapsed() < Duration::from_secs(2));
        });
    }

    #[test]
    fn happy_eyeballs_reports_failure_when_every_address_fails() {
        rt().block_on(async {
            // Bind then drop to get a port that refuses connections.
            let closed = {
                let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                l.local_addr().unwrap()
            };
            let result =
                happy_eyeballs_connect(vec![closed, closed], Duration::from_millis(50)).await;
            assert!(result.is_err());
            assert!(
                happy_eyeballs_connect(Vec::new(), Duration::from_millis(50))
                    .await
                    .is_err()
            );
        });
    }

    /// The bearer token must not go out over a certificate this task has not
    /// pinned. `tls` without a pin is the dangerous entry: desktop UpgradeToTls
    /// persists exactly that (with the saved token) before pairing.
    #[test]
    fn a_tls_config_without_a_pin_must_adopt_before_the_token_goes_out() {
        assert!(needs_certificate_adoption(&tls_config(None), true));
    }

    #[test]
    fn a_plain_config_reaching_tls_still_adopts_on_upgrade() {
        assert!(needs_certificate_adoption(
            &config("plain-remote", None),
            true
        ));
    }

    #[test]
    fn an_already_pinned_config_does_not_re_adopt() {
        // Its probe client was built from that pin, so it is already enforced.
        assert!(!needs_certificate_adoption(
            &tls_config(Some("abc123")),
            true
        ));
    }

    #[test]
    fn plain_http_and_unix_transports_have_no_certificate_to_adopt() {
        assert!(!needs_certificate_adoption(&tls_config(None), false));
        assert!(!needs_certificate_adoption(
            &config(LOCAL_DAEMON_CONNECTION_ID, unix_endpoint()),
            true
        ));
    }

    fn tls_config(pinned: Option<&str>) -> RemoteConnectionConfig {
        let mut cfg = config("tls-remote", None);
        cfg.tls = true;
        cfg.pinned_cert_sha256 = pinned.map(str::to_string);
        cfg
    }

    fn observed_with(fingerprint: &str) -> crate::client::tls::ObservedFingerprint {
        let observed = crate::client::tls::new_observed();
        *observed.lock().unwrap() = Some(fingerprint.to_string());
        observed
    }

    /// `run_ws_loop` takes only a `SessionConfig`, and this is its constructor:
    /// the config a paired session runs on carries the pairing handshake's
    /// certificate, and the manager is told to persist that same value.
    #[test]
    fn a_session_config_carries_the_handshake_certificate() {
        let (session_config, reported) =
            SessionConfig::adopt(tls_config(None), &observed_with("abc123"));

        assert_eq!(
            session_config.get().pinned_cert_sha256.as_deref(),
            Some("abc123"),
            "the WS connector, state HTTP call and reconnects all read this pin"
        );
        assert_eq!(
            reported.as_deref(),
            session_config.get().pinned_cert_sha256.as_deref(),
            "the fingerprint reported to the manager is the one the session enforces"
        );
        assert_eq!(
            session_config.into_inner().pinned_cert_sha256.as_deref(),
            Some("abc123"),
            "and it is still there when run_ws_loop unwraps it"
        );
    }

    /// Pairing over plain http, and the trusted Unix-socket transport, run no
    /// pinning verifier — there is nothing to adopt and nothing to report.
    #[test]
    fn a_session_config_without_a_handshake_has_no_pin() {
        let (session_config, reported) =
            SessionConfig::adopt(tls_config(None), &crate::client::tls::new_observed());

        assert_eq!(reported, None);
        assert_eq!(session_config.get().pinned_cert_sha256, None);
    }

    /// A plain-http connect first probes TLS, so the slot can hold a certificate
    /// from an attempt that was abandoned. A plain session must not carry it.
    #[test]
    fn a_plain_session_ignores_an_abandoned_tls_probe() {
        let (session_config, reported) =
            SessionConfig::adopt(config("plain-remote", None), &observed_with("abc123"));

        assert_eq!(reported, None);
        assert_eq!(session_config.get().pinned_cert_sha256, None);
    }

    #[test]
    fn observed_pin_is_adopted_into_the_task_config() {
        let mut cfg = tls_config(None);
        let adopted = adopt_observed_pin(&mut cfg, &observed_with("abc123"));

        assert_eq!(cfg.pinned_cert_sha256.as_deref(), Some("abc123"));
        assert_eq!(
            adopted.as_deref(),
            cfg.pinned_cert_sha256.as_deref(),
            "the reported fingerprint is the one the task pins"
        );
    }

    /// Plain http and Unix-socket transports never run the pinning verifier, so
    /// there is nothing to adopt — and an already-pinned config must keep its pin.
    #[test]
    fn nothing_observed_leaves_the_existing_pin_alone() {
        let mut cfg = tls_config(Some("previously-pinned"));
        let adopted = adopt_observed_pin(&mut cfg, &crate::client::tls::new_observed());

        assert_eq!(adopted, None);
        assert_eq!(cfg.pinned_cert_sha256.as_deref(), Some("previously-pinned"));
    }

    /// Adoption is idempotent: a second pass over the empty slot that
    /// `ws_session` allocates per attempt must not clear an adopted pin.
    #[test]
    fn re_adopting_from_an_empty_slot_keeps_the_pin() {
        let mut cfg = tls_config(None);
        adopt_observed_pin(&mut cfg, &observed_with("abc123"));
        adopt_observed_pin(&mut cfg, &crate::client::tls::new_observed());

        assert_eq!(cfg.pinned_cert_sha256.as_deref(), Some("abc123"));
    }

    #[test]
    fn ws_message_channel_absorbs_input_bursts_without_dropping() {
        let (tx, rx) = ws_message_channel();
        for byte in 0..=u8::MAX {
            tx.try_send(WsClientMessage::SendInput {
                terminal_id: "terminal".to_string(),
                data: vec![byte],
            })
            .unwrap();
        }
        assert_eq!(rx.len(), usize::from(u8::MAX) + 1);
    }

    #[test]
    fn explicit_reconnect_retains_last_state_and_terminal_objects() {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let handler = Arc::new(TestHandler::default());
        let (event_tx, _event_rx) = async_channel::unbounded();
        let mut client = RemoteClient::new(
            config("reconnect", None),
            runtime,
            handler.clone(),
            event_tx,
        );
        client.set_remote_state(Some(StateResponse {
            state_version: 1,
            projects: Vec::new(),
            focused_project_id: None,
            fullscreen_terminal: None,
            project_order: Vec::new(),
            folders: Vec::new(),
            windows: Vec::new(),
            hooks: Vec::new(),
            claude_dialogs: Default::default(),
            claude_sessions: Vec::new(),
        }));

        client.reconnect();

        assert!(client.remote_state().is_some());
        assert_eq!(handler.remove_all_calls.load(Ordering::Relaxed), 0);
    }
}
