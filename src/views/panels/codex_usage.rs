use crate::settings::settings_entity;
use crate::theme::theme;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use parking_lot::Mutex;
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

/// A rate limit window from the usage API
#[derive(Clone)]
struct RateLimitWindow {
    used_percent: u64,
    window_seconds: u64,
    reset_at: u64,
    /// Percentage of the window that has elapsed (0.0–100.0)
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

/// Codex usage indicator with hover popover.
pub struct CodexUsage {
    data: Arc<Mutex<Option<UsageData>>>,
    popover_visible: bool,
    trigger_bounds: Bounds<Pixels>,
    hover_token: Arc<AtomicU64>,
}

/// Read Codex OAuth credentials from ~/.codex/auth.json
fn read_codex_auth() -> Option<(String, String)> {
    let home = dirs::home_dir()?;
    let content = std::fs::read_to_string(home.join(".codex/auth.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    let refresh_token = v["tokens"]["refresh_token"].as_str()?.to_string();
    let account_id = v["tokens"]["account_id"].as_str()?.to_string();
    Some((refresh_token, account_id))
}

/// Refresh the OAuth access token using the refresh token
fn refresh_access_token(refresh_token: &str) -> Option<String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;

    let resp: serde_json::Value = client
        .post("https://auth.openai.com/oauth/token")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=refresh_token&client_id={}&refresh_token={}",
            CODEX_CLIENT_ID, refresh_token
        ))
        .send()
        .ok()?
        .json()
        .ok()?;

    resp["access_token"].as_str().map(String::from)
}

fn parse_window(v: &serde_json::Value) -> Option<RateLimitWindow> {
    let used = v["used_percent"].as_u64()?;
    let window_seconds = v["limit_window_seconds"].as_u64().unwrap_or(0);
    let reset_at = v["reset_at"].as_u64().unwrap_or(0);

    // Compute what percentage of the window has elapsed
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

fn fetch_usage() -> Option<UsageData> {
    let (refresh_token, account_id) = read_codex_auth()?;
    let access_token = refresh_access_token(&refresh_token)?;

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(format!("okena/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;

    let resp = client
        .get("https://chatgpt.com/backend-api/codex/usage")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("chatgpt-account-id", &account_id)
        .send()
        .ok()?;

    if !resp.status().is_success() {
        log::warn!("[codex-usage] API returned {}", resp.status());
        return None;
    }

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

impl CodexUsage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let data: Arc<Mutex<Option<UsageData>>> = Arc::new(Mutex::new(None));
        let data_for_task = data.clone();

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let mut consecutive_failures: u32 = 0;
            loop {
                // Skip fetch if codex_integration is disabled
                let enabled = this
                    .update(cx, |_, cx| {
                        settings_entity(cx).read(cx).settings.codex_integration
                    })
                    .unwrap_or(false);

                if !enabled {
                    smol::Timer::after(USAGE_INTERVAL).await;
                    continue;
                }

                let result = smol::unblock(fetch_usage).await;

                if let Some(fetched) = result {
                    *data_for_task.lock() = Some(fetched);
                    consecutive_failures = 0;
                    let _ = this.update(cx, |_this, cx| {
                        cx.notify();
                    });
                } else {
                    consecutive_failures = consecutive_failures.saturating_add(1);
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
        })
        .detach();

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
        t: &crate::theme::ThemeColors,
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
                                // Title
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .child(
                                            div()
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(t.text_primary))
                                                .child("Codex Usage"),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(10.0))
                                                .text_color(rgb(t.text_muted))
                                                .child(data.plan_type.clone()),
                                        ),
                                )
                                // Primary rate limit
                                .when_some(data.primary_window.as_ref(), |el, w| {
                                    el.child(render_window_row(t, "Rate Limit", w))
                                })
                                // Secondary rate limit
                                .when_some(data.secondary_window.as_ref(), |el, w| {
                                    el.child(render_window_row(t, "Secondary", w))
                                })
                                // Code review rate limit
                                .when_some(data.review_primary.as_ref(), |el, w| {
                                    el.child(render_window_row(t, "Code Review", w))
                                })
                                // Time pace hint
                                .when(
                                    data.primary_window.as_ref().and_then(|w| w.time_elapsed_pct).is_some()
                                        || data.secondary_window.as_ref().and_then(|w| w.time_elapsed_pct).is_some(),
                                    |el| {
                                        el.child(
                                            div()
                                                .text_size(px(9.0))
                                                .text_color(rgb(t.text_muted))
                                                .child("Bar color = pace · Marker = time elapsed"),
                                        )
                                    },
                                )
                                // Credits
                                .when_some(data.credits.as_ref(), |el, c| {
                                    if c.unlimited {
                                        el.child(
                                            h_flex()
                                                .justify_between()
                                                .child(
                                                    div()
                                                        .text_size(px(11.0))
                                                        .text_color(rgb(t.text_secondary))
                                                        .child("Credits"),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(11.0))
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
                                                        .text_size(px(11.0))
                                                        .text_color(rgb(t.text_secondary))
                                                        .child("Credits"),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(11.0))
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

fn utilization_color(t: &crate::theme::ThemeColors, pct: u64) -> u32 {
    if pct > 80 {
        t.metric_critical
    } else if pct > 60 {
        t.metric_warning
    } else {
        t.metric_normal
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
    t: &crate::theme::ThemeColors,
    label: &str,
    window: &RateLimitWindow,
) -> impl IntoElement {
    let pct = window.used_percent;
    let window_label = format_window_label(window.window_seconds);
    let reset = format_reset_time(window.reset_at);

    v_flex()
        .gap(px(2.0))
        .child(
            h_flex()
                .justify_between()
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(t.text_secondary))
                        .child(format!("{} ({})", label, window_label)),
                )
                .child(
                    h_flex()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(utilization_color(t, pct)))
                                .child(format!("{}%", pct)),
                        )
                        .when(!reset.is_empty(), |el| {
                            el.child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(rgb(t.text_muted))
                                    .child(format!("resets in {}", reset)),
                            )
                        }),
                ),
        )
        .child(render_usage_with_time_bar(t, pct, window.time_elapsed_pct))
}

/// Render a combined progress bar: usage fill on top of a time-elapsed marker.
/// The time marker is a thin vertical line showing where you "should" be.
fn render_usage_with_time_bar(
    t: &crate::theme::ThemeColors,
    usage_pct: u64,
    time_pct: Option<f64>,
) -> impl IntoElement {
    let clamped_usage = (usage_pct as f32).clamp(0.0, 100.0);

    let pace_color = match time_pct {
        Some(tp) if (usage_pct as f64) > tp + 15.0 => t.metric_critical,
        Some(tp) if (usage_pct as f64) > tp + 5.0 => t.metric_warning,
        _ => t.metric_normal,
    };

    div()
        .h(px(4.0))
        .w_full()
        .rounded(px(2.0))
        .bg(rgb(t.bg_secondary))
        .relative()
        // Usage fill
        .child(
            div()
                .h_full()
                .rounded(px(2.0))
                .bg(rgb(pace_color))
                .w(relative(clamped_usage / 100.0)),
        )
        // Time elapsed marker (thin vertical line)
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

        let data = self.data.lock();
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
                                        .text_size(px(11.0))
                                        .text_color(rgb(t.text_muted))
                                        .child(format_window_label(window_secs)),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(utilization_color(&t, pct)))
                                        .child(format!("{}%", pct)),
                                ),
                        )
                    })
                    .when_some(secondary, |el, (pct, window_secs)| {
                        el.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(t.text_muted))
                                .child("|"),
                        )
                        .child(
                            h_flex()
                                .gap(px(3.0))
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(t.text_muted))
                                        .child(format_window_label(window_secs)),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
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
