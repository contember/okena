//! Codex usage widget: status-bar trigger + hover popover with chart.
//!
//! The numbers come from the daemon (`GET /v1/usage` / `usage_changed`), via
//! the app-installed [`GlobalDaemonUsage`] entity. This widget never fetches;
//! opening the popover asks the daemon to refresh (throttled there to 60 s).

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::{
    ApiProviderUsage, ApiResetCredit, ApiResetCredits, ApiUsageProvider, ApiUsageStatus,
};
use okena_extensions::ThemeColors;
use okena_ui::expand::expand_toggle;
use okena_ui::tokens::{ui_text_ms, ui_text_xs};
use okena_usage::{
    DaemonUsage, GlobalDaemonUsage, format_reset_time_epoch, format_window_label, now_secs,
    provider_usage, read_working_days, render_usage_row, usage_body_container, usage_divider,
    usage_kv_row, usage_popover_container, usage_popover_header, usage_trigger_items,
    window_points, window_row, window_trigger_item,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Regular daemon refresh cadence; older data is labelled stale.
const USAGE_INTERVAL_SECS: f64 = 300.0;

/// Hover delay before showing the popover (ms)
const HOVER_DELAY_MS: u64 = 300;

fn theme(cx: &App) -> ThemeColors {
    okena_extensions::theme(cx)
}

/// Codex usage indicator with hover popover.
///
/// One of these exists per window; they all read the shared
/// [`GlobalDaemonUsage`] snapshot and hold only per-window UI state.
pub struct CodexUsage {
    store: Entity<DaemonUsage>,
    chart_state: Entity<okena_usage::HistoryChartState>,
    popover_visible: bool,
    resets_expanded: bool,
    trigger_bounds: Bounds<Pixels>,
    hover_token: Arc<AtomicU64>,
    freshness_clock: Option<Task<()>>,
}

impl CodexUsage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let store = GlobalDaemonUsage::entity(cx);
        let chart_state = cx.new(|_| okena_usage::HistoryChartState::default());
        cx.observe(&chart_state, |_, _, cx| cx.notify()).detach();
        // Re-render whenever the daemon pushes a new snapshot.
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        Self {
            store,
            chart_state,
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
                    GlobalDaemonUsage::request_refresh(ApiUsageProvider::Codex, cx);
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
        let data = provider_usage(ApiUsageProvider::Codex, cx).unwrap_or_else(|| {
            ApiProviderUsage::empty(ApiUsageProvider::Codex, ApiUsageStatus::Pending)
        });
        let status_rows = render_fetch_status(&data, t, cx);

        let working = read_working_days(cx);
        let source = self.store.read(cx).source_label().cloned();
        let plan = header_badge(data.plan.as_deref(), source.as_deref());
        let bounds = self.trigger_bounds;
        let position = point(bounds.origin.x, bounds.origin.y - px(4.0));

        let rows: Vec<AnyElement> = data
            .windows
            .iter()
            .filter_map(|window| {
                let (label, marker_id) = match window.id.as_str() {
                    "primary" => (window_title(window), "codex-marker-primary"),
                    "secondary" => (window_title(window), "codex-marker-secondary"),
                    "code_review" => ("Code review".to_string(), "codex-marker-review"),
                    _ => return None,
                };
                Some(
                    render_usage_row(
                        t,
                        cx,
                        &window_row(window, label, marker_id),
                        working,
                        window_points(window),
                        &self.chart_state,
                    )
                    .into_any_element(),
                )
            })
            .collect();

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
                            plan.as_deref(),
                            "https://chatgpt.com",
                            "Open usage settings on chatgpt.com",
                            t,
                            cx,
                        ))
                        .child(
                            usage_body_container()
                                .children(rows)
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
        resets: &ApiResetCredits,
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
}

/// Plan badge, plus the daemon's machine when it is not this one.
fn header_badge(plan: Option<&str>, source: Option<&str>) -> Option<String> {
    let plan = plan.filter(|plan| !plan.is_empty());
    match (plan, source) {
        (Some(plan), Some(source)) => Some(format!("{plan} · {source}")),
        (Some(plan), None) => Some(plan.to_string()),
        (None, Some(source)) => Some(source.to_string()),
        (None, None) => None,
    }
}

fn render_fetch_status(data: &ApiProviderUsage, t: &ThemeColors, cx: &App) -> AnyElement {
    let now = now_secs().unwrap_or(0.0);
    let since = |at: f64| Duration::from_secs_f64((now - at).max(0.0));
    let age = match data.fetched_at {
        Some(at) => format!(
            "Updated {} ago{}",
            format_age(since(at)),
            if data.error.is_some() || now - at > USAGE_INTERVAL_SECS {
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
        .when_some(data.error.as_ref(), |el, error| {
            el.child(format!("Update failed: {error}"))
                .when(data.consecutive_failures > 0, |el| {
                    el.child(format!("Failed attempts: {}", data.consecutive_failures))
                })
        })
        .when(data.fetching, |el| el.child("Updating…"))
        .when_some(
            data.next_attempt_at.filter(|_| !data.fetching),
            |el, next| {
                let label = if data.error.is_some() {
                    "Retry"
                } else {
                    "Next update"
                };
                el.child(format!(
                    "{label} in {}",
                    format_age(Duration::from_secs_f64((next - now).max(0.0)))
                ))
            },
        )
        .into_any_element()
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

/// Row title for a rate-limit window, named by its length like the mobile
/// app ("5-hour limit", "Weekly limit"); the API's primary/secondary order
/// says nothing about which window is which.
fn window_title(window: &okena_core::api::ApiUsageWindow) -> String {
    match format_window_label(window.window_seconds) {
        "5h" => "5-hour limit".to_string(),
        "7d" => "Weekly limit".to_string(),
        "1d" => "Daily limit".to_string(),
        period => format!("{period} limit"),
    }
}

fn reset_count_label(count: u64) -> String {
    if count == 1 {
        "1 reset available".to_string()
    } else {
        format!("{count} resets available")
    }
}

fn reset_expiry_label(credit: &ApiResetCredit) -> String {
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

fn first_reset_expiry_label(resets: &ApiResetCredits) -> Option<String> {
    let expires_at = resets.credits.iter().find_map(|credit| credit.expires_at)?;
    let formatted = format_reset_time_epoch(expires_at, true);
    (!formatted.is_empty()).then(|| format!("first expires {formatted}"))
}

impl Render for CodexUsage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let working = read_working_days(cx);
        let items: Vec<okena_usage::TriggerItem> = provider_usage(ApiUsageProvider::Codex, cx)
            .map(|data| {
                data.windows
                    .iter()
                    .filter(|window| window.id == "primary" || window.id == "secondary")
                    .map(|window| {
                        window_trigger_item(
                            window,
                            format_window_label(window.window_seconds),
                            working,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

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
    fn reset_summary_uses_singular_plural_and_first_expiry() {
        assert_eq!(reset_count_label(1), "1 reset available");
        let window = |window_seconds| okena_core::api::ApiUsageWindow {
            id: "primary".into(),
            label: "Rate Limit".into(),
            kind: okena_core::api::ApiUsageWindowKind::Other,
            used_percent: 0.0,
            window_seconds,
            reset_at: None,
            active: true,
            history: Vec::new(),
        };
        assert_eq!(window_title(&window(18000.0)), "5-hour limit");
        assert_eq!(window_title(&window(604800.0)), "Weekly limit");
        assert_eq!(window_title(&window(86400.0)), "Daily limit");
        assert_eq!(window_title(&window(2592000.0)), "30d limit");
        assert_eq!(reset_count_label(3), "3 resets available");

        let resets = ApiResetCredits {
            available_count: 1,
            credits: vec![ApiResetCredit {
                title: "Full reset".to_string(),
                expires_at: Some(1_784_334_733.0),
            }],
        };
        let summary = first_reset_expiry_label(&resets).expect("expiry summary should render");
        assert!(summary.starts_with("first expires "));

        let no_expiry = ApiResetCredit {
            title: "Full reset".to_string(),
            expires_at: None,
        };
        assert_eq!(reset_expiry_label(&no_expiry), "no expiration");
    }

    #[test]
    fn format_age_units() {
        assert_eq!(format_age(Duration::from_secs(5)), "5s");
        assert_eq!(format_age(Duration::from_secs(65)), "1m 5s");
        assert_eq!(format_age(Duration::from_secs(3720)), "1h 2m");
    }
}
