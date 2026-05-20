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
mod lane_layout;

pub use branch_status::{
    render_branch_status, BoundsCallback, BranchStatusCallbacks, ClickCallback, ClickHandler,
};
pub use commit_log::render_commit_log_content;
pub use diff_tree::render_diff_file_list_interactive;

/// Called with `(commit_hash, commit_message, commit_index)` when a commit row
/// is clicked. `None` renders the rows as non-interactive.
pub type CommitClickCallback =
    Option<std::sync::Arc<dyn Fn(&str, &str, usize, &mut Window, &mut App)>>;

/// Called with `(commit_hash, mouse_position)` when a commit row is
/// right-clicked. `None` disables the row context menu.
pub type CommitRightClickCallback =
    Option<std::sync::Arc<dyn Fn(&str, Point<Pixels>, &mut Window, &mut App)>>;

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

/// Tooltip text describing ahead/behind/unpushed counts.
///
/// `ahead`/`behind` are relative to the upstream tracking branch (which for
/// worktree branches off `origin/main` may be `origin/main` itself, not the
/// branch's own remote). `unpushed` counts commits missing from
/// `origin/<branch>` specifically — `None` when that ref doesn't exist.
///
/// Returns `None` when there is nothing meaningful to show.
pub fn ahead_behind_tooltip(
    ahead: Option<usize>,
    behind: Option<usize>,
    unpushed: Option<usize>,
) -> Option<String> {
    let a = ahead.unwrap_or(0);
    let b = behind.unwrap_or(0);
    let u = unpushed.unwrap_or(0);
    let plural = |n: usize| if n == 1 { "" } else { "s" };

    // When unpushed differs from ahead, the branch's upstream isn't its own
    // remote (typical worktree case). Surface both numbers so the user can
    // tell "ahead of main" from "not on origin/<branch>".
    let show_unpushed = unpushed.is_some() && Some(a) != unpushed;

    let mut parts: Vec<String> = Vec::new();
    if a > 0 {
        parts.push(format!("{a} commit{} ahead of upstream", plural(a)));
    }
    if b > 0 {
        parts.push(format!("{b} commit{} behind upstream", plural(b)));
    }
    if show_unpushed {
        parts.push(format!(
            "{u} commit{} not pushed to origin/<branch>",
            plural(u)
        ));
    }

    if parts.is_empty() { None } else { Some(parts.join("\n")) }
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

/// Render an ahead/behind/unpushed indicator.
///
/// - `↑N` (green) — commits ahead of the upstream tracking branch
/// - `↓M` (yellow) — commits behind the upstream tracking branch
/// - `↟K` (cyan, double-headed) — commits not on `origin/<branch>`. Only
///   shown when the count differs from `ahead` (i.e. upstream isn't the
///   branch's own remote — typical worktree case), so it doesn't double up
///   in the common case where `↑N` already means "to push".
///
/// Zero-count sides are hidden. Returns `None` when nothing is worth showing.
pub fn render_ahead_behind_badge(
    ahead: Option<usize>,
    behind: Option<usize>,
    unpushed: Option<usize>,
    t: &ThemeColors,
) -> Option<AnyElement> {
    let tooltip_text = ahead_behind_tooltip(ahead, behind, unpushed)?;
    let a = ahead.unwrap_or(0);
    let b = behind.unwrap_or(0);
    let u = unpushed.unwrap_or(0);
    let show_unpushed = unpushed.is_some() && Some(a) != unpushed && u > 0;

    Some(
        div()
            .id("ahead-behind-badge")
            .flex()
            .items_center()
            .gap(px(5.0))
            .px(px(3.0))
            .when(a > 0, |d| {
                d.child(render_sign_count("\u{2191}", a, t.term_green, 0.7))
            })
            .when(b > 0, |d| {
                d.child(render_sign_count("\u{2193}", b, t.term_yellow, 0.7))
            })
            .when(show_unpushed, |d| {
                d.child(render_sign_count("\u{219F}", u, t.term_cyan, 0.7))
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
