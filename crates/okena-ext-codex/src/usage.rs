use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_extensions::ThemeColors;
use okena_ui::expand::expand_toggle;
use okena_ui::tokens::{ui_text_ms, ui_text_xs};
use okena_usage::{
    SegmentUnit, UsageRow, effective_time_pct, format_reset_time_epoch, read_working_days,
    render_usage_row, usage_body_container, usage_divider, usage_kv_row, usage_popover_container,
    usage_popover_header, usage_trigger_items,
};
use parking_lot::Mutex;
use std::cmp::Ordering as CmpOrdering;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

const RESET_CREDITS_URL: &str = "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";
const USER_AGENT: &str = concat!("okena/", env!("CARGO_PKG_VERSION"));

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

#[derive(Clone)]
struct ResetCredit {
    title: String,
    expires_at: Option<f64>,
}

#[derive(Clone)]
struct ResetCreditsInfo {
    available_count: u64,
    credits: Vec<ResetCredit>,
}

/// All fetched usage data
#[derive(Clone, Default)]
struct UsageData {
    plan_type: String,
    primary_window: Option<RateLimitWindow>,
    secondary_window: Option<RateLimitWindow>,
    review_primary: Option<RateLimitWindow>,
    credits: Option<CreditsInfo>,
    reset_credits: Option<ResetCreditsInfo>,
}

/// Shared usage data + the single background poll task.
///
/// Decoupling this from the per-window view means the usage API is fetched
/// once for the whole app rather than once per open window. Per-window UI
/// state (popover, hover) lives on [`CodexUsage`] instead.
struct CodexUsageData {
    data: Arc<Mutex<Option<UsageData>>>,
    status: Arc<Mutex<FetchStatus>>,
    /// Send on this channel to wake up the fetch loop and retry immediately.
    wake_tx: smol::channel::Sender<()>,
    /// Whether a wake signal has already been sent (avoids spamming from render).
    wake_sent: Arc<AtomicBool>,
    /// Background polling task. Cancelled automatically when this entity is dropped.
    _poll_task: Task<()>,
}

#[derive(Default)]
struct FetchStatus {
    last_success: Option<Instant>,
    error: Option<String>,
    failures: u32,
    next_attempt: Option<Instant>,
    fetching: bool,
}

impl FetchStatus {
    fn complete(&mut self, result: &Result<UsageData, String>, now: Instant) -> Duration {
        self.fetching = false;
        match result {
            Ok(_) => {
                self.last_success = Some(now);
                self.error = None;
                self.failures = 0;
            }
            Err(error) => {
                self.error = Some(error.clone());
                self.failures = self.failures.saturating_add(1);
            }
        }
        let delay = if self.failures == 0 {
            USAGE_INTERVAL
        } else {
            MIN_RETRY_DELAY
                .saturating_mul(1 << self.failures.min(8).saturating_sub(1))
                .min(Duration::from_secs(3600))
        };
        self.next_attempt = Some(now + delay);
        delay
    }
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
    let resp: serde_json::Value = okena_transport::http::send(
        okena_transport::http::HttpRequest::post("https://auth.openai.com/oauth/token")
            .body(
                "application/x-www-form-urlencoded",
                format!(
                    "grant_type=refresh_token&client_id={}&refresh_token={}",
                    CODEX_CLIENT_ID, auth.refresh_token
                ),
            )
            .timeout(Duration::from_secs(10))
            .user_agent(USER_AGENT)
            .label("codex.token-refresh"),
    )
    .ok()?
    .json()
    .ok()?;

    let new_access = resp["access_token"].as_str()?;
    let new_refresh = resp["refresh_token"].as_str();

    // Persist new tokens back to auth.json
    if let Ok(content) = std::fs::read_to_string(&auth.auth_path)
        && let Ok(mut file_json) = serde_json::from_str::<serde_json::Value>(&content)
    {
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
    let window_seconds = v["limit_window_seconds"].as_u64().unwrap_or(0);
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

fn parse_iso8601_to_epoch(ts: &str) -> Option<f64> {
    let timestamp: jiff::Timestamp = ts.parse().ok()?;
    Some(timestamp.as_millisecond() as f64 / 1_000.0)
}

fn parse_reset_credits(body: &serde_json::Value) -> Option<ResetCreditsInfo> {
    let raw_credits = body["credits"].as_array()?;
    let mut credits: Vec<ResetCredit> = raw_credits
        .iter()
        .filter(|credit| credit["status"].as_str() == Some("available"))
        .map(|credit| ResetCredit {
            title: credit["title"]
                .as_str()
                .filter(|title| !title.trim().is_empty())
                .unwrap_or("Full reset")
                .to_string(),
            expires_at: credit["expires_at"]
                .as_str()
                .and_then(parse_iso8601_to_epoch),
        })
        .collect();

    credits.sort_by(|left, right| match (left.expires_at, right.expires_at) {
        (Some(left), Some(right)) => left.total_cmp(&right),
        (Some(_), None) => CmpOrdering::Less,
        (None, Some(_)) => CmpOrdering::Greater,
        (None, None) => CmpOrdering::Equal,
    });

    let available_count = body["available_count"]
        .as_u64()
        .unwrap_or(credits.len() as u64);

    Some(ResetCreditsInfo {
        available_count,
        credits,
    })
}

fn try_fetch_usage_with_token(
    access_token: &str,
    account_id: &str,
) -> Result<okena_transport::http::HttpResponse, Option<u16>> {
    // No min_interval floor here: a single tick can legitimately issue two
    // requests with this label (cached token → 401 → refresh → retry), and a
    // floor would clip the retry. The outer poll cadence is 300s.
    let resp = okena_transport::http::send(
        okena_transport::http::HttpRequest::get("https://chatgpt.com/backend-api/codex/usage")
            .bearer(access_token)
            .header("chatgpt-account-id", account_id)
            .timeout(Duration::from_secs(10))
            .user_agent(USER_AGENT)
            .header("Accept", "application/json")
            .label("codex.usage"),
    )
    .map_err(|_| None)?;

    if resp.is_success() {
        Ok(resp)
    } else {
        Err(Some(resp.status()))
    }
}

fn try_fetch_reset_credits_with_token(
    access_token: &str,
    account_id: &str,
) -> Result<okena_transport::http::HttpResponse, Option<u16>> {
    let resp = okena_transport::http::send(
        okena_transport::http::HttpRequest::get(RESET_CREDITS_URL)
            .bearer(access_token)
            .header("chatgpt-account-id", account_id)
            .timeout(Duration::from_secs(10))
            .user_agent(USER_AGENT)
            .header("Accept", "application/json")
            .label("codex.reset-credits"),
    )
    .map_err(|_| None)?;

    if resp.is_success() {
        Ok(resp)
    } else {
        Err(Some(resp.status()))
    }
}

fn fetch_reset_credits(access_token: &str, account_id: &str) -> Option<ResetCreditsInfo> {
    let resp = match try_fetch_reset_credits_with_token(access_token, account_id) {
        Ok(resp) => resp,
        Err(status) => {
            log::warn!("[codex-usage] reset credits API returned {:?}", status);
            return None;
        }
    };
    let body: serde_json::Value = match resp.json() {
        Ok(body) => body,
        Err(error) => {
            log::warn!("[codex-usage] failed to decode reset credits: {error}");
            return None;
        }
    };
    let parsed = parse_reset_credits(&body);
    if parsed.is_none() {
        log::warn!("[codex-usage] reset credits response had an unexpected shape");
    }
    parsed
}

fn fetch_usage() -> Result<UsageData, String> {
    let auth =
        read_codex_auth().ok_or("Codex credentials missing or invalid; sign in with Codex CLI")?;
    let mut access_token = auth.access_token.clone();

    // Try cached access token first, refresh on 401
    let resp = match try_fetch_usage_with_token(&access_token, &auth.account_id) {
        Ok(resp) => resp,
        Err(Some(401)) => {
            let new_token = refresh_access_token(&auth)
                .ok_or("Token refresh failed; sign in again with Codex CLI")?;
            access_token = new_token;
            match try_fetch_usage_with_token(&access_token, &auth.account_id) {
                Ok(resp) => resp,
                Err(status) => {
                    log::warn!(
                        "[codex-usage] API returned {:?} after token refresh",
                        status
                    );
                    return Err(usage_error(status));
                }
            }
        }
        Err(status) => {
            log::warn!("[codex-usage] API returned {:?}", status);
            return Err(usage_error(status));
        }
    };

    let body: serde_json::Value = resp.json().map_err(|_| "Invalid JSON in usage response")?;
    let mut data = parse_usage(&body)?;
    data.reset_credits = fetch_reset_credits(&access_token, &auth.account_id);
    Ok(data)
}

fn parse_usage(body: &serde_json::Value) -> Result<UsageData, String> {
    let plan_type = body["plan_type"].as_str().unwrap_or("unknown").to_string();

    let primary_window = body["rate_limit"]["primary_window"]
        .as_object()
        .and_then(|_| parse_window(&body["rate_limit"]["primary_window"]));

    let secondary_window = body["rate_limit"]["secondary_window"]
        .as_object()
        .and_then(|_| parse_window(&body["rate_limit"]["secondary_window"]));

    if primary_window.is_none() && secondary_window.is_none() {
        return Err("Usage response contains no rate-limit windows".into());
    }

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
        balance: c.get("balance").and_then(|v| v.as_f64()).unwrap_or(0.0),
    });

    Ok(UsageData {
        plan_type,
        primary_window,
        secondary_window,
        review_primary,
        credits,
        reset_credits: None,
    })
}

fn usage_error(status: Option<u16>) -> String {
    match status {
        Some(code) => format!("Usage request failed: HTTP {code}"),
        None => "Could not connect to usage API (network error or timeout)".into(),
    }
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
        let status = self.status.lock();
        if status.fetching || status.error.is_some() {
            return;
        }
        let stale = match status.last_success {
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
        let status = Arc::new(Mutex::new(FetchStatus::default()));
        let status_for_task = status.clone();
        let (wake_tx, wake_rx) = smol::channel::bounded::<()>(1);
        let wake_sent = Arc::new(AtomicBool::new(false));
        let wake_sent_for_task = wake_sent.clone();

        let poll_task = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                {
                    let mut status = status_for_task.lock();
                    status.fetching = true;
                    status.next_attempt = None;
                }
                wake_sent_for_task.store(false, Ordering::SeqCst);
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
                let result = smol::unblock(fetch_usage).await;
                let delay = status_for_task.lock().complete(&result, Instant::now());
                if let Ok(fetched) = result {
                    *data_for_task.lock() = Some(fetched);
                } else if let Err(error) = result {
                    log::warn!("[codex-usage] {error}");
                }
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
                // Hover can refresh healthy data early; failed attempts keep their backoff.
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
            status,
            wake_tx,
            wake_sent,
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
    resets_expanded: bool,
    trigger_bounds: Bounds<Pixels>,
    hover_token: Arc<AtomicU64>,
    freshness_clock: Option<Task<()>>,
}

impl CodexUsage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let data = CodexUsageData::shared(cx);
        // Re-render this window's widget whenever the shared poller updates.
        cx.observe(&data, |_, _, cx| cx.notify()).detach();
        Self {
            data,
            popover_visible: false,
            resets_expanded: false,
            trigger_bounds: Bounds::default(),
            hover_token: Arc::new(AtomicU64::new(0)),
            freshness_clock: None,
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
                    this.freshness_clock =
                        Some(cx.spawn(async move |this: WeakEntity<Self>, cx| {
                            loop {
                                smol::Timer::after(Duration::from_secs(1)).await;
                                if this.update(cx, |_, cx| cx.notify()).is_err() {
                                    break;
                                }
                            }
                        }));
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
                    this.freshness_clock = None;
                    this.resets_expanded = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn render_popover(&self, t: &ThemeColors, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.popover_visible {
            return div().size_0().into_any_element();
        }
        let data = {
            let shared = self.data.read(cx);
            let data = shared.data.lock();
            data.clone().unwrap_or_default()
        };
        let status_rows = self.render_fetch_status(t, cx);

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
                            (!plan.is_empty()).then_some(plan.as_str()),
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
                                })
                                .when_some(
                                    data.reset_credits.as_ref().filter(|resets| {
                                        resets.available_count > 0 || !resets.credits.is_empty()
                                    }),
                                    |el, resets| {
                                        el.child(usage_divider(t))
                                            .child(self.render_reset_credits(t, resets, cx))
                                    },
                                )
                                .child(usage_divider(t))
                                .child(status_rows),
                        ),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }

    fn render_reset_credits(
        &self,
        t: &ThemeColors,
        resets: &ResetCreditsInfo,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let has_details = !resets.credits.is_empty();
        let expanded = self.resets_expanded && has_details;
        let summary = reset_count_label(resets.available_count);
        let first_expiry = first_reset_expiry_label(resets);

        v_flex()
            .gap(px(5.0))
            .child(
                h_flex()
                    .id("codex-reset-credits-toggle")
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .px(px(2.0))
                    .py(px(2.0))
                    .rounded(px(3.0))
                    .when(has_details, |el| {
                        el.cursor_pointer()
                            .hover(|style| style.bg(rgb(t.bg_hover)))
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.resets_expanded = !this.resets_expanded;
                                cx.notify();
                            }))
                    })
                    .child(
                        h_flex()
                            .items_center()
                            .gap(px(5.0))
                            .child(expand_toggle(
                                "codex-reset-credits-chevron",
                                expanded,
                                has_details,
                                t,
                            ))
                            .child(
                                div()
                                    .text_size(ui_text_ms(cx))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(t.text_secondary))
                                    .child(summary),
                            ),
                    )
                    .when_some(first_expiry, |el, expiry| {
                        el.child(
                            div()
                                .text_size(ui_text_xs(cx))
                                .text_color(rgb(t.text_muted))
                                .child(expiry),
                        )
                    }),
            )
            .when(expanded, |el| {
                el.children(resets.credits.iter().map(|credit| {
                    h_flex()
                        .items_baseline()
                        .justify_between()
                        .gap(px(8.0))
                        .pl(px(19.0))
                        .pr(px(2.0))
                        .child(
                            div()
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.text_secondary))
                                .child(credit.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(ui_text_xs(cx))
                                .text_color(rgb(t.text_muted))
                                .child(reset_expiry_label(credit)),
                        )
                }))
                .when(
                    resets.available_count > resets.credits.len() as u64,
                    |el| {
                        let hidden = resets.available_count - resets.credits.len() as u64;
                        el.child(
                            div()
                                .pl(px(19.0))
                                .text_size(ui_text_xs(cx))
                                .text_color(rgb(t.text_muted))
                                .child(format!("+{hidden} more")),
                        )
                    },
                )
            })
    }

    fn render_fetch_status(&self, t: &ThemeColors, cx: &App) -> AnyElement {
        let status = self.data.read(cx).status.lock();
        let now = Instant::now();
        let age = match status.last_success {
            Some(at) => format!(
                "Updated {} ago{}",
                format_age(now.duration_since(at)),
                if status.error.is_some() || now.duration_since(at) > USAGE_INTERVAL {
                    " · stale"
                } else {
                    ""
                }
            ),
            None => "Usage unavailable · no successful update yet".into(),
        };
        v_flex()
            .gap(px(4.0))
            .text_size(ui_text_xs(cx))
            .text_color(rgb(t.text_secondary))
            .child(age)
            .when_some(status.error.as_ref(), |el, error| {
                el.child(format!("Update failed: {error}"))
                    .child(format!("Failed attempts: {}", status.failures))
            })
            .when(status.fetching, |el| el.child("Updating…"))
            .when_some(status.next_attempt, |el, next| {
                let label = if status.error.is_some() {
                    "Retry"
                } else {
                    "Next update"
                };
                el.child(format!(
                    "{label} in {}",
                    format_age(next.saturating_duration_since(now))
                ))
            })
            .into_any_element()
    }
}

fn format_age(age: Duration) -> String {
    let seconds = age.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60)
    }
}

fn reset_count_label(count: u64) -> String {
    if count == 1 {
        "1 reset available".to_string()
    } else {
        format!("{count} resets available")
    }
}

fn reset_expiry_label(credit: &ResetCredit) -> String {
    let Some(expires_at) = credit.expires_at else {
        return "no expiration".to_string();
    };
    let formatted = format_reset_time_epoch(expires_at, true);
    if formatted.is_empty() {
        "expiration unknown".to_string()
    } else {
        format!("expires {formatted}")
    }
}

fn first_reset_expiry_label(resets: &ResetCreditsInfo) -> Option<String> {
    let expires_at = resets.credits.iter().find_map(|credit| credit.expires_at)?;
    let formatted = format_reset_time_epoch(expires_at, true);
    (!formatted.is_empty()).then(|| format!("first expires {formatted}"))
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
        if let Some(d) = data.as_ref() {
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
        drop(data);

        let entity_handle = cx.entity().clone();

        div()
            .child(
                h_flex()
                    .id("codex-usage-trigger")
                    .cursor_pointer()
                    .gap(px(8.0))
                    .px(px(4.0))
                    .py(px(1.0))
                    .rounded(px(3.0))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .children(usage_trigger_items(&t, cx, &items))
                    .when(items.is_empty(), |el| {
                        el.child(
                            div()
                                .text_size(ui_text_xs(cx))
                                .text_color(rgb(t.text_muted))
                                .child("Codex —"),
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
    fn failed_updates_preserve_freshness_until_recovery() {
        let now = Instant::now();
        let mut status = FetchStatus::default();
        assert_eq!(
            status.complete(&Ok(UsageData::default()), now),
            USAGE_INTERVAL
        );
        let failed_at = now + USAGE_INTERVAL;
        assert_eq!(
            status.complete(&Err("HTTP 403".into()), failed_at),
            MIN_RETRY_DELAY
        );
        assert_eq!(status.last_success, Some(now));
        assert_eq!(status.error.as_deref(), Some("HTTP 403"));
        assert_eq!(status.next_attempt, Some(failed_at + MIN_RETRY_DELAY));
        assert_eq!(
            status.complete(&Err("HTTP 403".into()), failed_at + MIN_RETRY_DELAY),
            MIN_RETRY_DELAY * 2
        );
        assert_eq!(status.failures, 2);
        let recovered_at = failed_at + Duration::from_secs(90);
        assert_eq!(
            status.complete(&Ok(UsageData::default()), recovered_at),
            USAGE_INTERVAL
        );
        assert_eq!(status.last_success, Some(recovered_at));
        assert_eq!(status.failures, 0);
        assert!(status.error.is_none());
    }

    #[test]
    fn failure_before_first_success_does_not_invent_fresh_data() {
        let mut status = FetchStatus::default();
        let now = Instant::now();
        for _ in 0..100 {
            let delay = status.complete(&Err("Network error".into()), now);
            assert!(delay <= Duration::from_secs(3600));
            assert!(status.last_success.is_none());
        }
        assert_eq!(status.next_attempt, Some(now + Duration::from_secs(3600)));
    }

    #[test]
    fn usage_response_requires_usage_but_accepts_zero() {
        assert!(parse_usage(&serde_json::json!({})).is_err());
        assert!(
            parse_usage(&serde_json::json!({
                "rate_limit": {"primary_window": {"reset_at": 1782371508u64}}
            }))
            .is_err()
        );
        let data = parse_usage(&serde_json::json!({
            "rate_limit": {"primary_window": {
                "used_percent": 0,
                "limit_window_seconds": 604800,
                "reset_at": 1782371508u64
            }}
        }))
        .unwrap();
        assert_eq!(data.primary_window.unwrap().used_percent, 0);
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

    #[test]
    fn parse_reset_credits_filters_available_and_sorts_by_expiry() {
        let body = serde_json::json!({
            "available_count": 3,
            "credits": [
                {
                    "status": "available",
                    "title": "Later reset",
                    "expires_at": "2026-08-11T21:08:50.081157Z"
                },
                {
                    "status": "redeemed",
                    "title": "Used reset",
                    "expires_at": "2026-07-01T00:00:00Z"
                },
                {
                    "status": "available",
                    "title": "First reset",
                    "expires_at": "2026-07-18T00:32:13.609604Z"
                },
                {
                    "status": "available",
                    "title": "",
                    "expires_at": null
                }
            ]
        });

        let parsed = parse_reset_credits(&body).expect("reset credits should parse");
        assert_eq!(parsed.available_count, 3);
        assert_eq!(parsed.credits.len(), 3);
        assert_eq!(parsed.credits[0].title, "First reset");
        assert_eq!(parsed.credits[1].title, "Later reset");
        assert_eq!(parsed.credits[2].title, "Full reset");
        assert!(parsed.credits[0].expires_at.is_some());
        assert!(parsed.credits[2].expires_at.is_none());
    }

    #[test]
    fn parse_reset_credits_derives_missing_count() {
        let body = serde_json::json!({
            "credits": [
                {
                    "status": "available",
                    "title": "Full reset",
                    "expires_at": "2026-07-18T00:32:13Z"
                },
                {
                    "status": "redeeming",
                    "title": "In progress",
                    "expires_at": "2026-07-19T00:32:13Z"
                }
            ]
        });

        let parsed = parse_reset_credits(&body).expect("reset credits should parse");
        assert_eq!(parsed.available_count, 1);
        assert_eq!(parsed.credits.len(), 1);
    }

    #[test]
    fn reset_summary_uses_singular_plural_and_first_expiry() {
        assert_eq!(reset_count_label(1), "1 reset available");
        assert_eq!(reset_count_label(3), "3 resets available");

        let resets = ResetCreditsInfo {
            available_count: 1,
            credits: vec![ResetCredit {
                title: "Full reset".to_string(),
                expires_at: parse_iso8601_to_epoch("2026-07-18T00:32:13Z"),
            }],
        };
        let summary = first_reset_expiry_label(&resets).expect("expiry summary should render");
        assert!(summary.starts_with("first expires "));

        let no_expiry = ResetCredit {
            title: "Full reset".to_string(),
            expires_at: None,
        };
        assert_eq!(reset_expiry_label(&no_expiry), "no expiration");
    }
}
