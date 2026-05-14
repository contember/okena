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

// ── Standalone badges ───────────────────────────────────────────────────────

/// Render an ahead/behind badge (`↑N ↓M`). Returns `None` when both counts
/// are zero or unavailable — caller can `.when_some(...)` on the result.
pub fn render_ahead_behind_badge(
    counts: (Option<usize>, Option<usize>),
    t: &ThemeColors,
) -> Option<Div> {
    let ahead = counts.0.unwrap_or(0);
    let behind = counts.1.unwrap_or(0);
    if ahead == 0 && behind == 0 {
        return None;
    }
    Some(
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .px(px(4.0))
            .py(px(1.0))
            .rounded(px(3.0))
            .when(ahead > 0, |d| {
                d.child(
                    div()
                        .text_color(rgb(t.term_green))
                        .child(format!("\u{2191}{}", ahead)),
                )
            })
            .when(behind > 0, |d| {
                d.child(
                    div()
                        .text_color(rgb(t.term_yellow))
                        .child(format!("\u{2193}{}", behind)),
                )
            }),
    )
}

/// Render the diff stats badge (`+N / -M`).
///
/// Returns a `Div` (not yet stateful). The caller should:
/// - Assign an `id(...)` and attach hover/click handlers
/// - Attach a canvas to capture bounds for popover positioning
pub fn render_diff_stats_badge(lines_added: usize, lines_removed: usize, t: &ThemeColors) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(3.0))
        .px(px(4.0))
        .py(px(1.0))
        .rounded(px(3.0))
        .child(
            div()
                .text_color(rgb(t.term_green))
                .child(format!("+{}", lines_added)),
        )
        .child(div().text_color(rgb(t.text_muted)).child("/"))
        .child(
            div()
                .text_color(rgb(t.term_red))
                .child(format!("-{}", lines_removed)),
        )
}
