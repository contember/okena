#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

//! Shared usage-bar UI and logic for the Claude and Codex usage widgets.
//!
//! Both extensions render an identical popover (a header bar, one row per
//! rate-limit window, an optional extra/credits row) over the same primitives
//! defined here, so the two widgets cannot drift apart. The crate also owns
//! the **working-days** feature: a shared user preference that tailors the
//! multi-day (weekly) bar to the days the user actually works.

// The pure math and history live in the gpui-free `okena-agent-usage` crate
// (the daemon uses them too); re-exported so widget code keeps its paths.
pub use okena_agent_usage::history;
pub use okena_agent_usage::pace::*;
pub use okena_agent_usage::pool_text;
mod daemon_usage;
mod history_chart;
pub use daemon_usage::{
    DaemonUsage, GlobalDaemonUsage, HostUsage, PoolHistoryFn, UsageRefreshFn, provider_usage,
    window_points, window_row, window_trigger_item,
};
pub use history_chart::HistoryChartState;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use gpui_component::{h_flex, v_flex};
use okena_extensions::ExtensionSettingsStore;
use okena_ui::metrics::{metric_bar, status_bar_style};
use okena_ui::settings::{section_container, section_header, section_note};
use okena_ui::theme::ThemeColors;
use okena_ui::tokens::{ui_text_md, ui_text_ms, ui_text_sm, ui_text_xs};

// ============================================================================
// Severity
// ============================================================================

pub fn severity_color(t: &ThemeColors, s: Severity) -> u32 {
    match s {
        Severity::Normal => t.metric_normal,
        Severity::Warning => t.metric_warning,
        Severity::Critical => t.metric_critical,
    }
}

pub fn utilization_color(t: &ThemeColors, pct: f64) -> u32 {
    severity_color(t, abs_severity(pct))
}

/// The headline color for a metric: the worse of nearness-to-cap and burn
/// pace. The popover percentage and the status-bar trigger both use this, so
/// they always show the same color for the same metric.
pub fn headline_color(t: &ThemeColors, pct: f64, time_pct: Option<f64>) -> u32 {
    severity_color(t, headline_severity(pct, time_pct))
}

// ============================================================================
// Working days (shared setting)
// ============================================================================

/// Read the shared working-days preference (defaults to all days).
pub fn read_working_days(cx: &App) -> WorkingDays {
    let value = cx
        .try_global::<ExtensionSettingsStore>()
        .and_then(|store| store.get(USAGE_SETTINGS_KEY, cx));
    WorkingDays::from_value(value.as_ref())
}

/// Persist the shared working-days preference.
pub fn write_working_days(days: WorkingDays, cx: &mut App) {
    ExtensionSettingsStore::update(USAGE_SETTINGS_KEY, days.to_value(), cx);
}

// ============================================================================
// Popover chrome
// ============================================================================

/// Styled popover container (the caller adds `id`, `occlude`, hover/click
/// handlers and children).
pub fn usage_popover_container(t: &ThemeColors) -> Div {
    div()
        .min_w(px(300.0))
        .max_w(px(420.0))
        .bg(rgb(t.bg_primary))
        .border_1()
        .border_color(rgb(t.border))
        .rounded(px(8.0))
        .shadow_lg()
}

/// Bordered popover header: an uppercase title on the left, an optional muted
/// plan badge and a "Settings ↗" link on the right.
pub fn usage_popover_header(
    title: &str,
    plan: Option<&str>,
    settings_url: &'static str,
    settings_tooltip: &'static str,
    t: &ThemeColors,
    cx: &App,
) -> impl IntoElement {
    let muted = t.text_muted;
    let primary = t.text_primary;
    let link_id = SharedString::from(format!("{}-settings-link", title));

    h_flex()
        .px(px(12.0))
        .py(px(7.0))
        .items_center()
        .justify_between()
        .border_b_1()
        .border_color(rgb(t.border))
        .child(
            div()
                .text_size(ui_text_xs(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(t.text_secondary))
                .child(title.to_string()),
        )
        .child(
            h_flex()
                .gap(px(8.0))
                .items_center()
                .when_some(plan, |el, plan| {
                    el.child(
                        div()
                            .text_size(ui_text_xs(cx))
                            .text_color(rgb(t.text_muted))
                            .child(plan.to_string()),
                    )
                })
                .child(
                    h_flex()
                        .id(link_id)
                        // Left padding only, so the trailing icon sits flush
                        // with the header's 12px inset (matching the title on
                        // the left) instead of looking inset on the right.
                        .gap(px(4.0))
                        .items_center()
                        .pl(px(4.0))
                        .py(px(1.0))
                        .rounded(px(3.0))
                        .cursor_pointer()
                        .text_color(rgb(muted))
                        .hover(|s| s.text_color(rgb(primary)).bg(rgb(t.bg_hover)))
                        .child(
                            div()
                                .text_size(ui_text_xs(cx))
                                .line_height(px(10.0))
                                .child("Settings"),
                        )
                        // `currentColor` resolves from the svg's *own* text_color,
                        // not the parent's — without this the icon renders as an
                        // invisible black glyph, leaving its slot looking like
                        // stray right padding.
                        .child(
                            svg()
                                .path("icons/external-link.svg")
                                .size(px(10.0))
                                .text_color(rgb(muted)),
                        )
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_click(move |_, _, _cx| {
                            okena_core::process::open_url(settings_url);
                        })
                        .tooltip(move |window, cx| {
                            Tooltip::new(settings_tooltip).build(window, cx)
                        }),
                ),
        )
}

/// Padded popover body container (the caller adds the rows).
pub fn usage_body_container() -> Div {
    v_flex().px(px(12.0)).py(px(10.0)).gap(px(7.0))
}

pub fn usage_divider(t: &ThemeColors) -> impl IntoElement {
    div().h(px(1.0)).w_full().bg(rgb(t.border))
}

// ============================================================================
// Rows + bars
// ============================================================================

/// One rate-limit window's worth of data, ready to render as a row.
pub struct UsageRow {
    /// Primary label, e.g. `Session` / `Weekly` / `Rate Limit`.
    pub label: SharedString,
    /// Period badge, e.g. `5h` / `7d`.
    pub period: SharedString,
    /// Utilization 0–100.
    pub pct: f64,
    /// Linear time-elapsed fraction 0–100 from the API (overridden by
    /// working-day reshaping when active).
    pub time_pct: Option<f64>,
    /// Reset time as Unix epoch seconds, used for the grid + "resets …" label.
    pub reset_epoch: Option<f64>,
    /// Length of this rate-limit window in seconds.
    pub period_secs: f64,
    /// Grid granularity; `None` disables the grid (sub-hour windows).
    pub unit: Option<SegmentUnit>,
    /// Stable element id for the time marker (for its tooltip).
    pub marker_id: SharedString,
}

/// The recorded samples of `limit_name` inside `row`'s current window, for
/// [`render_usage_row`] when the widget keeps its own [`history::History`].
pub fn history_points(
    history: &history::History,
    limit_name: &str,
    row: &UsageRow,
) -> Vec<history::HistoryPoint> {
    match row.reset_epoch {
        Some(reset) if row.period_secs > 0.0 => history.points(
            limit_name,
            reset,
            row.period_secs,
            now_secs().unwrap_or(0.0),
        ),
        _ => Vec::new(),
    }
}

/// Render a usage row: label + period badge, percentage, the usage/time bar,
/// and a pace message + "resets …" line.
pub fn render_usage_row(
    t: &ThemeColors,
    cx: &App,
    row: &UsageRow,
    working: WorkingDays,
    points: Vec<history::HistoryPoint>,
    chart_state: &Entity<HistoryChartState>,
) -> impl IntoElement {
    let seg = match (row.unit, row.reset_epoch) {
        (Some(unit), Some(reset)) => reset_aligned_segments(reset, row.period_secs, unit, working),
        _ => Segments::default(),
    };
    // Working-day reshaping overrides the linear pace value when active.
    let now = now_secs().unwrap_or(0.0);
    let effective_time = row
        .reset_epoch
        .filter(|_| row.period_secs > 0.0)
        .map(|reset| seg.time_fraction(now, reset, row.period_secs) as f64 * 100.0)
        .or(row.time_pct);

    let pct = row.pct;
    let pace = pace_severity(pct, effective_time);
    // % text reflects whichever is worse: nearness to the cap, or burn pace.
    let pct_color = headline_color(t, pct, effective_time);
    let pace_msg: Option<(&str, u32)> = match pace {
        Severity::Critical => Some(("Slow down to last the period", t.metric_critical)),
        Severity::Warning => Some(("Ahead of pace", t.metric_warning)),
        Severity::Normal => None,
    };

    let include_date = matches!(row.unit, Some(SegmentUnit::Day));
    let resets_label = row
        .reset_epoch
        .map(|e| format_reset_time_epoch(e, include_date))
        .filter(|s| !s.is_empty());

    v_flex()
        .gap(px(5.0))
        .child(
            h_flex()
                .items_baseline()
                .justify_between()
                .child(
                    h_flex()
                        .gap(px(6.0))
                        .items_baseline()
                        .child(
                            div()
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.text_primary))
                                .child(row.label.clone()),
                        )
                        .child(
                            div()
                                .text_size(ui_text_xs(cx))
                                .text_color(rgb(t.text_muted))
                                .child(row.period.clone()),
                        ),
                )
                .child(
                    div()
                        .text_size(ui_text_md(cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(pct_color))
                        .child(format!("{:.0}%", pct)),
                ),
        )
        .child(render_usage_bar(
            t,
            pct,
            effective_time,
            row.marker_id.clone(),
            &seg,
        ))
        .when_some(
            row.reset_epoch.filter(|_| row.period_secs > 0.0),
            |el, _reset| {
                el.child(history_chart::render(
                    t,
                    cx,
                    row,
                    &seg,
                    points,
                    now,
                    chart_state,
                ))
            },
        )
        .when(pace_msg.is_some() || resets_label.is_some(), |el| {
            el.child(
                h_flex()
                    .justify_between()
                    .items_baseline()
                    .child(
                        div()
                            .text_size(ui_text_xs(cx))
                            .font_weight(FontWeight::MEDIUM)
                            .when_some(pace_msg, |d, (msg, col)| d.text_color(rgb(col)).child(msg)),
                    )
                    .when_some(resets_label, |el, label| {
                        el.child(
                            div()
                                .text_size(ui_text_xs(cx))
                                .text_color(rgb(t.text_muted))
                                .child(format!("resets {}", label)),
                        )
                    }),
            )
        })
}

/// A divider color that contrasts with whatever background sits *behind* it:
/// a translucent dark line over a light/bright fill, a translucent light line
/// over a dark fill or the dark empty track. Picked per-divider from the
/// background's luminance so the grid stays visible on any theme — a single
/// fixed color can't, since a dark cut vanishes on a dark track while a light
/// line vanishes on a pale fill (e.g. the `neumie-contrast` gray/tan fills).
fn divider_overlay(bg_hex: u32) -> Rgba {
    let (r, g, b) = ThemeColors::hex_to_rgb(bg_hex);
    let luminance = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    if luminance < 128.0 {
        // Dark background → a translucent white line.
        let mut c = rgb(0xffffff);
        c.a = 0.6;
        c
    } else {
        // Light/bright background → a translucent black line.
        let mut c = rgb(0x000000);
        c.a = 0.5;
        c
    }
}

/// The 6px usage bar: base fill (nearness to cap), overage band (usage beyond
/// the pace marker), the reset-anchored day/hour grid, the current-block
/// highlight, and the time-elapsed marker.
pub fn render_usage_bar(
    t: &ThemeColors,
    usage_pct: f64,
    time_pct: Option<f64>,
    marker_id: impl Into<ElementId>,
    seg: &Segments,
) -> impl IntoElement {
    let marker_id = marker_id.into();
    let clamped_usage = usage_pct.clamp(0.0, 100.0) as f32;
    let base_color = severity_color(t, abs_severity(usage_pct));

    let overage = time_pct.and_then(|tp| {
        let start = tp.clamp(0.0, 100.0) as f32;
        let width = clamped_usage - start;
        if width <= 0.0 {
            return None;
        }
        let color = if width > 15.0 {
            t.metric_critical
        } else {
            t.metric_warning
        };
        Some((start, width, color))
    });

    // Each divider takes a dark or light overlay depending on what's behind it
    // at that point — the fill (base or, within the overage band, the overage
    // color) where the bar is filled, otherwise the empty track. This keeps the
    // grid visible across both the colored fill and the (often dominant) empty
    // track on every theme. See [`divider_overlay`].
    let usage_frac = clamped_usage / 100.0;
    let track_hex = t.bg_secondary;
    let fill_hex = base_color;
    let overage_for_dividers = overage;
    let divider_els = seg.dividers.clone().into_iter().map(move |f| {
        let behind = if f > usage_frac {
            track_hex
        } else if let Some((ostart, owidth, ocolor)) = overage_for_dividers {
            let pct = f * 100.0;
            if pct >= ostart && pct <= ostart + owidth {
                ocolor
            } else {
                fill_hex
            }
        } else {
            fill_hex
        };
        div()
            .absolute()
            .top_0()
            .h_full()
            .w(px(1.5))
            .bg(divider_overlay(behind))
            .left(relative(f))
    });

    // Translucent band over the current block. Derived from text_primary so it
    // lightens on dark themes and darkens on light ones.
    let mut highlight = rgb(t.text_primary);
    highlight.a = 0.14;

    div()
        .h(px(6.0))
        .w_full()
        .rounded_full()
        .bg(rgb(t.bg_secondary))
        .relative()
        .child(
            div()
                .h_full()
                .rounded_full()
                .bg(rgb(base_color))
                .w(relative(clamped_usage / 100.0)),
        )
        .when_some(overage, |el, (start, width, color)| {
            el.child(
                div()
                    .absolute()
                    .top_0()
                    .h_full()
                    .left(relative(start / 100.0))
                    .w(relative(width / 100.0))
                    .rounded_r(px(3.0))
                    .bg(rgb(color)),
            )
        })
        .children(divider_els)
        .when_some(seg.current, |el, (start, end)| {
            el.child(
                div()
                    .absolute()
                    .top_0()
                    .h_full()
                    .left(relative(start))
                    .w(relative((end - start).max(0.0)))
                    .bg(highlight),
            )
        })
        .when_some(time_pct, |el, tp| {
            let clamped_time = tp.clamp(0.0, 100.0) as f32;
            let marker_color = t.text_primary;
            el.child(
                div()
                    .id(marker_id)
                    .absolute()
                    .top(px(-4.0))
                    .left(relative(clamped_time / 100.0))
                    .w(px(8.0))
                    .h(px(14.0))
                    .flex()
                    .items_center()
                    .justify_start()
                    .child(
                        div()
                            .w(px(2.0))
                            .h(px(10.0))
                            .rounded(px(1.0))
                            .bg(rgb(marker_color)),
                    )
                    .tooltip(|window, cx| {
                        Tooltip::new("Time elapsed in this period").build(window, cx)
                    }),
            )
        })
}

/// A plain progress bar (no grid/marker) for credit/extra-usage rows.
pub fn render_simple_bar(t: &ThemeColors, pct: f64) -> impl IntoElement {
    let clamped = pct.clamp(0.0, 100.0) as f32;
    let color = utilization_color(t, pct);

    div()
        .h(px(6.0))
        .w_full()
        .rounded_full()
        .bg(rgb(t.bg_secondary))
        .child(
            div()
                .h_full()
                .rounded_full()
                .bg(rgb(color))
                .w(relative(clamped / 100.0)),
        )
}

/// A simple label/value row (e.g. "Credits  Unlimited", "Extra Usage  $2 / $50").
pub fn usage_kv_row(
    t: &ThemeColors,
    cx: &App,
    label: &str,
    value: String,
    value_color: u32,
) -> impl IntoElement {
    h_flex()
        .items_baseline()
        .justify_between()
        .child(
            div()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_secondary))
                .child(label.to_string()),
        )
        .child(
            div()
                .text_size(ui_text_ms(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(value_color))
                .child(value),
        )
}

// ============================================================================
// Status-bar trigger
// ============================================================================

/// One status-bar trigger entry: `label`, utilization, and the effective
/// time-elapsed value (so the color matches the popover headline exactly).
pub type TriggerItem = (SharedString, f64, Option<f64>);

/// Bar width for one trigger item in the minimal status bar, where the bar
/// stands on its own instead of sitting under a `label + %` row.
const MINIMAL_TRIGGER_BAR_WIDTH: Pixels = px(28.0);

/// Build the inner content of the status-bar trigger with a bar below each value.
/// The caller wraps these in a hoverable, bounds-tracking container.
pub fn usage_trigger_items(t: &ThemeColors, cx: &App, items: &[TriggerItem]) -> Vec<AnyElement> {
    items
        .iter()
        .map(|(label, pct, time_pct)| {
            usage_trigger_item(
                t,
                cx,
                label.clone(),
                format!("{:.0}%", pct),
                Some(pct.clamp(0.0, 100.0) as f32 / 100.0),
                headline_color(t, *pct, *time_pct),
            )
        })
        .collect()
}

/// One status-bar value: `label value` over a bar filled to `bar` (0–1). A
/// value without a bar keeps the bar's height so it lines up with the others.
///
/// In the minimal status bar the value is dropped and the label sits next to
/// the bar — the exact figures are one hover away in the popover. A value
/// without a bar keeps its text there, having nothing else to show.
pub fn usage_trigger_item(
    t: &ThemeColors,
    cx: &App,
    label: SharedString,
    value: String,
    bar: Option<f32>,
    color: u32,
) -> AnyElement {
    let label_el = div()
        .text_size(ui_text_sm(cx))
        .text_color(rgb(t.text_muted))
        .child(label);
    let value_el = div()
        .text_size(ui_text_sm(cx))
        .text_color(rgb(color))
        .child(value);

    if status_bar_style(cx).is_minimal() {
        h_flex()
            .gap(px(4.0))
            .child(label_el)
            .map(|row| match bar {
                Some(fraction) => row.child(
                    div()
                        .w(MINIMAL_TRIGGER_BAR_WIDTH)
                        .child(metric_bar(fraction, color, t)),
                ),
                None => row.child(value_el),
            })
            .into_any_element()
    } else {
        v_flex()
            .gap(px(1.0))
            .child(h_flex().gap(px(3.0)).child(label_el).child(value_el))
            .map(|column| match bar {
                Some(fraction) => column.child(metric_bar(fraction, color, t)),
                None => column.child(div().h(px(2.0))),
            })
            .into_any_element()
    }
}

// ============================================================================
// Working-days settings control
// ============================================================================

/// A self-contained settings control for the shared working-days preference.
/// Both the Claude and Codex settings panels embed one of these.
pub struct WorkingDaysSetting;

impl WorkingDaysSetting {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Re-render if the shared setting changes elsewhere.
        cx.observe_global::<ExtensionSettingsStore>(|_, cx| cx.notify())
            .detach();
        Self
    }

    fn toggle(&mut self, idx: usize, cx: &mut Context<Self>) {
        let mut days = read_working_days(cx);
        // Keep at least one working day selected.
        if days.days[idx] && days.count() <= 1 {
            return;
        }
        days.days[idx] = !days.days[idx];
        write_working_days(days, cx);
        cx.notify();
    }
}

impl Render for WorkingDaysSetting {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = okena_extensions::theme(cx);
        let days = read_working_days(cx);
        const LABELS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

        let mut chips = h_flex().gap(px(6.0)).flex_wrap();
        for (i, label) in LABELS.iter().enumerate() {
            let active = days.days[i];
            let mut chip = div()
                .id(SharedString::from(format!("workday-{i}")))
                .cursor_pointer()
                .px(px(10.0))
                .py(px(4.0))
                .rounded(px(6.0))
                .border_1()
                .text_size(ui_text_sm(cx))
                .child(label.to_string())
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(i, cx)));
            if active {
                let mut bg = rgb(t.border_active);
                bg.a = 0.18;
                chip = chip
                    .bg(bg)
                    .border_color(rgb(t.border_active))
                    .text_color(rgb(t.text_primary))
                    .font_weight(FontWeight::MEDIUM);
            } else {
                chip = chip
                    .bg(rgb(t.bg_secondary))
                    .border_color(rgb(t.bg_secondary))
                    .text_color(rgb(t.text_muted))
                    .hover(|s| s.text_color(rgb(t.text_secondary)));
            }
            chips = chips.child(chip);
        }

        v_flex()
            .child(section_header("Working days", &t, cx))
            .child(section_note(
                "Tailor the weekly usage bar to the days you work — the bar shows \
                 one block per working day. Shared across Claude and Codex.",
                &t,
                cx,
            ))
            .child(section_container(&t).child(v_flex().px(px(12.0)).py(px(10.0)).child(chips)))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    // gpui::* re-exports a `test` attribute macro that conflicts with the built-in;
    // alias the built-in so `#[test]` works normally in this module.
    use core::prelude::rust_2024::test;

    #[test]
    fn divider_overlay_contrasts_background() {
        // Dark track (neumie-contrast bg_secondary) → light line.
        let on_dark = divider_overlay(0x252526);
        assert!(
            on_dark.r > 0.5 && on_dark.a > 0.0,
            "dark bg → light divider"
        );
        // Pale fill (neumie-contrast metric_normal/warning) → dark line.
        let on_pale = divider_overlay(0xa0a0a0);
        assert!(
            on_pale.r < 0.5 && on_pale.a > 0.0,
            "light bg → dark divider"
        );
        // Bright yellow fill → still a dark line (the earlier complaint).
        let on_yellow = divider_overlay(0xe5e510);
        assert!(on_yellow.r < 0.5, "bright bg → dark divider");
    }
}
