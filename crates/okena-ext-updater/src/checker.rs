use crate::status::UpdateChannel;
use anyhow::{Context, Result};
use okena_core::build_info::{BUILD_INFO, NightlyBuild};
use parking_lot::Mutex;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{Duration, Instant};

const RELEASES_URL: &str = "https://api.github.com/repos/contember/okena/releases";
const RELEASE_DOWNLOADS_URL: &str = "https://github.com/contember/okena/releases/download";
const CHECKSUMS_NAME: &str = "SHA256SUMS";
const NIGHTLY_MANIFEST_NAME: &str = "nightly.json";

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

/// One published release with its notes, for the What's New overlay.
/// Unlike [`RevertRelease`] it does not care about platform assets.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseNote {
    pub version: String,
    pub published_at: String,
    pub release_url: String,
    /// The GitHub release body, Markdown. Empty when the release has none.
    pub body: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseNotes {
    pub current_version: String,
    /// Newest first.
    pub releases: Vec<ReleaseNote>,
}

/// How many releases What's New shows at most.
pub const RELEASE_NOTES_CAP: usize = 20;

/// How long fetched release notes are served without asking GitHub again.
/// The unauthenticated API allows 60 calls an hour, shared with the update check.
const RELEASE_NOTES_TTL: Duration = Duration::from_secs(60 * 60);

/// A stable release with its parsed version, so selection can sort and compare.
#[derive(Clone, Debug)]
pub struct PublishedNote {
    version: Version,
    note: ReleaseNote,
}

/// The daemon's last successful release-notes fetch. Failures are not stored,
/// so a Retry after an error always reaches GitHub.
#[derive(Clone, Default)]
pub struct ReleaseNotesCache(Arc<Mutex<Option<FetchedNotes>>>);

struct FetchedNotes {
    fetched_at: Instant,
    published: Arc<Vec<PublishedNote>>,
}

impl ReleaseNotesCache {
    fn fresh_at(&self, now: Instant) -> Option<Arc<Vec<PublishedNote>>> {
        self.0
            .lock()
            .as_ref()
            .filter(|fetched| now.duration_since(fetched.fetched_at) < RELEASE_NOTES_TTL)
            .map(|fetched| fetched.published.clone())
    }

    fn store_at(&self, published: Arc<Vec<PublishedNote>>, now: Instant) {
        *self.0.lock() = Some(FetchedNotes {
            fetched_at: now,
            published,
        });
    }
}

/// Check the selected channel for a verified update on this platform.
pub async fn check_for_update(
    app_version: &str,
    channel: UpdateChannel,
) -> Result<Option<ReleaseAsset>> {
    let app_version = app_version.to_owned();
    smol::unblock(move || match channel {
        UpdateChannel::Release => {
            let release = fetch_latest_stable(&app_version)?;
            stable_update_asset(&release, &app_version, BUILD_INFO.nightly.is_some())
        }
        UpdateChannel::Nightly => check_nightly(&app_version, BUILD_INFO.nightly.as_ref()),
    })
    .await
}

/// Find an upgrade hint for installations managed by an external package manager.
pub async fn check_for_package_update(app_version: &str) -> Result<Option<ReleaseAsset>> {
    let app_version = app_version.to_owned();
    smol::unblock(move || {
        let release = fetch_latest_stable(&app_version)?;
        stable_offer(&release, &app_version, false)
    })
    .await
}

/// List stable releases older than the running version that have an asset for
/// this platform. GitHub returns releases newest-first; the order is preserved.
pub async fn list_revert_releases(app_version: String) -> Result<ReleaseCatalog> {
    smol::unblock(move || list_revert_releases_blocking(&app_version)).await
}

/// List the notes of stable releases up to and including `current`, from the
/// cache while it is fresh. See [`select_release_notes`] for the rules.
pub async fn list_release_notes(
    cache: &ReleaseNotesCache,
    app_version: String,
    current: Version,
) -> Result<ReleaseNotes> {
    let published = match cache.fresh_at(Instant::now()) {
        Some(published) => published,
        None => {
            let published =
                Arc::new(smol::unblock(move || fetch_published_notes(&app_version)).await?);
            cache.store_at(published.clone(), Instant::now());
            published
        }
    };
    Ok(ReleaseNotes {
        current_version: current.to_string(),
        releases: select_release_notes(&published, &current),
    })
}

/// Resolve one exact downgrade target from GitHub. The target must be older
/// than the running version and must contain this platform's release asset.
pub async fn release_for_revert(app_version: String, target: String) -> Result<ReleaseAsset> {
    smol::unblock(move || release_for_revert_blocking(&app_version, &target)).await
}

fn fetch_latest_stable(app_version: &str) -> Result<serde_json::Value> {
    fetch_json(
        &format!("{RELEASES_URL}/latest"),
        app_version,
        "updater.check",
    )
}

fn stable_update_asset(
    release: &serde_json::Value,
    app_version: &str,
    running_nightly: bool,
) -> Result<Option<ReleaseAsset>> {
    let Some(mut asset) = stable_offer(release, app_version, running_nightly)? else {
        return Ok(None);
    };
    asset.checksum_url = Some(
        asset_download_url(release_assets(release)?, CHECKSUMS_NAME)
            .with_context(|| format!("release v{} has no {CHECKSUMS_NAME}", asset.version))?
            .to_owned(),
    );
    Ok(Some(asset))
}

fn stable_offer(
    release: &serde_json::Value,
    app_version: &str,
    running_nightly: bool,
) -> Result<Option<ReleaseAsset>> {
    if !is_stable_release(release) {
        return Ok(None);
    }
    let Some(version) = release["tag_name"].as_str().and_then(parse_release_version) else {
        return Ok(None);
    };
    let current = Version::parse(app_version).context("invalid current version")?;
    if !version.pre.is_empty() || (!running_nightly && version <= current) {
        return Ok(None);
    }
    release_asset(release, &version)
}

fn check_nightly(
    app_version: &str,
    running: Option<&NightlyBuild>,
) -> Result<Option<ReleaseAsset>> {
    // The publisher retains only the 14 newest nightlies. The latest 100
    // releases include those recent builds and cost one GitHub API request.
    let response = fetch_json(
        &format!("{RELEASES_URL}?per_page=100"),
        app_version,
        "updater.nightly.releases",
    )?;
    let releases = response
        .as_array()
        .context("release list is not an array")?;
    select_nightly(releases, app_version, running, |url| {
        let response = okena_transport::http::send(
            okena_transport::http::HttpRequest::get(url)
                .timeout(Duration::from_secs(15))
                .label("updater.nightly.metadata"),
        )
        .with_context(|| format!("failed to fetch {url}"))?
        .error_for_status()
        .with_context(|| format!("failed to fetch {url}"))?;
        Ok(response.bytes().to_vec())
    })
}

fn select_nightly(
    releases: &[serde_json::Value],
    app_version: &str,
    running: Option<&NightlyBuild>,
    mut fetch: impl FnMut(&str) -> Result<Vec<u8>>,
) -> Result<Option<ReleaseAsset>> {
    let stable_version = running
        .is_none()
        .then(|| Version::parse(app_version))
        .transpose()
        .context("invalid current version")?;
    let mut first_error = None;
    let mut fetch_asset = |url: &str, first_error: &mut Option<anyhow::Error>| match fetch(url) {
        Ok(bytes) => Some(bytes),
        Err(error) => {
            first_error.get_or_insert(error);
            None
        }
    };
    let mut candidates = Vec::new();
    for release in releases {
        let Some(tag) = release["tag_name"].as_str() else {
            continue;
        };
        let Some((date, short_sha)) = nightly_tag_parts(tag) else {
            continue;
        };
        if release["draft"].as_bool() != Some(false)
            || release["prerelease"].as_bool() != Some(true)
            || release["published_at"].as_str().is_none_or(str::is_empty)
            || running.is_some_and(|build| build.tag == tag)
        {
            continue;
        }
        let Ok(assets) = release_assets(release) else {
            continue;
        };
        let asset_name = platform_asset_name();
        let official_url = |name: &str| {
            let url = asset_download_url(assets, name)?;
            (url == format!("{RELEASE_DOWNLOADS_URL}/{tag}/{name}")).then_some(url)
        };
        let (Some(asset_url), Some(checksum_url), Some(manifest_url)) = (
            official_url(asset_name),
            official_url(CHECKSUMS_NAME),
            official_url(NIGHTLY_MANIFEST_NAME),
        ) else {
            continue;
        };
        let Some(manifest) = fetch_asset(manifest_url, &mut first_error) else {
            continue;
        };
        let Ok(build) = serde_json::from_slice::<NightlyBuild>(&manifest) else {
            continue;
        };
        let Ok(base_version) = Version::parse(&build.base_version) else {
            continue;
        };
        if build.tag != tag
            || build.date != format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..])
            || build.sha.len() != 40
            || !build.sha.bytes().all(is_lower_hex)
            || !build.sha.starts_with(short_sha)
            || release["target_commitish"].as_str() != Some(build.sha.as_str())
            || build.run_number == 0
            || running.is_some_and(|current| build.run_number <= current.run_number)
            || stable_version
                .as_ref()
                .is_some_and(|stable| base_version.cmp_precedence(stable).is_lt())
        {
            continue;
        }
        candidates.push((build, manifest, asset_url, checksum_url));
    }

    candidates.sort_by(|(a, ..), (b, ..)| b.run_number.cmp(&a.run_number));
    for (build, manifest, asset_url, checksum_url) in candidates {
        let Some(sums) = fetch_asset(checksum_url, &mut first_error) else {
            continue;
        };
        if let Ok(sums) = std::str::from_utf8(&sums)
            && listed_sha256(sums, platform_asset_name()).is_some()
            && listed_sha256(sums, NIGHTLY_MANIFEST_NAME).is_some_and(|hash| {
                hash.eq_ignore_ascii_case(&format!("{:x}", Sha256::digest(&manifest)))
            })
        {
            return Ok(Some(ReleaseAsset {
                version: build.tag,
                asset_url: asset_url.to_owned(),
                asset_name: platform_asset_name().to_owned(),
                checksum_url: Some(checksum_url.to_owned()),
            }));
        }
        let error = anyhow::anyhow!("Nightly {} has invalid release checksums", build.tag);
        log::warn!("{error}");
        first_error.get_or_insert(error);
    }
    first_error.map_or(Ok(None), Err)
}

fn nightly_tag_parts(tag: &str) -> Option<(&str, &str)> {
    let (date, sha) = tag.strip_prefix("nightly-")?.split_once('-')?;
    (date.len() == 8
        && date.bytes().all(|byte| byte.is_ascii_digit())
        && sha.len() == 7
        && sha.bytes().all(is_lower_hex))
    .then_some((date, sha))
}

fn is_lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

pub(crate) fn listed_sha256<'a>(sums: &'a str, asset: &str) -> Option<&'a str> {
    let hash = sums.lines().find_map(|line| {
        let (hash, name) = line.split_once(char::is_whitespace)?;
        (name.trim() == asset).then_some(hash)
    })?;
    (hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(hash)
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

fn fetch_published_notes(app_version: &str) -> Result<Vec<PublishedNote>> {
    let response = fetch_json(
        &format!("{RELEASES_URL}?per_page=100"),
        app_version,
        "updater.release_notes",
    )?;
    let releases = response
        .as_array()
        .context("release list is not an array")?;
    Ok(published_notes(releases))
}

/// A version as tags and clients spell it, with or without the leading `v`.
pub fn parse_release_version(version: &str) -> Option<Version> {
    Version::parse(version.strip_prefix('v').unwrap_or(version)).ok()
}

/// The stable releases of a GitHub release list, newest version first.
/// Sorted here because GitHub orders by creation date, not by version.
pub fn published_notes(releases: &[serde_json::Value]) -> Vec<PublishedNote> {
    let mut published: Vec<PublishedNote> = releases
        .iter()
        .filter(|release| is_stable_release(release))
        .filter_map(|release| {
            let version = parse_release_version(release["tag_name"].as_str()?)?;
            Some(PublishedNote {
                note: release_note(&version, release),
                version,
            })
        })
        .collect();
    published.sort_by(|a, b| b.version.cmp(&a.version));
    published
}

/// Pick the releases What's New shows from [`published_notes`]: those
/// `<= current`, newest first, capped at [`RELEASE_NOTES_CAP`]. A build ahead
/// of the last tag (nothing `<= current`) gets the newest published release
/// instead of an empty list.
pub fn select_release_notes(published: &[PublishedNote], current: &Version) -> Vec<ReleaseNote> {
    let selected: Vec<ReleaseNote> = published
        .iter()
        .filter(|published| published.version <= *current)
        .take(RELEASE_NOTES_CAP)
        .map(|published| published.note.clone())
        .collect();
    if !selected.is_empty() {
        return selected;
    }
    published
        .first()
        .map(|newest| vec![newest.note.clone()])
        .unwrap_or_default()
}

fn release_note(version: &Version, release: &serde_json::Value) -> ReleaseNote {
    ReleaseNote {
        version: version.to_string(),
        published_at: release["published_at"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        release_url: release["html_url"].as_str().unwrap_or_default().to_string(),
        body: release["body"].as_str().unwrap_or_default().to_string(),
    }
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
    release_asset(&response, &target)?
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

fn release_asset(
    release: &serde_json::Value,
    remote_version: &Version,
) -> Result<Option<ReleaseAsset>> {
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
    let assets = release_assets(release)?;
    let asset_url = asset_download_url(assets, &expected).map(str::to_owned);
    let checksum_url = asset_download_url(assets, CHECKSUMS_NAME)
        .or_else(|| asset_download_url(assets, "sha256sums.txt"))
        .map(str::to_owned);
    Ok((expected, asset_url, checksum_url))
}

fn release_assets(release: &serde_json::Value) -> Result<&[serde_json::Value]> {
    release["assets"]
        .as_array()
        .map(Vec::as_slice)
        .context("missing assets array")
}

fn asset_download_url<'a>(assets: &'a [serde_json::Value], name: &str) -> Option<&'a str> {
    assets
        .iter()
        .filter(|asset| asset["name"].as_str() == Some(name))
        .find_map(|asset| {
            asset["browser_download_url"]
                .as_str()
                .filter(|url| !url.is_empty())
        })
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

    /// nightly.yml publishes these as prereleases; the non-semver tag keeps
    /// them out even if the prerelease flag were lost.
    #[test]
    fn nightly_releases_are_excluded_from_stable_history() {
        let current = Version::parse("0.27.0").unwrap();
        for prerelease in [true, false] {
            let mut nightly = release("0.26.0", false, prerelease, true);
            nightly["tag_name"] = "nightly-20261120-50c64ec".into();
            assert!(catalog_release(&nightly, &current, None).is_none());
            assert!(published_notes(std::slice::from_ref(&nightly)).is_empty());
            assert!(
                stable_update_asset(&nightly, "0.27.0", true)
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn returning_to_release_allows_same_or_lower_version() {
        let mut stable = release("0.42.0", false, false, true);
        stable["assets"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "name": "SHA256SUMS",
                "browser_download_url": "https://example.test/SHA256SUMS"
            }));
        assert!(
            stable_update_asset(&stable, "0.42.0", false)
                .unwrap()
                .is_none()
        );
        assert!(
            stable_update_asset(&stable, "0.43.0", false)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            stable_update_asset(&stable, "0.41.0", false)
                .unwrap()
                .unwrap()
                .version,
            "0.42.0"
        );
        for current in ["0.42.0", "0.43.0"] {
            assert_eq!(
                stable_update_asset(&stable, current, true)
                    .unwrap()
                    .unwrap()
                    .version,
                "0.42.0"
            );
        }
    }

    #[test]
    fn self_update_requires_checksums_but_legacy_rollback_still_resolves() {
        let old_release = release("0.42.0", false, false, true);
        assert!(stable_update_asset(&old_release, "0.41.0", false).is_err());
        assert!(
            stable_offer(&old_release, "0.41.0", false)
                .unwrap()
                .unwrap()
                .checksum_url
                .is_none()
        );
    }

    type NightlyFixture = (serde_json::Value, Vec<u8>, Vec<u8>);

    fn nightly_fixture(short_sha: &str, run_number: u64) -> NightlyFixture {
        nightly_fixture_for_version(short_sha, run_number, "0.42.0")
    }

    fn nightly_fixture_for_version(
        short_sha: &str,
        run_number: u64,
        base_version: &str,
    ) -> NightlyFixture {
        let tag = format!("nightly-20261122-{short_sha}");
        let sha = format!("{short_sha}{}", "a".repeat(33));
        let manifest = serde_json::json!({
            "tag": tag,
            "date": "2026-11-22",
            "sha": sha,
            "run_number": run_number,
            "base_version": base_version
        })
        .to_string()
        .into_bytes();
        let sums = nightly_test_sums(&manifest);
        let assets: Vec<_> = [platform_asset_name(), "SHA256SUMS", "nightly.json"]
            .into_iter()
            .map(|name| serde_json::json!({
                "name": name,
                "browser_download_url": format!("https://github.com/contember/okena/releases/download/{tag}/{name}")
            }))
            .collect();
        let release = serde_json::json!({
            "tag_name": tag,
            "target_commitish": sha,
            "draft": false,
            "prerelease": true,
            "published_at": "2026-11-22T12:00:00Z",
            "assets": assets
        });
        (release, manifest, sums)
    }

    fn nightly_test_sums(manifest: &[u8]) -> Vec<u8> {
        use sha2::{Digest, Sha256};
        format!(
            "{}  {}\n{:x}  nightly.json\n",
            "a".repeat(64),
            platform_asset_name(),
            Sha256::digest(manifest),
        )
        .into_bytes()
    }

    fn select_test_nightly(
        fixtures: &[NightlyFixture],
        running: Option<&NightlyBuild>,
    ) -> Option<ReleaseAsset> {
        select_test_nightly_at(fixtures, "0.42.0", running, None).unwrap()
    }

    fn select_test_nightly_at(
        fixtures: &[NightlyFixture],
        app_version: &str,
        running: Option<&NightlyBuild>,
        unavailable_url: Option<&str>,
    ) -> Result<Option<ReleaseAsset>> {
        let releases: Vec<_> = fixtures
            .iter()
            .map(|(release, _, _)| release.clone())
            .collect();
        select_nightly(&releases, app_version, running, |url| {
            anyhow::ensure!(Some(url) != unavailable_url, "fixture HTTP 404 for {url}");
            for (release, manifest, sums) in fixtures {
                let tag = release["tag_name"].as_str().unwrap();
                if url
                    == format!(
                        "https://github.com/contember/okena/releases/download/{tag}/nightly.json"
                    )
                {
                    return Ok(manifest.clone());
                }
                if url
                    == format!(
                        "https://github.com/contember/okena/releases/download/{tag}/SHA256SUMS"
                    )
                {
                    return Ok(sums.clone());
                }
            }
            panic!("unexpected asset URL {url}");
        })
    }

    #[test]
    fn selects_greatest_nightly_run_even_on_same_day_and_cargo_version() {
        let first = nightly_fixture("1111111", 19);
        let second = nightly_fixture("2222222", 20);
        let running_first = serde_json::from_slice(&first.1).unwrap();
        let running_second = serde_json::from_slice(&second.1).unwrap();
        let fixtures = [first, second];
        for running in [None, Some(&running_first)] {
            assert_eq!(
                select_test_nightly(&fixtures, running).unwrap().version,
                "nightly-20261122-2222222"
            );
        }
        assert!(select_test_nightly(&fixtures, Some(&running_second)).is_none());
    }

    #[test]
    fn stable_build_skips_older_nightly_base_versions() {
        let older = nightly_fixture_for_version("1111111", 30, "0.42.0");
        let equal = nightly_fixture_for_version("2222222", 20, "0.43.0");
        let newer = nightly_fixture_for_version("3333333", 19, "0.44.0");
        let fixtures = [older, newer, equal];

        for (available, expected) in [
            (&fixtures[..1], None),
            (&fixtures[..2], Some("nightly-20261122-3333333")),
            (&fixtures[..], Some("nightly-20261122-2222222")),
        ] {
            let found = select_test_nightly_at(available, "0.43.0", None, None).unwrap();
            assert_eq!(
                found.as_ref().map(|release| release.version.as_str()),
                expected
            );
        }

        let running = serde_json::from_slice(&fixtures[2].1).unwrap();
        let found = select_test_nightly_at(&fixtures, "0.43.0", Some(&running), None)
            .unwrap()
            .unwrap();
        assert_eq!(found.version, "nightly-20261122-1111111");
    }

    #[test]
    fn unavailable_nightly_assets_allow_verified_fallback_or_report_failure() {
        let newest = nightly_fixture("2222222", 20);
        let older = nightly_fixture("1111111", 19);
        let fixtures = [newest, older];

        for asset in [NIGHTLY_MANIFEST_NAME, CHECKSUMS_NAME] {
            let url = format!("{RELEASE_DOWNLOADS_URL}/nightly-20261122-2222222/{asset}");
            assert_eq!(
                select_test_nightly_at(&fixtures, "0.42.0", None, Some(&url))
                    .unwrap()
                    .unwrap()
                    .version,
                "nightly-20261122-1111111"
            );
            let error =
                select_test_nightly_at(&fixtures[..1], "0.42.0", None, Some(&url)).unwrap_err();
            assert!(error.to_string().contains("fixture HTTP 404"));
        }
    }

    #[test]
    fn invalid_nightly_checksums_without_fallback_are_reported() {
        let mut nightly = nightly_fixture("1111111", 19);
        nightly.1.push(b' ');
        let error = select_test_nightly_at(&[nightly], "0.42.0", None, None).unwrap_err();
        assert!(error.to_string().contains("invalid release checksums"));
    }

    #[test]
    fn ignores_ineligible_nightly_releases() {
        let mut draft = nightly_fixture("1111111", 21);
        draft.0["draft"] = true.into();
        let mut unrelated = nightly_fixture("2222222", 22);
        unrelated.0["tag_name"] = "v0.43.0-rc.1".into();
        let mut legacy = nightly_fixture("3333333", 23);
        legacy.0["assets"].as_array_mut().unwrap().pop();
        let mut wrong_platform = nightly_fixture("4444444", 24);
        wrong_platform.0["assets"].as_array_mut().unwrap().remove(0);
        let mut tampered_manifest = nightly_fixture("5555555", 25);
        tampered_manifest.1.push(b' ');
        let mut mismatched_metadata = nightly_fixture("6666666", 26);
        mismatched_metadata.0["target_commitish"] = "b".repeat(40).into();
        let mut wrong_checksum = nightly_fixture("7777777", 27);
        wrong_checksum.2 = nightly_test_sums(&wrong_checksum.1);
        wrong_checksum.2[0] = b'g';
        let mut invalid_metadata = nightly_fixture("8888888", 28);
        let mut manifest: serde_json::Value = serde_json::from_slice(&invalid_metadata.1).unwrap();
        manifest["base_version"] = "invalid".into();
        invalid_metadata.1 = manifest.to_string().into_bytes();
        invalid_metadata.2 = nightly_test_sums(&invalid_metadata.1);

        let fixtures = [
            draft,
            unrelated,
            legacy,
            wrong_platform,
            tampered_manifest,
            mismatched_metadata,
            wrong_checksum,
            invalid_metadata,
            nightly_fixture("9999999", 20),
        ];
        assert_eq!(
            select_test_nightly(&fixtures, None).unwrap().version,
            "nightly-20261122-9999999"
        );
    }

    #[test]
    fn exact_release_rejects_prerelease() {
        let candidate = release("0.26.0", false, true, true);
        assert!(!is_stable_release(&candidate));
    }

    fn note(
        version: &str,
        draft: bool,
        prerelease: bool,
        body: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({
            "tag_name": format!("v{version}"),
            "published_at": "2026-10-09T15:49:55Z",
            "html_url": format!("https://github.com/contember/okena/releases/tag/v{version}"),
            "draft": draft,
            "prerelease": prerelease,
            "body": body,
            "assets": []
        })
    }

    /// GitHub order, newest first: a draft, a prerelease, then four stable
    /// releases 0.35 … 0.32.
    fn notes_fixture() -> Vec<serde_json::Value> {
        vec![
            note("0.36.0", true, false, "## Draft".into()),
            note("0.36.0-rc.1", false, true, "## Prerelease".into()),
            note("0.35.0", false, false, "## What's new\n\n- 0.35".into()),
            note("0.34.0", false, false, serde_json::Value::Null),
            note("0.33.0", false, false, "## What's new\n\n- 0.33".into()),
            note("0.32.0", false, false, "## What's Changed".into()),
        ]
    }

    fn versions(notes: &[ReleaseNote]) -> Vec<&str> {
        notes.iter().map(|n| n.version.as_str()).collect()
    }

    fn select(releases: &[serde_json::Value], current: &str) -> Vec<ReleaseNote> {
        select_release_notes(
            &published_notes(releases),
            &Version::parse(current).unwrap(),
        )
    }

    #[test]
    fn notes_keep_everything_up_to_current() {
        assert_eq!(
            versions(&select(&notes_fixture(), "0.34.0")),
            ["0.34.0", "0.33.0", "0.32.0"]
        );
    }

    #[test]
    fn notes_for_a_bumped_unreleased_version_show_the_published_ones() {
        assert_eq!(
            versions(&select(&notes_fixture(), "0.36.0")),
            ["0.35.0", "0.34.0", "0.33.0", "0.32.0"]
        );
    }

    #[test]
    fn notes_fall_back_to_the_newest_release_when_nothing_is_older() {
        assert_eq!(versions(&select(&notes_fixture(), "0.31.0")), ["0.35.0"]);
    }

    #[test]
    fn notes_skip_drafts_and_prereleases() {
        let notes = select(&notes_fixture(), "0.99.0");
        assert!(
            notes
                .iter()
                .all(|n| !n.body.contains("Draft") && !n.body.contains("Prerelease"))
        );
    }

    #[test]
    fn notes_carry_body_url_and_date_and_a_null_body_becomes_empty() {
        let notes = select(&notes_fixture(), "0.35.0");
        assert_eq!(notes[0].body, "## What's new\n\n- 0.35");
        assert_eq!(notes[0].published_at, "2026-10-09T15:49:55Z");
        assert_eq!(
            notes[0].release_url,
            "https://github.com/contember/okena/releases/tag/v0.35.0"
        );
        assert_eq!(notes[1].version, "0.34.0");
        assert_eq!(notes[1].body, "");
    }

    #[test]
    fn notes_are_capped_at_the_newest_twenty() {
        let releases: Vec<serde_json::Value> = (0..25)
            .rev()
            .map(|minor| note(&format!("0.{minor}.0"), false, false, "x".into()))
            .collect();
        let notes = select(&releases, "0.24.0");
        assert_eq!(notes.len(), RELEASE_NOTES_CAP);
        assert_eq!(notes[0].version, "0.24.0");
        assert_eq!(notes[19].version, "0.5.0");
    }

    #[test]
    fn notes_are_ordered_and_capped_by_version_not_by_github_order() {
        // A patch for an older line published after the newer releases, then
        // the rest oldest-first: the cap must still keep the newest twenty.
        let releases: Vec<serde_json::Value> = std::iter::once("0.3.1".to_string())
            .chain((0..25).map(|minor| format!("0.{minor}.0")))
            .map(|version| note(&version, false, false, "x".into()))
            .collect();
        let notes = select(&releases, "0.24.0");
        assert_eq!(notes.len(), RELEASE_NOTES_CAP);
        assert_eq!(notes[0].version, "0.24.0");
        assert_eq!(notes[19].version, "0.5.0");
        assert!(notes.windows(2).all(|pair| {
            Version::parse(&pair[0].version).unwrap() > Version::parse(&pair[1].version).unwrap()
        }));
    }

    #[test]
    fn cached_notes_expire_after_the_ttl() {
        let cache = ReleaseNotesCache::default();
        let fetched_at = Instant::now();
        assert!(cache.fresh_at(fetched_at).is_none());

        cache.store_at(Arc::new(published_notes(&notes_fixture())), fetched_at);
        assert!(
            cache
                .fresh_at(fetched_at + RELEASE_NOTES_TTL - Duration::from_secs(1))
                .is_some()
        );
        assert!(cache.fresh_at(fetched_at + RELEASE_NOTES_TTL).is_none());
    }
}
