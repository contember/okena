//! Git branch status pill: branch chip + optional PR badge + CI status.

use super::{CiStatusColor, PrStateColor};

use okena_core::theme::ThemeColors;
use okena_git::GitStatus;

use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use gpui_component::tooltip::Tooltip;
use std::sync::Arc;

/// Callbacks for the interactive parts of the branch status pill.
///
/// The branch chip (icon + name + chevron) and the PR badge (#num + CI) are
/// rendered as two adjacent clickable regions so they can dispatch different
/// actions: branch chip opens the branch switcher; PR badge opens the PR URL.
pub struct BranchStatusCallbacks {
    /// Called when the user clicks the branch chip. When `None` the chip is
    /// rendered as plain (non-clickable) text — used for read-only providers.
    pub on_branch_click: Option<Arc<dyn Fn(&mut Window, &mut App)>>,
    /// Called when the user clicks the PR badge. When `None` the PR badge
    /// stays informational only (still rendered, but not clickable).
    pub on_pr_click: Option<Arc<dyn Fn(&mut Window, &mut App)>>,
    /// Called every layout pass with the on-screen bounds of the branch chip,
    /// so the caller can anchor a popover underneath it.
    pub on_branch_bounds: Option<Arc<dyn Fn(Bounds<Pixels>, &mut App)>>,
    /// Same as `on_branch_bounds` but for the PR badge — used to anchor the
    /// PR checks popover.
    pub on_pr_bounds: Option<Arc<dyn Fn(Bounds<Pixels>, &mut App)>>,
}

/// Render the git branch status pill (branch chip + PR badge + CI status).
///
/// The branch chip and PR badge are rendered as two separate clickable
/// regions so the caller can wire different actions to each — typically a
/// branch switcher popover and the PR URL respectively.
pub fn render_branch_status(
    status: &GitStatus,
    callbacks: BranchStatusCallbacks,
    t: &ThemeColors,
) -> AnyElement {
    let pr_info = status.pr_info.clone();
    let pr_number = pr_info.as_ref().map(|p| p.number);
    let pr_state_color = pr_info.as_ref().map(|p| p.state.color(t));
    let ci_checks = pr_info.as_ref().and_then(|p| p.ci_checks.clone());

    let BranchStatusCallbacks {
        on_branch_click,
        on_pr_click,
        on_branch_bounds,
        on_pr_bounds,
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

    // PR badge — only when a PR exists; clickable when an on_pr_click is
    // provided (PR URL open).
    let render_pr_badge = pr_number.is_some();
    let pr_badge = if render_pr_badge {
        let pr_clickable = on_pr_click.is_some();
        let badge_color = pr_state_color.unwrap_or(t.text_muted);
        let mut el = h_flex()
            .id("pr-badge")
            .relative()
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
            .when_some(pr_number, |d, num| {
                d.child(div().text_color(rgb(t.text_muted)).child(format!("#{num}")))
            })
            .when_some(ci_checks, |d, checks| {
                let tooltip = checks.tooltip_text();
                d.child(
                    div()
                        .id("ci-status")
                        .child(
                            svg()
                                .path(checks.status.icon())
                                .size(px(8.0))
                                .text_color(rgb(checks.status.color(t))),
                        )
                        .tooltip(move |_window, cx| Tooltip::new(tooltip.clone()).build(_window, cx)),
                )
            });
        if let Some(bcb) = on_pr_bounds {
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
        if let Some(cb) = on_pr_click {
            el = el.on_click(move |_, window, cx| {
                cb(window, cx);
            });
        }
        Some(el)
    } else {
        None
    };

    h_flex()
        .gap(px(2.0))
        .child(branch_chip)
        .when_some(pr_badge, |d, badge| d.child(badge))
        .into_any_element()
}
