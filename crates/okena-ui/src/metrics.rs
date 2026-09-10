//! Status-bar metric primitives.
//!
//! Three things live here because the status bar is assembled from widgets
//! that ship in different crates (system stats in `okena-app`, usage bars in
//! `okena-usage`, service status in the `okena-ext-*` crates) and they all
//! have to agree on how a metric looks:
//!
//! - [`metric_bar`] — the thin capacity bar drawn under a value,
//! - [`sparkline`] — a short history graph for a value sampled over time,
//! - [`status_bar_style`] — the host-registered reader for the user's
//!   Detailed/Minimal preference, so a widget in any crate can decide whether
//!   to spell its number out.

use crate::theme::ThemeColors;
use gpui::*;

pub use okena_core::types::StatusBarStyle;

// =============================================================================
// Global status bar style provider
// =============================================================================

/// Global function pointer that reads the current status bar style from the
/// host app's settings. The host registers this at startup; widgets in any
/// crate call [`status_bar_style`]. Unregistered (tests, headless) falls back
/// to [`StatusBarStyle::Detailed`].
pub struct GlobalStatusBarStyle(pub fn(&App) -> StatusBarStyle);

impl Global for GlobalStatusBarStyle {}

pub fn status_bar_style(cx: &App) -> StatusBarStyle {
    cx.try_global::<GlobalStatusBarStyle>()
        .map(|g| (g.0)(cx))
        .unwrap_or_default()
}

// =============================================================================
// Bars and graphs
// =============================================================================

/// Muted track drawn behind a metric bar or under a sparkline.
pub fn metric_track_color(t: &ThemeColors) -> Rgba {
    let mut color = rgb(t.text_muted);
    color.a = 0.55;
    color
}

/// Thin capacity bar. `fraction` is 0.0..=1.0 of the parent's width; the
/// caller sizes the width (usually `w_full` under a label).
pub fn metric_bar(fraction: f32, color: u32, t: &ThemeColors) -> Div {
    div()
        .h(px(2.0))
        .w_full()
        .rounded_full()
        .bg(metric_track_color(t))
        .child(
            div()
                .h_full()
                .w(relative(fraction.clamp(0.0, 1.0)))
                .rounded_full()
                .bg(rgb(color)),
        )
}

/// Geometry of a [`sparkline`]. `slots` is how many samples are drawn; a
/// shorter history is padded on the left so the graph never changes width
/// while it fills up.
#[derive(Clone, Copy, Debug)]
pub struct SparklineStyle {
    pub slots: usize,
    pub height: Pixels,
    pub bar_width: Pixels,
    pub gap: Pixels,
}

impl SparklineStyle {
    /// Compact graph that fits under a label in the detailed status bar.
    pub fn compact() -> Self {
        Self {
            slots: 14,
            height: px(6.0),
            bar_width: px(2.0),
            gap: px(1.0),
        }
    }

    /// Taller graph for the minimal status bar, where the graph replaces the
    /// number instead of sitting under it.
    pub fn tall() -> Self {
        Self {
            slots: 16,
            height: px(11.0),
            bar_width: px(2.0),
            gap: px(1.0),
        }
    }

    pub fn width(self) -> Pixels {
        self.bar_width * self.slots as f32 + self.gap * self.slots.saturating_sub(1) as f32
    }
}

/// A short history graph. `values` are 0.0..=1.0, oldest first; only the last
/// `style.slots` are drawn, so the newest sample is always at the right edge.
pub fn sparkline(values: &[f32], style: SparklineStyle, color: u32, t: &ThemeColors) -> Div {
    let track = metric_track_color(t);
    let shown = &values[values.len().saturating_sub(style.slots)..];
    let missing = style.slots.saturating_sub(shown.len());

    let mut row = div()
        .flex()
        .items_end()
        .gap(style.gap)
        .h(style.height)
        .w(style.width());

    // Empty slots keep the graph a fixed width while the history fills up.
    for _ in 0..missing {
        row = row.child(div().w(style.bar_width).h(px(1.0)).bg(track));
    }
    let full_height: f32 = style.height.into();
    for value in shown {
        // Never zero — an idle sample still reads as a baseline tick.
        let filled = px(f32::max(1.0, full_height * value.clamp(0.0, 1.0)));
        row = row.child(
            div()
                .w(style.bar_width)
                .h(style.height)
                .flex()
                .flex_col()
                .justify_end()
                .child(div().w_full().h(filled).bg(rgb(color))),
        );
    }

    row
}

// =============================================================================
// Service status trigger
// =============================================================================

/// Trigger content for a service-status widget (Claude Code, Codex, GitHub):
/// the service name plus its status word.
///
/// In the minimal status bar an all-clear status is reduced to a colored dot —
/// "OK" next to three service names is the most redundant text in the bar.
/// Anything abnormal keeps its label in both styles, so a problem never hides.
pub fn service_status_items(
    name: impl Into<SharedString>,
    label: impl Into<SharedString>,
    color: u32,
    healthy: bool,
    t: &ThemeColors,
    cx: &App,
) -> Vec<AnyElement> {
    let name_el = div()
        .text_color(rgb(t.text_muted))
        .child(name.into())
        .into_any_element();
    let label_el = div()
        .text_color(rgb(color))
        .child(label.into())
        .into_any_element();

    if !status_bar_style(cx).is_minimal() {
        return vec![name_el, label_el];
    }

    let dot = div()
        .flex_shrink_0()
        .w(px(6.0))
        .h(px(6.0))
        .rounded_full()
        .bg(rgb(color))
        .into_any_element();

    if healthy {
        vec![dot, name_el]
    } else {
        vec![dot, name_el, label_el]
    }
}

#[cfg(test)]
mod tests {
    // Import specific items rather than `use super::*` — the latter pulls in
    // gpui's `test` proc-macro (re-exported via `use gpui::*`), which shadows
    // the std `#[test]` attribute and blows the macro recursion limit.
    use super::SparklineStyle;
    use gpui::px;

    #[test]
    fn compact_width_counts_gaps_between_bars() {
        let style = SparklineStyle {
            slots: 4,
            height: px(6.0),
            bar_width: px(2.0),
            gap: px(1.0),
        };
        // 4 bars * 2px + 3 gaps * 1px
        assert_eq!(style.width(), px(11.0));
    }

    #[test]
    fn single_slot_has_no_gap() {
        let style = SparklineStyle {
            slots: 1,
            height: px(6.0),
            bar_width: px(2.0),
            gap: px(1.0),
        };
        assert_eq!(style.width(), px(2.0));
    }
}
