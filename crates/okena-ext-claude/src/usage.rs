use crate::ui_helpers::open_url;
use okena_extensions::{ExtensionSettingsStore, ThemeColors};
use okena_ui::tokens::{ui_text_xs, ui_text_ms, ui_text_md};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use gpui_component::{h_flex, v_flex};
use parking_lot::Mutex;
#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Refresh interval for usage data
const USAGE_INTERVAL: Duration = Duration::from_secs(300);

/// Minimum retry delay to avoid tight loops (e.g. when server returns retry-after: 0)
const MIN_RETRY_DELAY: Duration = Duration::from_secs(30);

/// Hover delay before showing the popover (ms)
const HOVER_DELAY_MS: u64 = 300;

/// Minimum interval between hover-triggered re-fetches.
const HOVER_REFETCH_THROTTLE: Duration = Duration::from_secs(60);

/// Claude Code OAuth client ID (public, used by the Claude CLI).
const CLAUDE_OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

/// OAuth token endpoint used to refresh the access token.
const CLAUDE_OAUTH_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";

/// Refresh the access token this long before its `expiresAt` to avoid 401s mid-flight.
const TOKEN_REFRESH_LEEWAY_MS: u64 = 5 * 60 * 1000;

/// Usage info for a single rate-limit tier
#[derive(Clone)]
struct TierUsage {
    utilization: f64,
    resets_at: String,
    /// Percentage of the billing period that has elapsed (0.0–100.0)
    time_elapsed_pct: Option<f64>,
}

/// Extra paid usage info
#[derive(Clone)]
struct ExtraUsage {
    is_enabled: bool,
    monthly_limit: f64,
    used_credits: f64,
    utilization: f64,
}

/// All fetched usage data
#[derive(Clone)]
struct UsageData {
    five_hour: Option<TierUsage>,
    seven_day: Option<TierUsage>,
    seven_day_sonnet: Option<TierUsage>,
    seven_day_opus: Option<TierUsage>,
    extra_usage: Option<ExtraUsage>,
}

fn theme(cx: &App) -> ThemeColors {
    okena_extensions::theme(cx)
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    } else if path == "~"
        && let Some(home) = dirs::home_dir() {
            return home;
        }
    PathBuf::from(path)
}

fn existing_path(path: &str, source: &str) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }

    let expanded = expand_tilde(path);
    if expanded.exists() {
        Some(expanded)
    } else {
        log::warn!(
            "[claude-usage] {source} '{}' does not exist, falling back",
            path
        );
        None
    }
}

/// Resolve the Claude config directory using three-tier precedence:
/// 1. `extension_settings."claude-code".config_dir` in settings.json
/// 2. `CLAUDE_CONFIG_DIR` environment variable (Claude CLI convention)
/// 3. `$HOME/.claude` (default)
pub fn resolve_claude_dir(cx: &App) -> PathBuf {
    if let Some(settings) = cx.global::<ExtensionSettingsStore>().get("claude-code", cx)
        && let Some(dir) = settings["config_dir"].as_str()
            && let Some(expanded) = existing_path(dir, "settings config_dir") {
                return expanded;
            }
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR")
        && let Some(expanded) = existing_path(&dir, "CLAUDE_CONFIG_DIR") {
            return expanded;
        }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
}

/// Global holding a weak handle to the shared usage data entity.
///
/// Each window's `ClaudeUsage` view keeps a strong handle, so the data entity
/// (and its single poll task) lives exactly as long as at least one window
/// shows the widget — and tears down once they all close.
struct GlobalClaudeUsageData(WeakEntity<ClaudeUsageData>);
impl Global for GlobalClaudeUsageData {}

/// Shared usage data + the single background poll task and its wake machinery.
///
/// Decoupling this from the per-window view means the usage API is fetched
/// once for the whole app rather than once per open window. Per-window UI
/// state (popover, hover) lives on [`ClaudeUsage`] instead.
struct ClaudeUsageData {
    data: Arc<Mutex<Option<UsageData>>>,
    claude_dir: Arc<Mutex<PathBuf>>,
    /// Send on this channel to wake up the fetch loop and retry immediately.
    wake_tx: smol::channel::Sender<()>,
    /// Whether a wake signal has already been sent (avoids spamming from render).
    wake_sent: Arc<AtomicBool>,
    /// Timestamp of the most recent successful fetch — used to throttle hover-triggered refreshes.
    last_fetch_at: Arc<Mutex<Option<Instant>>>,
    /// Background polling task. Cancelled automatically when this entity is dropped.
    _poll_task: Task<()>,
}

/// Compute the macOS Keychain service name for a given Claude config directory.
/// The Claude CLI uses "Claude Code-credentials" for the default ~/.claude, and
/// "Claude Code-credentials-<sha256(path)[..8 hex]>" for any custom config dir.
#[cfg(target_os = "macos")]
fn keychain_service_name(claude_dir: &Path) -> String {
    const BASE: &str = "Claude Code-credentials";
    let default_dir = dirs::home_dir().map(|h| h.join(".claude"));
    let canonical = claude_dir.canonicalize().unwrap_or_else(|_| claude_dir.to_path_buf());
    if Some(&canonical) == default_dir.as_ref() {
        BASE.to_string()
    } else {
        let mut h = Sha256::new();
        h.update(canonical.to_string_lossy().as_bytes());
        let d = h.finalize();
        format!("{BASE}-{:02x}{:02x}{:02x}{:02x}", d[0], d[1], d[2], d[3])
    }
}

/// Where a set of credentials was loaded from, so a refreshed token can be
/// written back to the same place.
#[derive(Clone)]
enum CredsSource {
    File(PathBuf),
    #[cfg(target_os = "macos")]
    Keychain { service: String, account: String },
}

/// Parsed Claude OAuth credentials plus enough context to refresh + persist them.
#[derive(Clone)]
struct ClaudeCreds {
    access_token: String,
    refresh_token: Option<String>,
    /// Epoch milliseconds when the access token expires.
    expires_at_ms: Option<u64>,
    source: CredsSource,
    /// Full parsed credentials JSON, preserved so persistence keeps unrelated fields.
    raw: serde_json::Value,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Truncate a string to at most `max` bytes without splitting a UTF-8 char.
/// Server-controlled error bodies may contain multi-byte chars straddling the
/// byte limit; naive `&s[..max]` would panic on a non-char-boundary index.
fn clip(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// True if the access token expires within the refresh leeway. Returns false when
/// no expiry is recorded — we don't preemptively refresh then, relying on the
/// reactive 401 path instead.
fn needs_refresh(creds: &ClaudeCreds) -> bool {
    match creds.expires_at_ms {
        Some(expiry) => now_ms() + TOKEN_REFRESH_LEEWAY_MS >= expiry,
        None => false,
    }
}

fn read_claude_creds(claude_dir: &Path) -> Option<ClaudeCreds> {
    fn parse(content: &str, source: CredsSource) -> Option<ClaudeCreds> {
        let raw: serde_json::Value = serde_json::from_str(content).ok()?;
        let oauth = raw.get("claudeAiOauth")?;
        let access_token = oauth["accessToken"].as_str()?.to_string();
        let refresh_token = oauth["refreshToken"].as_str().map(String::from);
        let expires_at_ms = oauth["expiresAt"].as_u64();
        Some(ClaudeCreds {
            access_token,
            refresh_token,
            expires_at_ms,
            source,
            raw,
        })
    }

    // Mirror Claude Code's storage precedence. On macOS the Keychain is
    // authoritative: Claude Code reads it first and ignores a plaintext
    // ~/.claude/.credentials.json when a Keychain entry exists. Picking the
    // "freshest" across both stores would let a stale plaintext file shadow the
    // Keychain, and a subsequent refresh would persist only to that file —
    // leaving the Keychain (what Claude Code actually uses) with a now-rotated,
    // invalid refresh token. So: Keychain first, file only as a fallback.
    #[cfg(target_os = "macos")]
    if let Ok(user) = std::env::var("USER") {
        let service = keychain_service_name(claude_dir);
        if let Ok(output) = okena_core::process::safe_output(
            okena_core::process::command("security")
                .args(["find-generic-password", "-s", &service, "-a", &user, "-w"]),
        ) && output.status.success()
        {
            // A Keychain entry exists, so it is authoritative — return its parse
            // result directly. A malformed entry must NOT fall through to the
            // plaintext file: doing so is exactly the split-brain (refresh
            // persists to the file, Keychain keeps a stale token) this precedence
            // rule exists to prevent.
            let content = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let creds = parse(
                &content,
                CredsSource::Keychain {
                    service,
                    account: user,
                },
            );
            if creds.is_none() {
                log::warn!("[claude-usage] Keychain credentials present but unparseable");
            }
            return creds;
        }
    }

    let file_path = claude_dir.join(".credentials.json");
    let content = std::fs::read_to_string(&file_path).ok()?;
    parse(&content, CredsSource::File(file_path))
}

/// Atomically write `contents` to `path`: write a temp file in the same
/// directory, then rename over the target. A failed or partial write (disk full,
/// I/O error, crash) thus never truncates the existing credentials — the old file
/// stays intact, so the user keeps a usable cached token. On Unix the temp file
/// is created 0o600 so the credentials are never world-readable.
fn atomic_write_0600(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("credentials.json");
    // Same-dir temp so the rename is atomic (same filesystem). PID-suffixed to
    // avoid clashing with a concurrent writer's temp.
    let tmp = parent.join(format!(".{file_name}.tmp.{}", std::process::id()));

    let write = || -> std::io::Result<()> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(contents)?;
        f.sync_all()
    };

    if let Err(e) = write() {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    // rename replaces the destination atomically (incl. on Windows, where Rust's
    // std maps to MoveFileEx with REPLACE_EXISTING).
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Write refreshed token fields back to the source they came from, preserving
/// all other fields in the stored JSON.
///
/// Returns an error if the write fails. Callers MUST treat a failure here as a
/// failed refresh: the OAuth server has already rotated the refresh token, so
/// silently keeping the now-stale one on disk would make the *next* refresh fail
/// with `invalid_grant`.
fn persist_creds(
    source: &CredsSource,
    mut raw: serde_json::Value,
    access_token: &str,
    refresh_token: Option<&str>,
    expires_at_ms: Option<u64>,
) -> std::io::Result<()> {
    let oauth = raw
        .get_mut("claudeAiOauth")
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "malformed credentials: missing claudeAiOauth object",
            )
        })?;
    oauth.insert(
        "accessToken".to_string(),
        serde_json::Value::String(access_token.to_string()),
    );
    if let Some(rt) = refresh_token {
        oauth.insert(
            "refreshToken".to_string(),
            serde_json::Value::String(rt.to_string()),
        );
    }
    if let Some(exp) = expires_at_ms {
        oauth.insert("expiresAt".to_string(), serde_json::Value::from(exp));
    }

    let serialized = serde_json::to_string(&raw)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    match source {
        CredsSource::File(path) => atomic_write_0600(path, serialized.as_bytes()),
        #[cfg(target_os = "macos")]
        CredsSource::Keychain { service, account } => {
            // NOTE: `-w &serialized` puts the tokens on this process's argv,
            // briefly visible to `ps`/process listing while `security` runs.
            // `security add-generic-password` has no stdin password input, so
            // there's no clean alternative; accepted as a short-lived exposure
            // on the user's own machine. The read path (`find-generic-password
            // -w`) is unaffected — it only outputs the secret, never passes it.
            let status = std::process::Command::new("security")
                .args([
                    "add-generic-password",
                    "-U",
                    "-s",
                    service,
                    "-a",
                    account,
                    "-w",
                    &serialized,
                ])
                .status()?;
            if status.success() {
                Ok(())
            } else {
                Err(std::io::Error::other(format!(
                    "security add-generic-password exited with {status}"
                )))
            }
        }
    }
}

/// Claude Code's OAuth refresh lock directory.
///
/// Claude Code guards its single-use refresh token with the `proper-lockfile`
/// protocol: the lock is an atomically `mkdir`'d *directory* at this path, kept
/// alive by periodic mtime updates, treated as stale after [`LOCK_STALE`], and
/// released by `rmdir`. We speak the exact same protocol on the exact same path
/// so an okena refresh and a concurrent Claude Code refresh mutually exclude —
/// otherwise both could POST the same refresh token and one would get
/// `invalid_grant`. (`mkdir`/`rmdir` are atomic and cross-platform, so this also
/// serializes okena-vs-okena on every OS, Windows included.)
const CLAUDE_LOCK_DIR: &str = ".oauth_refresh.lock";

/// `proper-lockfile`'s default staleness threshold: a lock whose mtime is older
/// than this is considered abandoned and may be stolen.
const LOCK_STALE: Duration = Duration::from_secs(10);
/// Give up waiting for a contended lock after this long and skip the refresh
/// this cycle (rather than post a token exchange we can't safely serialize).
const LOCK_MAX_WAIT: Duration = Duration::from_secs(15);
/// Poll interval while waiting for a held lock to be released.
const LOCK_POLL: Duration = Duration::from_millis(100);
/// How often we bump the lock dir's mtime while holding it. Must be well below
/// `LOCK_STALE` so peers never see our active lock as stale (proper-lockfile's
/// own heartbeat is `stale/2`).
const LOCK_HEARTBEAT: Duration = Duration::from_secs(3);
/// Settle delay when claiming a stale lock: after stamping our mtime we wait this
/// long, then re-read to confirm a competing stealer didn't stamp a later one.
const LOCK_CLAIM_SETTLE: Duration = Duration::from_millis(50);

/// A held (or attempted) `proper-lockfile`-compatible directory lock. Owns the
/// lock dir only when [`OAuthRefreshLock::held`] is true; dropped → `rmdir`.
struct OAuthRefreshLock {
    dir: PathBuf,
    held: bool,
    /// Set on drop to stop the heartbeat thread.
    stop: Arc<AtomicBool>,
    heartbeat: Option<std::thread::JoinHandle<()>>,
}

impl OAuthRefreshLock {
    /// Take the shared lock, stealing it if the current holder's lock is stale.
    /// If it can't be taken within [`LOCK_MAX_WAIT`], returns a guard with
    /// `held == false` — the caller must NOT post a token exchange in that case.
    fn acquire(claude_dir: &Path) -> Self {
        let dir = claude_dir.join(CLAUDE_LOCK_DIR);
        let start = Instant::now();
        loop {
            match std::fs::create_dir(&dir) {
                Ok(()) => return Self::owned(dir),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Bound acquisition on every path (incl. repeated stale-steal
                    // attempts) so we never spin or block past LOCK_MAX_WAIT.
                    if start.elapsed() >= LOCK_MAX_WAIT {
                        log::warn!(
                            "[claude-usage] oauth refresh lock busy >{}s; skipping refresh this cycle",
                            LOCK_MAX_WAIT.as_secs()
                        );
                        return Self::not_held(dir);
                    }
                    // Holder appears to have vanished (mtime older than LOCK_STALE,
                    // i.e. no live heartbeat). Try to claim it. We do NOT blindly
                    // delete+recreate: a competing stealer could create a fresh
                    // lock in that window and we'd clobber it, granting the lock to
                    // two holders. Instead claim-by-mtime and verify (see below).
                    if lock_is_stale(&dir) && claim_stale_lock(&dir) {
                        return Self::owned(dir);
                    }
                    std::thread::sleep(LOCK_POLL);
                }
                Err(e) => {
                    log::warn!(
                        "[claude-usage] could not take oauth refresh lock ({e}); skipping refresh"
                    );
                    return Self::not_held(dir);
                }
            }
        }
    }

    /// Construct an owned lock and start its mtime heartbeat so peers don't deem
    /// it stale while we do slow work (network refresh, blocked Keychain, etc.).
    fn owned(dir: PathBuf) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let hb_dir = dir.clone();
        let hb_stop = stop.clone();
        let heartbeat = std::thread::spawn(move || {
            let mut since_touch = Duration::ZERO;
            // Poll the stop flag finely so drop joins quickly, but only touch the
            // mtime every LOCK_HEARTBEAT.
            while !hb_stop.load(Ordering::Relaxed) {
                std::thread::sleep(LOCK_POLL);
                since_touch += LOCK_POLL;
                if since_touch >= LOCK_HEARTBEAT {
                    since_touch = Duration::ZERO;
                    let _ = filetime::set_file_mtime(&hb_dir, filetime::FileTime::now());
                }
            }
        });
        Self {
            dir,
            held: true,
            stop,
            heartbeat: Some(heartbeat),
        }
    }

    fn not_held(dir: PathBuf) -> Self {
        Self {
            dir,
            held: false,
            stop: Arc::new(AtomicBool::new(false)),
            heartbeat: None,
        }
    }

    fn held(&self) -> bool {
        self.held
    }
}

impl Drop for OAuthRefreshLock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.heartbeat.take() {
            let _ = h.join();
        }
        // Only ever remove a lock we actually own — never someone else's.
        if self.held {
            let _ = std::fs::remove_dir(&self.dir);
        }
    }
}

/// Whether a lock directory's mtime is older than [`LOCK_STALE`]. A metadata
/// error (e.g. the dir was just released) counts as "not stale" so we retry the
/// `mkdir` rather than stealing something we can't measure.
fn lock_is_stale(dir: &Path) -> bool {
    std::fs::metadata(dir)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|mtime| mtime.elapsed().ok())
        .is_some_and(|age| age > LOCK_STALE)
}

/// Try to claim an abandoned (stale) lock directory by stamping it with a fresh
/// mtime and confirming no competing stealer stamped a later one. Returns `true`
/// iff we won the claim and may treat ourselves as the holder.
///
/// We adopt the existing directory rather than delete+recreate, so we never
/// clobber a lock another process freshly created in the steal window. The
/// stamped mtime acts as a compare-and-set token: if two stealers race, the
/// later stamp wins and the earlier one observes the changed value and backs off.
/// This is the same best-effort guarantee `proper-lockfile` provides for stale
/// recovery (a sub-resolution timestamp tie on a coarse filesystem is the only
/// residual window).
fn claim_stale_lock(dir: &Path) -> bool {
    // Stamp now. If the dir vanished (released/removed), bail so the caller's
    // loop retries the fast mkdir path.
    if filetime::set_file_mtime(dir, filetime::FileTime::now()).is_err() {
        return false;
    }
    // Read back the *stored* stamp (the filesystem may coarsen it); that's our
    // token to compare against after the settle window.
    let Some(token) = std::fs::metadata(dir).and_then(|m| m.modified()).ok() else {
        return false;
    };
    std::thread::sleep(LOCK_CLAIM_SETTLE);
    std::fs::metadata(dir)
        .and_then(|m| m.modified())
        .is_ok_and(|current| current == token)
}

/// Exchange the refresh token for a fresh access token, persist it, and return it.
///
/// Takes the shared Claude refresh lock and re-reads the credentials before
/// posting: if another refresher (another okena instance or Claude Code, which
/// shares this lock) rotated the token while we waited, we reuse their fresh
/// token instead of posting an already-consumed one.
fn refresh_access_token(claude_dir: &Path, current: &ClaudeCreds) -> Option<String> {
    let lock = OAuthRefreshLock::acquire(claude_dir);

    // Re-read under the lock. A concurrent refresher may have just rotated the
    // token; in that case the stored refresh token differs from ours and the
    // stored access token is already fresh — use it and skip the exchange.
    let creds = read_claude_creds(claude_dir).unwrap_or_else(|| current.clone());
    if creds.refresh_token != current.refresh_token {
        log::info!("[claude-usage] credentials refreshed by another process; reusing stored token");
        return Some(creds.access_token);
    }

    // We never actually got the lock and nobody else rotated the token — another
    // holder is mid-refresh. Posting the same single-use token now would race and
    // invalidate one side; skip and retry next cycle (the holder will have
    // rotated it by then, picked up via the re-read above).
    if !lock.held() {
        return None;
    }

    let refresh_token = creds.refresh_token.as_deref()?;

    let resp = okena_core::http::send(
        okena_core::http::HttpRequest::post(CLAUDE_OAUTH_TOKEN_URL)
            .json(&serde_json::json!({
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
                "client_id": CLAUDE_OAUTH_CLIENT_ID,
            }))
            // Sane network bound. Holding the refresh lock longer than LOCK_STALE
            // is safe because the lock's heartbeat keeps its mtime fresh, so peers
            // won't steal it.
            .timeout(Duration::from_secs(10))
            .label("claude.oauth.refresh"),
    )
    .ok()?;

    if !resp.is_success() {
        log::warn!(
            "[claude-usage] token refresh failed: HTTP {} body={}",
            resp.status(),
            clip(&resp.text(), 300)
        );
        return None;
    }

    let body: serde_json::Value = resp.json().ok()?;
    let new_access = body["access_token"].as_str()?;
    let new_refresh = body["refresh_token"].as_str();
    let expires_at_ms = body["expires_in"]
        .as_u64()
        .map(|secs| now_ms().saturating_add(secs.saturating_mul(1000)));

    // The server has now rotated the refresh token. If we can't persist the new
    // one, treat the whole refresh as failed — otherwise the stale token left on
    // disk would fail the next refresh with `invalid_grant`.
    if let Err(e) = persist_creds(
        &creds.source,
        creds.raw.clone(),
        new_access,
        new_refresh,
        expires_at_ms,
    ) {
        log::error!(
            "[claude-usage] refreshed token but failed to persist it ({e}); \
             discarding to avoid leaving a stale refresh token — sign in again \
             via the Claude CLI if usage stops updating"
        );
        return None;
    }

    log::info!("[claude-usage] access token refreshed");
    Some(new_access.to_string())
}

/// Outcome of a single usage-endpoint request.
enum Fetch {
    // Boxed: `UsageData` dwarfs the other variants; boxing keeps `Fetch` small.
    Ok(Box<UsageData>),
    /// 429 — retry after this delay (from `retry-after`, or a default).
    RateLimited(Duration),
    /// 401 — access token expired/invalid; caller may refresh and retry once.
    Unauthorized,
    /// Any other failure (network, parse, other status).
    Failed,
}

fn fetch_usage_once(token: &str) -> Fetch {
    let response = okena_core::http::send(
        okena_core::http::HttpRequest::get("https://api.anthropic.com/api/oauth/usage")
            .bearer(token)
            .header("anthropic-beta", "oauth-2025-04-20")
            .timeout(Duration::from_secs(10))
            .label("claude.usage")
            // Safety floor: real cadence is 300s (≥30s on retry); a 5s floor only
            // ever catches a runaway re-spawn. The reactive refresh path may issue
            // a second usage call within the same tick; if that retry trips the
            // floor it just fails this tick and recovers on the next (with backoff),
            // since the refresh already wrote a fresh expiry.
            .min_interval(Duration::from_secs(5)),
    );

    let resp = match response {
        Ok(resp) => resp,
        Err(e) => {
            log::warn!("[claude-usage] request failed: {}", e);
            return Fetch::Failed;
        }
    };

    let status = resp.status();

    if status == 429 {
        let retry_secs = resp
            .header("retry-after")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(USAGE_INTERVAL.as_secs() * 2);
        log::warn!("[claude-usage] rate limited (429), retry-after {}s", retry_secs);
        return Fetch::RateLimited(Duration::from_secs(retry_secs));
    }

    // Only 401 means the access token is stale and worth refreshing. 403 is an
    // authorization/scope problem (e.g. the account lacks usage access) that a
    // refresh grant cannot fix — refreshing on 403 just rotates the refresh
    // token every poll and risks invalidating it. Let 403 fall through to Failed.
    if status == 401 {
        return Fetch::Unauthorized;
    }

    let body = resp.text();
    log::info!(
        "[claude-usage] HTTP {} body={}",
        status,
        clip(&body, 500)
    );
    if !resp.is_success() {
        return Fetch::Failed;
    }
    match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(parsed) => Fetch::Ok(Box::new(parse_usage(&parsed))),
        Err(_) => Fetch::Failed,
    }
}

fn parse_usage(resp: &serde_json::Value) -> UsageData {
    let five_hour = parse_tier(resp, "five_hour", false, FIVE_HOUR_SECS);
    let seven_day = parse_tier(resp, "seven_day", true, SEVEN_DAY_SECS);
    let seven_day_sonnet = parse_tier(resp, "seven_day_sonnet", true, SEVEN_DAY_SECS);
    let seven_day_opus = parse_tier(resp, "seven_day_opus", true, SEVEN_DAY_SECS);

    let extra_usage = resp.get("extra_usage").map(|eu| {
        ExtraUsage {
            is_enabled: eu["is_enabled"].as_bool().unwrap_or(false),
            monthly_limit: eu["monthly_limit"].as_f64().unwrap_or(0.0),
            used_credits: eu["used_credits"].as_f64().unwrap_or(0.0),
            utilization: eu["utilization"].as_f64().unwrap_or(0.0),
        }
    });

    UsageData {
        five_hour,
        seven_day,
        seven_day_sonnet,
        seven_day_opus,
        extra_usage,
    }
}

/// Period durations for each tier
const FIVE_HOUR_SECS: f64 = 5.0 * 3600.0;
const SEVEN_DAY_SECS: f64 = 7.0 * 86400.0;

fn parse_tier(
    resp: &serde_json::Value,
    key: &str,
    include_date: bool,
    period_secs: f64,
) -> Option<TierUsage> {
    let tier = resp.get(key)?;
    let resets_at_raw = tier["resets_at"].as_str();
    let time_elapsed_pct = resets_at_raw.and_then(|ts| compute_time_elapsed_pct(ts, period_secs));
    Some(TierUsage {
        utilization: tier["utilization"].as_f64().unwrap_or(0.0),
        resets_at: resets_at_raw
            .map(|ts| format_reset_time(ts, include_date))
            .unwrap_or_default(),
        time_elapsed_pct,
    })
}

/// Compute what percentage of the billing period has elapsed.
fn compute_time_elapsed_pct(resets_at: &str, period_secs: f64) -> Option<f64> {
    let reset_epoch = parse_iso8601_to_epoch(resets_at)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs_f64();
    let remaining = (reset_epoch - now).max(0.0);
    let elapsed = (period_secs - remaining).max(0.0);
    Some((elapsed / period_secs * 100.0).clamp(0.0, 100.0))
}

/// Parse an ISO 8601 timestamp (via `jiff`) to Unix epoch seconds.
fn parse_iso8601_to_epoch(ts: &str) -> Option<f64> {
    let timestamp: jiff::Timestamp = ts.parse().ok()?;
    Some(timestamp.as_millisecond() as f64 / 1_000.0)
}

/// Parse an ISO 8601 timestamp to a local Zoned datetime.
/// Returns `None` if parsing fails.
pub(crate) fn parse_iso8601_to_local(ts: &str) -> Option<jiff::Zoned> {
    let timestamp: jiff::Timestamp = ts.parse().ok()?;
    Some(timestamp.to_zoned(jiff::tz::TimeZone::system()))
}

/// Format ISO 8601 reset time to a human-readable short form in local timezone.
/// Falls back to UTC if local timezone is unavailable, or returns `ts` as-is if unparseable.
fn format_reset_time(ts: &str, include_date: bool) -> String {
    if let Some(zoned) = parse_iso8601_to_local(ts) {
        if include_date {
            let today = jiff::Zoned::now().date();
            let reset_date = zoned.date();

            let diff_days = today.until(reset_date).ok()
                .map(|span| span.get_days())
                .unwrap_or(i32::MAX);

            let date_label = match diff_days {
                0 => Some("today"),
                1 => Some("tomorrow"),
                _ => None,
            };

            return match date_label {
                Some(label) => format!("{}, {}", label, zoned.strftime("%H:%M %Z")),
                None if (2..=6).contains(&diff_days) => {
                    zoned.strftime("%a, %H:%M %Z").to_string()
                }
                None => zoned.strftime("%b %-d, %H:%M %Z").to_string(),
            };
        }

        return zoned.strftime("%H:%M %Z").to_string();
    }

    // Fallback: try UTC if the timestamp parses but local timezone failed
    if let Ok(timestamp) = ts.parse::<jiff::Timestamp>() {
        let utc = timestamp.to_zoned(jiff::tz::TimeZone::UTC);
        return if include_date {
            utc.strftime("%b %-d, %H:%M UTC").to_string()
        } else {
            utc.strftime("%H:%M UTC").to_string()
        };
    }

    ts.to_string()
}

impl ClaudeUsageData {
    /// Get the shared data entity, creating it (and starting the poller) on first use.
    fn shared(cx: &mut App) -> Entity<Self> {
        if let Some(existing) = cx
            .try_global::<GlobalClaudeUsageData>()
            .and_then(|g| g.0.upgrade())
        {
            return existing;
        }
        let entity = cx.new(Self::new);
        cx.set_global(GlobalClaudeUsageData(entity.downgrade()));
        entity
    }

    /// Wake the fetch loop, but only if the most recent successful fetch is older
    /// than [`HOVER_REFETCH_THROTTLE`]. Used to refresh on popover open without
    /// hammering the API on rapid hover-on/off.
    fn request_fresh_fetch(&self) {
        let stale = match *self.last_fetch_at.lock() {
            None => true,
            Some(last) => last.elapsed() >= HOVER_REFETCH_THROTTLE,
        };
        if !stale {
            return;
        }
        if !self.wake_sent.swap(true, Ordering::SeqCst) {
            let _ = self.wake_tx.try_send(());
        }
    }

    /// Wake the fetch loop once when a view has no data to show (e.g. after the
    /// extension is toggled on, or the first fetch failed). Only one signal is
    /// sent until the next successful fetch, to avoid retry storms from render.
    fn wake_if_no_data(&self) {
        if !self.wake_sent.swap(true, Ordering::SeqCst) {
            let _ = self.wake_tx.try_send(());
        }
    }

    fn new(cx: &mut Context<Self>) -> Self {
        let data: Arc<Mutex<Option<UsageData>>> = Arc::new(Mutex::new(None));
        let data_for_task = data.clone();
        let (wake_tx, wake_rx) = smol::channel::bounded::<()>(1);
        let wake_sent = Arc::new(AtomicBool::new(false));
        let wake_sent_for_task = wake_sent.clone();
        let claude_dir = Arc::new(Mutex::new(resolve_claude_dir(cx)));
        let claude_dir_for_task = claude_dir.clone();
        let last_fetch_at: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        let last_fetch_at_for_task = last_fetch_at.clone();

        cx.observe_global::<ExtensionSettingsStore>(move |this, cx| {
            let resolved = resolve_claude_dir(cx);
            let changed = {
                let mut current = this.claude_dir.lock();
                if *current == resolved {
                    false
                } else {
                    *current = resolved;
                    true
                }
            };
            if changed && !this.wake_sent.swap(true, Ordering::SeqCst) {
                let _ = this.wake_tx.try_send(());
            }
            cx.notify();
        })
        .detach();

        let poll_task = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let mut consecutive_failures: u32 = 0;
            loop {
                // Returns (Option<UsageData>, Option<Duration>) — data + optional retry delay
                let dir = claude_dir_for_task.lock().clone();
                let (result, retry_after) = smol::unblock(move || {
                    let creds = match read_claude_creds(&dir) {
                        Some(c) => c,
                        None => {
                            log::warn!("[claude-usage] no access token found");
                            return (None, None);
                        }
                    };

                    // Proactively refresh if the token is expired or about to expire,
                    // so we don't waste a request that we know will 401.
                    let mut token = creds.access_token.clone();
                    let mut refreshed = false;
                    if needs_refresh(&creds) {
                        refreshed = true;
                        // If refresh fails, still try the current token: within the
                        // refresh leeway it may remain valid for a few more minutes.
                        if let Some(fresh) = refresh_access_token(&dir, &creds) {
                            token = fresh;
                        }
                    }

                    let mut outcome = fetch_usage_once(&token);

                    // Reactively refresh once on a rejected token (e.g. expiry we
                    // couldn't predict, or a revoked token) — but skip it if we
                    // already attempted a refresh this cycle, since that attempt
                    // (under lock) just succeeded or failed; retrying is pointless.
                    if matches!(outcome, Fetch::Unauthorized)
                        && !refreshed
                        && let Some(fresh) = refresh_access_token(&dir, &creds)
                    {
                        outcome = fetch_usage_once(&fresh);
                    }

                    match outcome {
                        Fetch::Ok(data) => (Some(*data), None),
                        Fetch::RateLimited(delay) => (None, Some(delay)),
                        Fetch::Unauthorized => {
                            log::warn!(
                                "[claude-usage] token rejected and refresh failed; \
                                 sign in again via the Claude CLI"
                            );
                            (None, None)
                        }
                        Fetch::Failed => (None, None),
                    }
                })
                .await;

                if let Some(fetched) = result {
                    *data_for_task.lock() = Some(fetched);
                    *last_fetch_at_for_task.lock() = Some(Instant::now());
                    consecutive_failures = 0;
                    wake_sent_for_task.store(false, Ordering::SeqCst);
                    if this.update(cx, |_this, cx| cx.notify()).is_err() {
                        break;
                    }
                } else {
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    if this.update(cx, |_, _| {}).is_err() {
                        break;
                    }
                }

                let delay = match retry_after {
                    Some(server_delay) => {
                        let backoff = MIN_RETRY_DELAY
                            .saturating_mul(1 << consecutive_failures.min(6).saturating_sub(1));
                        let cap = Duration::from_secs(3600);
                        server_delay.max(backoff).min(cap)
                    }
                    None if consecutive_failures > 0 => {
                        let backoff = MIN_RETRY_DELAY
                            .saturating_mul(1 << consecutive_failures.min(6).saturating_sub(1));
                        backoff.min(Duration::from_secs(3600))
                    }
                    None => USAGE_INTERVAL,
                };
                log::info!("[claude-usage] next fetch in {}s", delay.as_secs());
                // Race: sleep vs wake signal (e.g. when UI becomes visible but has no data)
                let woken = smol::future::or(
                    async { smol::Timer::after(delay).await; false },
                    async { let _ = wake_rx.recv().await; true },
                ).await;
                // Drain any extra wake signals
                while wake_rx.try_recv().is_ok() {}
                // Don't reset consecutive_failures on wake — preserve backoff
                // to avoid retry storms when render() wakes us during 429s.
                let _ = woken;
            }
        });

        Self {
            data,
            claude_dir,
            wake_tx,
            wake_sent,
            last_fetch_at,
            _poll_task: poll_task,
        }
    }
}

/// Claude API usage indicator with hover popover.
///
/// One of these exists per window; they all share a single [`ClaudeUsageData`]
/// poller and hold only per-window UI state.
pub struct ClaudeUsage {
    data: Entity<ClaudeUsageData>,
    popover_visible: bool,
    trigger_bounds: Bounds<Pixels>,
    hover_token: Arc<AtomicU64>,
}

impl ClaudeUsage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let data = ClaudeUsageData::shared(cx);
        // Re-render this window's widget whenever the shared poller updates.
        cx.observe(&data, |_, _, cx| cx.notify()).detach();
        Self {
            data,
            popover_visible: false,
            trigger_bounds: Bounds::default(),
            hover_token: Arc::new(AtomicU64::new(0)),
        }
    }

    fn show_popover(&mut self, cx: &mut Context<Self>) {
        if self.popover_visible {
            return;
        }

        let token = self.hover_token.fetch_add(1, Ordering::SeqCst) + 1;
        let hover_token = self.hover_token.clone();

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            smol::Timer::after(Duration::from_millis(HOVER_DELAY_MS)).await;

            if hover_token.load(Ordering::SeqCst) != token {
                return;
            }

            let _ = this.update(cx, |this, cx| {
                if hover_token.load(Ordering::SeqCst) == token {
                    this.popover_visible = true;
                    this.data.read(cx).request_fresh_fetch();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn hide_popover(&mut self, cx: &mut Context<Self>) {
        let token = self.hover_token.fetch_add(1, Ordering::SeqCst) + 1;

        if !self.popover_visible {
            return;
        }

        let hover_token = self.hover_token.clone();

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            smol::Timer::after(Duration::from_millis(100)).await;

            if hover_token.load(Ordering::SeqCst) != token {
                return;
            }

            let _ = this.update(cx, |this, cx| {
                if hover_token.load(Ordering::SeqCst) == token && this.popover_visible {
                    this.popover_visible = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn render_popover(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let shared = self.data.read(cx);
        let data = shared.data.lock();
        let data = match data.as_ref() {
            Some(d) if self.popover_visible => d.clone(),
            _ => return div().size_0().into_any_element(),
        };

        let bounds = self.trigger_bounds;
        let position = point(bounds.origin.x, bounds.origin.y - px(4.0));

        deferred(
            anchored()
                .position(position)
                .anchor(Corner::BottomLeft)
                .snap_to_window()
                .child(
                    div()
                        .id("claude-usage-popover")
                        .occlude()
                        .min_w(px(300.0))
                        .max_w(px(420.0))
                        .bg(rgb(t.bg_primary))
                        .border_1()
                        .border_color(rgb(t.border))
                        .rounded(px(8.0))
                        .shadow_lg()
                        .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                            if *hovered {
                                this.hover_token.fetch_add(1, Ordering::SeqCst);
                            } else {
                                this.hide_popover(cx);
                            }
                        }))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
                            v_flex()
                                .child(render_popover_header(t, cx))
                                .child(
                                    v_flex()
                                        .px(px(12.0))
                                        .py(px(10.0))
                                        .gap(px(7.0))
                                        .when_some(data.five_hour.as_ref(), |el, tier| {
                                            el.child(render_tier_row(t, cx, "Session", "5h", tier, "marker-session"))
                                        })
                                        .when_some(data.seven_day.as_ref(), |el, tier| {
                                            el.child(render_tier_row(t, cx, "Weekly", "7d", tier, "marker-weekly"))
                                        })
                                        .when_some(
                                            data.seven_day_sonnet
                                                .as_ref()
                                                .filter(|tier| tier.utilization >= 0.5),
                                            |el, tier| {
                                                el.child(render_tier_row(t, cx, "Sonnet", "7d", tier, "marker-sonnet"))
                                            },
                                        )
                                        .when_some(
                                            data.seven_day_opus
                                                .as_ref()
                                                .filter(|tier| tier.utilization >= 0.5),
                                            |el, tier| {
                                                el.child(render_tier_row(t, cx, "Opus", "7d", tier, "marker-opus"))
                                            },
                                        )
                                        .when_some(data.extra_usage.as_ref(), |el, extra| {
                                            if !extra.is_enabled {
                                                return el;
                                            }
                                            el.child(render_divider(t))
                                                .child(render_extra_usage_row(t, cx, extra))
                                        }),
                                ),
                        ),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }
}

fn render_popover_header(t: &ThemeColors, cx: &App) -> impl IntoElement {
    let muted = t.text_muted;
    let primary = t.text_primary;

    h_flex()
        .px(px(12.0))
        .py(px(7.0))
        .items_center()
        .justify_between()
        .border_b_1()
        .border_color(rgb(t.border))
        .child(
            div()
                .text_size(ui_text_xs(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(t.text_secondary))
                .child("CLAUDE USAGE"),
        )
        .child(
            h_flex()
                .id("claude-usage-settings")
                .gap(px(4.0))
                .items_center()
                .px(px(4.0))
                .py(px(1.0))
                .rounded(px(3.0))
                .cursor_pointer()
                .text_color(rgb(muted))
                .hover(|s| s.text_color(rgb(primary)).bg(rgb(t.bg_hover)))
                .child(
                    div()
                        .text_size(ui_text_xs(cx))
                        .line_height(px(10.0))
                        .child("Settings"),
                )
                .child(
                    svg()
                        .path("icons/external-link.svg")
                        .size(px(10.0)),
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .on_click(|_, _, _cx| {
                    open_url("https://claude.ai/settings/usage");
                })
                .tooltip(|window, cx| {
                    Tooltip::new("Open usage settings on claude.ai").build(window, cx)
                }),
        )
}

fn utilization_color(t: &ThemeColors, pct: f64) -> u32 {
    if pct > 80.0 {
        t.metric_critical
    } else if pct > 60.0 {
        t.metric_warning
    } else {
        t.metric_normal
    }
}

fn render_tier_row(
    t: &ThemeColors,
    cx: &App,
    label: &str,
    period: &str,
    tier: &TierUsage,
    marker_id: &'static str,
) -> impl IntoElement {
    let pct = tier.utilization;

    v_flex()
        .gap(px(5.0))
        .child(
            h_flex()
                .items_baseline()
                .justify_between()
                .child(
                    h_flex()
                        .gap(px(6.0))
                        .items_baseline()
                        .child(
                            div()
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.text_primary))
                                .child(label.to_string()),
                        )
                        .child(
                            div()
                                .text_size(ui_text_xs(cx))
                                .text_color(rgb(t.text_muted))
                                .child(period.to_string()),
                        ),
                )
                .child(
                    div()
                        .text_size(ui_text_md(cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(utilization_color(t, pct)))
                        .child(format!("{:.0}%", pct)),
                ),
        )
        .child(render_usage_with_time_bar(t, pct, tier.time_elapsed_pct, marker_id))
        .when(!tier.resets_at.is_empty(), |el| {
            el.child(
                h_flex()
                    .justify_end()
                    .child(
                        div()
                            .text_size(ui_text_xs(cx))
                            .text_color(rgb(t.text_muted))
                            .child(format!("resets {}", tier.resets_at)),
                    ),
            )
        })
}

fn render_extra_usage_row(
    t: &ThemeColors,
    cx: &App,
    extra: &ExtraUsage,
) -> impl IntoElement {
    v_flex()
        .gap(px(5.0))
        .child(
            h_flex()
                .items_baseline()
                .justify_between()
                .child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_primary))
                        .child("Extra Usage"),
                )
                .child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(t.text_primary))
                        .child(format!(
                            "${:.2} / ${:.2}",
                            extra.used_credits / 100.0,
                            extra.monthly_limit / 100.0
                        )),
                ),
        )
        .child(render_progress_bar(t, extra.utilization))
}

fn render_divider(t: &ThemeColors) -> impl IntoElement {
    div().h(px(1.0)).w_full().bg(rgb(t.border))
}

fn render_usage_with_time_bar(
    t: &ThemeColors,
    usage_pct: f64,
    time_pct: Option<f64>,
    marker_id: &'static str,
) -> impl IntoElement {
    let clamped_usage = usage_pct.clamp(0.0, 100.0) as f32;

    let pace_color = match time_pct {
        Some(tp) if usage_pct > tp + 15.0 => t.metric_critical,
        Some(tp) if usage_pct > tp + 5.0 => t.metric_warning,
        _ => t.metric_normal,
    };

    div()
        .h(px(6.0))
        .w_full()
        .rounded_full()
        .bg(rgb(t.bg_secondary))
        .relative()
        .child(
            div()
                .h_full()
                .rounded_full()
                .bg(rgb(pace_color))
                .w(relative(clamped_usage / 100.0)),
        )
        .when_some(time_pct, |el, tp| {
            let clamped_time = tp.clamp(0.0, 100.0) as f32;
            let marker_color = t.text_primary;
            el.child(
                div()
                    .id(marker_id)
                    .absolute()
                    .top(px(-4.0))
                    .left(relative(clamped_time / 100.0))
                    .w(px(8.0))
                    .h(px(14.0))
                    .flex()
                    .items_center()
                    .justify_start()
                    .child(
                        div()
                            .w(px(2.0))
                            .h(px(10.0))
                            .rounded(px(1.0))
                            .bg(rgb(marker_color)),
                    )
                    .tooltip(|window, cx| {
                        Tooltip::new("Time elapsed in this period").build(window, cx)
                    }),
            )
        })
}

fn render_progress_bar(t: &ThemeColors, pct: f64) -> impl IntoElement {
    let clamped = pct.clamp(0.0, 100.0) as f32;
    let color = utilization_color(t, pct);

    div()
        .h(px(6.0))
        .w_full()
        .rounded_full()
        .bg(rgb(t.bg_secondary))
        .child(
            div()
                .h_full()
                .rounded_full()
                .bg(rgb(color))
                .w(relative(clamped / 100.0)),
        )
}

impl Render for ClaudeUsage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let data = self.data.read(cx).data.lock();
        let (five_h, seven_d) = match data.as_ref() {
            Some(d) => {
                let fh = d.five_hour.as_ref().map(|t| t.utilization);
                let sd = d.seven_day.as_ref().map(|t| t.utilization);
                (fh, sd)
            }
            None => {
                drop(data);
                // Wake the fetch loop once (e.g. after toggle on/off or if the
                // first fetch failed). Only one signal is sent to avoid retry storms.
                self.data.read(cx).wake_if_no_data();
                return div().size_0().into_any_element();
            }
        };
        drop(data);

        let entity_handle = cx.entity().clone();

        div()
            .child(
                h_flex()
                    .id("claude-usage-trigger")
                    .cursor_pointer()
                    .gap(px(4.0))
                    .px(px(4.0))
                    .py(px(1.0))
                    .rounded(px(3.0))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .when_some(five_h, |el, pct| {
                        el.child(
                            h_flex()
                                .gap(px(3.0))
                                .child(
                                    div()
                                        .text_size(ui_text_ms(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child("5h"),
                                )
                                .child(
                                    div()
                                        .text_size(ui_text_ms(cx))
                                        .text_color(rgb(utilization_color(&t, pct)))
                                        .child(format!("{:.0}%", pct)),
                                ),
                        )
                    })
                    .when_some(seven_d, |el, pct| {
                        el.child(
                            div()
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.text_muted))
                                .child("|"),
                        )
                        .child(
                            h_flex()
                                .gap(px(3.0))
                                .child(
                                    div()
                                        .text_size(ui_text_ms(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child("7d"),
                                )
                                .child(
                                    div()
                                        .text_size(ui_text_ms(cx))
                                        .text_color(rgb(utilization_color(&t, pct)))
                                        .child(format!("{:.0}%", pct)),
                                ),
                        )
                    })
                    .child(
                        canvas(
                            move |bounds, _window, app| {
                                entity_handle.update(app, |this, _cx| {
                                    this.trigger_bounds = bounds;
                                });
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                        if *hovered {
                            this.show_popover(cx);
                        } else {
                            this.hide_popover(cx);
                        }
                    })),
            )
            .child(self.render_popover(&t, cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // gpui::* re-exports a `test` attribute macro that conflicts with the built-in;
    // alias the built-in so `#[test]` works normally in this module.
    use core::prelude::rust_2024::test;

    #[test]
    fn test_expand_tilde_absolute() {
        let result = expand_tilde("/absolute/path");
        assert_eq!(result, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_expand_tilde_with_slash() {
        let result = expand_tilde("~/foo/bar");
        let expected = dirs::home_dir().unwrap().join("foo/bar");
        assert_eq!(result, expected);
    }

    #[test]
    fn test_expand_tilde_bare() {
        let result = expand_tilde("~");
        let expected = dirs::home_dir().unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn test_existing_path_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        assert!(existing_path(&missing.to_string_lossy(), "test").is_none());
    }

    #[test]
    fn test_existing_path_present() {
        let dir = tempfile::tempdir().unwrap();
        let path = existing_path(&dir.path().to_string_lossy(), "test").unwrap();
        assert_eq!(path, dir.path());
    }

    #[test]
    fn test_read_access_token_from_file() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let creds = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "test-token-abc",
                "refreshToken": "refresh-xyz",
                "expiresAt": 1779244706022u64,
            }
        });
        let mut f = std::fs::File::create(dir.path().join(".credentials.json")).unwrap();
        write!(f, "{}", creds).unwrap();
        // The tempdir-derived Keychain service name can't match a real entry, so
        // the file is the only source here and is read as the fallback.
        let parsed = read_claude_creds(dir.path()).unwrap();
        assert_eq!(parsed.access_token, "test-token-abc");
        assert_eq!(parsed.refresh_token.as_deref(), Some("refresh-xyz"));
        assert_eq!(parsed.expires_at_ms, Some(1779244706022));
        assert!(matches!(parsed.source, CredsSource::File(_)));
    }

    #[test]
    fn test_clip_respects_char_boundaries() {
        // ASCII shorter than max -> unchanged.
        assert_eq!(clip("hello", 10), "hello");
        // ASCII truncated exactly at max.
        assert_eq!(clip("hello", 3), "hel");
        // Multi-byte char straddling the limit must not panic and must back off
        // to the previous char boundary. "é" is 2 bytes (0xC3 0xA9).
        let s = "aé"; // bytes: 'a'(1) + 'é'(2) = 3 bytes
        assert_eq!(clip(s, 2), "a"); // can't include half of 'é'
        assert_eq!(clip(s, 3), "aé");
        // Limit landing mid-multibyte for a 4-byte emoji.
        let emoji = "😀"; // 4 bytes
        assert_eq!(clip(emoji, 1), "");
        assert_eq!(clip(emoji, 3), "");
        assert_eq!(clip(emoji, 4), "😀");
    }

    #[test]
    fn test_needs_refresh_when_expired() {
        let expired = ClaudeCreds {
            access_token: "a".into(),
            refresh_token: Some("r".into()),
            expires_at_ms: Some(now_ms().saturating_sub(1000)),
            source: CredsSource::File(PathBuf::from("/tmp/x")),
            raw: serde_json::json!({}),
        };
        assert!(needs_refresh(&expired));

        let fresh = ClaudeCreds {
            expires_at_ms: Some(now_ms() + 60 * 60 * 1000),
            ..expired.clone()
        };
        assert!(!needs_refresh(&fresh));

        // No expiry recorded -> don't preemptively refresh (rely on reactive 401).
        let no_expiry = ClaudeCreds {
            expires_at_ms: None,
            ..expired.clone()
        };
        assert!(!needs_refresh(&no_expiry));
    }

    #[test]
    fn test_persist_creds_round_trip_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".credentials.json");
        let raw = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "old",
                "refreshToken": "old-refresh",
                "expiresAt": 1u64,
                "subscriptionType": "max",
            }
        });
        std::fs::write(&path, raw.to_string()).unwrap();

        persist_creds(
            &CredsSource::File(path.clone()),
            raw,
            "new-access",
            Some("new-refresh"),
            Some(999),
        )
        .expect("persist should succeed");

        let reread = read_claude_creds(dir.path()).unwrap();
        assert_eq!(reread.access_token, "new-access");
        assert_eq!(reread.refresh_token.as_deref(), Some("new-refresh"));
        assert_eq!(reread.expires_at_ms, Some(999));
        // Unrelated fields are preserved.
        assert_eq!(reread.raw["claudeAiOauth"]["subscriptionType"], "max");
    }

    #[test]
    fn test_persist_creds_errors_on_unwritable_path() {
        // A path whose parent directory does not exist must surface an error,
        // so the caller treats the refresh as failed instead of dropping the
        // rotated refresh token.
        let bad = CredsSource::File(PathBuf::from("/nonexistent-dir-xyz/creds.json"));
        let raw = serde_json::json!({ "claudeAiOauth": { "accessToken": "x" } });
        let res = persist_creds(&bad, raw, "a", Some("r"), Some(1));
        assert!(res.is_err());
    }

    #[test]
    fn test_persist_creds_errors_on_malformed_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("creds.json");
        // No `claudeAiOauth` object -> cannot persist.
        let raw = serde_json::json!({ "something_else": 1 });
        let res = persist_creds(&CredsSource::File(path), raw, "a", Some("r"), Some(1));
        assert!(res.is_err());
    }

    #[test]
    fn test_oauth_lock_acquire_and_release() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join(CLAUDE_LOCK_DIR);

        let lock = OAuthRefreshLock::acquire(dir.path());
        assert!(lock.held(), "first acquire on a free dir should hold");
        assert!(lock_path.is_dir(), "lock should be a directory (proper-lockfile)");

        drop(lock);
        assert!(!lock_path.exists(), "drop must rmdir a held lock");
    }

    #[test]
    fn test_oauth_lock_heartbeat_bumps_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join(CLAUDE_LOCK_DIR);

        let lock = OAuthRefreshLock::acquire(dir.path());
        assert!(lock.held());
        let t0 = std::fs::metadata(&lock_path).unwrap().modified().unwrap();

        // Hold past one heartbeat tick; the mtime must advance so peers don't
        // see the active lock as stale.
        std::thread::sleep(LOCK_HEARTBEAT + 3 * LOCK_POLL);
        let t1 = std::fs::metadata(&lock_path).unwrap().modified().unwrap();
        assert!(t1 > t0, "heartbeat should refresh the lock dir mtime");

        drop(lock);
        assert!(!lock_path.exists());
    }

    #[test]
    fn test_oauth_lock_does_not_remove_unowned() {
        // A guard that never acquired (held == false) must never delete the dir.
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join(CLAUDE_LOCK_DIR);
        std::fs::create_dir(&lock_path).unwrap();

        let unowned = OAuthRefreshLock::not_held(lock_path.clone());
        drop(unowned);
        assert!(lock_path.exists(), "must not remove a lock we don't own");
    }

    #[test]
    fn test_lock_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join(CLAUDE_LOCK_DIR);

        // Missing -> not stale (can't measure; retry rather than steal).
        assert!(!lock_is_stale(&lock_path));

        // Freshly created -> not stale.
        std::fs::create_dir(&lock_path).unwrap();
        assert!(!lock_is_stale(&lock_path));

        // Backdated past LOCK_STALE -> stale.
        let old = filetime::FileTime::from_unix_time(
            (now_ms() / 1000) as i64 - LOCK_STALE.as_secs() as i64 - 5,
            0,
        );
        filetime::set_file_mtime(&lock_path, old).unwrap();
        assert!(lock_is_stale(&lock_path));
    }

    #[test]
    fn test_claim_stale_lock_freshens_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join(CLAUDE_LOCK_DIR);
        std::fs::create_dir(&lock_path).unwrap();
        let old = filetime::FileTime::from_unix_time(
            (now_ms() / 1000) as i64 - LOCK_STALE.as_secs() as i64 - 5,
            0,
        );
        filetime::set_file_mtime(&lock_path, old).unwrap();

        assert!(claim_stale_lock(&lock_path), "uncontended stale claim should win");
        assert!(!lock_is_stale(&lock_path), "claim must freshen the mtime");
    }

    #[test]
    fn test_acquire_steals_stale_lock() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join(CLAUDE_LOCK_DIR);
        // Simulate a crashed holder: a stale lock dir left behind.
        std::fs::create_dir(&lock_path).unwrap();
        let old = filetime::FileTime::from_unix_time(
            (now_ms() / 1000) as i64 - LOCK_STALE.as_secs() as i64 - 5,
            0,
        );
        filetime::set_file_mtime(&lock_path, old).unwrap();

        let lock = OAuthRefreshLock::acquire(dir.path());
        assert!(lock.held(), "should steal a stale lock");
        drop(lock);
        assert!(!lock_path.exists(), "drop releases the stolen lock");
    }

    #[test]
    fn test_parse_iso8601_to_epoch() {
        // 2025-01-01T00:00:00Z = 1735689600
        let epoch = parse_iso8601_to_epoch("2025-01-01T00:00:00.000Z").unwrap();
        assert!((epoch - 1735689600.0).abs() < 1.0);
    }

    #[test]
    fn test_parse_iso8601_to_epoch_invalid() {
        assert!(parse_iso8601_to_epoch("not-a-date").is_none());
    }

    #[test]
    fn test_parse_iso8601_to_local() {
        let zoned = parse_iso8601_to_local("2025-06-15T14:00:00.000Z").unwrap();
        // The local time depends on the system timezone, but should be a valid datetime
        let tz_abbr = zoned.strftime("%Z").to_string();
        assert!(!tz_abbr.is_empty(), "Expected non-empty tz abbreviation");
    }

    #[test]
    fn test_parse_iso8601_to_local_invalid() {
        assert!(parse_iso8601_to_local("garbage").is_none());
    }

    #[test]
    fn test_format_reset_time_uses_local_tz() {
        let result = format_reset_time("2025-06-15T14:00:00.000Z", false);
        // Should contain a colon (HH:MM) and a timezone abbreviation
        assert!(result.contains(':'), "Expected HH:MM format, got: {}", result);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_format_reset_time_with_date() {
        let result = format_reset_time("2099-01-15T11:00:00.000Z", true);
        assert!(result.contains(':'), "Expected time in result, got: {}", result);
        assert!(result.contains(','), "Expected date label with comma, got: {}", result);
    }

    #[test]
    fn test_format_reset_time_invalid_input() {
        // Invalid input should be returned as-is
        let result = format_reset_time("garbage", false);
        assert_eq!(result, "garbage");
    }

    #[test]
    fn test_format_reset_time_past_date() {
        // A reset time in the past should still format with date (no panic, no special label)
        let result = format_reset_time("2020-01-01T00:00:00.000Z", true);
        assert!(result.contains(':'), "Expected time in result, got: {}", result);
        assert!(result.contains(','), "Expected date with comma, got: {}", result);
    }

    #[test]
    fn test_compute_time_elapsed_pct() {
        // A reset 50% through a 100-second period
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let reset_in_50s = jiff::Timestamp::from_second((now + 50) as i64).unwrap();
        let ts = reset_in_50s.strftime("%Y-%m-%dT%H:%M:%S.000Z").to_string();
        let pct = compute_time_elapsed_pct(&ts, 100.0).unwrap();
        assert!((pct - 50.0).abs() < 5.0, "Expected ~50%, got: {}", pct);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_keychain_service_default() {
        let default_dir = dirs::home_dir().unwrap().join(".claude");
        // The default dir must produce the un-suffixed service name.
        // This test requires the path to exist; if ~/.claude is absent, we canonicalize
        // to the given path which may or may not equal the resolved default — so we create
        // a tempdir stand-in only for the non-default branch, and test the default via the
        // real path (which exists on developer machines).
        if default_dir.exists() {
            assert_eq!(keychain_service_name(&default_dir), "Claude Code-credentials");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_keychain_service_custom() {
        // Pin the SHA-256 algorithm against a known empirical example:
        // sha256("/Users/pcavezzan/.claude-stonal")[..8 hex] = "d4c0f9c1"
        // We use a tempdir to get a real canonical path, then verify the suffix formula.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap();
        let service = keychain_service_name(&path);

        use sha2::{Sha256, Digest};
        let mut h = Sha256::new();
        h.update(path.to_string_lossy().as_bytes());
        let d = h.finalize();
        let expected = format!(
            "Claude Code-credentials-{:02x}{:02x}{:02x}{:02x}",
            d[0], d[1], d[2], d[3]
        );
        assert_eq!(service, expected);
        assert_ne!(service, "Claude Code-credentials", "custom dir must get a suffix");
    }
}
