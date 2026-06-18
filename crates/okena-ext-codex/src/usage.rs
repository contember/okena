use okena_extensions::ThemeColors;
use okena_usage::{
    effective_time_pct, read_working_days, render_usage_row, usage_body_container, usage_divider,
    usage_kv_row, usage_popover_container, usage_popover_header, usage_trigger_items, SegmentUnit,
    UsageRow,
};
use base64::Engine as _;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::h_flex;
use parking_lot::Mutex;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Refresh interval for usage data
const USAGE_INTERVAL: Duration = Duration::from_secs(300);

/// Minimum retry delay
const MIN_RETRY_DELAY: Duration = Duration::from_secs(30);

/// Hover delay before showing the popover (ms)
const HOVER_DELAY_MS: u64 = 300;

/// Minimum interval between hover-triggered re-fetches.
const HOVER_REFETCH_THROTTLE: Duration = Duration::from_secs(60);

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
    /// Send on this channel to wake up the fetch loop and retry immediately.
    wake_tx: smol::channel::Sender<()>,
    /// Whether a wake signal has already been sent (avoids spamming from render).
    wake_sent: Arc<AtomicBool>,
    /// Timestamp of the most recent successful fetch — used to throttle hover-triggered refreshes.
    last_fetch_at: Arc<Mutex<Option<Instant>>>,
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
    // The live `/codex/usage` API uses `reset_at`; the local `token_count`
    // session events use `resets_at` (plural). Accept either — missing it leaves
    // the bar with no reset anchor, which silently disables the day/hour grid
    // and the working-days reshape (the bar collapses to a single block).
    let reset_at = v["reset_at"]
        .as_u64()
        .or_else(|| v["resets_at"].as_u64())
        .unwrap_or(0);

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

    fn new(cx: &mut Context<Self>) -> Self {
        let data: Arc<Mutex<Option<UsageData>>> = Arc::new(Mutex::new(None));
        let data_for_task = data.clone();
        let (wake_tx, wake_rx) = smol::channel::bounded::<()>(1);
        let wake_sent = Arc::new(AtomicBool::new(false));
        let wake_sent_for_task = wake_sent.clone();
        let last_fetch_at: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        let last_fetch_at_for_task = last_fetch_at.clone();

        let poll_task = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let mut consecutive_failures: u32 = 0;
            loop {
                let result = smol::unblock(fetch_usage).await;

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

                let delay = if consecutive_failures > 0 {
                    let backoff = MIN_RETRY_DELAY
                        .saturating_mul(1 << consecutive_failures.min(6).saturating_sub(1));
                    backoff.min(Duration::from_secs(3600))
                } else {
                    USAGE_INTERVAL
                };
                // Race: sleep vs wake signal (e.g. when the popover opens and the
                // data is stale). Don't reset consecutive_failures on wake — keep
                // the backoff to avoid retry storms during failures.
                smol::future::or(
                    async {
                        smol::Timer::after(delay).await;
                    },
                    async {
                        let _ = wake_rx.recv().await;
                    },
                )
                .await;
                // Drain any extra wake signals.
                while wake_rx.try_recv().is_ok() {}
            }
        });

        Self {
            data,
            wake_tx,
            wake_sent,
            last_fetch_at,
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
        let plan = data.plan_type.clone();
        let bounds = self.trigger_bounds;
        let position = point(bounds.origin.x, bounds.origin.y - px(4.0));

        deferred(
            anchored()
                .position(position)
                .anchor(Anchor::BottomLeft)
                .snap_to_window()
                .child(
                    usage_popover_container(t)
                        .id("codex-usage-popover")
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
                            "CODEX USAGE",
                            Some(plan.as_str()),
                            "https://chatgpt.com",
                            "Open usage settings on chatgpt.com",
                            t,
                            cx,
                        ))
                        .child(
                            usage_body_container()
                                .when_some(data.primary_window.as_ref(), |el, w| {
                                    el.child(render_usage_row(
                                        t,
                                        cx,
                                        &window_row("Rate Limit", w, "codex-marker-primary"),
                                        working,
                                    ))
                                })
                                .when_some(data.secondary_window.as_ref(), |el, w| {
                                    el.child(render_usage_row(
                                        t,
                                        cx,
                                        &window_row("Secondary", w, "codex-marker-secondary"),
                                        working,
                                    ))
                                })
                                .when_some(data.review_primary.as_ref(), |el, w| {
                                    el.child(render_usage_row(
                                        t,
                                        cx,
                                        &window_row("Code Review", w, "codex-marker-review"),
                                        working,
                                    ))
                                })
                                .when_some(data.credits.as_ref(), |el, c| {
                                    let value = if c.unlimited {
                                        Some(("Unlimited".to_string(), t.metric_normal))
                                    } else if c.has_credits {
                                        Some((format!("${:.2}", c.balance), t.text_primary))
                                    } else {
                                        None
                                    };
                                    match value {
                                        Some((text, color)) => el
                                            .child(usage_divider(t))
                                            .child(usage_kv_row(t, cx, "Credits", text, color)),
                                        None => el,
                                    }
                                }),
                        ),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }
}

/// Pick the grid granularity for a window: per-hour up to a day, per-day for
/// longer windows. Sub-hour windows get no grid.
fn segment_unit_for_window(window_seconds: u64) -> Option<SegmentUnit> {
    match window_seconds {
        0..=3600 => None,
        3601..=86400 => Some(SegmentUnit::Hour),
        _ => Some(SegmentUnit::Day),
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

/// Build a shared [`UsageRow`] from a Codex rate-limit window.
fn window_row(label: &str, window: &RateLimitWindow, marker_id: &'static str) -> UsageRow {
    let unit = segment_unit_for_window(window.window_seconds);
    let reset_epoch = (window.reset_at > 0).then_some(window.reset_at as f64);
    UsageRow {
        label: label.into(),
        period: format_window_label(window.window_seconds).into(),
        pct: window.used_percent as f64,
        time_pct: window.time_elapsed_pct,
        reset_epoch,
        period_secs: window.window_seconds as f64,
        unit,
        marker_id: marker_id.into(),
    }
}

impl Render for CodexUsage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let working = read_working_days(cx);
        let data = self.data.read(cx).data.lock();
        let mut items: Vec<(SharedString, f64, Option<f64>)> = Vec::new();
        match data.as_ref() {
            Some(d) => {
                for w in [d.primary_window.as_ref(), d.secondary_window.as_ref()]
                    .into_iter()
                    .flatten()
                {
                    let et = effective_time_pct(
                        (w.reset_at > 0).then_some(w.reset_at as f64),
                        w.window_seconds as f64,
                        segment_unit_for_window(w.window_seconds),
                        working,
                        w.time_elapsed_pct,
                    );
                    items.push((
                        format_window_label(w.window_seconds).into(),
                        w.used_percent as f64,
                        et,
                    ));
                }
            }
            None => return div().size_0().into_any_element(),
        }
        drop(data);

        let entity_handle = cx.entity().clone();

        div()
            .child(
                h_flex()
                    .id("codex-usage-trigger")
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
    fn test_segment_unit_for_window() {
        // Sub-hour windows get no grid.
        assert!(segment_unit_for_window(0).is_none());
        assert!(segment_unit_for_window(3600).is_none());
        // Up to a day → per-hour; longer → per-day.
        assert!(matches!(
            segment_unit_for_window(5 * 3600),
            Some(SegmentUnit::Hour)
        ));
        assert!(matches!(
            segment_unit_for_window(86400),
            Some(SegmentUnit::Hour)
        ));
        assert!(matches!(
            segment_unit_for_window(7 * 86400),
            Some(SegmentUnit::Day)
        ));
    }

    #[test]
    fn parse_window_reads_session_field_names() {
        // Local `token_count` session events use `window_minutes` + `resets_at`
        // (plural). Both must be picked up; missing the reset anchor used to
        // collapse the weekly bar to a single block.
        let v = serde_json::json!({
            "used_percent": 3.0,
            "window_minutes": 10080,
            "resets_at": 1782371508u64,
        });
        let w = parse_window(&v).expect("window should parse");
        assert_eq!(w.used_percent, 3);
        assert_eq!(w.window_seconds, 10080 * 60, "weekly window = 7 days");
        assert_eq!(w.reset_at, 1782371508, "must read `resets_at` (plural)");
    }

    #[test]
    fn parse_window_reads_live_api_field_names() {
        // The live `/codex/usage` API uses `limit_window_seconds` + `reset_at`.
        let v = serde_json::json!({
            "used_percent": 50,
            "limit_window_seconds": 604800,
            "reset_at": 1782371508u64,
        });
        let w = parse_window(&v).expect("window should parse");
        assert_eq!(w.window_seconds, 604800);
        assert_eq!(w.reset_at, 1782371508);
    }
}
