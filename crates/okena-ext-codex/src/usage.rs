use okena_extensions::ThemeColors;
use okena_ui::tokens::{ui_text_xs, ui_text_sm, ui_text_ms, ui_text_md};
use base64::Engine as _;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use parking_lot::Mutex;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Refresh interval for usage data
const USAGE_INTERVAL: Duration = Duration::from_secs(300);

/// Minimum retry delay
const MIN_RETRY_DELAY: Duration = Duration::from_secs(30);

/// Hover delay before showing the popover (ms)
const HOVER_DELAY_MS: u64 = 300;

/// Codex OAuth client ID (public, embedded in the Codex CLI binary)
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

fn theme(cx: &App) -> ThemeColors {
    okena_extensions::theme(cx)
}

/// Global holding a weak handle to the shared usage data entity.
///
/// Each window's `CodexUsage` view keeps a strong handle, so the data entity
/// (and its single poll task) lives exactly as long as at least one window
/// shows the widget — and tears down once they all close.
struct GlobalCodexUsageData(WeakEntity<CodexUsageData>);
impl Global for GlobalCodexUsageData {}

/// A rate limit window from the usage API
#[derive(Clone)]
struct RateLimitWindow {
    used_percent: u64,
    window_seconds: u64,
    reset_at: u64,
    time_elapsed_pct: Option<f64>,
}

/// Credits snapshot
#[derive(Clone)]
struct CreditsInfo {
    has_credits: bool,
    unlimited: bool,
    balance: f64,
}

/// All fetched usage data
#[derive(Clone)]
struct UsageData {
    plan_type: String,
    primary_window: Option<RateLimitWindow>,
    secondary_window: Option<RateLimitWindow>,
    review_primary: Option<RateLimitWindow>,
    credits: Option<CreditsInfo>,
}

/// Shared usage data + the single background poll task.
///
/// Decoupling this from the per-window view means the usage API is fetched
/// once for the whole app rather than once per open window. Per-window UI
/// state (popover, hover) lives on [`CodexUsage`] instead.
struct CodexUsageData {
    data: Arc<Mutex<Option<UsageData>>>,
    /// Background polling task. Cancelled automatically when this entity is dropped.
    _poll_task: Task<()>,
}

/// Read Codex OAuth credentials from ~/.codex/auth.json
fn read_codex_auth() -> Option<CodexAuth> {
    let home = dirs::home_dir()?;
    let path = home.join(".codex/auth.json");
    let content = std::fs::read_to_string(&path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    let tokens = &v["tokens"];
    Some(CodexAuth {
        access_token: tokens["access_token"].as_str()?.to_string(),
        refresh_token: tokens["refresh_token"].as_str()?.to_string(),
        account_id: tokens["account_id"].as_str()?.to_string(),
        auth_path: path,
    })
}

struct CodexAuth {
    access_token: String,
    refresh_token: String,
    account_id: String,
    auth_path: std::path::PathBuf,
}

/// Refresh the OAuth access token using the refresh token.
fn refresh_access_token(auth: &CodexAuth) -> Option<String> {
    let resp: serde_json::Value = okena_core::http::send(
        okena_core::http::HttpRequest::post("https://auth.openai.com/oauth/token")
            .body(
                "application/x-www-form-urlencoded",
                format!(
                    "grant_type=refresh_token&client_id={}&refresh_token={}",
                    CODEX_CLIENT_ID, auth.refresh_token
                ),
            )
            .timeout(Duration::from_secs(10))
            .label("codex.token-refresh"),
    )
    .ok()?
    .json()
    .ok()?;

    let new_access = resp["access_token"].as_str()?;
    let new_refresh = resp["refresh_token"].as_str();

    // Persist new tokens back to auth.json
    if let Ok(content) = std::fs::read_to_string(&auth.auth_path)
        && let Ok(mut file_json) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(tokens) = file_json.get_mut("tokens").and_then(|t| t.as_object_mut()) {
                tokens.insert(
                    "access_token".to_string(),
                    serde_json::Value::String(new_access.to_string()),
                );
                if let Some(rt) = new_refresh {
                    tokens.insert(
                        "refresh_token".to_string(),
                        serde_json::Value::String(rt.to_string()),
                    );
                }
            }
            if let Ok(updated) = serde_json::to_string_pretty(&file_json) {
                let _ = std::fs::write(&auth.auth_path, updated);
            }
        }

    Some(new_access.to_string())
}

fn parse_window(v: &serde_json::Value) -> Option<RateLimitWindow> {
    let used = v["used_percent"]
        .as_u64()
        .or_else(|| v["used_percent"].as_f64().map(|v| v.round() as u64))?;
    let window_seconds = v["limit_window_seconds"]
        .as_u64()
        .or_else(|| v["window_minutes"].as_u64().map(|v| v.saturating_mul(60)))
        .unwrap_or(0);
    let reset_at = v["reset_at"].as_u64().unwrap_or(0);

    let time_elapsed_pct = if window_seconds > 0 && reset_at > 0 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let remaining = reset_at.saturating_sub(now);
        let elapsed = window_seconds.saturating_sub(remaining);
        Some((elapsed as f64 / window_seconds as f64 * 100.0).clamp(0.0, 100.0))
    } else {
        None
    };

    Some(RateLimitWindow {
        used_percent: used,
        window_seconds,
        reset_at,
        time_elapsed_pct,
    })
}

fn plan_type_from_access_token(access_token: &str) -> Option<String> {
    let payload = access_token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let jwt_payload: serde_json::Value = serde_json::from_slice(&decoded).ok()?;

    jwt_payload["https://api.openai.com/auth"]["chatgpt_plan_type"]
        .as_str()
        .map(ToOwned::to_owned)
}

fn collect_session_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect_session_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "jsonl") {
            out.push(path);
        }
    }
}

fn fetch_usage_from_local_sessions(auth: &CodexAuth) -> Option<UsageData> {
    let sessions_dir = dirs::home_dir()?.join(".codex/sessions");
    let mut session_files = Vec::new();

    collect_session_files(&sessions_dir, &mut session_files);
    session_files.sort();

    for path in session_files.into_iter().rev() {
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(_) => continue,
        };
        let reader = BufReader::new(file);
        let mut latest_in_file = None;

        for line in reader.lines().map_while(Result::ok) {
            let parsed: serde_json::Value = match serde_json::from_str(&line) {
                Ok(value) => value,
                Err(_) => continue,
            };

            if parsed["type"].as_str() != Some("event_msg")
                || parsed["payload"]["type"].as_str() != Some("token_count")
            {
                continue;
            }

            let rate_limits = &parsed["payload"]["rate_limits"];
            if !rate_limits.is_object() {
                continue;
            }

            let primary_window = rate_limits["primary"]
                .as_object()
                .and_then(|_| parse_window(&rate_limits["primary"]));
            let secondary_window = rate_limits["secondary"]
                .as_object()
                .and_then(|_| parse_window(&rate_limits["secondary"]));
            let credits = rate_limits["credits"].as_object().map(|c| CreditsInfo {
                has_credits: c
                    .get("has_credits")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                unlimited: c
                    .get("unlimited")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                balance: c
                    .get("balance")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
            });

            if primary_window.is_some() || secondary_window.is_some() {
                latest_in_file = Some(UsageData {
                    plan_type: rate_limits["plan_type"]
                        .as_str()
                        .map(ToOwned::to_owned)
                        .or_else(|| plan_type_from_access_token(&auth.access_token))
                        .unwrap_or_else(|| "unknown".to_string()),
                    primary_window,
                    secondary_window,
                    review_primary: None,
                    credits,
                });
            }
        }

        if latest_in_file.is_some() {
            return latest_in_file;
        }
    }

    None
}

fn try_fetch_with_token(
    access_token: &str,
    account_id: &str,
) -> Result<okena_core::http::HttpResponse, Option<u16>> {
    // No min_interval floor here: a single tick can legitimately issue two
    // requests with this label (cached token → 401 → refresh → retry), and a
    // floor would clip the retry. The outer poll cadence is 300s.
    let resp = okena_core::http::send(
        okena_core::http::HttpRequest::get("https://chatgpt.com/backend-api/codex/usage")
            .bearer(access_token)
            .header("chatgpt-account-id", account_id)
            .timeout(Duration::from_secs(10))
            .label("codex.usage"),
    )
    .map_err(|_| None)?;

    if resp.is_success() {
        Ok(resp)
    } else {
        Err(Some(resp.status()))
    }
}

fn fetch_usage() -> Option<UsageData> {
    let auth = read_codex_auth()?;

    // Try cached access token first, refresh on 401
    let resp = match try_fetch_with_token(&auth.access_token, &auth.account_id) {
        Ok(resp) => resp,
        Err(Some(401)) => {
            let new_token = refresh_access_token(&auth)?;
            match try_fetch_with_token(&new_token, &auth.account_id) {
                Ok(resp) => resp,
                Err(status) => {
                    log::warn!("[codex-usage] API returned {:?} after token refresh", status);
                    return fetch_usage_from_local_sessions(&auth);
                }
            }
        }
        Err(status) => {
            log::warn!("[codex-usage] API returned {:?}", status);
            return fetch_usage_from_local_sessions(&auth);
        }
    };

    let body: serde_json::Value = resp.json().ok()?;

    let plan_type = body["plan_type"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();

    let primary_window = body["rate_limit"]["primary_window"]
        .as_object()
        .and_then(|_| parse_window(&body["rate_limit"]["primary_window"]));

    let secondary_window = body["rate_limit"]["secondary_window"]
        .as_object()
        .and_then(|_| parse_window(&body["rate_limit"]["secondary_window"]));

    let review_primary = body["code_review_rate_limit"]["primary_window"]
        .as_object()
        .and_then(|_| parse_window(&body["code_review_rate_limit"]["primary_window"]));

    let credits = body["credits"].as_object().map(|c| CreditsInfo {
        has_credits: c
            .get("has_credits")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        unlimited: c
            .get("unlimited")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        balance: c
            .get("balance")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
    });

    Some(UsageData {
        plan_type,
        primary_window,
        secondary_window,
        review_primary,
        credits,
    })
}

impl CodexUsageData {
    /// Get the shared data entity, creating it (and starting the poller) on first use.
    fn shared(cx: &mut App) -> Entity<Self> {
        if let Some(existing) = cx
            .try_global::<GlobalCodexUsageData>()
            .and_then(|g| g.0.upgrade())
        {
            return existing;
        }
        let entity = cx.new(Self::new);
        cx.set_global(GlobalCodexUsageData(entity.downgrade()));
        entity
    }

    fn new(cx: &mut Context<Self>) -> Self {
        let data: Arc<Mutex<Option<UsageData>>> = Arc::new(Mutex::new(None));
        let data_for_task = data.clone();

        let poll_task = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let mut consecutive_failures: u32 = 0;
            loop {
                let result = smol::unblock(fetch_usage).await;

                if let Some(fetched) = result {
                    *data_for_task.lock() = Some(fetched);
                    consecutive_failures = 0;
                    if this.update(cx, |_this, cx| cx.notify()).is_err() {
                        break;
                    }
                } else {
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    if this.update(cx, |_, _| {}).is_err() {
                        break;
                    }
                }

                let delay = if consecutive_failures > 0 {
                    let backoff = MIN_RETRY_DELAY
                        .saturating_mul(1 << consecutive_failures.min(6).saturating_sub(1));
                    backoff.min(Duration::from_secs(3600))
                } else {
                    USAGE_INTERVAL
                };
                smol::Timer::after(delay).await;
            }
        });

        Self {
            data,
            _poll_task: poll_task,
        }
    }
}

/// Codex usage indicator with hover popover.
///
/// One of these exists per window; they all share a single [`CodexUsageData`]
/// poller and hold only per-window UI state.
pub struct CodexUsage {
    data: Entity<CodexUsageData>,
    popover_visible: bool,
    trigger_bounds: Bounds<Pixels>,
    hover_token: Arc<AtomicU64>,
}

impl CodexUsage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let data = CodexUsageData::shared(cx);
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
                        .id("codex-usage-popover")
                        .occlude()
                        .min_w(px(280.0))
                        .max_w(px(400.0))
                        .bg(rgb(t.bg_primary))
                        .border_1()
                        .border_color(rgb(t.border))
                        .rounded(px(6.0))
                        .shadow_lg()
                        .p(px(10.0))
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
                                .gap(px(8.0))
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .child(
                                            div()
                                                .text_size(ui_text_md(cx))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(t.text_primary))
                                                .child("Codex Usage"),
                                        )
                                        .child(
                                            div()
                                                .text_size(ui_text_sm(cx))
                                                .text_color(rgb(t.text_muted))
                                                .child(data.plan_type.clone()),
                                        ),
                                )
                                .when_some(data.primary_window.as_ref(), |el, w| {
                                    el.child(render_window_row(t, cx, "Rate Limit", w))
                                })
                                .when_some(data.secondary_window.as_ref(), |el, w| {
                                    el.child(render_window_row(t, cx, "Secondary", w))
                                })
                                .when_some(data.review_primary.as_ref(), |el, w| {
                                    el.child(render_window_row(t, cx, "Code Review", w))
                                })
                                .when(
                                    data.primary_window.as_ref().and_then(|w| w.time_elapsed_pct).is_some()
                                        || data.secondary_window.as_ref().and_then(|w| w.time_elapsed_pct).is_some(),
                                    |el| {
                                        el.child(
                                            div()
                                                .text_size(ui_text_xs(cx))
                                                .text_color(rgb(t.text_muted))
                                                .child("Bar color = pace · Marker = time elapsed"),
                                        )
                                    },
                                )
                                .when_some(data.credits.as_ref(), |el, c| {
                                    if c.unlimited {
                                        el.child(
                                            h_flex()
                                                .justify_between()
                                                .child(
                                                    div()
                                                        .text_size(ui_text_ms(cx))
                                                        .text_color(rgb(t.text_secondary))
                                                        .child("Credits"),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(ui_text_ms(cx))
                                                        .text_color(rgb(t.metric_normal))
                                                        .child("Unlimited"),
                                                ),
                                        )
                                    } else if c.has_credits {
                                        el.child(
                                            h_flex()
                                                .justify_between()
                                                .child(
                                                    div()
                                                        .text_size(ui_text_ms(cx))
                                                        .text_color(rgb(t.text_secondary))
                                                        .child("Credits"),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(ui_text_ms(cx))
                                                        .text_color(rgb(t.text_primary))
                                                        .child(format!("${:.2}", c.balance)),
                                                ),
                                        )
                                    } else {
                                        el
                                    }
                                }),
                        ),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }
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
fn abs_severity(pct: u64) -> Severity {
    if pct > 80 {
        Severity::Critical
    } else if pct > 60 {
        Severity::Warning
    } else {
        Severity::Normal
    }
}

/// Severity from pace — how far ahead usage is of where it "should" be at this
/// point in the period. `Critical` means the user is burning budget fast enough
/// to run out before the period resets unless they slow down.
fn pace_severity(usage_pct: u64, time_pct: Option<f64>) -> Severity {
    match time_pct {
        Some(tp) if (usage_pct as f64) > tp + 15.0 => Severity::Critical,
        Some(tp) if (usage_pct as f64) > tp + 5.0 => Severity::Warning,
        _ => Severity::Normal,
    }
}

fn utilization_color(t: &ThemeColors, pct: u64) -> u32 {
    severity_color(t, abs_severity(pct))
}

/// Number of grid segments to split a usage bar into: per-hour for windows up
/// to a day, per-day for longer windows. 1 means no internal dividers.
fn segments_for_window(window_seconds: u64) -> u32 {
    match window_seconds {
        0..=3600 => 1,
        3601..=86400 => (window_seconds / 3600).clamp(2, 12) as u32,
        _ => (window_seconds / 86400).clamp(2, 14) as u32,
    }
}

fn format_window_label(window_seconds: u64) -> &'static str {
    match window_seconds {
        0..=3600 => "1h",
        3601..=18000 => "5h",
        18001..=86400 => "1d",
        86401..=604800 => "7d",
        _ => "30d",
    }
}

fn format_reset_time(reset_at: u64) -> String {
    if reset_at == 0 {
        return String::new();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if reset_at <= now {
        return "now".to_string();
    }
    let remaining = reset_at - now;
    let hours = remaining / 3600;
    let minutes = (remaining % 3600) / 60;
    if hours > 24 {
        let days = hours / 24;
        format!("{}d {}h", days, hours % 24)
    } else if hours > 0 {
        format!("{}h {}m", hours, minutes)
    } else {
        format!("{}m", minutes)
    }
}

fn render_window_row(
    t: &ThemeColors,
    cx: &App,
    label: &str,
    window: &RateLimitWindow,
) -> impl IntoElement {
    let pct = window.used_percent;
    let window_label = format_window_label(window.window_seconds);
    let reset = format_reset_time(window.reset_at);
    let pace = pace_severity(pct, window.time_elapsed_pct);
    // % text reflects whichever is worse: nearness to the cap, or burn pace.
    let pct_color = severity_color(t, abs_severity(pct).max(pace));
    let pace_msg: Option<(&str, u32)> = match pace {
        Severity::Critical => Some(("Slow down to last the period", t.metric_critical)),
        Severity::Warning => Some(("Ahead of pace", t.metric_warning)),
        Severity::Normal => None,
    };

    v_flex()
        .gap(px(2.0))
        .child(
            h_flex()
                .justify_between()
                .child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_secondary))
                        .child(format!("{} ({})", label, window_label)),
                )
                .child(
                    h_flex()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(pct_color))
                                .child(format!("{}%", pct)),
                        )
                        .when(!reset.is_empty(), |el| {
                            el.child(
                                div()
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(format!("resets in {}", reset)),
                            )
                        }),
                ),
        )
        .child(render_usage_with_time_bar(
            t,
            pct,
            window.time_elapsed_pct,
            segments_for_window(window.window_seconds),
        ))
        .when_some(pace_msg, |el, (msg, col)| {
            el.child(
                div()
                    .text_size(ui_text_sm(cx))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(col))
                    .child(msg),
            )
        })
}

fn render_usage_with_time_bar(
    t: &ThemeColors,
    usage_pct: u64,
    time_pct: Option<f64>,
    segments: u32,
) -> impl IntoElement {
    let clamped_usage = (usage_pct as f32).clamp(0.0, 100.0);

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

    // Divider lines splitting the bar into per-hour or per-day segments so the
    // pace marker can be read against a time grid.
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
    // hour). Derived from text_primary so it adapts to light and dark themes.
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
        .h(px(4.0))
        .w_full()
        .rounded(px(2.0))
        .bg(rgb(t.bg_secondary))
        .relative()
        .child(
            div()
                .h_full()
                .rounded(px(2.0))
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
                    .rounded_r(px(2.0))
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
            el.child(
                div()
                    .absolute()
                    .top(px(-1.0))
                    .left(relative(clamped_time / 100.0))
                    .w(px(1.5))
                    .h(px(6.0))
                    .rounded(px(1.0))
                    .bg(rgb(t.text_primary)),
            )
        })
}

impl Render for CodexUsage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let data = self.data.read(cx).data.lock();
        let (primary, secondary) = match data.as_ref() {
            Some(d) => (
                d.primary_window
                    .as_ref()
                    .map(|w| (w.used_percent, w.window_seconds)),
                d.secondary_window
                    .as_ref()
                    .map(|w| (w.used_percent, w.window_seconds)),
            ),
            None => return div().size_0().into_any_element(),
        };
        drop(data);

        let entity_handle = cx.entity().clone();

        div()
            .child(
                h_flex()
                    .id("codex-usage-trigger")
                    .cursor_pointer()
                    .gap(px(3.0))
                    .px(px(4.0))
                    .py(px(1.0))
                    .rounded(px(3.0))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .when_some(primary, |el, (pct, window_secs)| {
                        el.child(
                            h_flex()
                                .gap(px(3.0))
                                .child(
                                    div()
                                        .text_size(ui_text_ms(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child(format_window_label(window_secs)),
                                )
                                .child(
                                    div()
                                        .text_size(ui_text_ms(cx))
                                        .text_color(rgb(utilization_color(&t, pct)))
                                        .child(format!("{}%", pct)),
                                ),
                        )
                    })
                    .when_some(secondary, |el, (pct, window_secs)| {
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
                                        .child(format_window_label(window_secs)),
                                )
                                .child(
                                    div()
                                        .text_size(ui_text_ms(cx))
                                        .text_color(rgb(utilization_color(&t, pct)))
                                        .child(format!("{}%", pct)),
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
    fn test_segments_for_window() {
        // Sub-hour windows get no internal grid.
        assert_eq!(segments_for_window(0), 1);
        assert_eq!(segments_for_window(3600), 1);
        // Multi-hour windows split per hour.
        assert_eq!(segments_for_window(5 * 3600), 5);
        // Per-hour count is capped so the bar doesn't get crowded.
        assert_eq!(segments_for_window(86400), 12);
        // Multi-day windows split per day.
        assert_eq!(segments_for_window(7 * 86400), 7);
        // Per-day count is capped as well.
        assert_eq!(segments_for_window(30 * 86400), 14);
    }
}
