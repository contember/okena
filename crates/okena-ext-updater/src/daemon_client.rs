use crate::UpdateStatusSnapshot;
use anyhow::{Context, Result};
use std::time::Duration;

struct LocalUpdateEndpoint {
    client: reqwest::blocking::Client,
    url: String,
}

fn local_update_endpoint(path: &str) -> Result<LocalUpdateEndpoint> {
    let remote_path = okena_core::profiles::try_current()
        .map(|p| p.remote_json())
        .unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("okena")
                .join("remote.json")
        });
    let data = std::fs::read_to_string(&remote_path)
        .with_context(|| format!("failed to read {}", remote_path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&data).context("failed to parse remote.json")?;
    let port_value = value
        .get("port")
        .and_then(serde_json::Value::as_u64)
        .context("remote.json is missing port")?;
    let port = u16::try_from(port_value).context("remote.json port is out of range")?;
    #[cfg(unix)]
    if let Some(socket_path) = value
        .get("local_endpoint")
        .and_then(|endpoint| {
            if endpoint.get("kind").and_then(serde_json::Value::as_str) == Some("unix_socket") {
                endpoint.get("path").and_then(serde_json::Value::as_str)
            } else {
                None
            }
        })
    {
        let client = reqwest::blocking::Client::builder()
            .unix_socket(socket_path)
            .build()
            .with_context(|| format!("failed to build Unix socket client for {socket_path}"))?;
        return Ok(LocalUpdateEndpoint {
            client,
            url: format!("http://okena.local{path}"),
        });
    }

    let host = value
        .get("local_host")
        .and_then(serde_json::Value::as_str)
        .filter(|host| !host.is_empty())
        .unwrap_or("127.0.0.1");
    Ok(LocalUpdateEndpoint {
        client: reqwest::blocking::Client::new(),
        url: format!("http://{host}:{port}{path}"),
    })
}

pub fn fetch_status() -> Result<UpdateStatusSnapshot> {
    let endpoint = local_update_endpoint("/v1/update/status")?;
    let response = endpoint
        .client
        .get(&endpoint.url)
        .timeout(Duration::from_secs(5))
        .send()
        .context("failed to fetch update status")?
    .error_for_status()
    .context("update status request failed")?;
    response.json().context("failed to decode update status")
}

pub fn request_check() -> Result<UpdateStatusSnapshot> {
    post_snapshot("/v1/update/check", "updater.daemon_check")
}

pub fn request_install() -> Result<UpdateStatusSnapshot> {
    post_snapshot("/v1/update/install", "updater.daemon_install")
}

pub fn request_dismiss() -> Result<UpdateStatusSnapshot> {
    post_snapshot("/v1/update/dismiss", "updater.daemon_dismiss")
}

fn post_snapshot(path: &str, label: &'static str) -> Result<UpdateStatusSnapshot> {
    let endpoint = local_update_endpoint(path)?;
    let _ = label;
    let response = endpoint
        .client
        .post(&endpoint.url)
        .timeout(Duration::from_secs(10))
        .send()
        .with_context(|| format!("failed to POST {path}"))?
    .error_for_status()
    .with_context(|| format!("daemon rejected {path}"))?;
    response.json().context("failed to decode update status")
}
