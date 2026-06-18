//! Global HTTP bus: the single choke point for *outbound external* HTTP
//! requests (Anthropic, OpenAI, GitHub, status pages, …).
//!
//! Why this exists: the same reason as the command bus ([`super::process`]) —
//! one place every external call flows through, so the whole app's network I/O
//! is *auditable* (one log target, [`okena::http`](crate::http)), consistent
//! (one pooled client, one default user-agent, per-request timeouts) and
//! *mockable* in tests without touching the network.
//!
//! Scope: this governs okena's calls out to *third-party* services. It
//! deliberately does **not** carry the remote-control protocol (okena talking
//! to its own remote server) — that lives in [`crate::client`] /
//! [`crate::remote_action`] and is a separate, internal transport.
//!
//! Design notes:
//! - Blocking, like the command bus: callers block on [`HttpBus::send`]
//!   (typically inside `smol::unblock`, exactly where they previously built a
//!   `reqwest::blocking::Client`). No concurrency lanes — HTTP doesn't have the
//!   process-global FD-composition problem that motivated the command bus's
//!   caps; the value here is the single auditable seam, not throttling.
//! - One pooled [`reqwest::blocking::Client`] shared across every call, so TLS
//!   sessions and connections are reused.
//! - [`stream`](HttpBus::stream) returns a [`HttpStream`] (an opaque
//!   [`Read`](std::io::Read)) for large transfers (the updater's asset
//!   download) without buffering the whole body or leaking `reqwest` types.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Default total-request timeout when a request doesn't set its own.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// HTTP method. Only the verbs okena actually issues externally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
        }
    }
}

/// Request body.
#[derive(Debug, Clone)]
enum Body {
    Empty,
    /// JSON-serialized value (sets `Content-Type: application/json`).
    Json(serde_json::Value),
    /// Raw bytes with an explicit content type (e.g. form-urlencoded).
    Raw {
        content_type: String,
        bytes: Vec<u8>,
    },
}

/// A fully-described outbound request. Build it, then hand it to
/// [`send`](HttpBus::send) (buffered response) or [`stream`](HttpBus::stream)
/// (streaming response).
#[derive(Debug, Clone)]
pub struct HttpRequest {
    method: Method,
    url: String,
    headers: Vec<(String, String)>,
    body: Body,
    timeout: Option<Duration>,
    user_agent: Option<String>,
    /// Stable short label for the audit log (e.g. `"claude.usage"`). Falls back
    /// to the method when `None`. Also the key for [`min_interval`](Self::min_interval)
    /// throttling.
    label: Option<&'static str>,
    /// Client-side floor on how often this call site may hit the network. See
    /// [`min_interval`](Self::min_interval).
    min_interval: Option<Duration>,
}

impl HttpRequest {
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: Body::Empty,
            timeout: None,
            user_agent: None,
            label: None,
            min_interval: None,
        }
    }

    pub fn get(url: impl Into<String>) -> Self {
        Self::new(Method::Get, url)
    }

    pub fn post(url: impl Into<String>) -> Self {
        Self::new(Method::Post, url)
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Add an `Authorization: Bearer <token>` header.
    pub fn bearer(self, token: impl AsRef<str>) -> Self {
        self.header("Authorization", format!("Bearer {}", token.as_ref()))
    }

    /// JSON request body.
    pub fn json(mut self, value: &serde_json::Value) -> Self {
        self.body = Body::Json(value.clone());
        self
    }

    /// Raw string body with an explicit content type (e.g. a form-urlencoded
    /// payload).
    pub fn body(mut self, content_type: impl Into<String>, body: impl Into<String>) -> Self {
        self.body = Body::Raw {
            content_type: content_type.into(),
            bytes: body.into().into_bytes(),
        };
        self
    }

    /// Total-request timeout (defaults to [`DEFAULT_TIMEOUT`]).
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Override the user-agent for this request (defaults to the bus client's
    /// `okena/<version>`). The updater uses this to send the host app version.
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = Some(ua.into());
        self
    }

    pub fn label(mut self, label: &'static str) -> Self {
        self.label = Some(label);
        self
    }

    /// Client-side rate floor: the bus admits this call site to the network at
    /// most once per `interval`, keyed by [`label`](Self::label) (or the URL if
    /// unset). A call arriving sooner is short-circuited with
    /// [`HttpError::Throttled`] — *no* network request — so a runaway caller
    /// (e.g. a GPUI view that re-spawns its poll every redraw) can't hammer an
    /// endpoint 100×/s regardless of its own logic. Opt-in: unset = no floor.
    ///
    /// This is a *floor*, not a scheduler: set it well below the real cadence
    /// (e.g. 5s for a 60s poller) so it only ever catches a genuine runaway and
    /// never rejects a legitimate retry.
    pub fn min_interval(mut self, interval: Duration) -> Self {
        self.min_interval = Some(interval);
        self
    }

    /// Throttle bucket key: the stable label, or the URL when no label is set.
    fn throttle_key(&self) -> String {
        self.label
            .map(str::to_string)
            .unwrap_or_else(|| self.url.clone())
    }

    /// `method url` (+ label) for the audit log.
    fn audit_detail(&self) -> String {
        match self.label {
            Some(label) => format!("{label}: {} {}", self.method.as_str(), self.url),
            None => format!("{} {}", self.method.as_str(), self.url),
        }
    }

    /// Apply this request onto a reqwest builder (shared by send + stream).
    fn build(&self, client: &reqwest::blocking::Client) -> reqwest::blocking::RequestBuilder {
        let method = match self.method {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
        };
        let mut builder = client.request(method, &self.url);
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        if let Some(ua) = &self.user_agent {
            builder = builder.header(reqwest::header::USER_AGENT, ua);
        }
        builder = builder.timeout(self.timeout.unwrap_or(DEFAULT_TIMEOUT));
        match &self.body {
            Body::Empty => {}
            Body::Json(value) => builder = builder.json(value),
            Body::Raw {
                content_type,
                bytes,
            } => {
                builder = builder
                    .header(reqwest::header::CONTENT_TYPE, content_type)
                    .body(bytes.clone());
            }
        }
        builder
    }
}

/// What went wrong issuing a request or reading its response.
#[derive(Debug)]
pub enum HttpError {
    /// Network, TLS, timeout, or connection failure.
    Transport(String),
    /// Response body could not be decoded as the requested type.
    Decode(String),
    /// A non-success status when one was required (see
    /// [`HttpResponse::error_for_status`] / [`HttpStream::error_for_status`]).
    Status(u16),
    /// Short-circuited by the bus's client-side rate floor
    /// ([`HttpRequest::min_interval`]) — the call site asked for the network
    /// too soon after its previous admitted call. `retry_in` is how long until
    /// it would be admitted again. No network request was made.
    Throttled { retry_in: Duration },
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Transport(e) => write!(f, "transport error: {e}"),
            HttpError::Decode(e) => write!(f, "decode error: {e}"),
            HttpError::Status(code) => write!(f, "unexpected HTTP status {code}"),
            HttpError::Throttled { retry_in } => {
                write!(f, "rate floor: retry in {}ms", retry_in.as_millis())
            }
        }
    }
}

impl std::error::Error for HttpError {}

/// A fully-buffered response: status, headers and the complete body.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    /// Construct a response directly — used by [`testing::mock`] and tests.
    pub fn new(status: u16, headers: Vec<(String, String)>, body: Vec<u8>) -> Self {
        Self {
            status,
            headers,
            body,
        }
    }

    pub fn status(&self) -> u16 {
        self.status
    }

    /// `true` for a 2xx status.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Return `self` only if the status is 2xx, else [`HttpError::Status`].
    pub fn error_for_status(self) -> Result<Self, HttpError> {
        if self.is_success() {
            Ok(self)
        } else {
            Err(HttpError::Status(self.status))
        }
    }

    /// Case-insensitive header lookup.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn bytes(&self) -> &[u8] {
        &self.body
    }

    /// Decode the body as UTF-8 (lossily).
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// Decode the body as JSON.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, HttpError> {
        serde_json::from_slice(&self.body).map_err(|e| HttpError::Decode(e.to_string()))
    }
}

/// A streaming response. Implements [`Read`](std::io::Read) so the body can be
/// consumed incrementally (the updater downloads multi-MB assets this way),
/// without buffering it all or exposing `reqwest` to callers.
pub struct HttpStream {
    status: u16,
    content_length: Option<u64>,
    inner: reqwest::blocking::Response,
}

impl HttpStream {
    pub fn status(&self) -> u16 {
        self.status
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn content_length(&self) -> Option<u64> {
        self.content_length
    }

    /// Return `self` only if the status is 2xx, else [`HttpError::Status`].
    pub fn error_for_status(self) -> Result<Self, HttpError> {
        if self.is_success() {
            Ok(self)
        } else {
            Err(HttpError::Status(self.status))
        }
    }
}

impl std::io::Read for HttpStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

/// Optional test interceptor: when installed, [`HttpBus::send`] returns its
/// result instead of touching the network. See [`testing`].
type MockFn = Box<dyn Fn(&HttpRequest) -> Result<HttpResponse, HttpError> + Send + Sync>;

/// The process-global HTTP bus. Holds the one shared client and the test mock.
pub struct HttpBus {
    client: reqwest::blocking::Client,
    mock: Mutex<Option<MockFn>>,
    /// Last network-admission instant per throttle key (see
    /// [`HttpRequest::min_interval`]). Bounded by the number of distinct
    /// labels, so it never needs pruning.
    throttle: Mutex<HashMap<String, Instant>>,
}

static BUS: OnceLock<HttpBus> = OnceLock::new();

impl HttpBus {
    /// The process-global bus, lazily built on first use.
    pub fn global() -> &'static HttpBus {
        BUS.get_or_init(HttpBus::start)
    }

    fn start() -> HttpBus {
        // connect_timeout keeps a dead host from hanging the whole per-request
        // timeout budget; the per-request total timeout is applied per call.
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("okena/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|e| {
                log::error!(target: "okena::http", "failed to build shared client: {e}; using default");
                reqwest::blocking::Client::new()
            });
        HttpBus {
            client,
            mock: Mutex::new(None),
            throttle: Mutex::new(HashMap::new()),
        }
    }

    /// Enforce the request's client-side rate floor. Returns `Err(Throttled)`
    /// without admitting the call if it arrives sooner than `min_interval`
    /// after the previous admitted call for the same key; otherwise records
    /// this admission and returns `Ok`.
    fn check_throttle(&self, req: &HttpRequest) -> Result<(), HttpError> {
        let Some(min) = req.min_interval else {
            return Ok(());
        };
        let key = req.throttle_key();
        let now = Instant::now();
        let mut map = self.throttle.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&last) = map.get(&key) {
            let since = now.duration_since(last);
            if since < min {
                return Err(HttpError::Throttled {
                    retry_in: min - since,
                });
            }
        }
        map.insert(key, now);
        Ok(())
    }

    /// Issue a request and buffer the full response. Blocks until complete.
    /// Every call is logged under the `okena::http` target.
    pub fn send(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        let detail = req.audit_detail();

        // Client-side rate floor: drop a runaway caller before any I/O (and
        // before the mock, so the floor is consistent in tests too).
        if let Err(e @ HttpError::Throttled { .. }) = self.check_throttle(&req) {
            log::debug!(target: "okena::http", "throttled {detail}: {e}");
            return Err(e);
        }

        // Test fast-path: resolve against the installed mock without any I/O.
        if let Ok(guard) = self.mock.lock()
            && let Some(mock) = guard.as_ref()
        {
            return mock(&req);
        }

        let started = Instant::now();
        log::trace!(target: "okena::http", "start {detail}");

        let result = self.send_inner(&req);
        let elapsed = started.elapsed().as_millis();
        match &result {
            Ok(resp) => log::debug!(
                target: "okena::http",
                "{detail} -> {} ({elapsed}ms)", resp.status
            ),
            Err(e) => log::warn!(
                target: "okena::http",
                "{detail} failed: {e} ({elapsed}ms)"
            ),
        }
        result
    }

    fn send_inner(&self, req: &HttpRequest) -> Result<HttpResponse, HttpError> {
        let resp = req
            .build(&self.client)
            .send()
            .map_err(|e| HttpError::Transport(e.to_string()))?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let body = resp
            .bytes()
            .map_err(|e| HttpError::Transport(e.to_string()))?
            .to_vec();
        Ok(HttpResponse::new(status, headers, body))
    }

    /// Issue a request and return a streaming response without buffering the
    /// body. Logs the request + resulting status under `okena::http`; the body
    /// transfer itself happens as the caller reads.
    pub fn stream(&self, req: HttpRequest) -> Result<HttpStream, HttpError> {
        let detail = req.audit_detail();

        if let Err(e @ HttpError::Throttled { .. }) = self.check_throttle(&req) {
            log::debug!(target: "okena::http", "throttled stream {detail}: {e}");
            return Err(e);
        }

        let started = Instant::now();
        log::trace!(target: "okena::http", "stream start {detail}");

        let result = req
            .build(&self.client)
            .send()
            .map_err(|e| HttpError::Transport(e.to_string()));
        let elapsed = started.elapsed().as_millis();
        match &result {
            Ok(resp) => {
                let status = resp.status().as_u16();
                log::debug!(target: "okena::http", "stream {detail} -> {status} ({elapsed}ms)");
            }
            Err(e) => log::warn!(target: "okena::http", "stream {detail} failed: {e} ({elapsed}ms)"),
        }
        let resp = result?;
        Ok(HttpStream {
            status: resp.status().as_u16(),
            content_length: resp.content_length(),
            inner: resp,
        })
    }

    /// Install a test interceptor. See [`testing::mock`].
    #[cfg(any(test, feature = "test-support"))]
    fn set_mock(&self, mock: Option<MockFn>) {
        if let Ok(mut guard) = self.mock.lock() {
            *guard = mock;
        }
    }
}

/// Issue a request and buffer the response, via the global bus.
pub fn send(req: HttpRequest) -> Result<HttpResponse, HttpError> {
    HttpBus::global().send(req)
}

/// Issue a request and stream the response, via the global bus.
pub fn stream(req: HttpRequest) -> Result<HttpStream, HttpError> {
    HttpBus::global().stream(req)
}

/// Test helpers for intercepting bus requests without touching the network.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use super::*;

    /// Guard that restores real execution when dropped.
    pub struct MockGuard;

    impl Drop for MockGuard {
        fn drop(&mut self) {
            HttpBus::global().set_mock(None);
        }
    }

    /// Replace real HTTP execution with `f` until the returned guard drops.
    /// Only affects [`send`]; [`stream`] still hits the network.
    pub fn mock(
        f: impl Fn(&HttpRequest) -> Result<HttpResponse, HttpError> + Send + Sync + 'static,
    ) -> MockGuard {
        HttpBus::global().set_mock(Some(Box::new(f)));
        MockGuard
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // The bus's mock slot is process-global, so these tests must not run
    // concurrently (one test's mock would intercept another's requests).
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn builds_request_fields() {
        let req = HttpRequest::post("https://example.test/x")
            .bearer("tok")
            .header("x-extra", "1")
            .json(&serde_json::json!({"a": 1}))
            .label("test.post");
        assert_eq!(req.method, Method::Post);
        assert_eq!(req.audit_detail(), "test.post: POST https://example.test/x");
        assert!(req
            .headers
            .iter()
            .any(|(k, v)| k == "Authorization" && v == "Bearer tok"));
    }

    #[test]
    fn mock_intercepts_send() {
        let _g = guard();
        let _mock = testing::mock(|req| {
            assert_eq!(req.url, "https://example.test/usage");
            Ok(HttpResponse::new(
                200,
                vec![("content-type".into(), "application/json".into())],
                br#"{"ok":true}"#.to_vec(),
            ))
        });
        let resp = send(HttpRequest::get("https://example.test/usage")).expect("mocked");
        assert!(resp.is_success());
        assert_eq!(resp.header("Content-Type"), Some("application/json"));
        let v: serde_json::Value = resp.json().expect("json");
        assert_eq!(v["ok"], serde_json::json!(true));
    }

    #[test]
    fn min_interval_throttles_runaway_caller() {
        let _g = guard();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let calls_in_mock = calls.clone();
        let _mock = testing::mock(move |_req| {
            calls_in_mock.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(HttpResponse::new(200, vec![], Vec::new()))
        });

        let make = || {
            HttpRequest::get("https://example.test/poll")
                .label("test.throttle")
                .min_interval(Duration::from_secs(60))
        };

        // First call admitted, the next two (immediate) are short-circuited
        // before ever reaching the mock.
        assert!(send(make()).is_ok());
        assert!(matches!(
            send(make()),
            Err(HttpError::Throttled { .. })
        ));
        assert!(matches!(
            send(make()),
            Err(HttpError::Throttled { .. })
        ));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn distinct_labels_throttle_independently() {
        let _g = guard();
        let _mock = testing::mock(|_req| Ok(HttpResponse::new(200, vec![], Vec::new())));
        let req = |label| {
            HttpRequest::get("https://example.test/x")
                .label(label)
                .min_interval(Duration::from_secs(60))
        };
        assert!(send(req("test.a")).is_ok());
        // Different label → its own bucket, so still admitted.
        assert!(send(req("test.b")).is_ok());
    }

    #[test]
    fn error_for_status_rejects_non_2xx() {
        let resp = HttpResponse::new(429, vec![], Vec::new());
        assert!(matches!(
            resp.error_for_status(),
            Err(HttpError::Status(429))
        ));
    }
}
