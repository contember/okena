//! Claude Code usage widget: status-bar trigger + hover popover with chart.
//!
//! The numbers come from the daemon (`GET /v1/usage` / `usage_changed`), via
//! the app-installed [`GlobalDaemonUsage`] entity. This widget never fetches;
//! opening the popover asks the daemon to refresh (throttled there to 60 s).

use crate::ui_helpers::capitalize_first;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::{ApiExtraUsage, ApiUsageProvider, ApiUsageWindow, ApiUsageWindowKind};
use okena_extensions::ThemeColors;
use okena_usage::{
    DaemonUsage, GlobalDaemonUsage, provider_usage, read_working_days, render_simple_bar,
    render_usage_row, usage_body_container, usage_divider, usage_kv_row, usage_popover_container,
    usage_popover_header, usage_trigger_items, window_points, window_row, window_trigger_item,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Hover delay before showing the popover (ms)
const HOVER_DELAY_MS: u64 = 300;

fn theme(cx: &App) -> ThemeColors {
    okena_extensions::theme(cx)
}

/// Claude API usage indicator with hover popover.
///
/// One of these exists per window; they all read the shared
/// [`GlobalDaemonUsage`] snapshot and hold only per-window UI state.
pub struct ClaudeUsage {
    store: Entity<DaemonUsage>,
    chart_state: Entity<okena_usage::HistoryChartState>,
    popover_visible: bool,
    trigger_bounds: Bounds<Pixels>,
    hover_token: Arc<AtomicU64>,
}

impl ClaudeUsage {
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
                    GlobalDaemonUsage::request_refresh(ApiUsageProvider::Claude, cx);
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

    fn render_popover(&self, t: &ThemeColors, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.popover_visible {
            return div().size_0().into_any_element();
        }
        let Some(data) =
            provider_usage(ApiUsageProvider::Claude, cx).filter(|data| data.fetched_at.is_some())
        else {
            return div().size_0().into_any_element();
        };

        let working = read_working_days(cx);
        let source = self.store.read(cx).source_label().cloned();
        let plan = header_badge(data.plan.as_deref(), source.as_deref());
        let bounds = self.trigger_bounds;
        let position = point(bounds.origin.x, bounds.origin.y - px(4.0));

        let rows: Vec<AnyElement> = data
            .windows
            .iter()
            .filter_map(|window| {
                let (label, marker_id) = popover_row_identity(window)?;
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
                        .id("claude-usage-popover")
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
                            "CLAUDE USAGE",
                            plan.as_deref(),
                            "https://claude.ai/settings/usage",
                            "Open usage settings on claude.ai",
                            t,
                            cx,
                        ))
                        .child(usage_body_container().children(rows).when_some(
                            data.extra_usage.as_ref(),
                            |el, extra| {
                                if !extra.is_enabled {
                                    return el;
                                }
                                el.child(usage_divider(t))
                                    .child(render_extra_usage_row(t, cx, extra))
                            },
                        )),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }
}

/// Plan badge, plus the daemon's machine when it is not this one.
fn header_badge(plan: Option<&str>, source: Option<&str>) -> Option<String> {
    let plan = plan.map(capitalize_first);
    match (plan, source) {
        (Some(plan), Some(source)) => Some(format!("{plan} · {source}")),
        (Some(plan), None) => Some(plan),
        (None, Some(source)) => Some(source.to_string()),
        (None, None) => None,
    }
}

/// Popover label + marker id for a window, or `None` when it is hidden.
fn popover_row_identity(window: &ApiUsageWindow) -> Option<(SharedString, SharedString)> {
    match &window.kind {
        ApiUsageWindowKind::FiveHour => Some(("Session".into(), "claude-marker-session".into())),
        ApiUsageWindowKind::Weekly => Some(("Weekly".into(), "claude-marker-weekly".into())),
        ApiUsageWindowKind::WeeklyScoped { model } if should_show_scoped(window) => {
            Some((model.clone().into(), marker_id_for_scoped(model)))
        }
        _ => None,
    }
}

/// Status-bar label for a window, or `None` when it stays out of the trigger.
fn trigger_label(window: &ApiUsageWindow) -> Option<SharedString> {
    match &window.kind {
        ApiUsageWindowKind::FiveHour => Some("5h".into()),
        ApiUsageWindowKind::Weekly => Some("7d".into()),
        ApiUsageWindowKind::WeeklyScoped { model } if should_show_scoped_in_trigger(window) => {
            Some(model.clone().into())
        }
        _ => None,
    }
}

fn should_show_scoped(window: &ApiUsageWindow) -> bool {
    window.active || window.used_percent >= 0.5
}

fn should_show_scoped_in_trigger(window: &ApiUsageWindow) -> bool {
    window.active || window.used_percent >= 80.0
}

fn marker_id_for_scoped(label: &str) -> SharedString {
    let slug: String = label
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    format!("claude-marker-scoped-{slug}").into()
}

fn render_extra_usage_row(t: &ThemeColors, cx: &App, extra: &ApiExtraUsage) -> impl IntoElement {
    v_flex()
        .gap(px(5.0))
        .child(usage_kv_row(
            t,
            cx,
            "Extra Usage",
            format!(
                "${:.2} / ${:.2}",
                extra.used_credits_cents / 100.0,
                extra.monthly_limit_cents / 100.0
            ),
            t.text_primary,
        ))
        .child(render_simple_bar(t, extra.utilization))
}

impl Render for ClaudeUsage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let working = read_working_days(cx);
        let items: Vec<okena_usage::TriggerItem> = provider_usage(ApiUsageProvider::Claude, cx)
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
        if items.is_empty() {
            return div().size_0().into_any_element();
        }

        let entity_handle = cx.entity().clone();

        div()
            .child(
                h_flex()
                    .id("claude-usage-trigger")
                    .cursor_pointer()
                    .gap(px(8.0))
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

    fn window(kind: ApiUsageWindowKind, used_percent: f64, active: bool) -> ApiUsageWindow {
        ApiUsageWindow {
            id: "x".into(),
            label: "x".into(),
            kind,
            used_percent,
            window_seconds: 604800.0,
            reset_at: None,
            active,
            history: Vec::new(),
        }
    }

    #[test]
    fn scoped_rows_follow_visibility_thresholds() {
        let fable = |pct, active| {
            window(
                ApiUsageWindowKind::WeeklyScoped {
                    model: "Fable".into(),
                },
                pct,
                active,
            )
        };
        assert!(popover_row_identity(&fable(0.0, false)).is_none());
        assert!(popover_row_identity(&fable(0.5, false)).is_some());
        assert!(trigger_label(&fable(79.0, false)).is_none());
        assert_eq!(trigger_label(&fable(10.0, true)).as_deref(), Some("Fable"));
        assert_eq!(
            popover_row_identity(&fable(1.0, false)).map(|(_, id)| id),
            Some("claude-marker-scoped-fable".into())
        );
        assert_eq!(
            trigger_label(&window(ApiUsageWindowKind::FiveHour, 1.0, true)).as_deref(),
            Some("5h")
        );
    }

    #[test]
    fn header_badge_labels_remote_source() {
        assert_eq!(header_badge(Some("max"), None).as_deref(), Some("Max"));
        assert_eq!(
            header_badge(Some("max"), Some("studio")).as_deref(),
            Some("Max · studio")
        );
        assert_eq!(header_badge(None, None), None);
    }
}
