//! Git branch status pill: branch chip + optional PR badge + optional CI pill.
//!
//! These are rendered as three adjacent regions so the caller can wire
//! different actions to each: the branch chip opens the branch switcher,
//! the PR badge opens the PR URL on GitHub, and the CI pill opens the
//! checks popover. The CI pill is rendered independently of the PR badge
//! so it surfaces pipeline status on branches without a PR (e.g. `main`).

use super::{CiStatusColor, PrStateColor};

use okena_core::theme::ThemeColors;
use okena_git::GitStatus;

use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use gpui_component::tooltip::Tooltip;
use std::sync::Arc;

/// The shared `Arc<dyn Fn>` click handler, before being wrapped in `Option`.
pub type ClickHandler = Arc<dyn Fn(&mut Window, &mut App)>;
/// A click handler for an interactive part of the branch status pill.
pub type ClickCallback = Option<ClickHandler>;
/// A bounds-reporting callback used to anchor popovers under a pill element.
pub type BoundsCallback = Option<Arc<dyn Fn(Bounds<Pixels>, &mut App)>>;

/// Callbacks for the interactive parts of the branch status pill.
pub struct BranchStatusCallbacks {
    /// Called when the user clicks the branch chip. When `None` the chip is
    /// rendered as plain (non-clickable) text — used for read-only providers.
    pub on_branch_click: ClickCallback,
    /// Called when the user clicks the PR badge. When `None` the PR badge
    /// stays informational only (still rendered, but not clickable).
    pub on_pr_click: ClickCallback,
    /// Called when the user clicks the CI status pill. When `None` the pill
    /// stays informational only.
    pub on_ci_click: ClickCallback,
    /// Called every layout pass with the on-screen bounds of the branch chip,
    /// so the caller can anchor a popover underneath it.
    pub on_branch_bounds: BoundsCallback,
    /// Same as `on_branch_bounds` but for the CI pill — used to anchor the
    /// CI checks popover.
    pub on_ci_bounds: BoundsCallback,
}

/// Render the git branch status pill (branch chip + PR badge + CI pill).
///
/// Each region is independent: a PR may exist without CI yet (or vice versa
/// on `main` without a PR), and either part of the row may be missing.
pub fn render_branch_status(
    status: &GitStatus,
    callbacks: BranchStatusCallbacks,
    t: &ThemeColors,
) -> AnyElement {
    let pr_info = status.pr_info.clone();
    let pr_number = pr_info.as_ref().map(|p| p.number);
    let pr_state_color = pr_info.as_ref().map(|p| p.state.color(t));
    let ci_checks = status.ci_checks.clone();

    let BranchStatusCallbacks {
        on_branch_click,
        on_pr_click,
        on_ci_click,
        on_branch_bounds,
        on_ci_bounds,
    } = callbacks;

    let branch_clickable = on_branch_click.is_some();

    // Branch chip — icon + name + chevron (when interactive)
    let mut branch_chip = h_flex()
        .id("branch-chip")
        .relative()
        .gap(px(3.0))
        .px(px(2.0))
        .rounded(px(3.0))
        .when(branch_clickable, |d: Stateful<Div>| {
            d.cursor_pointer().hover(|s| s.bg(rgb(t.bg_hover)))
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .child(
            svg()
                .path("icons/git-branch.svg")
                .size(px(10.0))
                .text_color(rgb(t.text_muted)),
        )
        .child(
            div()
                .text_color(rgb(t.text_secondary))
                .max_w(px(100.0))
                .text_ellipsis()
                .overflow_hidden()
                .child(status.branch.clone().unwrap_or_default()),
        )
        .when(branch_clickable, |d| {
            d.child(
                svg()
                    .path("icons/chevron-down.svg")
                    .size(px(8.0))
                    .text_color(rgb(t.text_muted)),
            )
        });

    if let Some(bcb) = on_branch_bounds {
        branch_chip = branch_chip.child(
            canvas(
                move |bounds, _window, app| {
                    bcb(bounds, app);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        );
    }

    if let Some(cb) = on_branch_click {
        branch_chip = branch_chip.on_click(move |_, window, cx| {
            cb(window, cx);
        });
    }

    // PR badge — only when a PR exists. Clickable when `on_pr_click` is set,
    // typically wired to open the PR URL on GitHub.
    let pr_badge = pr_number.map(|num| {
        let pr_clickable = on_pr_click.is_some();
        let badge_color = pr_state_color.unwrap_or(t.text_muted);
        let mut el = h_flex()
            .id("pr-badge")
            .gap(px(3.0))
            .px(px(3.0))
            .rounded(px(3.0))
            .items_center()
            .when(pr_clickable, |d: Stateful<Div>| {
                d.cursor_pointer().hover(|s| s.bg(rgb(t.bg_hover)))
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .child(
                svg()
                    .path("icons/git-pull-request.svg")
                    .size(px(10.0))
                    .text_color(rgb(badge_color)),
            )
            .child(div().text_color(rgb(t.text_muted)).child(format!("#{num}")));
        if let Some(cb) = on_pr_click {
            el = el.on_click(move |_, window, cx| {
                cb(window, cx);
            });
        }
        el
    });

    // CI pill — only when we have a check summary for the current commit.
    // Independent of PR presence; rendered next to the PR badge when both
    // exist, and on its own otherwise.
    let ci_pill = ci_checks.map(|checks| {
        let ci_clickable = on_ci_click.is_some();
        let tooltip = checks.tooltip_text();
        let icon = checks.status.icon();
        let color = checks.status.color(t);
        let mut el = h_flex()
            .id("ci-pill")
            .relative()
            .px(px(3.0))
            .py(px(1.0))
            .rounded(px(3.0))
            .items_center()
            .when(ci_clickable, |d: Stateful<Div>| {
                d.cursor_pointer().hover(|s| s.bg(rgb(t.bg_hover)))
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .child(
                svg()
                    .path(icon)
                    .size(px(10.0))
                    .text_color(rgb(color)),
            )
            .tooltip(move |_window, cx| Tooltip::new(tooltip.clone()).build(_window, cx));
        if let Some(bcb) = on_ci_bounds {
            el = el.child(
                canvas(
                    move |bounds, _window, app| {
                        bcb(bounds, app);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );
        }
        if let Some(cb) = on_ci_click {
            el = el.on_click(move |_, window, cx| {
                cb(window, cx);
            });
        }
        el
    });

    h_flex()
        .gap(px(2.0))
        .child(branch_chip)
        .when_some(pr_badge, |d, badge| d.child(badge))
        .when_some(ci_pill, |d, pill| d.child(pill))
        .into_any_element()
}
