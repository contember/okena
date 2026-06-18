use crate::ui_helpers::capitalize_first;
use okena_extensions::{ExtensionSettingsStore, ThemeColors};
use okena_usage::{
    effective_time_pct, read_working_days, render_simple_bar, render_usage_row,
    usage_body_container, usage_divider, usage_kv_row, usage_popover_container,
    usage_popover_header, usage_trigger_items, SegmentUnit, UsageRow,
};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use parking_lot::Mutex;
#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Refresh interval for usage data
const USAGE_INTERVAL: Duration = Duration::from_secs(300);

/// Minimum retry delay to avoid tight loops (e.g. when server returns retry-after: 0)
const MIN_RETRY_DELAY: Duration = Duration::from_secs(30);

/// Hover delay before showing the popover (ms)
const HOVER_DELAY_MS: u64 = 300;

/// Minimum interval between hover-triggered re-fetches.
const HOVER_REFETCH_THROTTLE: Duration = Duration::from_secs(60);

/// Usage info for a single rate-limit tier
#[derive(Clone)]
struct TierUsage {
    utilization: f64,
    /// Reset time as Unix epoch seconds, anchoring the per-day/hour grid and
    /// the "resets …" label.
    reset_epoch: Option<f64>,
    /// Length of this rate-limit window in seconds.
    period_secs: f64,
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
    /// Subscription plan label (e.g. "Pro", "Max") read from the credentials.
    plan: Option<String>,
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

#[cfg(target_os = "macos")]
fn suffixed_keychain_service_name(claude_dir: &Path) -> String {
    const BASE: &str = "Claude Code-credentials";
    let canonical = claude_dir.canonicalize().unwrap_or_else(|_| claude_dir.to_path_buf());
    let mut h = Sha256::new();
    h.update(canonical.to_string_lossy().as_bytes());
    let d = h.finalize();
    format!("{BASE}-{:02x}{:02x}{:02x}{:02x}", d[0], d[1], d[2], d[3])
}

#[cfg(target_os = "macos")]
fn keychain_service_names(claude_dir: &Path) -> Vec<String> {
    let primary = keychain_service_name(claude_dir);
    let suffixed = suffixed_keychain_service_name(claude_dir);
    if primary == suffixed {
        vec![primary]
    } else {
        vec![primary, suffixed]
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn extract_access_token(json_str: &str, now_ms: u64) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let oauth = &v["claudeAiOauth"];
    let token = oauth["accessToken"].as_str()?.trim();
    if token.is_empty() {
        return None;
    }

    if let Some(expires_at) = oauth["expiresAt"].as_u64()
        && expires_at <= now_ms
    {
        return None;
    }

    Some(token.to_string())
}

fn read_access_token(claude_dir: &Path) -> Option<String> {
    let now = now_ms();

    // Try credentials file first
    if let Some(token) = std::fs::read_to_string(claude_dir.join(".credentials.json"))
        .ok()
        .and_then(|content| extract_access_token(&content, now))
    {
        return Some(token);
    }

    // macOS: fall back to Keychain using per-config-dir service names. Claude
    // Code can create both the default service and the suffixed service when
    // CLAUDE_CONFIG_DIR explicitly points at ~/.claude, so try both.
    #[cfg(target_os = "macos")]
    {
        let user = std::env::var("USER").ok()?;
        for service in keychain_service_names(claude_dir) {
            let output = okena_core::process::safe_output(
                okena_core::process::command("security")
                    .args(["find-generic-password", "-s", &service, "-a", &user, "-w"]),
            )
            .ok()?;
            if output.status.success() {
                let content = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if let Some(token) = extract_access_token(&content, now) {
                    return Some(token);
                }
            }
        }
    }

    None
}

/// Extract the subscription plan label (e.g. "pro", "max") from a Claude
/// credentials JSON blob.
fn extract_subscription_type(json_str: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let plan = v["claudeAiOauth"]["subscriptionType"].as_str()?.trim();
    if plan.is_empty() {
        None
    } else {
        Some(plan.to_string())
    }
}

/// Read the subscription plan label from the credentials file or, on macOS,
/// the Keychain — mirroring [`read_access_token`]'s lookup.
fn read_subscription_type(claude_dir: &Path) -> Option<String> {
    if let Some(plan) = std::fs::read_to_string(claude_dir.join(".credentials.json"))
        .ok()
        .and_then(|content| extract_subscription_type(&content))
    {
        return Some(plan);
    }

    #[cfg(target_os = "macos")]
    {
        let user = std::env::var("USER").ok()?;
        for service in keychain_service_names(claude_dir) {
            let output = okena_core::process::safe_output(
                okena_core::process::command("security")
                    .args(["find-generic-password", "-s", &service, "-a", &user, "-w"]),
            )
            .ok()?;
            if output.status.success() {
                let content = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if let Some(plan) = extract_subscription_type(&content) {
                    return Some(plan);
                }
            }
        }
    }

    None
}

fn parse_usage(resp: &serde_json::Value) -> UsageData {
    let five_hour = parse_tier(resp, "five_hour", FIVE_HOUR_SECS);
    let seven_day = parse_tier(resp, "seven_day", SEVEN_DAY_SECS);
    let seven_day_sonnet = parse_tier(resp, "seven_day_sonnet", SEVEN_DAY_SECS);
    let seven_day_opus = parse_tier(resp, "seven_day_opus", SEVEN_DAY_SECS);

    let extra_usage = resp.get("extra_usage").map(|eu| {
        ExtraUsage {
            is_enabled: eu["is_enabled"].as_bool().unwrap_or(false),
            monthly_limit: eu["monthly_limit"].as_f64().unwrap_or(0.0),
            used_credits: eu["used_credits"].as_f64().unwrap_or(0.0),
            utilization: eu["utilization"].as_f64().unwrap_or(0.0),
        }
    });

    UsageData {
        plan: None,
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

fn parse_tier(resp: &serde_json::Value, key: &str, period_secs: f64) -> Option<TierUsage> {
    let tier = resp.get(key)?;
    let resets_at_raw = tier["resets_at"].as_str();
    let reset_epoch = resets_at_raw.and_then(parse_iso8601_to_epoch);
    let time_elapsed_pct = resets_at_raw.and_then(|ts| compute_time_elapsed_pct(ts, period_secs));
    Some(TierUsage {
        utilization: tier["utilization"].as_f64().unwrap_or(0.0),
        reset_epoch,
        period_secs,
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
                    let token = match read_access_token(&dir) {
                        Some(t) => {
                            log::info!("[claude-usage] token found (len={})", t.len());
                            t
                        }
                        None => {
                            log::warn!("[claude-usage] no access token found");
                            return (None, None);
                        }
                    };

                    let response = okena_transport::http::send(
                        okena_transport::http::HttpRequest::get(
                            "https://api.anthropic.com/api/oauth/usage",
                        )
                        .bearer(&token)
                        .header("anthropic-beta", "oauth-2025-04-20")
                        .timeout(Duration::from_secs(10))
                        .label("claude.usage")
                        // Safety floor: real cadence is 300s (≥30s on retry); a
                        // 5s floor only ever catches a runaway re-spawn. One
                        // request per tick, so it never clips a legit retry.
                        .min_interval(Duration::from_secs(5)),
                    );

                    match response {
                        Ok(resp) => {
                            let status = resp.status();

                            if status == 429 {
                                let retry_secs = resp
                                    .header("retry-after")
                                    .and_then(|v| v.parse::<u64>().ok())
                                    .unwrap_or(USAGE_INTERVAL.as_secs() * 2);
                                let effective = Duration::from_secs(retry_secs)
                                    .max(MIN_RETRY_DELAY);
                                log::warn!(
                                    "[claude-usage] rate limited (429), retrying in {}s",
                                    effective.as_secs()
                                );
                                return (None, Some(Duration::from_secs(retry_secs)));
                            }

                            let body = resp.text();
                            log::info!(
                                "[claude-usage] HTTP {} body={}",
                                status,
                                &body[..body.len().min(500)]
                            );
                            if !resp.is_success() {
                                return (None, None);
                            }
                            let parsed: serde_json::Value =
                                match serde_json::from_str(&body) {
                                    Ok(v) => v,
                                    Err(_) => return (None, None),
                                };
                            let mut usage = parse_usage(&parsed);
                            usage.plan = read_subscription_type(&dir);
                            (Some(usage), None)
                        }
                        Err(e) => {
                            log::warn!("[claude-usage] request failed: {}", e);
                            (None, None)
                        }
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

        let working = read_working_days(cx);
        let plan = data.plan.as_deref().map(capitalize_first);
        let bounds = self.trigger_bounds;
        let position = point(bounds.origin.x, bounds.origin.y - px(4.0));

        deferred(
            anchored()
                .position(position)
                .anchor(Anchor::BottomLeft)
                .snap_to_window()
                .child(
                    usage_popover_container(t)
                        .id("claude-usage-popover")
                        .occlude()
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
                        .child(usage_popover_header(
                            "CLAUDE USAGE",
                            plan.as_deref(),
                            "https://claude.ai/settings/usage",
                            "Open usage settings on claude.ai",
                            t,
                            cx,
                        ))
                        .child(
                            usage_body_container()
                                .when_some(data.five_hour.as_ref(), |el, tier| {
                                    el.child(render_usage_row(
                                        t,
                                        cx,
                                        &tier_row("Session", "5h", tier, "claude-marker-session", SegmentUnit::Hour),
                                        working,
                                    ))
                                })
                                .when_some(data.seven_day.as_ref(), |el, tier| {
                                    el.child(render_usage_row(
                                        t,
                                        cx,
                                        &tier_row("Weekly", "7d", tier, "claude-marker-weekly", SegmentUnit::Day),
                                        working,
                                    ))
                                })
                                .when_some(
                                    data.seven_day_sonnet
                                        .as_ref()
                                        .filter(|tier| tier.utilization >= 0.5),
                                    |el, tier| {
                                        el.child(render_usage_row(
                                            t,
                                            cx,
                                            &tier_row("Sonnet", "7d", tier, "claude-marker-sonnet", SegmentUnit::Day),
                                            working,
                                        ))
                                    },
                                )
                                .when_some(
                                    data.seven_day_opus
                                        .as_ref()
                                        .filter(|tier| tier.utilization >= 0.5),
                                    |el, tier| {
                                        el.child(render_usage_row(
                                            t,
                                            cx,
                                            &tier_row("Opus", "7d", tier, "claude-marker-opus", SegmentUnit::Day),
                                            working,
                                        ))
                                    },
                                )
                                .when_some(data.extra_usage.as_ref(), |el, extra| {
                                    if !extra.is_enabled {
                                        return el;
                                    }
                                    el.child(usage_divider(t))
                                        .child(render_extra_usage_row(t, cx, extra))
                                }),
                        ),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }
}

/// Build a shared [`UsageRow`] from a Claude rate-limit tier.
fn tier_row(
    label: &str,
    period: &str,
    tier: &TierUsage,
    marker_id: &'static str,
    unit: SegmentUnit,
) -> UsageRow {
    UsageRow {
        label: label.into(),
        period: period.into(),
        pct: tier.utilization,
        time_pct: tier.time_elapsed_pct,
        reset_epoch: tier.reset_epoch,
        period_secs: tier.period_secs,
        unit: Some(unit),
        marker_id: marker_id.into(),
    }
}

fn render_extra_usage_row(t: &ThemeColors, cx: &App, extra: &ExtraUsage) -> impl IntoElement {
    v_flex()
        .gap(px(5.0))
        .child(usage_kv_row(
            t,
            cx,
            "Extra Usage",
            format!(
                "${:.2} / ${:.2}",
                extra.used_credits / 100.0,
                extra.monthly_limit / 100.0
            ),
            t.text_primary,
        ))
        .child(render_simple_bar(t, extra.utilization))
}

impl Render for ClaudeUsage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let working = read_working_days(cx);
        let data = self.data.read(cx).data.lock();
        let mut items: Vec<(SharedString, f64, Option<f64>)> = Vec::new();
        match data.as_ref() {
            Some(d) => {
                if let Some(tier) = d.five_hour.as_ref() {
                    let et = effective_time_pct(
                        tier.reset_epoch,
                        tier.period_secs,
                        Some(SegmentUnit::Hour),
                        working,
                        tier.time_elapsed_pct,
                    );
                    items.push(("5h".into(), tier.utilization, et));
                }
                if let Some(tier) = d.seven_day.as_ref() {
                    let et = effective_time_pct(
                        tier.reset_epoch,
                        tier.period_secs,
                        Some(SegmentUnit::Day),
                        working,
                        tier.time_elapsed_pct,
                    );
                    items.push(("7d".into(), tier.utilization, et));
                }
            }
            None => {
                drop(data);
                // Wake the fetch loop once (e.g. after toggle on/off or if the
                // first fetch failed). Only one signal is sent to avoid retry storms.
                self.data.read(cx).wake_if_no_data();
                return div().size_0().into_any_element();
            }
        }
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
                    .children(usage_trigger_items(&t, cx, &items))
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
            "claudeAiOauth": { "accessToken": "test-token-abc" }
        });
        let mut f = std::fs::File::create(dir.path().join(".credentials.json")).unwrap();
        write!(f, "{}", creds).unwrap();
        // The file-based path should win over Keychain when a valid file is present
        let token = read_access_token(dir.path()).unwrap();
        assert_eq!(token, "test-token-abc");
    }

    #[test]
    fn test_extract_access_token_rejects_empty_token() {
        let creds = serde_json::json!({
            "claudeAiOauth": { "accessToken": "" }
        });

        assert!(extract_access_token(&creds.to_string(), 1_000).is_none());
    }

    #[test]
    fn test_extract_access_token_rejects_expired_token() {
        let creds = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "expired-token",
                "expiresAt": 999
            }
        });

        assert!(extract_access_token(&creds.to_string(), 1_000).is_none());
    }

    #[test]
    fn test_extract_access_token_accepts_unexpired_token() {
        let creds = serde_json::json!({
            "claudeAiOauth": {
                "accessToken": "fresh-token",
                "expiresAt": 1_001
            }
        });

        assert_eq!(
            extract_access_token(&creds.to_string(), 1_000).as_deref(),
            Some("fresh-token")
        );
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
    fn test_keychain_service_names_default_include_suffixed_fallback() {
        let default_dir = dirs::home_dir().unwrap().join(".claude");
        if default_dir.exists() {
            let names = keychain_service_names(&default_dir);
            assert_eq!(names.len(), 2);
            assert_eq!(names[0], "Claude Code-credentials");
            assert!(names[1].starts_with("Claude Code-credentials-"));
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

    #[cfg(target_os = "macos")]
    #[test]
    fn test_keychain_service_names_custom_has_single_service() {
        let dir = tempfile::tempdir().unwrap();
        let names = keychain_service_names(dir.path());

        assert_eq!(names.len(), 1);
        assert!(names[0].starts_with("Claude Code-credentials-"));
    }
}
