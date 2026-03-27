use okena_extensions::ThemeColors;
use okena_ui::tokens::{ui_text_xs, ui_text_sm, ui_text_ms, ui_text_md};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Refresh interval for usage data
const USAGE_INTERVAL: Duration = Duration::from_secs(300);

/// Minimum retry delay to avoid tight loops (e.g. when server returns retry-after: 0)
const MIN_RETRY_DELAY: Duration = Duration::from_secs(30);

/// Hover delay before showing the popover (ms)
const HOVER_DELAY_MS: u64 = 300;

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

/// Claude API usage indicator with hover popover.
pub struct ClaudeUsage {
    data: Arc<Mutex<Option<UsageData>>>,
    popover_visible: bool,
    trigger_bounds: Bounds<Pixels>,
    hover_token: Arc<AtomicU64>,
    /// Send on this channel to wake up the fetch loop and retry immediately.
    wake_tx: smol::channel::Sender<()>,
    /// Whether a wake signal has already been sent (avoids spamming from render).
    wake_sent: Arc<AtomicBool>,
    /// Background polling task. Cancelled automatically when this entity is dropped.
    _poll_task: Task<()>,
}

fn read_access_token() -> Option<String> {
    let home = dirs::home_dir()?;
    let content = std::fs::read_to_string(home.join(".claude/.credentials.json"))
        .ok()
        .or_else(|| {
            // macOS: credentials stored in Keychain
            #[cfg(target_os = "macos")]
            {
                let user = std::env::var("USER").ok()?;
                let output = std::process::Command::new("security")
                    .args(["find-generic-password", "-s", "Claude Code-credentials", "-a", &user, "-w"])
                    .output()
                    .ok()?;
                if output.status.success() {
                    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
                } else {
                    None
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                None
            }
        })?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    v["claudeAiOauth"]["accessToken"].as_str().map(String::from)
}

fn parse_usage(resp: &serde_json::Value) -> UsageData {
    let five_hour = parse_tier(resp, "five_hour", false, FIVE_HOUR_SECS);
    let seven_day = parse_tier(resp, "seven_day", true, SEVEN_DAY_SECS);
    let seven_day_sonnet = parse_tier(resp, "seven_day_sonnet", true, SEVEN_DAY_SECS);
    let seven_day_opus = parse_tier(resp, "seven_day_opus", true, SEVEN_DAY_SECS);

    let extra_usage = resp.get("extra_usage").and_then(|eu| {
        Some(ExtraUsage {
            is_enabled: eu["is_enabled"].as_bool().unwrap_or(false),
            monthly_limit: eu["monthly_limit"].as_f64().unwrap_or(0.0),
            used_credits: eu["used_credits"].as_f64().unwrap_or(0.0),
            utilization: eu["utilization"].as_f64().unwrap_or(0.0),
        })
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

/// Parse a simplified ISO 8601 timestamp to Unix epoch seconds.
pub(crate) fn parse_iso8601_to_epoch(ts: &str) -> Option<f64> {
    let parts: Vec<&str> = ts.split('T').collect();
    if parts.len() != 2 {
        return None;
    }
    let date_parts: Vec<&str> = parts[0].split('-').collect();
    if date_parts.len() != 3 {
        return None;
    }
    let year: i32 = date_parts[0].parse().ok()?;
    let month: u32 = date_parts[1].parse().ok()?;
    let day: u32 = date_parts[2].parse().ok()?;

    let time_str = parts[1].split('.').next().unwrap_or(parts[1]);
    let time_str = time_str.trim_end_matches('Z');
    let hms: Vec<&str> = time_str.split(':').collect();
    if hms.len() < 2 {
        return None;
    }
    let hour: u32 = hms[0].parse().ok()?;
    let min: u32 = hms[1].parse().ok()?;
    let sec: u32 = hms.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);

    let days = days_from_civil(year, month, day);
    Some(days as f64 * 86400.0 + hour as f64 * 3600.0 + min as f64 * 60.0 + sec as f64)
}

/// Convert a civil date to days since Unix epoch (Howard Hinnant's algorithm).
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year as i64 - 1 } else { year as i64 };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = (y - era * 400) as u64;
    let m = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * m as u64 + 2) / 5 + day as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe as i64 - 719468
}

/// Components of a broken-down time with timezone info.
pub(crate) struct LocalTime {
    pub(crate) year: i32,
    pub(crate) month: u32,  // 1-12
    pub(crate) day: u32,    // 1-31
    pub(crate) hour: u32,
    pub(crate) min: u32,
    pub(crate) tz_abbr: String,
}

/// Convert a UTC epoch timestamp to local time components.
/// Returns `None` if the conversion fails, in which case callers fall back to UTC.
pub(crate) fn epoch_to_local_time(epoch_secs: f64) -> Option<LocalTime> {
    #[cfg(unix)]
    {
        let t = epoch_secs as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let result = unsafe { libc::localtime_r(&t, &mut tm) };
        if result.is_null() {
            return None;
        }
        let tz_abbr = {
            // tm_zone is a pointer to a static string on most Unix systems
            let ptr = tm.tm_zone;
            if ptr.is_null() {
                String::new()
            } else {
                unsafe { std::ffi::CStr::from_ptr(ptr) }
                    .to_string_lossy()
                    .into_owned()
            }
        };
        Some(LocalTime {
            year: tm.tm_year + 1900,
            month: (tm.tm_mon + 1) as u32,
            day: tm.tm_mday as u32,
            hour: tm.tm_hour as u32,
            min: tm.tm_min as u32,
            tz_abbr,
        })
    }
    #[cfg(windows)]
    {
        // On Windows, use the C runtime's localtime_s
        let t = epoch_secs as i64;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let err = unsafe { libc::localtime_s(&mut tm, &t) };
        if err != 0 {
            return None;
        }
        // Windows libc::tm does not have tm_zone; use the TIME_ZONE_INFORMATION API
        let tz_abbr = windows_tz_abbr().unwrap_or_default();
        Some(LocalTime {
            year: tm.tm_year + 1900,
            month: (tm.tm_mon + 1) as u32,
            day: tm.tm_mday as u32,
            hour: tm.tm_hour as u32,
            min: tm.tm_min as u32,
            tz_abbr,
        })
    }
}

#[cfg(windows)]
fn windows_tz_abbr() -> Option<String> {
    use std::mem::MaybeUninit;
    // SAFETY: GetTimeZoneInformation is a safe Windows API call
    unsafe {
        let mut tzi = MaybeUninit::<windows_sys::Win32::System::Time::TIME_ZONE_INFORMATION>::zeroed();
        let result = windows_sys::Win32::System::Time::GetTimeZoneInformation(tzi.as_mut_ptr());
        if result == 0xFFFFFFFF {
            return None;
        }
        let tzi = tzi.assume_init();
        // Use DaylightName if in daylight time (result == 2), else StandardName
        let name = if result == 2 {
            &tzi.DaylightName
        } else {
            &tzi.StandardName
        };
        let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        Some(String::from_utf16_lossy(&name[..len]))
    }
}

/// Get today's local date as (year, month, day).
fn local_today() -> Option<(i32, u32, u32)> {
    let now_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs_f64();
    let lt = epoch_to_local_time(now_epoch)?;
    Some((lt.year, lt.month, lt.day))
}

/// Format ISO 8601 reset time to a human-readable short form in local timezone.
/// Falls back to UTC display if local timezone conversion fails.
fn format_reset_time(ts: &str, include_date: bool) -> String {
    // Try to convert to local time first
    if let Some(epoch) = parse_iso8601_to_epoch(ts) {
        if let Some(local) = epoch_to_local_time(epoch) {
            let tz_label = if local.tz_abbr.is_empty() { "UTC".to_string() } else { local.tz_abbr };

            if include_date {
                if let Some((today_y, today_m, today_d)) = local_today() {
                    let reset_days = days_from_civil(local.year, local.month, local.day);
                    let today_days = days_from_civil(today_y, today_m, today_d);
                    let diff = reset_days - today_days;

                    let date_label = if diff == 0 {
                        "today".to_string()
                    } else if diff == 1 {
                        "tomorrow".to_string()
                    } else if (2..=6).contains(&diff) {
                        let dow = ((reset_days % 7) + 7) % 7;
                        ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][dow as usize].to_string()
                    } else {
                        let month_name = match local.month {
                            1 => "Jan", 2 => "Feb", 3 => "Mar", 4 => "Apr",
                            5 => "May", 6 => "Jun", 7 => "Jul", 8 => "Aug",
                            9 => "Sep", 10 => "Oct", 11 => "Nov", 12 => "Dec",
                            _ => "?",
                        };
                        format!("{} {}", month_name, local.day)
                    };

                    return format!("{}, {:02}:{:02} {}", date_label, local.hour, local.min, tz_label);
                }
            }

            return format!("{:02}:{:02} {}", local.hour, local.min, tz_label);
        }
    }

    // Fallback: parse and display as UTC
    format_reset_time_utc(ts, include_date)
}

/// UTC fallback for format_reset_time when local timezone conversion fails.
fn format_reset_time_utc(ts: &str, include_date: bool) -> String {
    let parts: Vec<&str> = ts.split('T').collect();
    if parts.len() != 2 {
        return ts.to_string();
    }
    let time = parts[1].split('.').next().unwrap_or(parts[1]);
    let time = time.trim_end_matches('Z');
    let hm: Vec<&str> = time.split(':').collect();
    if hm.len() < 2 {
        return ts.to_string();
    }

    if include_date {
        let date_parts: Vec<&str> = parts[0].split('-').collect();
        if date_parts.len() == 3 {
            let year: i32 = date_parts[0].parse().unwrap_or(0);
            let month: u32 = date_parts[1].parse().unwrap_or(0);
            let day: u32 = date_parts[2].parse().unwrap_or(0);

            if year > 0 && (1..=12).contains(&month) && (1..=31).contains(&day) {
                let reset_days = days_from_civil(year, month, day);
                let today_days = (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    / 86400) as i64;
                let diff = reset_days - today_days;

                let date_label = if diff == 0 {
                    "today".to_string()
                } else if diff == 1 {
                    "tomorrow".to_string()
                } else if (2..=6).contains(&diff) {
                    let dow = ((reset_days % 7) + 7) % 7;
                    ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][dow as usize].to_string()
                } else {
                    let month_name = match month {
                        1 => "Jan", 2 => "Feb", 3 => "Mar", 4 => "Apr",
                        5 => "May", 6 => "Jun", 7 => "Jul", 8 => "Aug",
                        9 => "Sep", 10 => "Oct", 11 => "Nov", 12 => "Dec",
                        _ => "?",
                    };
                    format!("{} {}", month_name, day)
                };

                return format!("{}, {}:{} UTC", date_label, hm[0], hm[1]);
            }
        }
    }

    format!("{}:{} UTC", hm[0], hm[1])
}

impl ClaudeUsage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let data: Arc<Mutex<Option<UsageData>>> = Arc::new(Mutex::new(None));
        let data_for_task = data.clone();
        let (wake_tx, wake_rx) = smol::channel::bounded::<()>(1);
        let wake_sent = Arc::new(AtomicBool::new(false));
        let wake_sent_for_task = wake_sent.clone();

        let poll_task = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let mut consecutive_failures: u32 = 0;
            loop {
                // Returns (Option<UsageData>, Option<Duration>) — data + optional retry delay
                let (result, retry_after) = smol::unblock(|| {
                    let token = match read_access_token() {
                        Some(t) => {
                            log::info!("[claude-usage] token found (len={})", t.len());
                            t
                        }
                        None => {
                            log::warn!("[claude-usage] no access token found");
                            return (None, None);
                        }
                    };

                    let client = match reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(10))
                        .user_agent(format!("okena/{}", env!("CARGO_PKG_VERSION")))
                        .build()
                    {
                        Ok(c) => c,
                        Err(_) => return (None, None),
                    };

                    let response = client
                        .get("https://api.anthropic.com/api/oauth/usage")
                        .header("Authorization", format!("Bearer {}", token))
                        .header("anthropic-beta", "oauth-2025-04-20")
                        .send();

                    match response {
                        Ok(resp) => {
                            let status = resp.status();

                            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                                let retry_secs = resp
                                    .headers()
                                    .get("retry-after")
                                    .and_then(|v| v.to_str().ok())
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

                            let body = resp.text().unwrap_or_default();
                            log::info!(
                                "[claude-usage] HTTP {} body={}",
                                status,
                                &body[..body.len().min(500)]
                            );
                            if !status.is_success() {
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
            popover_visible: false,
            trigger_bounds: Bounds::default(),
            hover_token: Arc::new(AtomicU64::new(0)),
            wake_tx,
            wake_sent,
            _poll_task: poll_task,
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
        let data = self.data.lock();
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
                                    div()
                                        .text_size(ui_text_md(cx))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(t.text_primary))
                                        .child("Claude Usage"),
                                )
                                .when_some(data.five_hour.as_ref(), |el, tier| {
                                    el.child(render_tier_row(t, cx, "Session (5h)", tier))
                                })
                                .when_some(data.seven_day.as_ref(), |el, tier| {
                                    el.child(render_tier_row(t, cx, "Weekly (7d)", tier))
                                })
                                .when_some(data.seven_day_sonnet.as_ref(), |el, tier| {
                                    el.child(render_tier_row(t, cx, "Sonnet (7d)", tier))
                                })
                                .when_some(data.seven_day_opus.as_ref(), |el, tier| {
                                    el.child(render_tier_row(t, cx, "Opus (7d)", tier))
                                })
                                .when(
                                    data.five_hour.as_ref().and_then(|t| t.time_elapsed_pct).is_some()
                                        || data.seven_day.as_ref().and_then(|t| t.time_elapsed_pct).is_some(),
                                    |el| {
                                        el.child(
                                            div()
                                                .text_size(ui_text_xs(cx))
                                                .text_color(rgb(t.text_muted))
                                                .child("Bar color = pace · Marker = time elapsed"),
                                        )
                                    },
                                )
                                .when_some(data.extra_usage.as_ref(), |el, extra| {
                                    if !extra.is_enabled {
                                        return el;
                                    }
                                    el.child(
                                        v_flex()
                                            .gap(px(2.0))
                                            .child(
                                                h_flex()
                                                    .justify_between()
                                                    .child(
                                                        div()
                                                            .text_size(ui_text_ms(cx))
                                                            .text_color(rgb(t.text_secondary))
                                                            .child("Extra Usage"),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(ui_text_ms(cx))
                                                            .text_color(rgb(t.text_primary))
                                                            .child(format!(
                                                                "${:.2} / ${:.2}",
                                                                extra.used_credits / 100.0,
                                                                extra.monthly_limit / 100.0
                                                            )),
                                                    ),
                                            )
                                            .child(render_progress_bar(
                                                t,
                                                extra.utilization,
                                            )),
                                    )
                                }),
                        ),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }
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
    tier: &TierUsage,
) -> impl IntoElement {
    let pct = tier.utilization;

    v_flex()
        .gap(px(2.0))
        .child(
            h_flex()
                .justify_between()
                .child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_secondary))
                        .child(label.to_string()),
                )
                .child(
                    h_flex()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(utilization_color(t, pct)))
                                .child(format!("{:.0}%", pct)),
                        )
                        .when(!tier.resets_at.is_empty(), |el| {
                            el.child(
                                div()
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(format!("resets {}", tier.resets_at)),
                            )
                        }),
                ),
        )
        .child(render_usage_with_time_bar(t, pct, tier.time_elapsed_pct))
}

fn render_usage_with_time_bar(
    t: &ThemeColors,
    usage_pct: f64,
    time_pct: Option<f64>,
) -> impl IntoElement {
    let clamped_usage = usage_pct.clamp(0.0, 100.0) as f32;

    let pace_color = match time_pct {
        Some(tp) if usage_pct > tp + 15.0 => t.metric_critical,
        Some(tp) if usage_pct > tp + 5.0 => t.metric_warning,
        _ => t.metric_normal,
    };

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
                .bg(rgb(pace_color))
                .w(relative(clamped_usage / 100.0)),
        )
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

fn render_progress_bar(t: &ThemeColors, pct: f64) -> impl IntoElement {
    let clamped = pct.clamp(0.0, 100.0) as f32;
    let color = utilization_color(t, pct);

    div()
        .h(px(4.0))
        .w_full()
        .rounded(px(2.0))
        .bg(rgb(t.bg_secondary))
        .child(
            div()
                .h_full()
                .rounded(px(2.0))
                .bg(rgb(color))
                .w(relative(clamped / 100.0)),
        )
}

impl Render for ClaudeUsage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let data = self.data.lock();
        let (five_h, seven_d) = match data.as_ref() {
            Some(d) => {
                let fh = d.five_hour.as_ref().map(|t| t.utilization);
                let sd = d.seven_day.as_ref().map(|t| t.utilization);
                (fh, sd)
            }
            None => {
                // Wake the fetch loop once (e.g. after toggle on/off or if the
                // first fetch failed). Only send one signal to avoid retry storms.
                if !self.wake_sent.swap(true, Ordering::SeqCst) {
                    let _ = self.wake_tx.try_send(());
                }
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
    fn test_parse_iso8601_to_epoch() {
        // 2025-01-01T00:00:00Z = 1735689600
        let epoch = parse_iso8601_to_epoch("2025-01-01T00:00:00.000Z").unwrap();
        assert!((epoch - 1735689600.0).abs() < 1.0);
    }

    #[test]
    fn test_parse_iso8601_to_epoch_invalid() {
        assert!(parse_iso8601_to_epoch("not-a-date").is_none());
        assert!(parse_iso8601_to_epoch("2025-01-01").is_none());
    }

    #[test]
    fn test_epoch_to_local_time_roundtrip() {
        // Verify that epoch_to_local_time returns a valid result for a known epoch
        let lt = epoch_to_local_time(1735689600.0).unwrap();
        // The local time depends on the system timezone, but year should be 2024 or 2025
        assert!(lt.year == 2024 || lt.year == 2025);
        assert!((1..=12).contains(&lt.month));
        assert!((1..=31).contains(&lt.day));
        assert!(lt.hour < 24);
        assert!(lt.min < 60);
    }

    #[test]
    fn test_format_reset_time_uses_local_tz() {
        // format_reset_time should NOT end with "UTC" when local tz conversion works
        // (unless the system is literally in UTC, in which case it shows "UTC")
        let result = format_reset_time("2025-06-15T14:00:00.000Z", false);
        // Should contain a colon (HH:MM) and a timezone abbreviation
        assert!(result.contains(':'), "Expected HH:MM format, got: {}", result);
        // Should NOT be empty
        assert!(!result.is_empty());
    }

    #[test]
    fn test_format_reset_time_with_date() {
        let result = format_reset_time("2099-01-15T11:00:00.000Z", true);
        // Far future date should show "Jan 15" (or local-adjusted date)
        assert!(result.contains(':'), "Expected time in result, got: {}", result);
        assert!(result.contains(','), "Expected date label with comma, got: {}", result);
    }

    #[test]
    fn test_format_reset_time_utc_fallback() {
        // Test the UTC fallback directly
        let result = format_reset_time_utc("2025-06-15T14:00:00.000Z", false);
        assert_eq!(result, "14:00 UTC");

        let result = format_reset_time_utc("2099-01-15T11:00:00.000Z", true);
        assert!(result.ends_with("UTC"), "Expected UTC suffix, got: {}", result);
        assert!(result.contains("11:00"));
    }

    #[test]
    fn test_format_reset_time_invalid_input() {
        // Invalid input should be returned as-is
        let result = format_reset_time("garbage", false);
        assert_eq!(result, "garbage");
    }

    #[test]
    fn test_local_today_returns_valid_date() {
        let (y, m, d) = local_today().unwrap();
        assert!(y >= 2025);
        assert!((1..=12).contains(&m));
        assert!((1..=31).contains(&d));
    }

    #[test]
    fn test_tz_abbreviation_not_empty() {
        // On most systems, the timezone abbreviation should be non-empty
        let now_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let lt = epoch_to_local_time(now_epoch).unwrap();
        assert!(!lt.tz_abbr.is_empty(), "Expected non-empty tz abbreviation");
    }
}
