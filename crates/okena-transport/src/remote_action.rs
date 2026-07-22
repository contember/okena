//! Shared async and blocking clients for actions sent to an Okena daemon.

use crate::RemoteConnectionConfig;
use okena_core::api::ActionRequest;
#[cfg(feature = "blocking-http")]
use std::sync::{Arc, OnceLock};

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
}

fn client_kind_for(action: &ActionRequest) -> ActionClientKind {
    match action {
        ActionRequest::ReadFileBytes { .. } => ActionClientKind::Bytes,
        ActionRequest::SearchContent { .. } => ActionClientKind::Search,
        _ => ActionClientKind::Fast,
    }
}

fn timeout_for(action: &ActionRequest) -> u64 {
    match client_kind_for(action) {
        ActionClientKind::Fast => FAST_TIMEOUT_SECS,
        ActionClientKind::Bytes => BYTES_TIMEOUT_SECS,
        ActionClientKind::Search => SEARCH_TIMEOUT_SECS,
    }
}

#[cfg(feature = "blocking-http")]
type ClientAndUrl = (reqwest::blocking::Client, String);

#[cfg(feature = "blocking-http")]
struct RemoteActionClientInner {
    config: RemoteConnectionConfig,
    token: String,
    fast: OnceLock<Result<ClientAndUrl, String>>,
    bytes: OnceLock<Result<ClientAndUrl, String>>,
    search: OnceLock<Result<ClientAndUrl, String>>,
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
            }),
        }
    }

    /// Post an action and return its optional JSON payload.
    pub fn post_action(&self, action: ActionRequest) -> Result<Option<serde_json::Value>, String> {
        let transport = match client_kind_for(&action) {
            ActionClientKind::Fast => &self.inner.fast,
            ActionClientKind::Bytes => &self.inner.bytes,
            ActionClientKind::Search => &self.inner.search,
        };
        let timeout = std::time::Duration::from_secs(timeout_for(&action));
        let client_and_url = transport.get_or_init(|| {
            crate::remote_http::blocking_client_and_url(&self.inner.config, "/v1/actions", timeout)
        });
        let (client, url) = match client_and_url {
            Ok(client_and_url) => client_and_url,
            Err(error) => return Err(error.clone()),
        };
        post_action_inner(client, url, &self.inner.token, action)
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
#[cfg(feature = "client")]
pub async fn post_action_async_with_client(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    action: ActionRequest,
) -> Result<Option<serde_json::Value>, String> {
    let url = format!("{base_url}/v1/actions");
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

    #[test]
    fn content_search_uses_long_timeout() {
        assert_eq!(timeout_for(&search_action()), SEARCH_TIMEOUT_SECS);
        assert_eq!(SEARCH_TIMEOUT_SECS, 90);
    }

    #[test]
    fn content_search_has_a_dedicated_cached_client() {
        assert_eq!(client_kind_for(&search_action()), ActionClientKind::Search);
        assert_eq!(
            client_kind_for(&ActionRequest::ReadFile {
                project_id: "project".to_string(),
                relative_path: "README.md".to_string(),
            }),
            ActionClientKind::Fast
        );
        assert_eq!(
            client_kind_for(&ActionRequest::ReadFileBytes {
                project_id: "project".to_string(),
                relative_path: "image.png".to_string(),
            }),
            ActionClientKind::Bytes
        );
    }
}
