use crate::UpdateStatusSnapshot;
use anyhow::{Context, Result};
use std::time::Duration;

fn local_update_url(path: &str) -> Result<String> {
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
    Ok(format!("http://127.0.0.1:{port}{path}"))
}

pub fn fetch_status() -> Result<UpdateStatusSnapshot> {
    let url = local_update_url("/v1/update/status")?;
    let response = okena_transport::http::send(
        okena_transport::http::HttpRequest::get(url)
            .timeout(Duration::from_secs(5))
            .label("updater.daemon_status"),
    )
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
    let url = local_update_url(path)?;
    let response = okena_transport::http::send(
        okena_transport::http::HttpRequest::post(url)
            .timeout(Duration::from_secs(10))
            .label(label),
    )
    .with_context(|| format!("failed to POST {path}"))?
    .error_for_status()
    .with_context(|| format!("daemon rejected {path}"))?;
    response.json().context("failed to decode update status")
}
