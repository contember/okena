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

                    let response = okena_core::http::send(
                        okena_core::http::HttpRequest::get(
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
                            (Some(parse_usage(&parsed)), None)
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
                                            el.child(render_tier_row(t, cx, "Session", "5h", tier, "marker-session", 5))
                                        })
                                        .when_some(data.seven_day.as_ref(), |el, tier| {
                                            el.child(render_tier_row(t, cx, "Weekly", "7d", tier, "marker-weekly", 7))
                                        })
                                        .when_some(
                                            data.seven_day_sonnet
                                                .as_ref()
                                                .filter(|tier| tier.utilization >= 0.5),
                                            |el, tier| {
                                                el.child(render_tier_row(t, cx, "Sonnet", "7d", tier, "marker-sonnet", 7))
                                            },
                                        )
                                        .when_some(
                                            data.seven_day_opus
                                                .as_ref()
                                                .filter(|tier| tier.utilization >= 0.5),
                                            |el, tier| {
                                                el.child(render_tier_row(t, cx, "Opus", "7d", tier, "marker-opus", 7))
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

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Severity {
    Normal,
    Warning,
    Critical,
}

fn severity_color(t: &ThemeColors, s: Severity) -> u32 {
    match s {
        Severity::Normal => t.metric_normal,
        Severity::Warning => t.metric_warning,
        Severity::Critical => t.metric_critical,
    }
}

/// Severity from absolute utilization — how close to the hard cap.
fn abs_severity(pct: f64) -> Severity {
    if pct > 80.0 {
        Severity::Critical
    } else if pct > 60.0 {
        Severity::Warning
    } else {
        Severity::Normal
    }
}

/// Severity from pace — how far ahead usage is of where it "should" be at this
/// point in the period. `Critical` means the user is burning budget fast enough
/// to run out before the period resets unless they slow down.
fn pace_severity(usage_pct: f64, time_pct: Option<f64>) -> Severity {
    match time_pct {
        Some(tp) if usage_pct > tp + 15.0 => Severity::Critical,
        Some(tp) if usage_pct > tp + 5.0 => Severity::Warning,
        _ => Severity::Normal,
    }
}

fn utilization_color(t: &ThemeColors, pct: f64) -> u32 {
    severity_color(t, abs_severity(pct))
}

fn render_tier_row(
    t: &ThemeColors,
    cx: &App,
    label: &str,
    period: &str,
    tier: &TierUsage,
    marker_id: &'static str,
    segments: u32,
) -> impl IntoElement {
    let pct = tier.utilization;
    let pace = pace_severity(pct, tier.time_elapsed_pct);
    // % text reflects whichever is worse: nearness to the cap, or burn pace.
    let pct_color = severity_color(t, abs_severity(pct).max(pace));
    let pace_msg: Option<(&str, u32)> = match pace {
        Severity::Critical => Some(("Slow down to last the period", t.metric_critical)),
        Severity::Warning => Some(("Ahead of pace", t.metric_warning)),
        Severity::Normal => None,
    };

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
                        .text_color(rgb(pct_color))
                        .child(format!("{:.0}%", pct)),
                ),
        )
        .child(render_usage_with_time_bar(t, pct, tier.time_elapsed_pct, marker_id, segments))
        .when(pace_msg.is_some() || !tier.resets_at.is_empty(), |el| {
            el.child(
                h_flex()
                    .justify_between()
                    .items_baseline()
                    .child(
                        div()
                            .text_size(ui_text_xs(cx))
                            .font_weight(FontWeight::MEDIUM)
                            .when_some(pace_msg, |d, (msg, col)| {
                                d.text_color(rgb(col)).child(msg)
                            }),
                    )
                    .child(
                        div()
                            .text_size(ui_text_xs(cx))
                            .text_color(rgb(t.text_muted))
                            .when(!tier.resets_at.is_empty(), |d| {
                                d.child(format!("resets {}", tier.resets_at))
                            }),
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
    segments: u32,
) -> impl IntoElement {
    let clamped_usage = usage_pct.clamp(0.0, 100.0) as f32;

    // Base fill reflects nearness to the hard cap. Any usage *beyond* the pace
    // marker is overage — drawn on top in warning/critical — so being over the
    // budget for this point in the period is visible directly on the bar.
    let base_color = severity_color(t, abs_severity(usage_pct));
    let overage = time_pct.and_then(|tp| {
        let start = tp.clamp(0.0, 100.0) as f32;
        let width = clamped_usage - start;
        if width <= 0.0 {
            return None;
        }
        let color = if width > 15.0 {
            t.metric_critical
        } else {
            t.metric_warning
        };
        Some((start, width, color))
    });

    // Divider lines splitting the bar into per-day (7d) or per-hour (5h)
    // segments, so the pace marker can be read against a time grid.
    let dividers = (1..segments).map(move |i| {
        div()
            .absolute()
            .top_0()
            .h_full()
            .w(px(1.0))
            .bg(rgb(t.border))
            .left(relative(i as f32 / segments as f32))
    });

    // Translucent band over the segment the user is currently in (today / this
    // hour). Derived from text_primary so it lightens on dark themes and darkens
    // on light ones.
    let current_seg = time_pct.and_then(|tp| {
        if segments <= 1 {
            return None;
        }
        let idx = (tp / 100.0 * segments as f64).floor() as i64;
        Some(idx.clamp(0, segments as i64 - 1) as u32)
    });
    let mut highlight = rgb(t.text_primary);
    highlight.a = 0.14;

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
                .bg(rgb(base_color))
                .w(relative(clamped_usage / 100.0)),
        )
        .when_some(overage, |el, (start, width, color)| {
            el.child(
                div()
                    .absolute()
                    .top_0()
                    .h_full()
                    .left(relative(start / 100.0))
                    .w(relative(width / 100.0))
                    .rounded_r(px(3.0))
                    .bg(rgb(color)),
            )
        })
        .children(dividers)
        .when_some(current_seg, |el, seg| {
            el.child(
                div()
                    .absolute()
                    .top_0()
                    .h_full()
                    .left(relative(seg as f32 / segments as f32))
                    .w(relative(1.0 / segments as f32))
                    .bg(highlight),
            )
        })
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
