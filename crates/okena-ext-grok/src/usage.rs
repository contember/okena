use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_extensions::ThemeColors;
use okena_ui::tokens::ui_text_xs;
use okena_usage::{
    SegmentUnit, UsageRow, effective_time_pct, read_working_days, render_usage_row,
    usage_body_container, usage_divider, usage_kv_row, usage_popover_container,
    usage_popover_header, usage_trigger_items,
};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Refresh interval for usage data.
const USAGE_INTERVAL: Duration = Duration::from_secs(300);
const MIN_RETRY_DELAY: Duration = Duration::from_secs(30);
const HOVER_DELAY_MS: u64 = 300;
const HOVER_REFETCH_THROTTLE: Duration = Duration::from_secs(60);

/// The Grok CLI requests this with its grok.com session. `grok.com` itself
/// returns 404 for the same path.
const BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
const DEFAULT_TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
const USER_AGENT: &str = concat!("okena/", env!("CARGO_PKG_VERSION"));

fn theme(cx: &App) -> ThemeColors {
    okena_extensions::theme(cx)
}

struct GlobalGrokUsageData(WeakEntity<GrokUsageData>);
impl Global for GlobalGrokUsageData {}

#[derive(Clone)]
struct CreditWindow {
    used_percent: f64,
    window_seconds: f64,
    reset_at: Option<f64>,
    time_elapsed_pct: Option<f64>,
}

#[derive(Clone, Default)]
struct UsageData {
    history: okena_usage::history::History,
    plan: String,
    included: Option<CreditWindow>,
    on_demand: Option<CreditWindow>,
    prepaid_balance: Option<f64>,
    source: String,
}

struct GrokUsageData {
    data: Arc<Mutex<Option<UsageData>>>,
    status: Arc<Mutex<FetchStatus>>,
    wake_tx: smol::channel::Sender<()>,
    wake_sent: Arc<AtomicBool>,
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

struct GrokAuth {
    entry_key: String,
    access_token: String,
    refresh_token: Option<String>,
    client_id: Option<String>,
    issuer: Option<String>,
    user_id: String,
    expires_at: Option<String>,
    auth_path: PathBuf,
}

/// `GROK_HOME` when it exists, otherwise `~/.grok`.
fn resolve_grok_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("GROK_HOME") {
        let path = PathBuf::from(dir);
        if path.is_dir() {
            return path;
        }
        log::warn!("[grok-usage] GROK_HOME does not exist, falling back to ~/.grok");
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".grok")
}

fn read_grok_auth(dir: &Path) -> Option<GrokAuth> {
    let path = dir.join("auth.json");
    let content = std::fs::read_to_string(&path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    let entries = value.as_object()?;
    let now = jiff::Timestamp::now();
    let (entry_key, entry) = entries
        .iter()
        .filter(|(_, entry)| {
            entry["key"]
                .as_str()
                .is_some_and(|token| !token.trim().is_empty())
        })
        .min_by_key(|(_, entry)| access_token_expired(entry["expires_at"].as_str(), now) as u8)?;
    let access_token = entry["key"].as_str()?.trim().to_string();
    let user_id = entry["user_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .unwrap_or("default")
        .to_string();
    Some(GrokAuth {
        entry_key: entry_key.clone(),
        access_token,
        refresh_token: non_empty(entry["refresh_token"].as_str()),
        client_id: non_empty(entry["oidc_client_id"].as_str()),
        issuer: non_empty(entry["oidc_issuer"].as_str()),
        user_id,
        expires_at: non_empty(entry["expires_at"].as_str()),
        auth_path: path,
    })
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn access_token_expired(expires_at: Option<&str>, now: jiff::Timestamp) -> bool {
    let Some(expires_at) = expires_at.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    let Ok(expires) = expires_at.parse::<jiff::Timestamp>() else {
        return true;
    };
    expires <= now
}

fn token_endpoint(issuer: Option<&str>) -> String {
    match issuer.map(|issuer| issuer.trim_end_matches('/')) {
        Some("https://auth.x.ai") | None => DEFAULT_TOKEN_URL.to_string(),
        Some(issuer) => format!("{issuer}/oauth2/token"),
    }
}

fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn refresh_form(client_id: &str, refresh_token: &str) -> String {
    format!(
        "grant_type=refresh_token&client_id={}&refresh_token={}",
        form_encode(client_id),
        form_encode(refresh_token)
    )
}

fn client_version(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("version.json"))
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok())
        .and_then(|value| non_empty(value["version"].as_str()))
        .unwrap_or_else(|| "okena".to_string())
}

fn refresh_access_token(auth: &GrokAuth) -> Result<String, String> {
    let client_id = auth
        .client_id
        .as_deref()
        .ok_or("Grok session has no OIDC client id; sign in again with `grok login`")?;
    let refresh_token = auth
        .refresh_token
        .as_deref()
        .ok_or("Grok session has no refresh token; sign in again with `grok login`")?;
    let response = okena_transport::http::send(
        okena_transport::http::HttpRequest::post(token_endpoint(auth.issuer.as_deref()))
            .body(
                "application/x-www-form-urlencoded",
                refresh_form(client_id, refresh_token),
            )
            .timeout(Duration::from_secs(10))
            .user_agent(USER_AGENT)
            .header("Accept", "application/json")
            .label("grok.token-refresh"),
    )
    .map_err(|_| "Token refresh failed; sign in again with `grok login`".to_string())?;
    if !response.is_success() {
        return Err(format!(
            "Token refresh failed: HTTP {}; sign in again with `grok login`",
            response.status()
        ));
    }
    let body: serde_json::Value = response
        .json()
        .map_err(|_| "Token refresh returned invalid JSON".to_string())?;
    let access =
        non_empty(body["access_token"].as_str()).ok_or("Token refresh returned no access token")?;
    let refreshed = non_empty(body["refresh_token"].as_str());
    let expires_at = body["expires_in"].as_i64().map(|seconds| {
        let expires = jiff::Timestamp::now() + jiff::SignedDuration::from_secs(seconds.max(0));
        expires.to_string()
    });
    persist_tokens(
        &auth.auth_path,
        &auth.entry_key,
        &access,
        refreshed.as_deref(),
        expires_at.as_deref(),
    );
    Ok(access)
}

fn persist_tokens(
    path: &Path,
    entry_key: &str,
    access_token: &str,
    refresh_token: Option<&str>,
    expires_at: Option<&str>,
) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(mut file) = serde_json::from_str::<serde_json::Value>(&content) else {
        return;
    };
    let Some(entry) = file
        .get_mut(entry_key)
        .and_then(|entry| entry.as_object_mut())
    else {
        return;
    };
    entry.insert(
        "key".to_string(),
        serde_json::Value::String(access_token.to_string()),
    );
    if let Some(refresh_token) = refresh_token {
        entry.insert(
            "refresh_token".to_string(),
            serde_json::Value::String(refresh_token.to_string()),
        );
    }
    if let Some(expires_at) = expires_at {
        entry.insert(
            "expires_at".to_string(),
            serde_json::Value::String(expires_at.to_string()),
        );
    }
    let Ok(updated) = serde_json::to_string_pretty(&file) else {
        return;
    };
    let temporary = path.with_extension("json.okena-tmp");
    if std::fs::write(&temporary, updated).is_ok() && std::fs::rename(&temporary, path).is_err() {
        let _ = std::fs::remove_file(temporary);
    }
}

fn billing_request(access_token: &str, version: &str) -> okena_transport::http::HttpRequest {
    okena_transport::http::HttpRequest::get(BILLING_URL)
        .bearer(access_token)
        .header("Accept", "application/json")
        .header("x-grok-client-version", version)
        .header("x-grok-client-identifier", "xai-grok-cli")
        .header("X-XAI-Token-Auth", "xai-grok-cli")
        .timeout(Duration::from_secs(10))
        .user_agent(USER_AGENT)
        .label("grok.usage")
}

fn fetch_usage() -> Result<UsageData, String> {
    let dir = resolve_grok_dir();
    let auth = read_grok_auth(&dir)
        .ok_or("Grok credentials missing; sign in with `grok login`".to_string())?;
    let version = client_version(&dir);
    let mut access_token = auth.access_token.clone();
    if access_token_expired(auth.expires_at.as_deref(), jiff::Timestamp::now()) {
        access_token = refresh_access_token(&auth)?;
    }

    let response = match okena_transport::http::send(billing_request(&access_token, &version)) {
        Ok(response) if response.status() == 401 => {
            access_token = refresh_access_token(&auth)?;
            okena_transport::http::send(billing_request(&access_token, &version))
                .map_err(|_| "Could not connect to Grok usage API".to_string())?
        }
        Ok(response) => response,
        Err(_) => return Err("Could not connect to Grok usage API".to_string()),
    };
    if !response.is_success() {
        return Err(format!("Usage request failed: HTTP {}", response.status()));
    }
    let body: serde_json::Value = response
        .json()
        .map_err(|_| "Invalid JSON in usage response".to_string())?;
    let mut data = parse_usage(&body, now_epoch())?;
    data.source = auth.user_id.clone();
    let mut limits = Vec::new();
    if let Some(window) = &data.included {
        limits.push(sample("included", window));
    }
    if let Some(window) = &data.on_demand {
        limits.push(sample("on_demand", window));
    }
    data.history = okena_usage::history::record_and_load(
        okena_usage::history::Provider::Grok,
        auth.user_id,
        (!data.plan.is_empty()).then_some(data.plan.clone()),
        limits,
    );
    Ok(data)
}

fn sample(name: &str, window: &CreditWindow) -> okena_usage::history::LimitSample {
    okena_usage::history::LimitSample {
        name: name.to_string(),
        used_percent: window.used_percent,
        window_seconds: window.window_seconds,
        reset_at: window.reset_at,
    }
}

fn now_epoch() -> f64 {
    jiff::Timestamp::now().as_millisecond() as f64 / 1_000.0
}

fn parse_usage(body: &serde_json::Value, now: f64) -> Result<UsageData, String> {
    let plan = find_string(
        body,
        &[
            "subscriptionTier",
            "subscription_tier",
            "subscription_tier_display",
        ],
    )
    .unwrap_or_default();
    let included_used = find_number(body, &["includedUsed", "included_used"])
        .or_else(|| find_number(body, &["totalUsed", "total_used"]));
    let monthly_limit = find_number(body, &["monthlyLimit", "monthly_limit"]);
    let included_percent = usage_percent(
        find_number(body, &["creditUsagePercent", "credit_usage_percent"]),
        included_used,
        monthly_limit,
    );
    let (window_seconds, reset_at, time_elapsed_pct) = billing_window(
        find_string(body, &["billingPeriodStart", "billing_period_start"]).as_deref(),
        find_string(body, &["billingPeriodEnd", "billing_period_end"]).as_deref(),
        now,
    );
    let included = included_percent.map(|used_percent| CreditWindow {
        used_percent,
        window_seconds,
        reset_at,
        time_elapsed_pct,
    });

    let on_demand_cap = find_number(body, &["onDemandCap", "on_demand_cap"]);
    let on_demand_used = find_number(body, &["onDemandUsed", "on_demand_used"]).unwrap_or(0.0);
    let on_demand = on_demand_cap
        .filter(|cap| *cap > 0.0)
        .map(|cap| CreditWindow {
            used_percent: (on_demand_used / cap * 100.0).clamp(0.0, 100.0),
            window_seconds,
            reset_at,
            time_elapsed_pct,
        });
    if included.is_none() && on_demand.is_none() {
        log::warn!(
            "[grok-usage] usage response had no recognized credit fields; keys: {}",
            value_keys(body)
        );
        return Err("Usage response contains no credit limit".to_string());
    }
    Ok(UsageData {
        history: okena_usage::history::History::default(),
        plan,
        included,
        on_demand,
        prepaid_balance: find_number(body, &["prepaidBalance", "prepaid_balance"]),
        source: String::new(),
    })
}

fn usage_percent(explicit: Option<f64>, used: Option<f64>, limit: Option<f64>) -> Option<f64> {
    let ratio = match (used, limit) {
        (Some(used), Some(limit)) if limit > 0.0 => Some((used / limit * 100.0).clamp(0.0, 100.0)),
        _ => None,
    };
    match explicit.filter(|value| value.is_finite() && *value >= 0.0) {
        Some(value) if value <= 1.0 => {
            if let Some(ratio) = ratio
                && (ratio - value * 100.0).abs() <= 1.5
            {
                return Some(ratio);
            }
            Some(value.clamp(0.0, 100.0))
        }
        Some(value) => Some(value.clamp(0.0, 100.0)),
        None => ratio,
    }
}

fn billing_window(
    start: Option<&str>,
    end: Option<&str>,
    now: f64,
) -> (f64, Option<f64>, Option<f64>) {
    let start = start.and_then(parse_epoch);
    let end = end.and_then(parse_epoch);
    match (start, end) {
        (Some(start), Some(end)) if end > start => {
            let window = end - start;
            let elapsed = ((now - start) / window * 100.0).clamp(0.0, 100.0);
            (window, Some(end), Some(elapsed))
        }
        _ => (0.0, end, None),
    }
}

fn parse_epoch(value: &str) -> Option<f64> {
    let normalized = value.trim().replace("+00:00", "Z");
    let timestamp: jiff::Timestamp = normalized.parse().ok()?;
    Some(timestamp.as_millisecond() as f64 / 1_000.0)
}

fn find_number(value: &serde_json::Value, names: &[&str]) -> Option<f64> {
    find_value(value, names).and_then(number)
}

fn find_string(value: &serde_json::Value, names: &[&str]) -> Option<String> {
    find_value(value, names).and_then(|value| non_empty(value.as_str()))
}

fn find_value<'a>(value: &'a serde_json::Value, names: &[&str]) -> Option<&'a serde_json::Value> {
    let mut queue = vec![value];
    while let Some(current) = queue.pop() {
        if let Some(object) = current.as_object() {
            for name in names {
                if let Some(found) = object.get(*name) {
                    return Some(found);
                }
            }
            queue.extend(object.values());
        } else if let Some(items) = current.as_array() {
            queue.extend(items);
        }
    }
    None
}

fn number(value: &serde_json::Value) -> Option<f64> {
    if let Some(inner) = value.get("val") {
        return number(inner);
    }
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|number| number as f64))
        .or_else(|| value.as_u64().map(|number| number as f64))
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        .filter(|number| number.is_finite())
}

fn value_keys(value: &serde_json::Value) -> String {
    value
        .as_object()
        .map(|object| object.keys().cloned().collect::<Vec<_>>().join(", "))
        .unwrap_or_else(|| value.to_string())
}

fn period_label(window_seconds: f64) -> String {
    if window_seconds <= 0.0 {
        return "period".to_string();
    }
    let days = window_seconds / 86_400.0;
    if (27.0..=32.0).contains(&days) {
        "30d".to_string()
    } else if (6.5..=7.5).contains(&days) {
        "7d".to_string()
    } else if (0.9..=1.1).contains(&days) {
        "1d".to_string()
    } else if days >= 1.0 {
        format!("{days:.0}d")
    } else {
        format!("{:.0}h", window_seconds / 3_600.0)
    }
}

fn segment_unit(window_seconds: f64) -> Option<SegmentUnit> {
    if window_seconds > 86_400.0 {
        Some(SegmentUnit::Day)
    } else if window_seconds > 3_600.0 {
        Some(SegmentUnit::Hour)
    } else {
        None
    }
}

fn window_row(label: &str, window: &CreditWindow, marker_id: &str) -> UsageRow {
    UsageRow {
        label: label.into(),
        period: period_label(window.window_seconds).into(),
        pct: window.used_percent,
        time_pct: window.time_elapsed_pct,
        reset_epoch: window.reset_at,
        period_secs: window.window_seconds,
        unit: segment_unit(window.window_seconds),
        marker_id: marker_id.into(),
    }
}

fn format_balance(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
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

impl GrokUsageData {
    fn shared(cx: &mut App) -> Entity<Self> {
        if let Some(existing) = cx
            .try_global::<GlobalGrokUsageData>()
            .and_then(|global| global.0.upgrade())
        {
            return existing;
        }
        let entity = cx.new(Self::new);
        cx.set_global(GlobalGrokUsageData(entity.downgrade()));
        entity
    }

    fn request_fresh_fetch(&self) {
        let status = self.status.lock();
        if status.fetching || status.error.is_some() {
            return;
        }
        let stale = match status.last_success {
            None => true,
            Some(last) => last.elapsed() >= HOVER_REFETCH_THROTTLE,
        };
        if stale && !self.wake_sent.swap(true, Ordering::SeqCst) {
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
                match result {
                    Ok(fetched) => *data_for_task.lock() = Some(fetched),
                    Err(error) => log::warn!("[grok-usage] {error}"),
                }
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
                smol::future::or(
                    async {
                        smol::Timer::after(delay).await;
                    },
                    async {
                        let _ = wake_rx.recv().await;
                    },
                )
                .await;
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

pub struct GrokUsage {
    data: Entity<GrokUsageData>,
    chart_state: Entity<okena_usage::HistoryChartState>,
    popover_visible: bool,
    trigger_bounds: Bounds<Pixels>,
    hover_token: Arc<AtomicU64>,
    freshness_clock: Option<Task<()>>,
}

impl GrokUsage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let data = GrokUsageData::shared(cx);
        let chart_state = cx.new(|_| okena_usage::HistoryChartState::default());
        cx.observe(&chart_state, |_, _, cx| cx.notify()).detach();
        cx.observe(&data, |_, _, cx| cx.notify()).detach();
        Self {
            data,
            chart_state,
            popover_visible: false,
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
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn render_popover(&self, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        if !self.popover_visible {
            return div().size_0().into_any_element();
        }
        let data = self.data.read(cx).data.lock().clone().unwrap_or_default();
        let working = read_working_days(cx);
        let bounds = self.trigger_bounds;
        let position = point(bounds.origin.x, bounds.origin.y - px(4.0));
        let plan = (!data.plan.is_empty()).then_some(data.plan.as_str());
        deferred(
            anchored()
                .position(position)
                .anchor(Anchor::BottomLeft)
                .snap_to_window()
                .child(
                    usage_popover_container(t)
                        .id("grok-usage-popover")
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
                            "GROK USAGE",
                            plan,
                            "https://grok.com/?_s=usage",
                            "Open Grok usage",
                            t,
                            cx,
                        ))
                        .child(
                            usage_body_container()
                                .when_some(data.included.as_ref(), |element, window| {
                                    element.child(render_usage_row(
                                        t,
                                        cx,
                                        &window_row("Credits", window, "grok-marker-included"),
                                        working,
                                        &data.history,
                                        "included",
                                        &self.chart_state,
                                    ))
                                })
                                .when_some(data.on_demand.as_ref(), |element, window| {
                                    element.child(render_usage_row(
                                        t,
                                        cx,
                                        &window_row("On-demand", window, "grok-marker-on-demand"),
                                        working,
                                        &data.history,
                                        "on_demand",
                                        &self.chart_state,
                                    ))
                                })
                                .when_some(
                                    data.prepaid_balance.filter(|balance| *balance > 0.0),
                                    |element, balance| {
                                        element.child(usage_divider(t)).child(usage_kv_row(
                                            t,
                                            cx,
                                            "Prepaid",
                                            format_balance(balance),
                                            t.text_primary,
                                        ))
                                    },
                                )
                                .child(usage_divider(t))
                                .child(self.render_fetch_status(t, cx)),
                        ),
                ),
        )
        .with_priority(1)
        .into_any_element()
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
            None => "Usage unavailable · no successful update yet".to_string(),
        };
        v_flex()
            .gap(px(4.0))
            .text_size(ui_text_xs(cx))
            .text_color(rgb(t.text_secondary))
            .child(age)
            .when_some(status.error.as_ref(), |element, error| {
                element
                    .child(format!("Update failed: {error}"))
                    .child(format!("Failed attempts: {}", status.failures))
            })
            .when(status.fetching, |element| element.child("Updating…"))
            .when_some(status.next_attempt, |element, next| {
                let label = if status.error.is_some() {
                    "Retry"
                } else {
                    "Next update"
                };
                element.child(format!(
                    "{label} in {}",
                    format_age(next.saturating_duration_since(now))
                ))
            })
            .into_any_element()
    }
}

impl Render for GrokUsage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let working = read_working_days(cx);
        let data = self.data.read(cx).data.lock();
        let mut items: Vec<(SharedString, f64, Option<f64>)> = Vec::new();
        if let Some(data) = data.as_ref() {
            if let Some(window) = &data.included {
                items.push((
                    "Grok".into(),
                    window.used_percent,
                    effective_time_pct(
                        window.reset_at,
                        window.window_seconds,
                        segment_unit(window.window_seconds),
                        working,
                        window.time_elapsed_pct,
                    ),
                ));
            }
            if let Some(window) = &data.on_demand {
                items.push((
                    "Extra".into(),
                    window.used_percent,
                    effective_time_pct(
                        window.reset_at,
                        window.window_seconds,
                        segment_unit(window.window_seconds),
                        working,
                        window.time_elapsed_pct,
                    ),
                ));
            }
        }
        drop(data);
        let entity = cx.entity().clone();

        div()
            .child(
                h_flex()
                    .id("grok-usage-trigger")
                    .cursor_pointer()
                    .gap(px(8.0))
                    .px(px(4.0))
                    .py(px(1.0))
                    .rounded(px(3.0))
                    .hover(|style| style.bg(rgb(t.bg_hover)))
                    .children(usage_trigger_items(&t, cx, &items))
                    .when(items.is_empty(), |element| {
                        element.child(
                            div()
                                .text_size(ui_text_xs(cx))
                                .text_color(rgb(t.text_muted))
                                .child("Grok —"),
                        )
                    })
                    .child(
                        canvas(
                            move |bounds, _window, app| {
                                entity.update(app, |this, _cx| {
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
    use core::prelude::rust_2024::test;

    fn epoch(value: &str) -> f64 {
        parse_epoch(value).unwrap()
    }

    #[test]
    fn credit_fields_from_the_grok_cli_become_usage_windows() {
        let now = epoch("2026-10-13T00:00:00Z");
        let data = parse_usage(
            &serde_json::json!({
                "creditUsagePercent": 37,
                "monthlyLimit": 1000,
                "includedUsed": 370,
                "onDemandCap": 200,
                "onDemandUsed": 20,
                "prepaidBalance": 15.5,
                "billingPeriodStart": "2026-10-01T00:00:00Z",
                "billingPeriodEnd": "2026-11-01T00:00:00Z",
                "subscriptionTier": "SuperGrok"
            }),
            now,
        )
        .unwrap();
        let included = data.included.unwrap();
        assert_eq!(included.used_percent, 37.0);
        assert_eq!(included.reset_at, Some(epoch("2026-11-01T00:00:00Z")));
        assert!((included.window_seconds - 31.0 * 86_400.0).abs() < 1.0);
        assert!(included.time_elapsed_pct.unwrap() > 30.0);
        assert_eq!(data.on_demand.unwrap().used_percent, 10.0);
        assert_eq!(data.prepaid_balance, Some(15.5));
        assert_eq!(data.plan, "SuperGrok");
    }

    #[test]
    fn the_cli_proxy_response_is_a_weekly_credit_window() {
        let now = epoch("2026-10-13T00:00:00Z");
        let data = parse_usage(
            &serde_json::json!({
                "config": {
                    "currentPeriod": {
                        "type": "USAGE_PERIOD_TYPE_WEEKLY",
                        "start": "2026-10-08T00:00:00+00:00",
                        "end": "2026-10-15T00:00:00+00:00"
                    },
                    "creditUsagePercent": 3.0,
                    "onDemandCap": {"val": 0},
                    "onDemandUsed": {"val": 0},
                    "prepaidBalance": {"val": 0},
                    "billingPeriodStart": "2026-10-08T00:00:00+00:00",
                    "billingPeriodEnd": "2026-10-15T00:00:00+00:00"
                }
            }),
            now,
        )
        .unwrap();
        let included = data.included.unwrap();
        assert_eq!(included.used_percent, 3.0);
        assert_eq!(included.window_seconds, 7.0 * 86_400.0);
        assert_eq!(included.reset_at, Some(epoch("2026-10-15T00:00:00Z")));
        assert!(data.on_demand.is_none());
        assert_eq!(data.prepaid_balance, Some(0.0));
    }

    #[test]
    fn a_fractional_credit_percent_is_scaled_when_usage_confirms_it() {
        let data = parse_usage(
            &serde_json::json!({
                "creditUsagePercent": 0.37,
                "includedUsed": 370,
                "monthlyLimit": 1000
            }),
            0.0,
        )
        .unwrap();
        assert_eq!(data.included.unwrap().used_percent, 37.0);
    }

    #[test]
    fn used_and_limit_are_enough_when_the_percent_is_absent() {
        let data = parse_usage(
            &serde_json::json!({"included_used": 25, "monthly_limit": 100}),
            0.0,
        )
        .unwrap();
        assert_eq!(data.included.unwrap().used_percent, 25.0);
    }

    #[test]
    fn zero_usage_is_a_real_reading() {
        let data = parse_usage(
            &serde_json::json!({
                "subscriptionTier": "Free",
                "config": {"creditUsagePercent": 0, "monthlyLimit": 100, "includedUsed": 0}
            }),
            0.0,
        )
        .unwrap();
        assert_eq!(data.included.unwrap().used_percent, 0.0);
        assert_eq!(data.plan, "Free");
    }

    #[test]
    fn a_response_without_a_credit_limit_is_rejected() {
        assert!(parse_usage(&serde_json::json!({"subscriptionTier": "Free"}), 0.0).is_err());
    }

    #[test]
    fn auth_file_selects_the_usable_session_and_detects_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let auth = serde_json::json!({
            "https://auth.x.ai::client": {
                "key": "access-token",
                "auth_mode": "oidc",
                "refresh_token": "refresh-token",
                "expires_at": "2026-07-10T17:58:56.997660Z",
                "oidc_issuer": "https://auth.x.ai",
                "oidc_client_id": "b1a00492-073a-47ea-816f-4c329264a828",
                "user_id": "user-1"
            }
        });
        std::fs::write(dir.path().join("auth.json"), auth.to_string()).unwrap();
        let session = read_grok_auth(dir.path()).unwrap();
        assert_eq!(session.user_id, "user-1");
        assert_eq!(session.issuer.as_deref(), Some("https://auth.x.ai"));
        assert!(access_token_expired(
            session.expires_at.as_deref(),
            "2026-10-13T00:00:00Z".parse().unwrap()
        ));
        assert!(!access_token_expired(
            Some("2026-10-14T00:00:00Z"),
            "2026-10-13T00:00:00Z".parse().unwrap()
        ));
    }

    #[test]
    fn refresh_form_encodes_reserved_characters() {
        assert_eq!(
            refresh_form("client id", "a/b+c"),
            "grant_type=refresh_token&client_id=client%20id&refresh_token=a%2Fb%2Bc"
        );
        assert_eq!(token_endpoint(Some("https://auth.x.ai")), DEFAULT_TOKEN_URL);
    }

    #[test]
    fn rotated_tokens_are_written_back_without_dropping_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(
            &path,
            serde_json::json!({
                "https://auth.x.ai::client": {
                    "key": "old",
                    "refresh_token": "old-refresh",
                    "user_id": "user-1"
                }
            })
            .to_string(),
        )
        .unwrap();
        persist_tokens(
            &path,
            "https://auth.x.ai::client",
            "new-access",
            Some("new-refresh"),
            Some("2026-10-13T12:00:00Z"),
        );
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let entry = &saved["https://auth.x.ai::client"];
        assert_eq!(entry["key"], "new-access");
        assert_eq!(entry["refresh_token"], "new-refresh");
        assert_eq!(entry["expires_at"], "2026-10-13T12:00:00Z");
        assert_eq!(entry["user_id"], "user-1");
    }

    #[test]
    fn failed_updates_preserve_freshness_until_recovery() {
        let now = Instant::now();
        let mut status = FetchStatus::default();
        assert_eq!(
            status.complete(&Ok(UsageData::default()), now),
            USAGE_INTERVAL
        );
        assert_eq!(
            status.complete(&Err("HTTP 401".into()), now),
            MIN_RETRY_DELAY
        );
        assert_eq!(status.last_success, Some(now));
        assert_eq!(status.failures, 1);
    }
}
