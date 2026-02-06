use anyhow::{Context, Result};
use semver::Version;
use std::time::Duration;

/// Info about an available release asset.
#[derive(Clone, Debug)]
pub struct ReleaseAsset {
    pub version: String,
    pub asset_url: String,
    pub asset_name: String,
    /// URL to a SHA256SUMS file in the release (if present).
    pub checksum_url: Option<String>,
}

/// Check GitHub for the latest release.
/// Returns `Some(ReleaseAsset)` when a newer version exists, `None` otherwise.
pub async fn check_for_update() -> Result<Option<ReleaseAsset>> {
    smol::unblock(check_blocking).await
}

fn check_blocking() -> Result<Option<ReleaseAsset>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(format!("okena/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("failed to build HTTP client")?;

    let http_resp = client
        .get("https://api.github.com/repos/contember/term-manager/releases/latest")
        .send()
        .context("failed to fetch latest release")?;

    let status = http_resp.status();
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        anyhow::bail!("GitHub API rate limit exceeded — try again later");
    }
    if !status.is_success() {
        anyhow::bail!("GitHub API returned status {}", status);
    }

    let resp: serde_json::Value = http_resp
        .json()
        .context("failed to parse release JSON")?;

    let tag = resp["tag_name"]
        .as_str()
        .context("missing tag_name")?;

    let remote_version_str = tag.strip_prefix('v').unwrap_or(tag);
    let remote_version = Version::parse(remote_version_str)
        .context("invalid remote version")?;

    let current_version = Version::parse(env!("CARGO_PKG_VERSION"))
        .context("invalid current version")?;

    if remote_version <= current_version {
        log::info!("No update available (current={}, latest={})", current_version, remote_version);
        return Ok(None);
    }

    log::info!("Update available: {} -> {}", current_version, remote_version);

    let expected_asset = platform_asset_name();
    let assets = resp["assets"]
        .as_array()
        .context("missing assets array")?;

    // Look for the platform asset and an optional SHA256SUMS file
    let mut found_asset: Option<(String, String)> = None;
    let mut checksum_url: Option<String> = None;

    for asset in assets {
        let name = asset["name"].as_str().unwrap_or_default();
        if name == expected_asset {
            let url = asset["browser_download_url"]
                .as_str()
                .context("missing download URL")?
                .to_string();
            found_asset = Some((name.to_string(), url));
        } else if name == "SHA256SUMS" || name == "sha256sums.txt" {
            if let Some(url) = asset["browser_download_url"].as_str() {
                checksum_url = Some(url.to_string());
            }
        }
    }

    if let Some((asset_name, asset_url)) = found_asset {
        return Ok(Some(ReleaseAsset {
            version: remote_version.to_string(),
            asset_url,
            asset_name,
            checksum_url,
        }));
    }

    // Asset not uploaded yet for this platform — treat as "no update" rather than error
    log::warn!(
        "Release {} exists but no matching asset '{}' found",
        remote_version, expected_asset
    );
    Ok(None)
}

/// Return the expected asset filename for the current platform.
fn platform_asset_name() -> &'static str {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return "okena-linux-x64.tar.gz";
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return "okena-linux-arm64.tar.gz";
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return "okena-macos-arm64.zip";
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return "okena-macos-x64.zip";
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return "okena-windows-x64.zip";
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    return "okena-windows-arm64.zip";

    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "aarch64"),
    )))]
    compile_error!("unsupported platform for auto-update");
}
