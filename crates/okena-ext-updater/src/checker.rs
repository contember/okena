use anyhow::{Context, Result};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const RELEASES_URL: &str = "https://api.github.com/repos/contember/okena/releases";

/// Download data for one platform-specific release asset.
#[derive(Clone, Debug)]
pub struct ReleaseAsset {
    pub version: String,
    pub asset_url: String,
    pub asset_name: String,
    pub checksum_url: Option<String>,
}

/// Stable release shown in the version history UI and CLI.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RevertRelease {
    pub version: String,
    pub published_at: String,
    pub release_url: String,
    pub asset_name: String,
    pub config_snapshot: Option<okena_core::profiles::ConfigSnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseCatalog {
    pub current_version: String,
    pub releases: Vec<RevertRelease>,
}

/// Check GitHub for the latest release newer than `app_version`.
pub async fn check_for_update(app_version: String) -> Result<Option<ReleaseAsset>> {
    smol::unblock(move || check_blocking(&app_version)).await
}

/// List stable releases older than the running version that have an asset for
/// this platform. GitHub returns releases newest-first; the order is preserved.
pub async fn list_revert_releases(app_version: String) -> Result<ReleaseCatalog> {
    smol::unblock(move || list_revert_releases_blocking(&app_version)).await
}

/// Resolve one exact downgrade target from GitHub. The target must be older
/// than the running version and must contain this platform's release asset.
pub async fn release_for_revert(app_version: String, target: String) -> Result<ReleaseAsset> {
    smol::unblock(move || release_for_revert_blocking(&app_version, &target)).await
}

fn check_blocking(app_version: &str) -> Result<Option<ReleaseAsset>> {
    let response = fetch_json(
        &format!("{RELEASES_URL}/latest"),
        app_version,
        "updater.check",
    )?;
    release_asset(&response, Some(app_version), VersionRelation::Newer)
}

fn list_revert_releases_blocking(app_version: &str) -> Result<ReleaseCatalog> {
    // One page only: 100 releases back is far beyond any sane revert target.
    let response = fetch_json(
        &format!("{RELEASES_URL}?per_page=100"),
        app_version,
        "updater.releases",
    )?;
    let releases = response
        .as_array()
        .context("release list is not an array")?;
    let current = Version::parse(app_version).context("invalid current version")?;
    let snapshots = okena_core::profiles::try_current();

    let releases = releases
        .iter()
        .filter_map(|release| catalog_release(release, &current, snapshots))
        .collect();
    Ok(ReleaseCatalog {
        current_version: app_version.to_string(),
        releases,
    })
}

fn release_for_revert_blocking(app_version: &str, target: &str) -> Result<ReleaseAsset> {
    let target = Version::parse(target).context("invalid target version")?;
    let current = Version::parse(app_version).context("invalid current version")?;
    if target >= current {
        anyhow::bail!("revert target v{target} must be older than v{current}");
    }
    let response = fetch_json(
        &format!("{RELEASES_URL}/tags/v{target}"),
        app_version,
        "updater.revert.resolve",
    )?;
    if !is_stable_release(&response) {
        anyhow::bail!("release v{target} is not stable");
    }
    // The direction is already enforced above, so `None` here means one thing only.
    release_asset(&response, None, VersionRelation::Older)?
        .with_context(|| format!("release v{target} has no asset for this platform"))
}

fn fetch_json(url: &str, app_version: &str, label: &'static str) -> Result<serde_json::Value> {
    let response = okena_transport::http::send(
        okena_transport::http::HttpRequest::get(url)
            .user_agent(format!("okena/{app_version}"))
            .timeout(Duration::from_secs(15))
            .label(label),
    )
    .with_context(|| format!("failed to fetch {url}"))?;
    let status = response.status();
    if status == 403 || status == 429 {
        anyhow::bail!("GitHub API rate limit exceeded — try again later");
    }
    if status == 404 {
        anyhow::bail!("release not found");
    }
    if !response.is_success() {
        anyhow::bail!("GitHub API returned status {status}");
    }
    response.json().context("failed to parse release JSON")
}

#[derive(Clone, Copy)]
enum VersionRelation {
    Newer,
    Older,
}

fn release_asset(
    release: &serde_json::Value,
    app_version: Option<&str>,
    relation: VersionRelation,
) -> Result<Option<ReleaseAsset>> {
    let tag = release["tag_name"].as_str().context("missing tag_name")?;
    let remote_version =
        Version::parse(tag.strip_prefix('v').unwrap_or(tag)).context("invalid remote version")?;
    if let Some(app_version) = app_version {
        let current = Version::parse(app_version).context("invalid current version")?;
        let matches = match relation {
            VersionRelation::Newer => remote_version > current,
            VersionRelation::Older => remote_version < current,
        };
        if !matches {
            return Ok(None);
        }
    }

    let (asset_name, asset_url, checksum_url) = platform_asset(release)?;
    if asset_url.is_none() {
        log::warn!("Release {remote_version} exists but has no asset named '{asset_name}'");
    }
    Ok(asset_url.map(|asset_url| ReleaseAsset {
        version: remote_version.to_string(),
        asset_url,
        asset_name,
        checksum_url,
    }))
}

fn catalog_release(
    release: &serde_json::Value,
    current: &Version,
    paths: Option<&okena_core::profiles::ProfilePaths>,
) -> Option<RevertRelease> {
    if !is_stable_release(release) {
        return None;
    }
    let tag = release["tag_name"].as_str()?;
    let version = Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()?;
    if version >= *current {
        return None;
    }
    let (asset_name, asset_url, _) = platform_asset(release).ok()?;
    asset_url.as_ref()?;
    let version = version.to_string();
    Some(RevertRelease {
        config_snapshot: paths
            .and_then(|paths| okena_core::profiles::config_snapshot_for_version(paths, &version)),
        version,
        published_at: release["published_at"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        release_url: release["html_url"].as_str().unwrap_or_default().to_string(),
        asset_name,
    })
}

fn is_stable_release(release: &serde_json::Value) -> bool {
    !release["draft"].as_bool().unwrap_or(false)
        && !release["prerelease"].as_bool().unwrap_or(false)
}

fn platform_asset(release: &serde_json::Value) -> Result<(String, Option<String>, Option<String>)> {
    let expected = platform_asset_name().to_string();
    let assets = release["assets"]
        .as_array()
        .context("missing assets array")?;
    let mut asset_url = None;
    let mut checksum_url = None;
    for asset in assets {
        let name = asset["name"].as_str().unwrap_or_default();
        if name == expected {
            asset_url = asset["browser_download_url"].as_str().map(str::to_string);
        } else if matches!(name, "SHA256SUMS" | "sha256sums.txt") {
            checksum_url = asset["browser_download_url"].as_str().map(str::to_string);
        }
    }
    Ok((expected, asset_url, checksum_url))
}

pub fn platform_asset_name() -> &'static str {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: &str, draft: bool, prerelease: bool, has_asset: bool) -> serde_json::Value {
        let assets = if has_asset {
            serde_json::json!([{
                "name": platform_asset_name(),
                "browser_download_url": "https://example.test/okena"
            }])
        } else {
            serde_json::json!([])
        };
        serde_json::json!({
            "tag_name": format!("v{version}"),
            "published_at": "2026-08-01T12:00:00Z",
            "html_url": format!("https://example.test/v{version}"),
            "draft": draft,
            "prerelease": prerelease,
            "assets": assets
        })
    }

    #[test]
    fn catalog_keeps_only_older_stable_platform_releases() {
        let current = Version::parse("0.27.0").unwrap();
        assert!(catalog_release(&release("0.26.0", false, false, true), &current, None).is_some());
        assert!(catalog_release(&release("0.28.0", false, false, true), &current, None).is_none());
        assert!(catalog_release(&release("0.26.0", true, false, true), &current, None).is_none());
        assert!(catalog_release(&release("0.26.0", false, true, true), &current, None).is_none());
        assert!(catalog_release(&release("0.26.0", false, false, false), &current, None).is_none());
    }

    #[test]
    fn exact_release_rejects_wrong_direction() {
        let newer = release_asset(
            &release("0.28.0", false, false, true),
            Some("0.27.0"),
            VersionRelation::Older,
        )
        .unwrap();
        assert!(newer.is_none());
    }

    #[test]
    fn exact_release_rejects_prerelease() {
        let candidate = release("0.26.0", false, true, true);
        assert!(!is_stable_release(&candidate));
    }
}
