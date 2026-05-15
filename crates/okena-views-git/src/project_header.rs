//! Git-related rendering for project column headers.
//!
//! Pure render functions extracted from `ProjectColumn` so they can be
//! reused without depending on the full view entity. Implementation is
//! split across `project_header/` submodules; this file holds the small
//! theme-trait impls and standalone badges, and re-exports each
//! submodule's public surface so callers can keep using `project_header::*`.

use okena_core::theme::ThemeColors;
use okena_git::{CiStatus, PrState};

use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;

mod branch_status;
mod commit_log;
mod diff_tree;
mod graph;

pub use branch_status::{render_branch_status, BranchStatusCallbacks};
pub use commit_log::{render_commit_log_content, render_commit_log_header};
pub use diff_tree::render_diff_file_list_interactive;
pub use graph::{
    render_graph_column, render_graph_row, render_ref_label,
    COMMIT_ROW_H, CONNECTOR_ROW_H, DOT_SIZE, GRAPH_CELL_W, RAIL_W,
};

// ── Theme-dependent color traits ────────────────────────────────────────────

/// Extension trait: map `PrState` to a theme color.
pub trait PrStateColor {
    fn color(&self, t: &ThemeColors) -> u32;
}

impl PrStateColor for PrState {
    fn color(&self, t: &ThemeColors) -> u32 {
        match self {
            PrState::Open => t.term_green,
            PrState::Draft => t.text_muted,
            PrState::Merged => t.term_magenta,
            PrState::Closed => t.term_red,
        }
    }
}

/// Extension trait: map `CiStatus` to a theme color.
pub trait CiStatusColor {
    fn color(&self, t: &ThemeColors) -> u32;
}

impl CiStatusColor for CiStatus {
    fn color(&self, t: &ThemeColors) -> u32 {
        match self {
            CiStatus::Success => t.term_green,
            CiStatus::Failure => t.term_red,
            CiStatus::Pending => t.term_yellow,
        }
    }
}

// ── Color helpers ───────────────────────────────────────────────────────────

/// Convert a packed `0xRRGGBB` color into an `Rgba` with the given alpha.
/// Handy for subtle tinted backgrounds derived from theme colors.
pub(crate) fn tint(color: u32, alpha: f32) -> Rgba {
    let r = ((color >> 16) & 0xFF) as f32 / 255.0;
    let g = ((color >> 8) & 0xFF) as f32 / 255.0;
    let b = (color & 0xFF) as f32 / 255.0;
    Rgba { r, g, b, a: alpha }
}

// ── Standalone badges ───────────────────────────────────────────────────────

/// Tooltip text describing ahead/behind counts; `None` when both zero.
pub fn ahead_behind_tooltip(counts: (Option<usize>, Option<usize>)) -> Option<String> {
    let ahead = counts.0.unwrap_or(0);
    let behind = counts.1.unwrap_or(0);
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    match (ahead, behind) {
        (0, 0) => None,
        (a, 0) => Some(format!("{a} commit{} to push", plural(a))),
        (0, b) => Some(format!("{b} commit{} to pull", plural(b))),
        (a, b) => Some(format!("{a} to push, {b} to pull")),
    }
}

/// Render a single "<sign> <count>" pair where the sign character is rendered
/// in a muted tone of the color and the number itself gets full color +
/// medium weight. Used for both diff stats and ahead/behind so the row reads
/// as typography rather than CLI output.
fn render_sign_count(sign: &str, count: usize, color: u32, alpha: f32) -> Div {
    div()
        .flex()
        .items_baseline()
        .gap(px(1.0))
        .child(div().text_color(tint(color, alpha)).child(sign.to_string()))
        .child(
            div()
                .text_color(rgb(color))
                .font_weight(FontWeight::MEDIUM)
                .child(format!("{count}")),
        )
}

/// Render an ahead/behind indicator (`↑N ↓M`). Zero-count sides are hidden so
/// `↑10 ↓0` becomes just `↑10`. Returns `None` when both counts are zero.
pub fn render_ahead_behind_badge(
    counts: (Option<usize>, Option<usize>),
    t: &ThemeColors,
) -> Option<AnyElement> {
    let tooltip_text = ahead_behind_tooltip(counts)?;
    let ahead = counts.0.unwrap_or(0);
    let behind = counts.1.unwrap_or(0);

    Some(
        div()
            .id("ahead-behind-badge")
            .flex()
            .items_center()
            .gap(px(5.0))
            .px(px(3.0))
            .when(ahead > 0, |d| {
                d.child(render_sign_count("\u{2191}", ahead, t.term_green, 0.7))
            })
            .when(behind > 0, |d| {
                d.child(render_sign_count("\u{2193}", behind, t.term_yellow, 0.7))
            })
            .tooltip(move |window, cx| Tooltip::new(tooltip_text.clone()).build(window, cx))
            .into_any_element(),
    )
}

/// Render the diff stats badge as `+N −M` (typographic minus, no slash).
/// Zero sides are hidden so a pure-additions diff reads as just `+495`. The
/// sign glyph is muted; the number gets full color + medium weight to make
/// the count the primary glyph.
///
/// Returns a `Div` (not yet stateful). The caller should:
/// - Assign an `id(...)` and attach hover/click handlers
/// - Attach a canvas to capture bounds for popover positioning
pub fn render_diff_stats_badge(lines_added: usize, lines_removed: usize, t: &ThemeColors) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(5.0))
        .px(px(4.0))
        .py(px(1.0))
        .when(lines_added > 0, |d| {
            d.child(render_sign_count("+", lines_added, t.term_green, 0.7))
        })
        .when(lines_removed > 0, |d| {
            d.child(render_sign_count("\u{2212}", lines_removed, t.term_red, 0.7))
        })
}
