use crate::{ReleaseCatalog, UpdateStatusSnapshot};
use anyhow::{Context, Result};
use std::time::Duration;

struct LocalUpdateEndpoint {
    client: reqwest::blocking::Client,
    url: String,
}

fn local_update_endpoint(path: &str) -> Result<LocalUpdateEndpoint> {
    let remote_path = remote_json_path();
    let data = std::fs::read_to_string(&remote_path)
        .with_context(|| format!("failed to read {}", remote_path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&data).context("failed to parse remote.json")?;
    endpoint_from_remote_json(path, &value)
}

fn remote_json_path() -> std::path::PathBuf {
    okena_core::profiles::try_current()
        .map(|p| p.remote_json())
        .unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("okena")
                .join("remote.json")
        })
}

fn endpoint_from_remote_json(path: &str, value: &serde_json::Value) -> Result<LocalUpdateEndpoint> {
    let port_value = value
        .get("port")
        .and_then(serde_json::Value::as_u64)
        .context("remote.json is missing port")?;
    let port = u16::try_from(port_value).context("remote.json port is out of range")?;
    #[cfg(unix)]
    if let Some(socket_path) = value.get("local_endpoint").and_then(|endpoint| {
        if endpoint.get("kind").and_then(serde_json::Value::as_str) == Some("unix_socket") {
            endpoint.get("path").and_then(serde_json::Value::as_str)
        } else {
            None
        }
    }) {
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

pub fn fetch_releases() -> Result<ReleaseCatalog> {
    let endpoint = local_update_endpoint("/v1/update/releases")?;
    endpoint
        .client
        .get(&endpoint.url)
        .timeout(Duration::from_secs(20))
        .send()
        .context("failed to fetch release history")?
        .error_for_status()
        .context("release history request failed")?
        .json()
        .context("failed to decode release history")
}

pub fn request_revert(version: &str, keep_config: bool) -> Result<UpdateStatusSnapshot> {
    let endpoint = local_update_endpoint("/v1/update/revert")?;
    endpoint
        .client
        .post(&endpoint.url)
        .timeout(Duration::from_secs(10))
        .json(&serde_json::json!({
            "version": version,
            "keep_config": keep_config,
        }))
        .send()
        .context("failed to request version revert")?
        .error_for_status()
        .context("version revert request failed")?
        .json()
        .context("failed to decode update status")
}

/// Restart into the replacement daemon and wait for its new endpoint.
pub fn restart_daemon_and_wait() -> Result<()> {
    let remote_path = remote_json_path();
    let initial: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&remote_path)
            .with_context(|| format!("failed to read {}", remote_path.display()))?,
    )
    .context("failed to parse remote.json")?;
    let old_pid = initial
        .get("pid")
        .and_then(serde_json::Value::as_u64)
        .and_then(|pid| u32::try_from(pid).ok())
        .unwrap_or(0);
    let endpoint = endpoint_from_remote_json("/v1/restart", &initial)?;
    match endpoint
        .client
        .post(&endpoint.url)
        .timeout(Duration::from_secs(10))
        .send()
    {
        Ok(response) if response.status().is_success() => {}
        Ok(response) => anyhow::bail!("daemon restart returned {}", response.status()),
        Err(error) => log::warn!("Restart response was interrupted: {error}"),
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(25);
    while std::time::Instant::now() < deadline {
        if let Ok(data) = std::fs::read(&remote_path)
            && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&data)
        {
            let new_pid = value
                .get("pid")
                .and_then(serde_json::Value::as_u64)
                .and_then(|pid| u32::try_from(pid).ok())
                .unwrap_or(0);
            if new_pid != 0
                && new_pid != old_pid
                && okena_core::process::is_process_alive(new_pid)
                && fetch_status().is_ok()
            {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    anyhow::bail!("replacement daemon did not become ready within 25 seconds")
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
