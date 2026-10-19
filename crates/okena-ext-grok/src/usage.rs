//! Grok usage widget: status-bar trigger + hover popover with chart.
//!
//! The numbers come from the daemon (`GET /v1/usage` / `usage_changed`), via
//! the app-installed [`GlobalDaemonUsage`] entity. This widget never fetches;
//! opening the popover asks the daemon to refresh (throttled there to 60 s).

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::{ApiProviderUsage, ApiUsageProvider, ApiUsageStatus, ApiUsageWindow};
use okena_extensions::ThemeColors;
use okena_ui::tokens::ui_text_xs;
use okena_usage::{
    DaemonUsage, GlobalDaemonUsage, UsageRow, now_secs, provider_usage, read_working_days,
    render_usage_row, usage_body_container, usage_divider, usage_kv_row, usage_popover_container,
    usage_popover_header, usage_trigger_items, window_points, window_row, window_trigger_item,
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

/// Grok usage indicator with hover popover.
///
/// One of these exists per window; they all read the shared
/// [`GlobalDaemonUsage`] snapshot and hold only per-window UI state.
pub struct GrokUsage {
    store: Entity<DaemonUsage>,
    chart_state: Entity<okena_usage::HistoryChartState>,
    popover_visible: bool,
    trigger_bounds: Bounds<Pixels>,
    hover_token: Arc<AtomicU64>,
    freshness_clock: Option<Task<()>>,
}

impl GrokUsage {
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
                    GlobalDaemonUsage::request_refresh(ApiUsageProvider::Grok, cx);
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
        let data = provider_usage(ApiUsageProvider::Grok, cx).unwrap_or_else(|| {
            ApiProviderUsage::empty(ApiUsageProvider::Grok, ApiUsageStatus::Pending)
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
                let row = popover_row(window)?;
                Some(
                    render_usage_row(
                        t,
                        cx,
                        &row,
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
                            plan.as_deref(),
                            "https://grok.com/?_s=usage",
                            "Open Grok usage",
                            t,
                            cx,
                        ))
                        .child(
                            usage_body_container()
                                .children(rows)
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
                                .child(status_rows),
                        ),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }
}

/// Popover row for a wire window; `None` for a window this widget does not show.
fn popover_row(window: &ApiUsageWindow) -> Option<UsageRow> {
    let marker_id = match window.id.as_str() {
        "included" => "grok-marker-included",
        "on_demand" => "grok-marker-on-demand",
        _ => return None,
    };
    Some(UsageRow {
        period: period_label(window.window_seconds).into(),
        ..window_row(window, window.label.clone(), marker_id)
    })
}

/// Status-bar label for a wire window; `None` keeps it out of the trigger.
fn trigger_label(window: &ApiUsageWindow) -> Option<&'static str> {
    match window.id.as_str() {
        "included" => Some("Grok"),
        "on_demand" => Some("Extra"),
        _ => None,
    }
}

/// Period badge for a billing window. Grok periods are monthly or weekly and
/// may be unknown (`0`), which the shared 1h..30d buckets cannot express.
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

fn format_balance(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
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

impl Render for GrokUsage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let working = read_working_days(cx);
        let items: Vec<okena_usage::TriggerItem> = provider_usage(ApiUsageProvider::Grok, cx)
            .map(|data| {
                data.windows
                    .iter()
                    .filter_map(|window| {
                        let label = trigger_label(window)?;
                        Some(window_trigger_item(window, label, working))
                    })
                    .collect()
            })
            .unwrap_or_default();

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
    // gpui::* re-exports a `test` attribute macro that conflicts with the built-in;
    // alias the built-in so `#[test]` works normally in this module.
    use core::prelude::rust_2024::test;

    fn window(id: &str, label: &str, window_seconds: f64) -> ApiUsageWindow {
        ApiUsageWindow {
            id: id.into(),
            label: label.into(),
            kind: okena_core::api::ApiUsageWindowKind::Other,
            used_percent: 37.0,
            window_seconds,
            reset_at: None,
            active: true,
            history: Vec::new(),
        }
    }

    #[test]
    fn credit_windows_become_labelled_rows_and_trigger_items() {
        let included = popover_row(&window("included", "Daemon credits", 31.0 * 86_400.0))
            .expect("included row");
        assert_eq!(included.label.as_ref(), "Daemon credits");
        assert_eq!(included.period.as_ref(), "30d");
        assert_eq!(included.marker_id.as_ref(), "grok-marker-included");
        assert_eq!(included.pct, 37.0);

        let on_demand =
            popover_row(&window("on_demand", "On-demand", 7.0 * 86_400.0)).expect("on-demand row");
        assert_eq!(on_demand.label.as_ref(), "On-demand");
        assert_eq!(on_demand.period.as_ref(), "7d");
        assert_eq!(on_demand.marker_id.as_ref(), "grok-marker-on-demand");

        assert!(popover_row(&window("primary", "Rate Limit", 0.0)).is_none());
        assert_eq!(trigger_label(&window("included", "", 0.0)), Some("Grok"));
        assert_eq!(trigger_label(&window("on_demand", "", 0.0)), Some("Extra"));
        assert_eq!(trigger_label(&window("primary", "", 0.0)), None);
    }

    #[test]
    fn an_unknown_billing_period_is_labelled_as_such() {
        assert_eq!(period_label(0.0), "period");
        assert_eq!(period_label(14.0 * 86_400.0), "14d");
        assert_eq!(period_label(5.0 * 3_600.0), "5h");
    }

    #[test]
    fn balance_drops_the_fraction_only_when_whole() {
        assert_eq!(format_balance(15.0), "15");
        assert_eq!(format_balance(15.5), "15.50");
    }

    #[test]
    fn format_age_units() {
        assert_eq!(format_age(Duration::from_secs(5)), "5s");
        assert_eq!(format_age(Duration::from_secs(65)), "1m 5s");
        assert_eq!(format_age(Duration::from_secs(3720)), "1h 2m");
    }
}
