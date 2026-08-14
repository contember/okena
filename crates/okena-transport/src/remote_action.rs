//! Shared async and blocking clients for actions sent to an Okena daemon.

use crate::RemoteConnectionConfig;
use okena_core::api::{ActionRequest, FileDownloadRequest};
#[cfg(feature = "blocking-http")]
use std::sync::{Arc, OnceLock};

#[cfg(feature = "cancellable-http")]
const CANCELLATION_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

/// Total request timeout for "fast" actions (terminal control, listings,
/// metadata). 10 s is generous for these; longer would mask real failures.
const FAST_TIMEOUT_SECS: u64 = 10;

/// Total request timeout for byte-payload reads (ReadFileBytes). A 20 MB
/// image base64-encodes to ~27 MB; over a 5 Mbit/s link that's ~45 s on the
/// wire alone, which would time out the fast client with no useful signal.
const BYTES_TIMEOUT_SECS: u64 = 90;

/// Total request timeout for repository content searches. Unlike metadata
/// actions, these may need to walk and inspect an entire large checkout.
const SEARCH_TIMEOUT_SECS: u64 = 90;

/// Total request timeout for synchronous filesystem mutations. Direct
/// worktree removal may run two sequential five-minute close hooks before Git.
const LONG_MUTATION_TIMEOUT_SECS: u64 = 11 * 60;

const ACTIONS_PATH: &str = "/v1/actions";
const DOWNLOAD_PATH: &str = "/v1/files/download";

/// Hard ceiling on response body size accepted by the remote bridge. Cuts
/// off arbitrarily large or runaway responses before they're buffered into
/// memory (peak resident is ~4× the file size while the base64 + JSON +
/// decoded Vec all co-exist). Mirrors the server-side cap in
/// `src/workspace/actions/execute/files.rs`.
const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActionClientKind {
    Fast,
    Bytes,
    Search,
    LongMutation,
}

fn client_kind_for(action: &ActionRequest) -> ActionClientKind {
    match action {
        ActionRequest::ReadFileBytes { .. }
        | ActionRequest::ReadTerminalFileBytes { .. }
        | ActionRequest::ReadPathFileBytes { .. } => ActionClientKind::Bytes,
        ActionRequest::SearchContent { .. } | ActionRequest::SearchPathContent { .. } => {
            ActionClientKind::Search
        }
        ActionRequest::RemoveWorktreeProject { .. }
        | ActionRequest::ForceRemoveWorktreeProject { .. }
        | ActionRequest::RenameProjectDirectory { .. } => ActionClientKind::LongMutation,
        _ => ActionClientKind::Fast,
    }
}

fn timeout_for(action: &ActionRequest) -> u64 {
    match client_kind_for(action) {
        ActionClientKind::Fast => FAST_TIMEOUT_SECS,
        ActionClientKind::Bytes => BYTES_TIMEOUT_SECS,
        ActionClientKind::Search => SEARCH_TIMEOUT_SECS,
        ActionClientKind::LongMutation => LONG_MUTATION_TIMEOUT_SECS,
    }
}

fn action_url(base_url: &str) -> String {
    format!("{base_url}{ACTIONS_PATH}")
}

#[cfg(feature = "blocking-http")]
type ClientAndUrl = (reqwest::blocking::Client, String);

#[cfg(feature = "cancellable-http")]
type AsyncClientAndUrl = (reqwest::Client, String);

#[cfg(feature = "blocking-http")]
struct RemoteActionClientInner {
    config: RemoteConnectionConfig,
    token: String,
    fast: OnceLock<Result<ClientAndUrl, String>>,
    bytes: OnceLock<Result<ClientAndUrl, String>>,
    search: OnceLock<Result<ClientAndUrl, String>>,
    long_mutation: OnceLock<Result<ClientAndUrl, String>>,
    download: OnceLock<Result<ClientAndUrl, String>>,
    #[cfg(feature = "cancellable-http")]
    async_search: OnceLock<Result<AsyncClientAndUrl, String>>,
}

/// Cloneable blocking action client backed by one complete connection config.
/// Every value-returning provider uses this type so local sockets, TLS scheme,
/// certificate pinning, auth, and timeouts cannot diverge between features.
#[derive(Clone)]
#[cfg(feature = "blocking-http")]
pub struct RemoteActionClient {
    inner: Arc<RemoteActionClientInner>,
}

#[cfg(feature = "blocking-http")]
impl RemoteActionClient {
    pub fn new(config: RemoteConnectionConfig, token: String) -> Self {
        Self {
            inner: Arc::new(RemoteActionClientInner {
                config,
                token,
                fast: OnceLock::new(),
                bytes: OnceLock::new(),
                search: OnceLock::new(),
                long_mutation: OnceLock::new(),
                download: OnceLock::new(),
                #[cfg(feature = "cancellable-http")]
                async_search: OnceLock::new(),
            }),
        }
    }

    /// Post an action and return its optional JSON payload.
    pub fn post_action(&self, action: ActionRequest) -> Result<Option<serde_json::Value>, String> {
        let transport = match client_kind_for(&action) {
            ActionClientKind::Fast => &self.inner.fast,
            ActionClientKind::Bytes => &self.inner.bytes,
            ActionClientKind::Search => &self.inner.search,
            ActionClientKind::LongMutation => &self.inner.long_mutation,
        };
        let timeout = std::time::Duration::from_secs(timeout_for(&action));
        let client_and_url = transport.get_or_init(|| {
            crate::remote_http::blocking_client_and_url(&self.inner.config, ACTIONS_PATH, timeout)
        });
        let (client, url) = match client_and_url {
            Ok(client_and_url) => client_and_url,
            Err(error) => return Err(error.clone()),
        };
        post_action_inner(client, url, &self.inner.token, action)
    }

    pub fn connection_id(&self) -> &str {
        &self.inner.config.id
    }

    pub fn connection_name(&self) -> &str {
        &self.inner.config.name
    }

    pub fn is_local_daemon(&self) -> bool {
        self.inner.config.id == crate::LOCAL_DAEMON_CONNECTION_ID
    }

    /// Stream a daemon-side file directly into a local writer.
    pub fn download_file(
        &self,
        request: &FileDownloadRequest,
        writer: &mut dyn std::io::Write,
    ) -> Result<(), String> {
        let client_and_url = self.inner.download.get_or_init(|| {
            crate::remote_http::blocking_client_and_url(
                &self.inner.config,
                DOWNLOAD_PATH,
                std::time::Duration::from_secs(30 * 60),
            )
        });
        let (client, url) = match client_and_url {
            Ok(client_and_url) => client_and_url,
            Err(error) => return Err(error.clone()),
        };
        let mut response = client
            .post(url)
            .bearer_auth(&self.inner.token)
            .json(request)
            .send()
            .map_err(|error| format!("Download request failed: {error}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let message = response
                .text()
                .unwrap_or_else(|_| "download failed".to_string());
            return Err(format!("Download failed ({status}): {message}"));
        }
        std::io::copy(&mut response, writer)
            .map(|_| ())
            .map_err(|error| format!("Cannot save downloaded file: {error}"))
    }

    /// Post a content search while observing the request-local cancellation flag.
    ///
    /// The HTTP future runs on a shared async runtime, while this blocking API
    /// waits through a bounded channel so synchronous filesystem providers can
    /// still cancel an in-flight request. Dropping the future closes the HTTP
    /// response and, in turn, the daemon bridge reply receiver.
    #[cfg(feature = "cancellable-http")]
    pub fn post_action_cancellable(
        &self,
        action: ActionRequest,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<Option<serde_json::Value>, String> {
        if !matches!(action, ActionRequest::SearchContent { .. }) {
            return Err("cancellable remote actions only support content search".to_string());
        }
        if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("content search cancelled".to_string());
        }

        let runtime = cancellable_runtime()?;
        let client_and_url = self
            .inner
            .async_search
            .get_or_init(|| crate::remote_http::async_client_and_url(&self.inner.config, ""));
        let (client, url) = match client_and_url {
            Ok((client, url)) => (client.clone(), url.clone()),
            Err(error) => return Err(error.clone()),
        };
        let token = self.inner.token.clone();
        let timeout = std::time::Duration::from_secs(timeout_for(&action));
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
        let mut cancellation = RequestCancellation::new(cancel_tx);

        runtime.spawn(async move {
            let request = post_action_async_with_client(&client, &url, &token, action);
            let result = await_cancellable_request(request, cancel_rx, timeout).await;
            let _ = result_tx.send(result);
        });

        loop {
            match result_rx.recv_timeout(CANCELLATION_POLL_INTERVAL) {
                Ok(result) => {
                    cancellation.disarm();
                    return result;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                        cancellation.cancel();
                        return result_rx
                            .recv_timeout(std::time::Duration::from_secs(1))
                            .unwrap_or_else(|_| {
                                Err("content search cancellation failed".to_string())
                            });
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    cancellation.disarm();
                    return Err("content search request task stopped".to_string());
                }
            }
        }
    }
}

#[cfg(feature = "cancellable-http")]
fn cancellable_runtime() -> Result<&'static tokio::runtime::Handle, String> {
    static RUNTIME: OnceLock<Result<tokio::runtime::Runtime, String>> = OnceLock::new();
    match RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("okena-remote-search")
            .build()
            .map_err(|error| format!("Cannot initialise content search runtime: {error}"))
    }) {
        Ok(runtime) => Ok(runtime.handle()),
        Err(error) => Err(error.clone()),
    }
}

#[cfg(feature = "cancellable-http")]
struct RequestCancellation(Option<tokio::sync::oneshot::Sender<()>>);

#[cfg(feature = "cancellable-http")]
impl RequestCancellation {
    fn new(sender: tokio::sync::oneshot::Sender<()>) -> Self {
        Self(Some(sender))
    }

    fn cancel(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }

    fn disarm(&mut self) {
        self.0 = None;
    }
}

#[cfg(feature = "cancellable-http")]
impl Drop for RequestCancellation {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(feature = "cancellable-http")]
async fn await_cancellable_request<F, T>(
    request: F,
    cancel_rx: tokio::sync::oneshot::Receiver<()>,
    timeout: std::time::Duration,
) -> Result<T, String>
where
    F: std::future::Future<Output = Result<T, String>>,
{
    tokio::select! {
        result = request => result,
        _ = cancel_rx => Err("content search cancelled".to_string()),
        _ = tokio::time::sleep(timeout) => Err("content search request timed out".to_string()),
    }
}

/// Post an action asynchronously using the same connection-aware transport and
/// response contract as [`RemoteActionClient`].
#[cfg(feature = "client")]
pub async fn post_action_async(
    config: &RemoteConnectionConfig,
    token: &str,
    action: ActionRequest,
) -> Result<Option<serde_json::Value>, String> {
    let (client, base_url) = crate::remote_http::async_client_and_url(config, "")?;
    post_action_async_with_client(&client, &base_url, token, action).await
}

/// Post through an already-selected async client. Connection setup uses this
/// while it is still negotiating the final config, but keeps action response
/// parsing and size limits on the same path as every other caller.
#[cfg(any(feature = "client", feature = "cancellable-http"))]
pub async fn post_action_async_with_client(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    action: ActionRequest,
) -> Result<Option<serde_json::Value>, String> {
    let url = action_url(base_url);
    let mut response = client
        .post(url)
        .bearer_auth(token)
        .json(&action)
        .timeout(std::time::Duration::from_secs(timeout_for(&action)))
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {e}"))?;

    reject_declared_oversize(response.content_length())?;
    let status = response.status();
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("Failed to read response: {e}"))?
    {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES as usize {
            return Err(response_too_large_error());
        }
        body.extend_from_slice(&chunk);
    }
    parse_action_response(status, &body)
}

#[cfg(feature = "blocking-http")]
fn post_action_inner(
    client: &reqwest::blocking::Client,
    url: &str,
    token: &str,
    action: ActionRequest,
) -> Result<Option<serde_json::Value>, String> {
    let resp = client
        .post(url)
        .bearer_auth(token)
        .json(&action)
        .send()
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    reject_declared_oversize(resp.content_length())?;
    let status = resp.status();
    use std::io::Read as _;
    let mut body_bytes = Vec::new();
    resp.take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut body_bytes)
        .map_err(|e| format!("Failed to read response: {}", e))?;
    if body_bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(response_too_large_error());
    }
    parse_action_response(status, &body_bytes)
}

fn reject_declared_oversize(content_length: Option<u64>) -> Result<(), String> {
    if let Some(len) = content_length
        && len > MAX_RESPONSE_BYTES
    {
        return Err(format!(
            "Response too large ({:.1} MB). Max {} MB.",
            len as f64 / 1024.0 / 1024.0,
            MAX_RESPONSE_BYTES / 1024 / 1024
        ));
    }
    Ok(())
}

fn response_too_large_error() -> String {
    format!(
        "Response too large (>{} MB).",
        MAX_RESPONSE_BYTES / 1024 / 1024
    )
}

fn parse_action_response(
    status: reqwest::StatusCode,
    body_bytes: &[u8],
) -> Result<Option<serde_json::Value>, String> {
    if !status.is_success() {
        return Err(format!(
            "Server returned {status}: {}",
            String::from_utf8_lossy(body_bytes)
        ));
    }
    let body: serde_json::Value = serde_json::from_slice(body_bytes)
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    if let Some(error) = body.get("error").and_then(|e| e.as_str()) {
        return Err(error.to_string());
    }

    // Server returns {"ok": true} for void (None-payload) actions.
    if body.get("ok").is_some() {
        return Ok(None);
    }

    Ok(Some(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "cancellable-http")]
    use std::sync::atomic::{AtomicBool, Ordering};

    fn search_action() -> ActionRequest {
        ActionRequest::SearchContent {
            project_id: "project".to_string(),
            query: "needle".to_string(),
            case_sensitive: false,
            mode: "literal".to_string(),
            max_results: 1000,
            file_glob: None,
            context_lines: 0,
            show_ignored: false,
        }
    }

    fn remove_worktree_action() -> ActionRequest {
        ActionRequest::RemoveWorktreeProject {
            project_id: "project".to_string(),
            force: false,
        }
    }

    fn rename_project_directory_action() -> ActionRequest {
        ActionRequest::RenameProjectDirectory {
            project_id: "project".to_string(),
            new_name: "renamed".to_string(),
        }
    }

    fn ordinary_fast_action() -> ActionRequest {
        ActionRequest::ReadFile {
            project_id: "project".to_string(),
            relative_path: "README.md".to_string(),
        }
    }

    #[cfg(feature = "cancellable-http")]
    fn remote_config(port: u16) -> RemoteConnectionConfig {
        RemoteConnectionConfig {
            id: "test".to_string(),
            name: "Test".to_string(),
            host: "127.0.0.1".to_string(),
            port,
            saved_token: None,
            token_obtained_at: None,
            tls: false,
            pinned_cert_sha256: None,
            local_endpoint: None,
        }
    }

    #[test]
    fn content_search_uses_long_timeout() {
        assert_eq!(timeout_for(&search_action()), SEARCH_TIMEOUT_SECS);
        assert_eq!(SEARCH_TIMEOUT_SECS, 90);
    }

    #[test]
    fn synchronous_long_mutations_use_dedicated_clients() {
        for action in [remove_worktree_action(), rename_project_directory_action()] {
            assert_eq!(client_kind_for(&action), ActionClientKind::LongMutation);
            assert_eq!(timeout_for(&action), LONG_MUTATION_TIMEOUT_SECS);
        }
        assert_eq!(LONG_MUTATION_TIMEOUT_SECS, 660);
    }

    #[test]
    fn ordinary_actions_keep_the_fast_timeout() {
        let action = ordinary_fast_action();
        assert_eq!(client_kind_for(&action), ActionClientKind::Fast);
        assert_eq!(timeout_for(&action), FAST_TIMEOUT_SECS);
        assert_eq!(FAST_TIMEOUT_SECS, 10);
    }

    #[test]
    fn action_posts_use_the_canonical_route() {
        assert_eq!(ACTIONS_PATH, "/v1/actions");
        assert_eq!(
            action_url("https://okena.example"),
            "https://okena.example/v1/actions"
        );
    }

    #[test]
    fn content_search_has_a_dedicated_cached_client() {
        assert_eq!(client_kind_for(&search_action()), ActionClientKind::Search);
        assert_eq!(
            client_kind_for(&ordinary_fast_action()),
            ActionClientKind::Fast
        );
        assert_eq!(
            client_kind_for(&ActionRequest::ReadFileBytes {
                project_id: "project".to_string(),
                relative_path: "image.png".to_string(),
            }),
            ActionClientKind::Bytes
        );
        assert_eq!(
            client_kind_for(&ActionRequest::ReadTerminalFileBytes {
                terminal_id: "terminal-1".into(),
                path: "/tmp/image.png".into(),
            }),
            ActionClientKind::Bytes
        );
        assert_eq!(
            client_kind_for(&ActionRequest::ReadPathFileBytes {
                root: "/tmp".into(),
                relative_path: "image.png".into(),
            }),
            ActionClientKind::Bytes
        );
    }

    #[cfg(feature = "cancellable-http")]
    struct PendingRequest {
        dropped: Arc<AtomicBool>,
    }

    #[cfg(feature = "cancellable-http")]
    impl std::future::Future for PendingRequest {
        type Output = Result<(), String>;

        fn poll(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            std::task::Poll::Pending
        }
    }

    #[cfg(feature = "cancellable-http")]
    impl Drop for PendingRequest {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Relaxed);
        }
    }

    #[cfg(feature = "cancellable-http")]
    #[tokio::test]
    async fn cancellation_drops_the_request_future() {
        let dropped = Arc::new(AtomicBool::new(false));
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
        let request = PendingRequest {
            dropped: dropped.clone(),
        };

        cancel_tx.send(()).unwrap();
        let error =
            await_cancellable_request(request, cancel_rx, std::time::Duration::from_secs(3600))
                .await
                .unwrap_err();

        assert_eq!(error, "content search cancelled");
        assert!(dropped.load(Ordering::Relaxed));
    }

    #[cfg(feature = "cancellable-http")]
    #[tokio::test]
    async fn timeout_drops_the_request_future() {
        let dropped = Arc::new(AtomicBool::new(false));
        let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
        let request = PendingRequest {
            dropped: dropped.clone(),
        };

        let error =
            await_cancellable_request(request, cancel_rx, std::time::Duration::from_millis(1))
                .await
                .unwrap_err();

        assert_eq!(error, "content search request timed out");
        assert!(dropped.load(Ordering::Relaxed));
    }

    #[cfg(feature = "cancellable-http")]
    #[test]
    fn blocking_facade_closes_the_in_flight_request_on_cancellation() {
        use std::io::Read as _;

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (request_started_tx, request_started_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..read]);
                if let Some(header_end) =
                    request.windows(4).position(|window| window == b"\r\n\r\n")
                {
                    let headers = std::str::from_utf8(&request[..header_end]).unwrap();
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    if request.len() >= header_end + 4 + content_length {
                        break;
                    }
                }
            }
            let request_line = std::str::from_utf8(&request)
                .unwrap()
                .lines()
                .next()
                .unwrap();
            assert_eq!(request_line, "POST /v1/actions HTTP/1.1");
            request_started_tx.send(()).unwrap();
            stream.read(&mut buffer).unwrap_or_default() == 0
        });

        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let client = RemoteActionClient::new(remote_config(port), "token".to_string());
        let request = std::thread::spawn(move || {
            client.post_action_cancellable(search_action(), &worker_cancelled)
        });

        request_started_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        cancelled.store(true, Ordering::Relaxed);
        let error = request.join().unwrap().unwrap_err();

        assert_eq!(error, "content search cancelled");
        assert!(
            server.join().unwrap(),
            "cancelled request must close its socket"
        );
    }
}
