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

/// Strip a leading `origin/` so a base ref reads as a plain branch name
/// (`origin/main` → `main`) in the comparison chip.
pub(crate) fn base_short_name(base: &str) -> &str {
    base.strip_prefix("origin/").unwrap_or(base)
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
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

/// Render the "commits to push" indicator: a green `↑N` counting commits not
/// yet on `origin/<branch>`. This is the standard git convention (matches the
/// upstream-sync arrow in VS Code, lazygit, etc.) — it drops to 0 after a push.
///
/// Returns `None` when nothing is unpushed (or the upstream ref is unknown).
pub fn render_unpushed_badge(unpushed: Option<usize>, t: &ThemeColors) -> Option<AnyElement> {
    let u = unpushed.filter(|&u| u > 0)?;
    let tooltip = format!("{u} commit{} to push (not on origin/<branch>)", plural(u));
    Some(
        div()
            .id("unpushed-badge")
            .flex()
            .items_center()
            .px(px(3.0))
            .child(render_sign_count("\u{2191}", u, t.term_green, 0.7))
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            .into_any_element(),
    )
}

/// Render the branch-vs-base comparison content: ahead (`+N`) and behind (`−M`)
/// commit counts, e.g. `main +2 −1`. The base name is shown only when it isn't
/// the repository's default branch (`default_branch`) — comparing against the
/// default is the common case, so its name would be redundant noise. The
/// caller pairs this with a branch glyph and the click-to-review affordance.
///
/// Zero-count sides are hidden. Returns `None` when the branch is level with
/// its base (nothing to review).
pub fn render_base_compare_badge(
    ahead: Option<usize>,
    behind: Option<usize>,
    base: &str,
    default_branch: Option<&str>,
    t: &ThemeColors,
) -> Option<AnyElement> {
    let a = ahead.unwrap_or(0);
    let b = behind.unwrap_or(0);
    if a == 0 && b == 0 {
        return None;
    }

    let base_label = base_short_name(base).to_string();
    // Hide the label when the base is the default branch (the redundant case);
    // show it only for a non-default base so it carries information.
    let show_label = default_branch.is_none_or(|d| d != base_label);
    let tooltip = {
        let mut parts = Vec::new();
        if a > 0 {
            parts.push(format!("{a} commit{} ahead of {base_label}", plural(a)));
        }
        if b > 0 {
            parts.push(format!("{b} commit{} behind {base_label}", plural(b)));
        }
        parts.push("click to review".to_string());
        parts.join("\n")
    };

    Some(
        div()
            .id("base-compare-badge")
            .flex()
            .items_center()
            .gap(px(4.0))
            .when(show_label, |d| {
                d.child(
                    div()
                        .text_color(rgb(t.text_muted))
                        .child(base_label.clone()),
                )
            })
            .when(a > 0, |d| {
                d.child(render_sign_count("+", a, t.term_green, 0.7))
            })
            .when(b > 0, |d| {
                d.child(render_sign_count("\u{2212}", b, t.term_yellow, 0.7))
            })
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
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
