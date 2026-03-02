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

/// Hover delay before showing the popover (ms)
const HOVER_DELAY_MS: u64 = 300;

/// Usage info for a single rate-limit tier
#[derive(Clone)]
struct TierUsage {
    utilization: f64,
    resets_at: String,
}

/// Extra paid usage info
#[derive(Clone)]
struct ExtraUsage {
    is_enabled: bool,
    monthly_limit: f64,
    used_credits: f64,
    utilization: f64, // 0.0 if null from API
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

/// Claude API usage indicator with hover popover.
pub struct ClaudeUsage {
    data: Arc<Mutex<Option<UsageData>>>,
    popover_visible: bool,
    trigger_bounds: Bounds<Pixels>,
    hover_token: Arc<AtomicU64>,
}

fn read_access_token() -> Option<String> {
    let home = dirs::home_dir()?;
    let content = std::fs::read_to_string(home.join(".claude/.credentials.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    v["claudeAiOauth"]["accessToken"].as_str().map(String::from)
}

fn parse_usage(resp: &serde_json::Value) -> UsageData {
    let five_hour = parse_tier(resp, "five_hour");
    let seven_day = parse_tier(resp, "seven_day");
    let seven_day_sonnet = parse_tier(resp, "seven_day_sonnet");
    let seven_day_opus = parse_tier(resp, "seven_day_opus");

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

fn parse_tier(resp: &serde_json::Value, key: &str) -> Option<TierUsage> {
    let tier = resp.get(key)?;
    Some(TierUsage {
        utilization: tier["utilization"].as_f64().unwrap_or(0.0),
        resets_at: tier["resets_at"]
            .as_str()
            .map(format_reset_time)
            .unwrap_or_default(),
    })
}

/// Format ISO 8601 reset time to a human-readable relative or short form
fn format_reset_time(ts: &str) -> String {
    // Parse enough to show "HH:MM UTC"
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
    format!("{}:{} UTC", hm[0], hm[1])
}

impl ClaudeUsage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let data: Arc<Mutex<Option<UsageData>>> = Arc::new(Mutex::new(None));
        let data_for_task = data.clone();

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                // Skip fetch if claude_code_integration is disabled
                let enabled = this.update(cx, |_, cx| {
                    settings_entity(cx).read(cx).settings.claude_code_integration
                }).unwrap_or(false);

                if !enabled {
                    smol::Timer::after(USAGE_INTERVAL).await;
                    continue;
                }

                let result = smol::unblock(|| {
                    let token = match read_access_token() {
                        Some(t) => {
                            log::info!("[claude-usage] token found (len={})", t.len());
                            t
                        }
                        None => {
                            log::warn!("[claude-usage] no access token found");
                            return None;
                        }
                    };

                    let client = reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(10))
                        .user_agent(format!("okena/{}", env!("CARGO_PKG_VERSION")))
                        .build()
                        .ok()?;

                    let response = client
                        .get("https://api.anthropic.com/api/oauth/usage")
                        .header("Authorization", format!("Bearer {}", token))
                        .header("anthropic-beta", "oauth-2025-04-20")
                        .send();

                    match response {
                        Ok(resp) => {
                            let status = resp.status();
                            let body = resp.text().unwrap_or_default();
                            log::info!("[claude-usage] HTTP {} body={}", status, &body[..body.len().min(500)]);
                            if !status.is_success() {
                                return None;
                            }
                            let parsed: serde_json::Value = serde_json::from_str(&body).ok()?;
                            Some(parse_usage(&parsed))
                        }
                        Err(e) => {
                            log::warn!("[claude-usage] request failed: {}", e);
                            None
                        }
                    }
                })
                .await;

                match &result {
                    Some(_) => log::info!("[claude-usage] data updated successfully"),
                    None => log::info!("[claude-usage] no data returned"),
                }

                if let Some(fetched) = result {
                    *data_for_task.lock() = Some(fetched);
                    let _ = this.update(cx, |_this, cx| {
                        cx.notify();
                    });
                }

                smol::Timer::after(USAGE_INTERVAL).await;
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
                                // Title
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(t.text_primary))
                                        .child("Claude Usage"),
                                )
                                // Session (5h) row
                                .when_some(data.five_hour.as_ref(), |el, tier| {
                                    el.child(render_tier_row(t, "Session (5h)", tier))
                                })
                                // Weekly (7d) row
                                .when_some(data.seven_day.as_ref(), |el, tier| {
                                    el.child(render_tier_row(t, "Weekly (7d)", tier))
                                })
                                // Sonnet row
                                .when_some(data.seven_day_sonnet.as_ref(), |el, tier| {
                                    el.child(render_tier_row(t, "Sonnet (7d)", tier))
                                })
                                // Opus row
                                .when_some(data.seven_day_opus.as_ref(), |el, tier| {
                                    el.child(render_tier_row(t, "Opus (7d)", tier))
                                })
                                // Extra usage row
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
                                                            .text_size(px(11.0))
                                                            .text_color(rgb(t.text_secondary))
                                                            .child("Extra Usage"),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_size(px(11.0))
                                                            .text_color(rgb(t.text_primary))
                                                            .child(format!(
                                                                "${:.2} / ${:.2}",
                                                                extra.used_credits,
                                                                extra.monthly_limit
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

fn utilization_color(t: &crate::theme::ThemeColors, pct: f64) -> u32 {
    if pct > 80.0 {
        t.metric_critical
    } else if pct > 60.0 {
        t.metric_warning
    } else {
        t.metric_normal
    }
}

fn render_tier_row(
    t: &crate::theme::ThemeColors,
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
                        .text_size(px(11.0))
                        .text_color(rgb(t.text_secondary))
                        .child(label.to_string()),
                )
                .child(
                    h_flex()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(utilization_color(t, pct)))
                                .child(format!("{:.0}%", pct)),
                        )
                        .when(!tier.resets_at.is_empty(), |el| {
                            el.child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(rgb(t.text_muted))
                                    .child(format!("resets {}", tier.resets_at)),
                            )
                        }),
                ),
        )
        .child(render_progress_bar(t, pct))
}

fn render_progress_bar(t: &crate::theme::ThemeColors, pct: f64) -> impl IntoElement {
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
            None => return div().size_0().into_any_element(),
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
                                        .text_size(px(11.0))
                                        .text_color(rgb(t.text_muted))
                                        .child("5h"),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(utilization_color(&t, pct)))
                                        .child(format!("{:.0}%", pct)),
                                ),
                        )
                    })
                    .when_some(seven_d, |el, pct| {
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
                                        .child("7d"),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
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
