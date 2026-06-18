#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

//! Shared usage-bar UI and logic for the Claude and Codex usage widgets.
//!
//! Both extensions render an identical popover (a header bar, one row per
//! rate-limit window, an optional extra/credits row) over the same primitives
//! defined here, so the two widgets cannot drift apart. The crate also owns
//! the **working-days** feature: a shared user preference that tailors the
//! multi-day (weekly) bar to the days the user actually works.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use gpui_component::{h_flex, v_flex};
use okena_extensions::ExtensionSettingsStore;
use okena_ui::settings::{section_container, section_header};
use okena_ui::theme::ThemeColors;
use okena_ui::tokens::{ui_text_md, ui_text_ms, ui_text_sm, ui_text_xs};

// ============================================================================
// Severity
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Severity {
    Normal,
    Warning,
    Critical,
}

pub fn severity_color(t: &ThemeColors, s: Severity) -> u32 {
    match s {
        Severity::Normal => t.metric_normal,
        Severity::Warning => t.metric_warning,
        Severity::Critical => t.metric_critical,
    }
}

/// Severity from absolute utilization — how close to the hard cap.
pub fn abs_severity(pct: f64) -> Severity {
    if pct > 80.0 {
        Severity::Critical
    } else if pct > 60.0 {
        Severity::Warning
    } else {
        Severity::Normal
    }
}

/// Severity from pace — how far ahead usage is of where it "should" be at this
/// point in the period. `Critical` means the user is burning budget fast enough
/// to run out before the period resets unless they slow down.
pub fn pace_severity(usage_pct: f64, time_pct: Option<f64>) -> Severity {
    match time_pct {
        Some(tp) if usage_pct > tp + 15.0 => Severity::Critical,
        Some(tp) if usage_pct > tp + 5.0 => Severity::Warning,
        _ => Severity::Normal,
    }
}

pub fn utilization_color(t: &ThemeColors, pct: f64) -> u32 {
    severity_color(t, abs_severity(pct))
}

/// The headline color for a metric: the worse of nearness-to-cap and burn
/// pace. The popover percentage and the status-bar trigger both use this, so
/// they always show the same color for the same metric.
pub fn headline_color(t: &ThemeColors, pct: f64, time_pct: Option<f64>) -> u32 {
    severity_color(t, abs_severity(pct).max(pace_severity(pct, time_pct)))
}

// ============================================================================
// Working days (shared setting)
// ============================================================================

/// Settings namespace for the shared usage preferences blob. Stored in the
/// generic `extension_settings` map (not tied to one extension) so both the
/// Claude and Codex widgets read the same value.
pub const USAGE_SETTINGS_KEY: &str = "usage";

/// The days of the week the user works, indexed `0 = Monday .. 6 = Sunday`.
///
/// Default (and the "I work every day" selection) is all seven days, which
/// reproduces the original behavior — the bar is not reshaped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WorkingDays {
    pub days: [bool; 7],
}

impl WorkingDays {
    pub fn all() -> Self {
        Self { days: [true; 7] }
    }

    pub fn count(&self) -> usize {
        self.days.iter().filter(|&&d| d).count()
    }

    pub fn is_all(&self) -> bool {
        self.days.iter().all(|&d| d)
    }

    /// Parse from the persisted `{"working_days": [..]}` blob. An absent or
    /// empty list means "every day" (no tailoring).
    pub fn from_value(v: Option<&serde_json::Value>) -> Self {
        match v
            .and_then(|v| v.get("working_days"))
            .and_then(|a| a.as_array())
        {
            Some(arr) if !arr.is_empty() => {
                let mut days = [false; 7];
                for x in arr {
                    if let Some(i) = x.as_u64()
                        && (i as usize) < 7
                    {
                        days[i as usize] = true;
                    }
                }
                // A list that parsed to nothing valid falls back to all days.
                if days.iter().any(|&d| d) {
                    Self { days }
                } else {
                    Self::all()
                }
            }
            _ => Self::all(),
        }
    }

    pub fn to_value(&self) -> serde_json::Value {
        let idx: Vec<u64> = (0..7).filter(|&i| self.days[i]).map(|i| i as u64).collect();
        serde_json::json!({ "working_days": idx })
    }
}

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
// Reset-anchored grid + working-day reshaping
// ============================================================================

#[derive(Clone, Copy)]
pub enum SegmentUnit {
    Hour,
    Day,
}

/// Grid geometry for a usage bar.
#[derive(Clone, Default)]
pub struct Segments {
    /// Divider fractions in `(0, 1)`, excluding the ends.
    pub dividers: Vec<f32>,
    /// `(start, end)` fractions of the block that currently contains "now".
    pub current: Option<(f32, f32)>,
    /// When working-day reshaping is active, the time-elapsed fraction (0–100)
    /// recomputed on *working* time. `None` → the caller's own linear value is
    /// used instead.
    pub time_pct: Option<f64>,
}

fn epoch_of(z: &jiff::Zoned) -> f64 {
    z.timestamp().as_millisecond() as f64 / 1_000.0
}

fn epoch_to_local(epoch: f64) -> Option<jiff::Zoned> {
    let ts = jiff::Timestamp::from_millisecond((epoch * 1_000.0) as i64).ok()?;
    Some(ts.to_zoned(jiff::tz::TimeZone::system()))
}

fn now_secs() -> Option<f64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs_f64())
}

/// Compute reset-anchored grid boundaries for a usage bar, plus the block that
/// currently contains "now".
///
/// Boundaries land on the user's *actual* reset time-of-day — e.g. a Sunday
/// 12:00 weekly reset yields noon-to-noon day blocks — computed with calendar
/// arithmetic so they stay correct across DST and for any reset time.
///
/// When `working` is a strict subset of the week and the window is roughly a
/// week long, the weekly grid is **reshaped to one equal block per working
/// day** (N blocks instead of 7): non-working days are dropped from the axis,
/// and the returned [`Segments::time_pct`] advances only across working time so
/// the pace marker reflects the user's real schedule.
pub fn reset_aligned_segments(
    reset_epoch: f64,
    period_secs: f64,
    unit: SegmentUnit,
    working: WorkingDays,
) -> Segments {
    if period_secs <= 0.0 || reset_epoch <= 0.0 {
        return Segments::default();
    }
    let reset = match epoch_to_local(reset_epoch) {
        Some(z) => z,
        None => return Segments::default(),
    };
    let start_epoch = reset_epoch - period_secs;
    let now_epoch = now_secs().unwrap_or(start_epoch);

    // Reshape to working-day blocks only for ~weekly windows with a real subset
    // of days selected. Hour windows and longer/shorter day windows keep the
    // plain calendar grid.
    let day_units = (period_secs / 86_400.0).round() as i64;
    let remap = matches!(unit, SegmentUnit::Day)
        && working.count() >= 1
        && !working.is_all()
        && (2..=8).contains(&day_units);
    // If no working day lands in the window, `working_day_reshape` returns
    // `None` and we fall through to the plain grid below.
    if remap
        && let Some(seg) = working_day_reshape(start_epoch, reset_epoch, working, now_epoch)
    {
        return seg;
    }

    // Keep the grid legible on a thin bar: if a window spans more units than
    // this, step by a whole multiple (e.g. every 2 days), still anchored to the
    // reset time-of-day.
    let (unit_secs, max_blocks) = match unit {
        SegmentUnit::Hour => (3_600.0, 12.0),
        SegmentUnit::Day => (86_400.0, 14.0),
    };
    let units = (period_secs / unit_secs).round().max(1.0);
    let multiplier = (units / max_blocks).ceil().max(1.0) as i64;
    let step = match unit {
        SegmentUnit::Hour => jiff::Span::new().hours(multiplier),
        SegmentUnit::Day => jiff::Span::new().days(multiplier),
    };

    // Walk back from the reset, one block at a time, until the window start is
    // covered. `bounds` ends up holding boundary datetimes in ascending order.
    let mut bounds = vec![reset.clone()];
    let mut cursor = reset;
    for _ in 0..64 {
        cursor = match cursor.checked_sub(step) {
            Ok(z) => z,
            Err(_) => break,
        };
        let epoch = epoch_of(&cursor);
        bounds.push(cursor.clone());
        if epoch <= start_epoch {
            break;
        }
    }
    bounds.reverse();

    let frac = |epoch: f64| ((epoch - start_epoch) / period_secs) as f32;
    let dividers = bounds
        .iter()
        .map(|z| frac(epoch_of(z)))
        .filter(|&f| f > 0.001 && f < 0.999)
        .collect();
    let epochs: Vec<f64> = bounds.iter().map(epoch_of).collect();
    let current = epochs.windows(2).find_map(|w| {
        (now_epoch >= w[0] && now_epoch < w[1])
            .then(|| (frac(w[0]).max(0.0), frac(w[1]).min(1.0)))
    });
    Segments {
        dividers,
        current,
        time_pct: None,
    }
}

/// Reshape a ~weekly window to one equal block per **working calendar day**.
///
/// Blocks are keyed by calendar date (local midnight to midnight), not by the
/// reset time-of-day, so "today" always maps to today's block. With a reset at,
/// say, 13:00, a reset-anchored grid would put Thursday morning in the block
/// that *started* Wednesday — one block too early. Keying by date fixes that.
///
/// The window's two partial edge days together cover one weekday; we keep
/// whichever of them has more coverage, leaving exactly one date per weekday.
/// Working days are then laid out as equal segments in chronological order, and
/// the pace marker advances across working days only (frozen on off-days).
/// Returns `None` if no working day falls in the window (caller uses the plain
/// grid instead).
fn working_day_reshape(
    start_epoch: f64,
    reset_epoch: f64,
    working: WorkingDays,
    now_epoch: f64,
) -> Option<Segments> {
    let tz = jiff::tz::TimeZone::system();
    let start_date = epoch_to_local(start_epoch)?.date();
    let end_date = epoch_to_local(reset_epoch)?.date();

    // For each weekday keep the in-window calendar date with the most coverage,
    // so the two partial edge days collapse to a single weekday.
    let mut best: [Option<(jiff::civil::Date, f64)>; 7] = [None; 7];
    let mut d = start_date;
    for _ in 0..10 {
        let day_start = epoch_of(&d.to_zoned(tz.clone()).ok()?);
        let next = d.tomorrow().ok()?;
        let day_end = epoch_of(&next.to_zoned(tz.clone()).ok()?);
        let overlap = day_end.min(reset_epoch) - day_start.max(start_epoch);
        if overlap > 0.0 {
            let wd = (d.weekday().to_monday_zero_offset() as usize) % 7;
            if best[wd].is_none_or(|(_, ov)| overlap > ov) {
                best[wd] = Some((d, overlap));
            }
        }
        if d == end_date {
            break;
        }
        d = next;
    }

    // The working calendar dates, chronological.
    let mut dates: Vec<jiff::civil::Date> = (0..7)
        .filter(|&wd| working.days[wd])
        .filter_map(|wd| best[wd].map(|(date, _)| date))
        .collect();
    dates.sort();
    let n = dates.len();
    if n == 0 {
        return None;
    }

    let seg_w = 1.0_f32 / n as f32;
    let dividers: Vec<f32> = (1..n).map(|k| k as f32 * seg_w).collect();

    // Place the marker by where today sits among the working dates.
    let today = epoch_to_local(now_epoch)?.date();
    let (current, time_pct) = if let Some(k) = dates.iter().position(|&dt| dt == today) {
        let midnight = today
            .to_zoned(tz.clone())
            .ok()
            .map(|z| epoch_of(&z))
            .unwrap_or(now_epoch);
        let frac = (((now_epoch - midnight) / 86_400.0).clamp(0.0, 1.0)) as f32;
        let seg_start = k as f32 * seg_w;
        let tp = ((k as f32 + frac) / n as f32 * 100.0).clamp(0.0, 100.0) as f64;
        (Some((seg_start, seg_start + seg_w)), tp)
    } else {
        // Off-day (or outside the window): sit on the boundary after the last
        // elapsed working day; no current-block highlight.
        let elapsed = dates.iter().filter(|&&dt| dt < today).count();
        let tp = (elapsed as f64 / n as f64 * 100.0).clamp(0.0, 100.0);
        (None, tp)
    };

    Some(Segments {
        dividers,
        current,
        time_pct: Some(time_pct),
    })
}

/// The time-elapsed fraction a row will actually use: the working-day-reshaped
/// value when reshaping applies, otherwise the caller's linear value. Lets the
/// status-bar trigger compute the same pace color the popover row shows.
pub fn effective_time_pct(
    reset_epoch: Option<f64>,
    period_secs: f64,
    unit: Option<SegmentUnit>,
    working: WorkingDays,
    linear: Option<f64>,
) -> Option<f64> {
    match (unit, reset_epoch) {
        (Some(u), Some(r)) => reset_aligned_segments(r, period_secs, u, working)
            .time_pct
            .or(linear),
        _ => linear,
    }
}

// ============================================================================
// Reset-time formatting (absolute, with smart labels)
// ============================================================================

/// Format a reset time (Unix epoch seconds) to a human-readable short form in
/// the local timezone, e.g. `today, 14:00 PST` / `Tue, 09:00 PST` /
/// `Jun 21, 09:00 PST`. Returns an empty string for an unset/invalid time.
pub fn format_reset_time_epoch(reset_epoch: f64, include_date: bool) -> String {
    if reset_epoch <= 0.0 {
        return String::new();
    }
    let Some(zoned) = epoch_to_local(reset_epoch) else {
        return String::new();
    };

    if include_date {
        let today = jiff::Zoned::now().date();
        let reset_date = zoned.date();

        let diff_days = today
            .until(reset_date)
            .ok()
            .map(|span| span.get_days())
            .unwrap_or(i32::MAX);

        let date_label = match diff_days {
            0 => Some("today"),
            1 => Some("tomorrow"),
            _ => None,
        };

        return match date_label {
            Some(label) => format!("{}, {}", label, zoned.strftime("%H:%M %Z")),
            None if (2..=6).contains(&diff_days) => zoned.strftime("%a, %H:%M %Z").to_string(),
            None => zoned.strftime("%b %-d, %H:%M %Z").to_string(),
        };
    }

    zoned.strftime("%H:%M %Z").to_string()
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
                        .tooltip(move |window, cx| Tooltip::new(settings_tooltip).build(window, cx)),
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

/// Render a usage row: label + period badge, percentage, the usage/time bar,
/// and a pace message + "resets …" line.
pub fn render_usage_row(
    t: &ThemeColors,
    cx: &App,
    row: &UsageRow,
    working: WorkingDays,
) -> impl IntoElement {
    let seg = match (row.unit, row.reset_epoch) {
        (Some(unit), Some(reset)) => {
            reset_aligned_segments(reset, row.period_secs, unit, working)
        }
        _ => Segments::default(),
    };
    // Working-day reshaping overrides the linear pace value when active.
    let effective_time = seg.time_pct.or(row.time_pct);

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

/// Build the inner content of the status-bar trigger — `5h 42% | 7d 70%`.
/// The caller wraps these in a hoverable, bounds-tracking container.
pub fn usage_trigger_items(t: &ThemeColors, cx: &App, items: &[TriggerItem]) -> Vec<AnyElement> {
    let mut out = Vec::new();
    for (i, (label, pct, time_pct)) in items.iter().enumerate() {
        if i > 0 {
            out.push(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child("|")
                    .into_any_element(),
            );
        }
        out.push(
            h_flex()
                .gap(px(3.0))
                .child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child(label.clone()),
                )
                .child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(headline_color(t, *pct, *time_pct)))
                        .child(format!("{:.0}%", pct)),
                )
                .into_any_element(),
        );
    }
    out
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
            .gap(px(8.0))
            .child(section_header("Working days", &t, cx))
            .child(
                section_container(&t).child(
                    v_flex()
                        .px(px(12.0))
                        .py(px(10.0))
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_size(ui_text_sm(cx))
                                .text_color(rgb(t.text_muted))
                                .child(
                                    "Tailor the weekly usage bar to the days you work — \
                                     the bar shows one block per working day. \
                                     Shared across Claude and Codex.",
                                ),
                        )
                        .child(chips),
                ),
            )
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
    fn working_days_default_is_all() {
        assert!(WorkingDays::from_value(None).is_all());
        let empty = serde_json::json!({ "working_days": [] });
        assert!(WorkingDays::from_value(Some(&empty)).is_all());
    }

    #[test]
    fn working_days_round_trip() {
        let wd = WorkingDays {
            days: [true, true, true, true, true, false, false], // Mon-Fri
        };
        assert_eq!(wd.count(), 5);
        assert!(!wd.is_all());
        let parsed = WorkingDays::from_value(Some(&wd.to_value()));
        assert_eq!(parsed, wd);
    }

    #[test]
    fn working_days_ignores_out_of_range() {
        let v = serde_json::json!({ "working_days": [0, 9, 3] });
        let wd = WorkingDays::from_value(Some(&v));
        assert!(wd.days[0] && wd.days[3]);
        assert_eq!(wd.count(), 2);
    }

    #[test]
    fn all_days_keeps_calendar_grid() {
        // All days selected → no reshaping → linear grid, no working-time override.
        let period = 7.0 * 86_400.0;
        let seg = reset_aligned_segments(1_700_000_000.0, period, SegmentUnit::Day, WorkingDays::all());
        assert!(seg.time_pct.is_none(), "all-days must not override pace");
        assert!(
            (5..=7).contains(&seg.dividers.len()),
            "expected ~6 dividers, got {}",
            seg.dividers.len()
        );
        assert!(seg.dividers.iter().all(|&f| f > 0.0 && f < 1.0));
        assert!(seg.dividers.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn five_working_days_reshape_to_five_blocks() {
        // Mon-Fri over a 7-day window → 5 equal blocks → 4 internal dividers,
        // and a working-time pace override.
        let period = 7.0 * 86_400.0;
        let mon_fri = WorkingDays {
            days: [true, true, true, true, true, false, false],
        };
        let seg = reset_aligned_segments(1_700_000_000.0, period, SegmentUnit::Day, mon_fri);
        assert_eq!(seg.dividers.len(), 4, "5 blocks → 4 dividers, got {:?}", seg.dividers);
        // Equal blocks at 1/5, 2/5, 3/5, 4/5.
        for (i, &d) in seg.dividers.iter().enumerate() {
            let expected = (i + 1) as f32 / 5.0;
            assert!((d - expected).abs() < 1e-4, "divider {i} = {d}, expected {expected}");
        }
        assert!(seg.time_pct.is_some(), "reshaping must provide a working-time pace");
        let tp = seg.time_pct.unwrap();
        assert!((0.0..=100.0).contains(&tp));
    }

    #[test]
    fn reshape_marker_tracks_calendar_day_not_reset_time() {
        // Weekly window resetting Sat 2026-06-20 11:00Z, Mon–Fri working.
        // "Now" is Thursday 2026-06-18 09:00Z. Thursday is the 4th of 5 working
        // days, so the marker must land in the 4th block (second-to-last),
        // *not* the 3rd — the reset is at 11:00/13:00 local, but blocks key off
        // the calendar date, not the reset time-of-day.
        let reset = 1_781_953_200.0_f64; // 2026-06-20T11:00:00Z (Sat)
        let now = 1_781_773_200.0_f64; // 2026-06-18T09:00:00Z (Thu)
        let start = reset - 7.0 * 86_400.0;
        let mon_fri = WorkingDays {
            days: [true, true, true, true, true, false, false],
        };
        let seg = working_day_reshape(start, reset, mon_fri, now).expect("reshape");
        assert_eq!(seg.dividers, vec![0.2, 0.4, 0.6, 0.8], "5 equal blocks");
        let (cs, ce) = seg.current.expect("Thursday is a working day → highlighted");
        assert!(
            (cs - 0.6).abs() < 1e-4 && (ce - 0.8).abs() < 1e-4,
            "Thursday must be the 4th block, got ({cs}, {ce})"
        );
        let tp = seg.time_pct.expect("working pace");
        assert!((60.0..80.0).contains(&tp), "marker mid-Thursday, got {tp}");
    }

    #[test]
    fn reshape_off_day_pegs_to_boundary() {
        // Same window, but "now" is Saturday 2026-06-20 09:00Z (an off-day,
        // after the whole Mon–Fri week): the marker pegs to 100% with no
        // current-block highlight.
        let reset = 1_781_953_200.0_f64;
        let now = 1_781_946_000.0_f64; // 2026-06-20T09:00:00Z (Sat)
        let start = reset - 7.0 * 86_400.0;
        let mon_fri = WorkingDays {
            days: [true, true, true, true, true, false, false],
        };
        let seg = working_day_reshape(start, reset, mon_fri, now).expect("reshape");
        assert!(seg.current.is_none(), "off-day → no highlight");
        assert_eq!(seg.time_pct, Some(100.0), "all 5 working days elapsed");
    }

    #[test]
    fn hour_window_never_reshapes() {
        let mon_fri = WorkingDays {
            days: [true, true, true, true, true, false, false],
        };
        let seg = reset_aligned_segments(1_700_000_000.0, 5.0 * 3_600.0, SegmentUnit::Hour, mon_fri);
        assert!(seg.time_pct.is_none(), "hour windows keep linear pace");
    }

    #[test]
    fn guards_reject_bad_input() {
        let seg = reset_aligned_segments(0.0, 7.0 * 86_400.0, SegmentUnit::Day, WorkingDays::all());
        assert!(seg.dividers.is_empty() && seg.current.is_none());
        let seg = reset_aligned_segments(1_700_000_000.0, 0.0, SegmentUnit::Day, WorkingDays::all());
        assert!(seg.dividers.is_empty() && seg.current.is_none());
    }

    #[test]
    fn headline_severity_takes_worse_of_abs_and_pace() {
        // 65% absolute is only Warning, but far ahead of a 30% pace → Critical.
        // The popover row and the status-bar trigger both color from this same
        // max(abs, pace), so they can never show different colors for one metric.
        let ahead = abs_severity(65.0).max(pace_severity(65.0, Some(30.0)));
        let on_pace = abs_severity(65.0).max(pace_severity(65.0, Some(64.0)));
        assert_eq!(ahead, Severity::Critical, "over-pace 65% must be critical");
        assert_eq!(on_pace, Severity::Warning, "on-pace 65% stays warning");
    }

    #[test]
    fn divider_overlay_contrasts_background() {
        // Dark track (neumie-contrast bg_secondary) → light line.
        let on_dark = divider_overlay(0x252526);
        assert!(on_dark.r > 0.5 && on_dark.a > 0.0, "dark bg → light divider");
        // Pale fill (neumie-contrast metric_normal/warning) → dark line.
        let on_pale = divider_overlay(0xa0a0a0);
        assert!(on_pale.r < 0.5 && on_pale.a > 0.0, "light bg → dark divider");
        // Bright yellow fill → still a dark line (the earlier complaint).
        let on_yellow = divider_overlay(0xe5e510);
        assert!(on_yellow.r < 0.5, "bright bg → dark divider");
    }

    #[test]
    fn format_reset_time_empty_for_unset() {
        assert_eq!(format_reset_time_epoch(0.0, true), "");
        assert_eq!(format_reset_time_epoch(-5.0, false), "");
    }

    #[test]
    fn format_reset_time_has_clock_and_tz() {
        let result = format_reset_time_epoch(1_700_000_000.0, false);
        assert!(result.contains(':'), "expected HH:MM, got {result}");
        assert!(!result.is_empty());
    }
}
