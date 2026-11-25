use crate::status::UpdateChannel;
use anyhow::{Context, Result};
use okena_core::build_info::{BUILD_INFO, NightlyBuild};
use parking_lot::Mutex;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Reverse;
use std::sync::Arc;
use std::time::{Duration, Instant};

const RELEASES_URL: &str = "https://api.github.com/repos/contember/okena/releases";
const RELEASE_DOWNLOADS_URL: &str = "https://github.com/contember/okena/releases/download";
const CHECKSUMS_NAME: &str = "SHA256SUMS";
const NIGHTLY_MANIFEST_NAME: &str = "nightly.json";
const NIGHTLY_HISTORY_WORKERS: usize = 8;

/// Download data for one platform-specific release asset.
#[derive(Clone, Debug)]
pub struct ReleaseAsset {
    pub version: String,
    pub asset_url: String,
    pub asset_name: String,
    /// Lowercase hex SHA-256 the archive must match, read from the release's `SHA256SUMS`.
    pub sha256: String,
}

/// Published release shown in the version history UI and CLI.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RevertRelease {
    pub version: String,
    pub published_at: String,
    pub release_url: String,
    pub asset_name: String,
    pub config_snapshot: Option<okena_core::profiles::ConfigSnapshot>,
    #[serde(default)]
    pub legacy_nightly: bool,
    #[serde(default)]
    pub newer: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseCatalog {
    pub current_version: String,
    #[serde(default)]
    pub channel: UpdateChannel,
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
            stable_update_asset(
                &release,
                &app_version,
                BUILD_INFO.nightly.is_some(),
                fetch_release_file,
            )
        }
        UpdateChannel::Nightly => check_nightly(&app_version, BUILD_INFO.nightly.as_ref()),
    })
    .await
}

/// Find the newer stable version an external package manager should install.
pub async fn check_for_package_update(app_version: &str) -> Result<Option<Version>> {
    let app_version = app_version.to_owned();
    smol::unblock(move || {
        let release = fetch_latest_stable(&app_version)?;
        package_offer(&release, &app_version)
    })
    .await
}

/// The cask installs this platform's archive, so a release without one is not an update yet.
fn package_offer(release: &serde_json::Value, app_version: &str) -> Result<Option<Version>> {
    let has_archive = platform_asset(release)?.1.is_some();
    Ok(stable_offer(release, app_version, false)?.filter(|_| has_archive))
}

/// List published versions available for manual installation.
pub async fn list_revert_releases(
    app_version: String,
    channel: UpdateChannel,
) -> Result<ReleaseCatalog> {
    smol::unblock(move || list_revert_releases_blocking(&app_version, channel)).await
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

/// Revalidate the exact, selected version before downloading.
pub async fn release_for_revert(
    app_version: String,
    target: String,
    channel: UpdateChannel,
) -> Result<(ReleaseAsset, bool)> {
    smol::unblock(move || {
        let running = BUILD_INFO.nightly.as_ref();
        let (asset, published_at) =
            if channel == UpdateChannel::Nightly && nightly_tag_parts(&target).is_some() {
                nightly_for_revert(&app_version, &target, running)
            } else {
                release_for_revert_with(
                    &app_version,
                    &target,
                    channel,
                    running.is_some(),
                    |url, label| fetch_json(url, &app_version, label),
                    fetch_release_file,
                )
            }?;
        let current = Version::parse(&app_version).context("invalid current version")?;
        let target_version = Version::parse(&asset.version).ok();
        let running_release =
            if history_needs_publication(target_version.as_ref(), &current, running.is_some()) {
                let tag = running.map_or_else(|| format!("v{current}"), |build| build.tag.clone());
                fetch_json(
                    &format!("{RELEASES_URL}/tags/{tag}"),
                    &app_version,
                    "updater.revert.running_release",
                )
                .ok()
            } else {
                None
            };
        let running_publication = running_release
            .as_ref()
            .and_then(|release| history_running_publication(release, &current, running));
        let newer = history_install_is_newer(
            target_version.as_ref(),
            &current,
            running.is_some(),
            &published_at,
            running_publication,
        );
        Ok((asset, newer))
    })
    .await
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
    fetch: impl FnMut(&str) -> Result<Vec<u8>>,
) -> Result<Option<ReleaseAsset>> {
    let Some(version) = stable_offer(release, app_version, running_nightly)? else {
        return Ok(None);
    };
    release_asset(release, &version, fetch)
}

fn stable_offer(
    release: &serde_json::Value,
    app_version: &str,
    running_nightly: bool,
) -> Result<Option<Version>> {
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
    Ok(Some(version))
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
    select_nightly(releases, app_version, running, fetch_release_file)
}

/// Fetch a small release asset such as `SHA256SUMS` or `nightly.json`.
fn fetch_release_file(url: &str) -> Result<Vec<u8>> {
    let response = okena_transport::http::send(
        okena_transport::http::HttpRequest::get(url)
            .timeout(Duration::from_secs(15))
            .label("updater.release_file"),
    )
    .with_context(|| format!("failed to fetch {url}"))?
    .error_for_status()
    .with_context(|| format!("failed to fetch {url}"))?;
    Ok(response.bytes().to_vec())
}

struct PublishedNightly<'a> {
    tag: &'a str,
    day: &'a str,
    sha: &'a str,
    published_at: &'a str,
    asset_url: &'a str,
    checksum_url: &'a str,
    manifest_url: Option<&'a str>,
}

struct VerifiedManifest {
    run_number: u64,
    base_version: Version,
    bytes: Vec<u8>,
}

impl<'a> PublishedNightly<'a> {
    fn from_release(release: &'a serde_json::Value) -> Option<Self> {
        let tag = release["tag_name"].as_str()?;
        let (day, short_sha) = nightly_tag_parts(tag)?;
        let sha = release["target_commitish"].as_str()?;
        let published_at = release["published_at"].as_str()?;
        if release["draft"].as_bool() != Some(false)
            || release["prerelease"].as_bool() != Some(true)
            || published_at.is_empty()
            || sha.len() != 40
            || !sha.bytes().all(is_lower_hex)
            || !sha.starts_with(short_sha)
        {
            return None;
        }
        let assets = release_assets(release).ok()?;
        let official_url = |name: &str| {
            let url = asset_download_url(assets, name)?;
            (url == format!("{RELEASE_DOWNLOADS_URL}/{tag}/{name}")).then_some(url)
        };
        let manifest_url = official_url(NIGHTLY_MANIFEST_NAME);
        if manifest_url.is_none()
            && assets
                .iter()
                .any(|asset| asset["name"].as_str() == Some(NIGHTLY_MANIFEST_NAME))
        {
            return None;
        }
        Some(Self {
            tag,
            day,
            sha,
            published_at,
            asset_url: official_url(platform_asset_name())?,
            checksum_url: official_url(CHECKSUMS_NAME)?,
            manifest_url,
        })
    }

    fn load_manifest(
        &self,
        url: &str,
        fetch: &mut impl FnMut(&str) -> Result<Vec<u8>>,
    ) -> Result<Option<VerifiedManifest>> {
        let bytes = fetch(url)?;
        let Ok(build) = serde_json::from_slice::<NightlyBuild>(&bytes) else {
            return Ok(None);
        };
        let Ok(base_version) = Version::parse(&build.base_version) else {
            return Ok(None);
        };
        if build.tag != self.tag
            || build.date != formatted_nightly_day(self.day)
            || build.sha != self.sha
            || build.run_number == 0
        {
            return Ok(None);
        }
        Ok(Some(VerifiedManifest {
            run_number: build.run_number,
            base_version,
            bytes,
        }))
    }

    /// Check the manifest against `SHA256SUMS` and return the archive's expected hash.
    fn verify_checksums(
        &self,
        manifest: Option<&[u8]>,
        fetch: &mut impl FnMut(&str) -> Result<Vec<u8>>,
    ) -> Result<String> {
        let bytes = fetch(self.checksum_url)?;
        let sums = std::str::from_utf8(&bytes).context("invalid checksum file encoding")?;
        let manifest_matches = match manifest {
            Some(manifest) => listed_sha256(sums, NIGHTLY_MANIFEST_NAME).is_some_and(|hash| {
                hash.eq_ignore_ascii_case(&format!("{:x}", Sha256::digest(manifest)))
            }),
            // A missing manifest asset alone does not establish a legacy release.
            None => !sums.split_whitespace().any(|field| {
                field
                    .trim_start_matches('*')
                    .eq_ignore_ascii_case(NIGHTLY_MANIFEST_NAME)
            }),
        };
        let archive_hash = listed_sha256(sums, platform_asset_name())
            .filter(|_| manifest_matches)
            .with_context(|| format!("Nightly {} has invalid release checksums", self.tag))?;
        Ok(archive_hash.to_ascii_lowercase())
    }

    fn download(&self, sha256: String) -> ReleaseAsset {
        ReleaseAsset {
            version: self.tag.to_owned(),
            asset_url: self.asset_url.to_owned(),
            asset_name: platform_asset_name().to_owned(),
            sha256,
        }
    }

    fn comparable_publication(&self) -> Option<&'a str> {
        // GitHub publishes UTC timestamps. Compare only a canonical time on
        // the shared tag day, so a legacy same-day release has proven order.
        let day = formatted_nightly_day(self.day);
        let time = self
            .published_at
            .strip_prefix(&day)?
            .strip_prefix('T')?
            .strip_suffix('Z')?;
        let mut fields = time.split(':');
        for limit in [23, 59, 59] {
            let field = fields.next()?;
            if field.len() != 2
                || !field.bytes().all(|b| b.is_ascii_digit())
                || field.parse::<u8>().ok()? > limit
            {
                return None;
            }
        }
        fields.next().is_none().then_some(self.published_at)
    }
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
    let mut candidates = Vec::new();
    for release in releases.iter().filter_map(PublishedNightly::from_release) {
        let Some(url) = release.manifest_url else {
            continue;
        };
        if running.is_some_and(|build| build.tag == release.tag) {
            continue;
        }
        let manifest = match release.load_manifest(url, &mut fetch) {
            Ok(Some(manifest)) => manifest,
            Ok(None) => continue,
            Err(error) => {
                first_error.get_or_insert(error);
                continue;
            }
        };
        if running.is_some_and(|current| manifest.run_number <= current.run_number)
            || stable_version
                .as_ref()
                .is_some_and(|stable| manifest.base_version.cmp_precedence(stable).is_lt())
        {
            continue;
        }
        candidates.push((release, manifest));
    }

    candidates.sort_by_key(|(_, manifest)| Reverse(manifest.run_number));
    for (release, manifest) in candidates {
        match release.verify_checksums(Some(&manifest.bytes), &mut fetch) {
            Ok(sha256) => return Ok(Some(release.download(sha256))),
            Err(error) => {
                log::warn!("{error}");
                first_error.get_or_insert(error);
            }
        }
    }
    first_error.map_or(Ok(None), Err)
}

fn formatted_nightly_day(day: &str) -> String {
    format!("{}-{}-{}", &day[..4], &day[4..6], &day[6..])
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

fn list_revert_releases_blocking(
    app_version: &str,
    channel: UpdateChannel,
) -> Result<ReleaseCatalog> {
    // One page only: 100 releases back is far beyond any sane revert target.
    let response = fetch_json(
        &format!("{RELEASES_URL}?per_page=100"),
        app_version,
        "updater.releases",
    )?;
    let releases = response
        .as_array()
        .context("release list is not an array")?;
    catalog_for_channel(
        releases,
        app_version,
        channel,
        BUILD_INFO.nightly.as_ref(),
        okena_core::profiles::try_current(),
        fetch_release_file,
    )
}

fn catalog_for_channel(
    releases: &[serde_json::Value],
    app_version: &str,
    channel: UpdateChannel,
    running: Option<&NightlyBuild>,
    snapshots: Option<&okena_core::profiles::ProfilePaths>,
    fetch: impl Fn(&str) -> Result<Vec<u8>> + Sync,
) -> Result<ReleaseCatalog> {
    let current = Version::parse(app_version).context("invalid current version")?;
    let mut versions: Vec<_> = releases
        .iter()
        .filter_map(|release| {
            catalog_release(release, &current, channel, running.is_some(), snapshots)
        })
        .collect();
    if channel == UpdateChannel::Nightly {
        match nightly_catalog(releases, running, &fetch) {
            Ok(nightlies) => versions.extend(nightlies),
            Err(error) if versions.is_empty() => return Err(error),
            Err(error) => log::warn!("Could not load nightly history: {error}"),
        }
        let running_publication = releases
            .iter()
            .find_map(|release| history_running_publication(release, &current, running));
        for row in &mut versions {
            row.newer = history_install_is_newer(
                Version::parse(&row.version).ok().as_ref(),
                &current,
                running.is_some(),
                &row.published_at,
                running_publication,
            );
        }
        // GitHub publishes UTC timestamps in a fixed-width RFC 3339 format.
        versions.sort_by(|a, b| b.published_at.cmp(&a.published_at));
    }
    Ok(ReleaseCatalog {
        current_version: app_version.to_owned(),
        channel,
        releases: versions,
    })
}

fn history_needs_publication(
    target: Option<&Version>,
    current: &Version,
    running_nightly: bool,
) -> bool {
    match target {
        Some(target) => running_nightly && target == current,
        None => !running_nightly,
    }
}

fn history_install_is_newer(
    target: Option<&Version>,
    current: &Version,
    running_nightly: bool,
    target_publication: &str,
    running_publication: Option<&str>,
) -> bool {
    target.is_some_and(|version| version > current)
        || (history_needs_publication(target, current, running_nightly)
            && running_publication.is_some_and(|published_at| target_publication > published_at))
}

fn history_running_publication<'a>(
    release: &'a serde_json::Value,
    current: &Version,
    running: Option<&NightlyBuild>,
) -> Option<&'a str> {
    if let Some(running) = running {
        let nightly = PublishedNightly::from_release(release)?;
        // Unlike legacy-nightly eligibility, comparison with a stable release
        // uses the actual publication time even when the build crosses midnight.
        return (nightly.tag == running.tag && nightly.sha == running.sha)
            .then_some(nightly.published_at);
    }
    if is_stable_release(release)
        && release["tag_name"]
            .as_str()
            .and_then(parse_release_version)
            .as_ref()
            == Some(current)
    {
        return release["published_at"]
            .as_str()
            .filter(|date| !date.is_empty());
    }
    None
}

fn nightly_catalog(
    releases: &[serde_json::Value],
    running: Option<&NightlyBuild>,
    fetch: &(impl Fn(&str) -> Result<Vec<u8>> + Sync),
) -> Result<Vec<RevertRelease>> {
    let running_publication = running.and_then(|running| {
        releases
            .iter()
            .find_map(|release| running_nightly_publication(release, running))
    });
    let candidates: Vec<_> = releases
        .iter()
        .filter_map(PublishedNightly::from_release)
        .collect();
    // The publisher keeps 14 nightlies. Two releases per worker allow for
    // the release list and four 15-second asset requests on the slowest
    // worker while staying within the client's 90-second history timeout.
    let chunk_size = candidates.len().div_ceil(NIGHTLY_HISTORY_WORKERS).max(1);
    let batches = std::thread::scope(|scope| {
        let workers: Vec<_> = candidates
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    let mut worker_fetch = fetch;
                    chunk
                        .iter()
                        .map(|release| {
                            let result = verified_history_nightly(
                                release,
                                running,
                                running_publication,
                                &mut worker_fetch,
                            );
                            (release, result)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join())
            .collect::<Vec<_>>()
    });
    let mut rows = Vec::new();
    let mut first_error = None;
    for batch in batches {
        let checked = batch.map_err(|_| anyhow::anyhow!("nightly history worker panicked"))?;
        for (release, result) in checked {
            match result {
                Ok(Some((order, _))) => rows.push((order, release)),
                Ok(None) => {}
                Err(error) => {
                    log::warn!("Nightly history skipped {}: {error}", release.tag);
                    first_error.get_or_insert(error);
                }
            }
        }
    }
    if rows.is_empty()
        && let Some(error) = first_error
    {
        return Err(error);
    }
    rows.sort_by_key(|(run, release)| Reverse((*run, release.day, release.published_at)));
    Ok(rows
        .into_iter()
        .map(|(run, release)| RevertRelease {
            version: release.tag.to_owned(),
            published_at: release.published_at.to_owned(),
            release_url: format!(
                "https://github.com/contember/okena/releases/tag/{}",
                release.tag
            ),
            asset_name: platform_asset_name().to_owned(),
            config_snapshot: None,
            legacy_nightly: run == NightlyHistoryOrder::Legacy,
            newer: false,
        })
        .collect())
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum NightlyHistoryOrder {
    Legacy,
    Modern(u64),
}

fn running_nightly_publication<'a>(
    release: &'a serde_json::Value,
    running: &NightlyBuild,
) -> Option<&'a str> {
    let release = PublishedNightly::from_release(release)?;
    if release.tag == running.tag && release.sha == running.sha {
        return release.comparable_publication();
    }
    None
}

fn verified_history_nightly(
    release: &PublishedNightly<'_>,
    running: Option<&NightlyBuild>,
    running_publication: Option<&str>,
    fetch: &mut impl FnMut(&str) -> Result<Vec<u8>>,
) -> Result<Option<(NightlyHistoryOrder, String)>> {
    if running.is_some_and(|build| build.tag == release.tag) {
        return Ok(None);
    }
    if let Some(url) = release.manifest_url {
        let Some(manifest) = release.load_manifest(url, fetch)? else {
            return Ok(None);
        };
        if running.is_some_and(|build| manifest.run_number >= build.run_number) {
            return Ok(None);
        }
        let sha256 = release.verify_checksums(Some(&manifest.bytes), fetch)?;
        return Ok(Some((
            NightlyHistoryOrder::Modern(manifest.run_number),
            sha256,
        )));
    }

    if let Some(running) = running {
        let Some((running_day, _)) = nightly_tag_parts(&running.tag) else {
            return Ok(None);
        };
        let published_earlier = release.day == running_day
            && release
                .comparable_publication()
                .zip(running_publication)
                .is_some_and(|(published, current)| published < current);
        if release.day >= running_day && !published_earlier {
            return Ok(None);
        }
    }
    let sha256 = release.verify_checksums(None, fetch)?;
    Ok(Some((NightlyHistoryOrder::Legacy, sha256)))
}

fn nightly_for_revert(
    app_version: &str,
    target: &str,
    running: Option<&NightlyBuild>,
) -> Result<(ReleaseAsset, String)> {
    anyhow::ensure!(nightly_tag_parts(target).is_some(), "invalid nightly tag");
    let response = fetch_json(
        &format!("{RELEASES_URL}/tags/{target}"),
        app_version,
        "updater.revert.resolve",
    )?;
    let needs_running_publication = PublishedNightly::from_release(&response)
        .filter(|release| release.tag == target && release.manifest_url.is_none())
        .zip(running)
        .is_some_and(|(release, running)| {
            release.tag != running.tag
                && nightly_tag_parts(&running.tag).is_some_and(|(day, _)| day == release.day)
                && release.comparable_publication().is_some()
        });
    let running_release = if needs_running_publication && let Some(running) = running {
        fetch_json(
            &format!("{RELEASES_URL}/tags/{}", running.tag),
            app_version,
            "updater.revert.running_release",
        )
        .ok()
    } else {
        None
    };
    let running_publication = running_release
        .as_ref()
        .zip(running)
        .and_then(|(release, running)| running_nightly_publication(release, running));
    let asset = resolve_nightly_revert(
        &response,
        target,
        running,
        running_publication,
        fetch_release_file,
    )?;
    Ok((
        asset,
        response["published_at"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    ))
}

fn resolve_nightly_revert(
    release: &serde_json::Value,
    target: &str,
    running: Option<&NightlyBuild>,
    running_publication: Option<&str>,
    mut fetch: impl FnMut(&str) -> Result<Vec<u8>>,
) -> Result<ReleaseAsset> {
    let release = PublishedNightly::from_release(release)
        .filter(|release| release.tag == target)
        .with_context(|| format!("{target} is not a published nightly for this platform"))?;
    let (_, sha256) = verified_history_nightly(&release, running, running_publication, &mut fetch)?
        .with_context(|| format!("{target} is not available for nightly history"))?;
    Ok(release.download(sha256))
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

fn release_for_revert_with(
    app_version: &str,
    target: &str,
    channel: UpdateChannel,
    running_nightly: bool,
    fetch_release: impl FnOnce(&str, &'static str) -> Result<serde_json::Value>,
    fetch: impl FnMut(&str) -> Result<Vec<u8>>,
) -> Result<(ReleaseAsset, String)> {
    let target = Version::parse(target).context("invalid target version")?;
    let current = Version::parse(app_version).context("invalid current version")?;
    if !stable_history_target(&target, &current, channel, running_nightly) {
        anyhow::bail!("release v{target} is unavailable for manual installation from v{current}");
    }
    let response = fetch_release(
        &format!("{RELEASES_URL}/tags/v{target}"),
        "updater.revert.resolve",
    )?;
    if !is_stable_release(&response)
        || !target.pre.is_empty()
        || response["tag_name"]
            .as_str()
            .and_then(parse_release_version)
            .as_ref()
            != Some(&target)
    {
        anyhow::bail!("release v{target} is not stable");
    }
    let asset = release_asset(&response, &target, fetch)?
        .with_context(|| format!("release v{target} has no asset for this platform"))?;
    Ok((
        asset,
        response["published_at"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    ))
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

/// Resolve a stable release's archive with the hash it must match. A release
/// without `SHA256SUMS` cannot be installed: nothing would verify the archive.
fn release_asset(
    release: &serde_json::Value,
    version: &Version,
    mut fetch: impl FnMut(&str) -> Result<Vec<u8>>,
) -> Result<Option<ReleaseAsset>> {
    let (asset_name, asset_url, checksum_url) = platform_asset(release)?;
    let Some(asset_url) = asset_url else {
        log::warn!("Release {version} exists but has no asset named '{asset_name}'");
        return Ok(None);
    };
    let checksum_url =
        checksum_url.with_context(|| format!("release v{version} has no {CHECKSUMS_NAME}"))?;
    let bytes = fetch(&checksum_url)?;
    let sums = std::str::from_utf8(&bytes).context("invalid checksum file encoding")?;
    let sha256 = listed_sha256(sums, &asset_name)
        .with_context(|| format!("release v{version} lists no valid checksum for '{asset_name}'"))?
        .to_ascii_lowercase();
    Ok(Some(ReleaseAsset {
        version: version.to_string(),
        asset_url,
        asset_name,
        sha256,
    }))
}

fn stable_history_target(
    version: &Version,
    current: &Version,
    channel: UpdateChannel,
    running_nightly: bool,
) -> bool {
    version.pre.is_empty()
        && match channel {
            UpdateChannel::Release => version < current,
            UpdateChannel::Nightly => running_nightly || version != current,
        }
}

fn catalog_release(
    release: &serde_json::Value,
    current: &Version,
    channel: UpdateChannel,
    running_nightly: bool,
    paths: Option<&okena_core::profiles::ProfilePaths>,
) -> Option<RevertRelease> {
    if !is_stable_release(release) {
        return None;
    }
    let tag = release["tag_name"].as_str()?;
    let version = Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()?;
    if !stable_history_target(&version, current, channel, running_nightly) {
        return None;
    }
    // Only releases that publish checksums can be installed, so only those are offered.
    let (asset_name, asset_url, checksum_url) = platform_asset(release).ok()?;
    asset_url.as_ref()?;
    checksum_url.as_ref()?;
    let version = version.to_string();
    Some(RevertRelease {
        legacy_nightly: false,
        newer: false,
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
        let offered = |version, draft, prerelease, has_asset| {
            let release = with_checksums(release(version, draft, prerelease, has_asset));
            catalog_release(&release, &current, UpdateChannel::Release, false, None).is_some()
        };
        assert!(offered("0.26.0", false, false, true));
        assert!(!offered("0.28.0", false, false, true));
        assert!(!offered("0.26.0", true, false, true));
        assert!(!offered("0.26.0", false, true, true));
        assert!(!offered("0.26.0", false, false, false));
    }

    /// nightly.yml publishes these as prereleases; the non-semver tag keeps
    /// them out even if the prerelease flag were lost.
    #[test]
    fn nightly_releases_are_excluded_from_stable_history() {
        let current = Version::parse("0.27.0").unwrap();
        for prerelease in [true, false] {
            let mut nightly = release("0.26.0", false, prerelease, true);
            nightly["tag_name"] = "nightly-20261120-50c64ec".into();
            assert!(
                catalog_release(&nightly, &current, UpdateChannel::Release, false, None).is_none()
            );
            assert!(published_notes(std::slice::from_ref(&nightly)).is_empty());
            assert!(
                stable_update_asset(&nightly, "0.27.0", true, |_| {
                    panic!("a nightly must be rejected before its checksums are fetched")
                })
                .unwrap()
                .is_none()
            );
        }
    }

    const STABLE_SUMS_URL: &str = "https://example.test/SHA256SUMS";

    fn with_checksums(mut release: serde_json::Value) -> serde_json::Value {
        release["assets"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "name": "SHA256SUMS",
                "browser_download_url": STABLE_SUMS_URL
            }));
        release
    }

    fn stable_sums(url: &str) -> Result<Vec<u8>> {
        assert_eq!(url, STABLE_SUMS_URL);
        Ok(format!("{}  {}\n", "AB".repeat(32), platform_asset_name()).into_bytes())
    }

    #[test]
    fn returning_to_release_allows_same_or_lower_version() {
        let stable = with_checksums(release("0.42.0", false, false, true));
        assert!(
            stable_update_asset(&stable, "0.42.0", false, stable_sums)
                .unwrap()
                .is_none()
        );
        assert!(
            stable_update_asset(&stable, "0.43.0", false, stable_sums)
                .unwrap()
                .is_none()
        );
        let update = stable_update_asset(&stable, "0.41.0", false, stable_sums)
            .unwrap()
            .unwrap();
        assert_eq!(update.version, "0.42.0");
        assert_eq!(update.sha256, "ab".repeat(32));
        for current in ["0.42.0", "0.43.0"] {
            assert_eq!(
                stable_update_asset(&stable, current, true, stable_sums)
                    .unwrap()
                    .unwrap()
                    .version,
                "0.42.0"
            );
        }
    }

    #[test]
    fn package_updates_wait_for_this_platforms_archive() {
        let without_archive = release("0.43.0", false, false, false);
        assert!(package_offer(&without_archive, "0.42.0").unwrap().is_none());
        let with_archive = release("0.43.0", false, false, true);
        assert_eq!(
            package_offer(&with_archive, "0.42.0").unwrap(),
            Some(Version::new(0, 43, 0))
        );
    }

    #[test]
    fn stable_installs_require_a_listed_checksum() {
        let unverifiable = release("0.40.0", false, false, true);
        let current = Version::parse("0.42.0").unwrap();
        let no_sums = |_: &str| -> Result<Vec<u8>> { panic!("no SHA256SUMS asset to fetch") };
        assert!(stable_update_asset(&unverifiable, "0.39.0", false, no_sums).is_err());
        assert!(
            catalog_release(&unverifiable, &current, UpdateChannel::Release, false, None).is_none()
        );
        let error = release_for_revert_with(
            "0.42.0",
            "0.40.0",
            UpdateChannel::Release,
            false,
            |_, _| Ok(unverifiable.clone()),
            no_sums,
        )
        .unwrap_err();
        assert!(error.to_string().contains("SHA256SUMS"), "{error}");

        let verifiable = with_checksums(release("0.40.0", false, false, true));
        assert!(
            catalog_release(&verifiable, &current, UpdateChannel::Release, false, None).is_some()
        );
        let (revert, _) = release_for_revert_with(
            "0.42.0",
            "0.40.0",
            UpdateChannel::Release,
            false,
            |_, _| Ok(verifiable.clone()),
            stable_sums,
        )
        .unwrap();
        assert_eq!(revert.sha256, "ab".repeat(32));

        let unlisted = |_: &str| Ok(format!("{}  other.zip\n", "ab".repeat(32)).into_bytes());
        assert!(
            release_for_revert_with(
                "0.42.0",
                "0.40.0",
                UpdateChannel::Release,
                false,
                |_, _| Ok(verifiable.clone()),
                unlisted
            )
            .is_err()
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

    fn legacy_fixture(day: &str, short_sha: &str) -> NightlyFixture {
        let (mut release, _, _) = nightly_fixture(short_sha, 1);
        let tag = format!("nightly-{day}-{short_sha}");
        release["tag_name"] = tag.clone().into();
        let assets = release["assets"].as_array_mut().unwrap();
        assets.pop();
        for asset in assets {
            let name = asset["name"].as_str().unwrap();
            asset["browser_download_url"] = format!("{RELEASE_DOWNLOADS_URL}/{tag}/{name}").into();
        }
        let sums = format!("{}  {}\n", "a".repeat(64), platform_asset_name());
        (release, Vec::new(), sums.into_bytes())
    }

    fn history_fixture_asset(fixtures: &[NightlyFixture], url: &str) -> Result<Vec<u8>> {
        for (release, manifest, sums) in fixtures {
            let tag = release["tag_name"].as_str().unwrap();
            if url == format!("{RELEASE_DOWNLOADS_URL}/{tag}/{NIGHTLY_MANIFEST_NAME}") {
                return Ok(manifest.clone());
            }
            if url == format!("{RELEASE_DOWNLOADS_URL}/{tag}/{CHECKSUMS_NAME}") {
                return Ok(sums.clone());
            }
        }
        panic!("unexpected asset URL {url}");
    }

    #[test]
    fn nightly_history_interleaves_stable_releases_and_older_nightly_runs_by_publication() {
        let running_fixture = nightly_fixture("3333333", 23);
        let running: NightlyBuild = serde_json::from_slice(&running_fixture.1).unwrap();
        let mut fixtures = [
            nightly_fixture("1111111", 19),
            running_fixture,
            nightly_fixture("2222222", 22),
            nightly_fixture("4444444", 24),
            legacy_fixture("20261121", "5555555"),
            legacy_fixture("20261122", "6666666"),
        ];
        let mut releases = vec![
            with_checksums(release("0.44.0", false, false, true)),
            with_checksums(release("0.42.0", false, false, true)),
            with_checksums(release("0.41.0", false, false, true)),
        ];
        fixtures[0].0["published_at"] = "2026-11-22T11:00:00Z".into();
        fixtures[2].0["published_at"] = "2026-11-22T11:50:00Z".into();
        fixtures[4].0["published_at"] = "2026-11-21T23:00:00Z".into();
        releases[0]["published_at"] = "2026-11-23T12:00:00Z".into();
        releases[1]["published_at"] = "2026-11-22T11:30:00Z".into();
        releases.extend(fixtures.iter().map(|fixture| fixture.0.clone()));

        let stable = catalog_for_channel(
            &releases,
            "0.42.0",
            UpdateChannel::Release,
            Some(&running),
            None,
            |_| panic!("stable history must not fetch nightly assets"),
        )
        .unwrap();
        assert_eq!(stable.channel, UpdateChannel::Release);
        assert_eq!(stable.releases.len(), 1);
        assert_eq!(stable.releases[0].version, "0.41.0");

        let nightly = catalog_for_channel(
            &releases,
            "0.42.0",
            UpdateChannel::Nightly,
            Some(&running),
            None,
            |url| history_fixture_asset(&fixtures, url),
        )
        .unwrap();
        assert_eq!(nightly.channel, UpdateChannel::Nightly);
        assert_eq!(nightly.current_version, "0.42.0");
        assert_eq!(
            nightly
                .releases
                .iter()
                .map(|row| row.version.as_str())
                .collect::<Vec<_>>(),
            [
                "0.44.0",
                "nightly-20261122-2222222",
                "0.42.0",
                "nightly-20261122-1111111",
                "nightly-20261121-5555555",
                "0.41.0",
            ],
        );
        assert!(
            nightly
                .releases
                .iter()
                .all(|row| row.config_snapshot.is_none())
        );
        assert!(!nightly.releases[1].legacy_nightly);
        assert!(nightly.releases[4].legacy_nightly);
        assert!(nightly.releases[0].newer);
        assert!(nightly.releases[1..].iter().all(|row| !row.newer));

        // Releasing the same version after the running nightly is an upgrade.
        releases[1]["published_at"] = "2026-11-22T12:10:00Z".into();
        let after_stable_release = catalog_for_channel(
            &releases,
            "0.42.0",
            UpdateChannel::Nightly,
            Some(&running),
            None,
            |url| history_fixture_asset(&fixtures, url),
        )
        .unwrap();
        assert!(
            after_stable_release
                .releases
                .iter()
                .any(|row| row.version == "0.42.0" && row.newer)
        );

        let mut current_release = with_checksums(release("0.43.0", false, false, true));
        current_release["published_at"] = "2026-11-22T11:30:00Z".into();
        releases.push(current_release);
        let stable_running = catalog_for_channel(
            &releases,
            "0.43.0",
            UpdateChannel::Nightly,
            None,
            None,
            |url| history_fixture_asset(&fixtures, url),
        )
        .unwrap();
        assert_eq!(stable_running.current_version, "0.43.0");
        assert_eq!(stable_running.releases.len(), 9);
        assert!(
            stable_running
                .releases
                .iter()
                .any(|row| row.version == "nightly-20261122-2222222" && row.newer)
        );
        assert!(
            stable_running
                .releases
                .iter()
                .any(|row| row.version == "nightly-20261122-1111111" && !row.newer)
        );
    }

    #[test]
    fn equal_version_needs_proof_of_publication_order() {
        let mut fixture = nightly_fixture("3333333", 23);
        let running: NightlyBuild = serde_json::from_slice(&fixture.1).unwrap();
        let current = Version::new(0, 42, 0);
        let target_publication = "2026-11-23T00:06:00Z";

        assert!(!history_install_is_newer(
            Some(&current),
            &current,
            true,
            target_publication,
            None,
        ));
        fixture.0["published_at"] = "2026-11-23T00:05:00Z".into();
        let published_at = history_running_publication(&fixture.0, &current, Some(&running));
        assert_eq!(published_at, Some("2026-11-23T00:05:00Z"));
        assert!(history_install_is_newer(
            Some(&current),
            &current,
            true,
            target_publication,
            published_at,
        ));
    }

    #[test]
    fn stable_versions_are_available_when_nightly_metadata_is_unreachable() {
        let fixtures = [
            with_checksums(release("0.44.0", false, false, true)),
            nightly_fixture("1111111", 19).0,
        ];
        let catalog = catalog_for_channel(
            &fixtures,
            "0.43.0",
            UpdateChannel::Nightly,
            None,
            None,
            |_| anyhow::bail!("nightly asset request failed"),
        )
        .unwrap();
        assert_eq!(catalog.releases.len(), 1);
        assert_eq!(catalog.releases[0].version, "0.44.0");
        assert!(catalog.releases[0].newer);
    }

    #[test]
    fn newer_stable_can_be_installed_explicitly_while_following_nightly() {
        let release = with_checksums(release("0.44.0", false, false, true));
        let (asset, _) = release_for_revert_with(
            "0.43.0",
            "0.44.0",
            UpdateChannel::Nightly,
            true,
            |url, _| {
                assert!(url.ends_with("/tags/v0.44.0"));
                Ok(release.clone())
            },
            stable_sums,
        )
        .unwrap();
        assert_eq!(asset.version, "0.44.0");

        assert!(
            release_for_revert_with(
                "0.43.0",
                "0.44.0",
                UpdateChannel::Release,
                false,
                |_, _| panic!("Release history cannot install a newer target"),
                stable_sums,
            )
            .is_err()
        );
    }

    #[test]
    fn exact_nightly_revert_applies_order_identity_and_checksum_rules() {
        let running = nightly_fixture("3333333", 23);
        let mut corrupt_modern = legacy_fixture("20261120", "2222222");
        corrupt_modern.2 = nightly_test_sums(b"missing modern manifest");
        let mut wrong_checksum = legacy_fixture("20261120", "6666666");
        wrong_checksum.2[0] = b'g';
        let mut wrong_target = legacy_fixture("20261119", "8888888");
        wrong_target.0["target_commitish"] = "9".repeat(40).into();
        let mut invalid_manifest = nightly_fixture("9999999", 18);
        invalid_manifest.1 = b"malformed manifest".to_vec();
        invalid_manifest.2 = nightly_test_sums(&invalid_manifest.1);
        let fixtures = [
            running,
            corrupt_modern,
            wrong_checksum,
            wrong_target,
            nightly_fixture("1111111", 19),
            nightly_fixture("4444444", 24),
            legacy_fixture("20261121", "5555555"),
            legacy_fixture("20261122", "7777777"),
            invalid_manifest,
        ];
        let running: NightlyBuild = serde_json::from_slice(&fixtures[0].1).unwrap();
        for (index, fixture) in fixtures.iter().enumerate() {
            let tag = fixture.0["tag_name"].as_str().unwrap();
            let result = resolve_nightly_revert(&fixture.0, tag, Some(&running), None, |url| {
                history_fixture_asset(&fixtures, url)
            });
            if matches!(index, 4 | 6) {
                let asset = result.unwrap();
                assert_eq!(asset.version, tag);
                assert_eq!(asset.sha256, "a".repeat(64));
            } else {
                assert!(result.is_err(), "accepted ineligible release {tag}");
            }
        }
        assert!(
            resolve_nightly_revert(
                &fixtures[4].0,
                "nightly-20261122-abcdef0",
                Some(&running),
                None,
                |_| panic!("wrong exact target must be rejected before fetching assets"),
            )
            .is_err()
        );
    }

    #[test]
    fn same_day_legacy_history_requires_proof_of_earlier_publication() {
        let running = nightly_fixture("3333333", 23);
        let build: NightlyBuild = serde_json::from_slice(&running.1).unwrap();
        let mut older = legacy_fixture("20261122", "1111111");
        older.0["published_at"] = "2026-11-22T11:59:59Z".into();
        let mut later = legacy_fixture("20261122", "2222222");
        later.0["published_at"] = "2026-11-22T12:00:01Z".into();
        let fixtures = [older, later, running];
        let releases: Vec<_> = fixtures.iter().map(|fixture| fixture.0.clone()).collect();
        let fetch = |url: &str| history_fixture_asset(&fixtures, url);
        assert!(
            nightly_catalog(&releases[..2], Some(&build), &fetch)
                .unwrap()
                .is_empty()
        );
        let available = nightly_catalog(&releases, Some(&build), &fetch).unwrap();
        assert_eq!(available.len(), 1);
        assert_eq!(available[0].version, "nightly-20261122-1111111");
        assert!(
            resolve_nightly_revert(
                &releases[0],
                &available[0].version,
                Some(&build),
                running_nightly_publication(&releases[2], &build),
                fetch,
            )
            .is_ok()
        );
    }

    #[test]
    fn nightly_history_reports_worker_panics_as_a_catalog_error() {
        let releases = [
            nightly_fixture("1111111", 19).0,
            nightly_fixture("2222222", 20).0,
        ];
        let error = nightly_catalog(&releases, None, &|_| panic!("test worker panic")).unwrap_err();
        assert_eq!(error.to_string(), "nightly history worker panicked");
    }

    #[test]
    fn nightly_history_checks_distinct_releases_concurrently() {
        let fixtures = [
            nightly_fixture("1111111", 19),
            nightly_fixture("2222222", 20),
        ];
        let releases: Vec<_> = fixtures.iter().map(|fixture| fixture.0.clone()).collect();
        let started = std::sync::Mutex::new(0);
        let both_ready = std::sync::Condvar::new();
        let rows = nightly_catalog(&releases, None, &|url| {
            if url.ends_with(NIGHTLY_MANIFEST_NAME) {
                let mut count = started.lock().unwrap();
                *count += 1;
                both_ready.notify_all();
                let (count, _) = both_ready
                    .wait_timeout_while(count, Duration::from_secs(5), |count| *count < 2)
                    .unwrap();
                anyhow::ensure!(*count == 2, "another release's verification did not start");
            }
            history_fixture_asset(&fixtures, url)
        })
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].version, "nightly-20261122-2222222");
    }

    #[test]
    fn older_release_catalogs_default_to_release_history() {
        let catalog: ReleaseCatalog = serde_json::from_value(serde_json::json!({
            "current_version": "0.42.0",
            "releases": [{
                "version": "0.41.0",
                "published_at": "2026-11-21T12:00:00Z",
                "release_url": "https://example.test/releases/v0.41.0",
                "asset_name": platform_asset_name()
            }]
        }))
        .unwrap();
        assert_eq!(catalog.channel, UpdateChannel::Release);
        assert!(!catalog.releases[0].legacy_nightly);
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
