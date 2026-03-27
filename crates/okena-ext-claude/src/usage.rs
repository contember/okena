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

/// Parse a simplified ISO 8601 timestamp to Unix epoch seconds.
pub(crate) fn parse_iso8601_to_epoch(ts: &str) -> Option<f64> {
    let timestamp: jiff::Timestamp = ts.parse().ok()?;
    Some(timestamp.as_millisecond() as f64 / 1_000.0)
}

/// Parse an ISO 8601 timestamp to a local Zoned datetime.
/// Returns `None` if parsing or timezone conversion fails.
pub(crate) fn parse_iso8601_to_local(ts: &str) -> Option<jiff::Zoned> {
    let timestamp: jiff::Timestamp = ts.parse().ok()?;
    Some(timestamp.to_zoned(jiff::tz::TimeZone::system()))
}

/// Format ISO 8601 reset time to a human-readable short form in local timezone.
/// Falls back to UTC display if local timezone conversion fails.
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

    // Fallback: return as-is if we can't parse at all
    ts.to_string()
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
}
