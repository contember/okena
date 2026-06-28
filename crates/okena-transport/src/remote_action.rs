//! Shared blocking HTTP helper for posting ActionRequests to a remote server.

use okena_core::api::ActionRequest;
use crate::client::config::{LocalEndpoint, RemoteConnectionConfig};

/// Total request timeout for "fast" actions (terminal control, listings,
/// metadata). 10 s is generous for these; longer would mask real failures.
const FAST_TIMEOUT_SECS: u64 = 10;

/// Total request timeout for byte-payload reads (ReadFileBytes). A 20 MB
/// image base64-encodes to ~27 MB; over a 5 Mbit/s link that's ~45 s on the
/// wire alone, which would time out the fast client with no useful signal.
const BYTES_TIMEOUT_SECS: u64 = 90;

/// Hard ceiling on response body size accepted by the remote bridge. Cuts
/// off arbitrarily large or runaway responses before they're buffered into
/// memory (peak resident is ~4× the file size while the base64 + JSON +
/// decoded Vec all co-exist). Mirrors the server-side cap in
/// `src/workspace/actions/execute/files.rs`.
const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;

/// Build a reqwest blocking client. Returns Err on TLS backend init
/// failure (corrupt cert store, sandboxed Keychain denial, FIPS mode
/// rejecting default ciphers, container without ca-certificates). Caller
/// is expected to surface the error to the user — a panic here would
/// abort the whole app at first remote probe and OnceLock-poison every
/// retry.
fn build_client(timeout_secs: u64) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| format!("Cannot initialise HTTP client: {}", e))
}

/// Cached fast-action client. We store the build result so a TLS init
/// failure surfaces as a recoverable error rather than poisoning the
/// OnceLock and panicking every retry.
fn shared_client() -> Result<&'static reqwest::blocking::Client, String> {
    use std::sync::OnceLock;
    static CLIENT: OnceLock<Result<reqwest::blocking::Client, String>> = OnceLock::new();
    match CLIENT.get_or_init(|| build_client(FAST_TIMEOUT_SECS)) {
        Ok(c) => Ok(c),
        Err(e) => Err(e.clone()),
    }
}

/// Cached bytes-action client with a longer timeout, for ReadFileBytes.
fn bytes_client() -> Result<&'static reqwest::blocking::Client, String> {
    use std::sync::OnceLock;
    static CLIENT: OnceLock<Result<reqwest::blocking::Client, String>> = OnceLock::new();
    match CLIENT.get_or_init(|| build_client(BYTES_TIMEOUT_SECS)) {
        Ok(c) => Ok(c),
        Err(e) => Err(e.clone()),
    }
}

/// Pick the right client for the action. Bytes-bearing actions get the
/// larger timeout so a slow remote doesn't surface as a generic transport
/// error mid-download.
fn client_for(action: &ActionRequest) -> Result<&'static reqwest::blocking::Client, String> {
    match action {
        ActionRequest::ReadFileBytes { .. } => bytes_client(),
        _ => shared_client(),
    }
}

fn timeout_for(action: &ActionRequest) -> u64 {
    match action {
        ActionRequest::ReadFileBytes { .. } => BYTES_TIMEOUT_SECS,
        _ => FAST_TIMEOUT_SECS,
    }
}

#[cfg(unix)]
fn unix_client(path: &str, timeout_secs: u64) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .unix_socket(path)
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| format!("Cannot initialise Unix socket HTTP client: {}", e))
}

/// Post an action request to a remote server and return the JSON response body.
pub fn post_action(
    host: &str,
    port: u16,
    token: &str,
    action: ActionRequest,
) -> Result<Option<serde_json::Value>, String> {
    let url = format!("http://{}:{}/v1/actions", host, port);
    let client = client_for(&action)?;
    post_action_inner(client, &url, token, action)
}

/// Post an action using the full connection config. Local daemon configs can use
/// their same-host transport endpoint while normal remotes keep host/port TCP.
pub fn post_action_with_config(
    config: &RemoteConnectionConfig,
    token: &str,
    action: ActionRequest,
) -> Result<Option<serde_json::Value>, String> {
    post_action_with_endpoint(
        &config.host,
        config.port,
        token,
        config.local_endpoint.as_ref(),
        action,
    )
}

/// Post an action with an optional same-host endpoint. This keeps older view
/// models that store host/port/token from needing the whole connection config.
pub fn post_action_with_endpoint(
    host: &str,
    port: u16,
    token: &str,
    local_endpoint: Option<&LocalEndpoint>,
    action: ActionRequest,
) -> Result<Option<serde_json::Value>, String> {
    #[cfg(unix)]
    if let Some(LocalEndpoint::UnixSocket { path }) = local_endpoint {
        let client = unix_client(path, timeout_for(&action))?;
        return post_action_inner(&client, "http://okena.local/v1/actions", token, action);
    }

    post_action(host, port, token, action)
}

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

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!("Server returned {}: {}", status, body));
    }

    // Cap how much body we buffer. A hostile / older / desync'd server can't
    // make us swallow a multi-GB JSON+base64 stream into memory. Content-Length
    // is only a hint; we still bound the actual read.
    if let Some(len) = resp.content_length()
        && len > MAX_RESPONSE_BYTES {
            return Err(format!(
                "Response too large ({:.1} MB). Max {} MB.",
                len as f64 / 1024.0 / 1024.0,
                MAX_RESPONSE_BYTES / 1024 / 1024
            ));
        }
    use std::io::Read as _;
    let mut body_bytes = Vec::new();
    resp.take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut body_bytes)
        .map_err(|e| format!("Failed to read response: {}", e))?;
    if body_bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(format!(
            "Response too large (>{} MB).",
            MAX_RESPONSE_BYTES / 1024 / 1024
        ));
    }
    let body: serde_json::Value = serde_json::from_slice(&body_bytes)
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
