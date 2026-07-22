//! CI checks popover — list of CI runs for the current branch's HEAD commit.
//! When a PR exists, the header doubles as a link to the PR on GitHub.
//! When there's no PR (e.g. on the default branch), the popover still shows
//! the branch-level check-runs / statuses fetched for the HEAD commit.

use super::GitHeader;
use crate::project_header::{CiStatusColor, PrStateColor};

use okena_core::process::open_url;
use okena_core::theme::ThemeColors;
use okena_git as git;
use okena_ui::tokens::{ui_text_ms, ui_text_sm};

use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use gpui_component::{h_flex, v_flex};

impl GitHeader {
    /// Toggle the CI checks popover. Caller is responsible for ensuring
    /// the CI pill is actually rendered (otherwise the popover anchors
    /// to stale bounds).
    pub fn toggle_ci_checks(&mut self, cx: &mut Context<Self>) {
        self.ci_checks_visible = !self.ci_checks_visible;
        if self.ci_checks_visible {
            // Hide siblings so they don't overlap. Route through hide_branch_picker
            // so the modal focus context is restored — otherwise the previously
            // focused terminal stays "stolen" by the picker.
            self.diff_popover_visible = false;
            self.commit_log_visible = false;
            self.hide_branch_picker(cx);
        }
        cx.notify();
    }

    pub(super) fn hide_ci_checks(&mut self, cx: &mut Context<Self>) {
        if !self.ci_checks_visible {
            return;
        }
        self.ci_checks_visible = false;
        cx.notify();
    }

    /// Record the on-screen bounds of the CI pill so the popover can anchor
    /// underneath it. Change-detected to avoid notify churn.
    pub fn set_ci_badge_bounds(&mut self, bounds: Bounds<Pixels>) {
        if self.ci_badge_bounds != bounds {
            self.ci_badge_bounds = bounds;
        }
    }

    /// Render the CI checks popover anchored under the CI pill. Returns a
    /// zero-size element when hidden or when there's no CI summary.
    pub fn render_ci_checks_popover(
        &self,
        ci_summary: Option<&git::CiCheckSummary>,
        pr_info: Option<&git::PrInfo>,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.ci_checks_visible {
            return div().size_0().into_any_element();
        }
        let Some(summary) = ci_summary else {
            return div().size_0().into_any_element();
        };

        let bounds = self.ci_badge_bounds;
        let position = point(
            bounds.origin.x,
            bounds.origin.y + bounds.size.height + px(6.0),
        );

        let pr_number = pr_info.map(|p| p.number);
        let pr_url = pr_info.map(|p| p.url.clone());
        let pr_state_label = pr_info.map(|p| p.state.label());
        let pr_state_color = pr_info.map(|p| p.state.color(t));
        let summary_tooltip = summary.tooltip_text();
        let summary_status_color = summary.status.color(t);
        let summary_status_icon = summary.status.icon();
        let checks: Vec<git::CiCheck> = summary.checks.clone();

        let row = |check: git::CiCheck, key: String, cx: &mut Context<Self>| -> AnyElement {
            let link = check.link.clone();
            let elapsed = check.elapsed_label();
            let workflow = check.workflow.clone();
            let description = check.description.clone();
            let icon_path = if check.is_skipped {
                "icons/eye-off.svg"
            } else {
                check.status.icon()
            };
            let icon_color = if check.is_skipped {
                t.text_muted
            } else {
                check.status.color(t)
            };
            let is_clickable = link.is_some();
            let mut el = h_flex()
                .id(ElementId::Name(key.into()))
                .px(px(10.0))
                .py(px(4.0))
                .gap(px(8.0))
                .items_center()
                .text_size(ui_text_ms(cx))
                .when(is_clickable, |d: Stateful<Div>| {
                    d.cursor_pointer().hover(|s| s.bg(rgb(t.bg_hover)))
                })
                .child(
                    svg()
                        .path(icon_path)
                        .size(px(10.0))
                        .text_color(rgb(icon_color)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(1.0))
                        .child(
                            div()
                                .text_color(rgb(t.text_primary))
                                .text_ellipsis()
                                .overflow_hidden()
                                .child(check.name.clone()),
                        )
                        .when_some(workflow, |d, wf| {
                            d.child(
                                div()
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.text_muted))
                                    .text_ellipsis()
                                    .overflow_hidden()
                                    .child(wf),
                            )
                        }),
                )
                .child(
                    div()
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.text_muted))
                        .flex_shrink_0()
                        .child(elapsed),
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                });
            if let Some(desc) = description {
                el = el.tooltip(move |_window, cx| Tooltip::new(desc.clone()).build(_window, cx));
            }
            if let Some(url) = link {
                el = el.on_click(move |_, _window, _cx| {
                    open_url(&url);
                });
            }
            el.into_any_element()
        };

        // Header: either a PR badge linking to the PR, or a branch-only
        // "Checks" label when no PR exists.
        let summary_tooltip_for_header = summary_tooltip.clone();
        let header = if let (Some(num), Some(label), Some(color)) =
            (pr_number, pr_state_label, pr_state_color)
        {
            h_flex()
                .px(px(10.0))
                .py(px(6.0))
                .gap(px(6.0))
                .items_center()
                .border_b_1()
                .border_color(rgb(t.border))
                .child(
                    svg()
                        .path("icons/git-pull-request.svg")
                        .size(px(11.0))
                        .text_color(rgb(color)),
                )
                .child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_secondary))
                        .child(format!("#{} \u{2014} {}", num, label)),
                )
                .child(
                    div()
                        .flex_1()
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.text_muted))
                        .text_ellipsis()
                        .overflow_hidden()
                        .child(summary_tooltip_for_header),
                )
        } else {
            h_flex()
                .px(px(10.0))
                .py(px(6.0))
                .gap(px(6.0))
                .items_center()
                .border_b_1()
                .border_color(rgb(t.border))
                .child(
                    svg()
                        .path(summary_status_icon)
                        .size(px(11.0))
                        .text_color(rgb(summary_status_color)),
                )
                .child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_secondary))
                        .child("Checks"),
                )
                .child(
                    div()
                        .flex_1()
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.text_muted))
                        .text_ellipsis()
                        .overflow_hidden()
                        .child(summary_tooltip_for_header),
                )
        };

        // Footer "Open on GitHub" — only when we have a PR URL to open.
        let footer = pr_url.map(|url| {
            h_flex()
                .px(px(10.0))
                .py(px(6.0))
                .justify_end()
                .border_t_1()
                .border_color(rgb(t.border))
                .child(
                    div()
                        .id("ci-checks-open-github")
                        .cursor_pointer()
                        .px(px(8.0))
                        .py(px(3.0))
                        .rounded(px(4.0))
                        .hover(|s| s.bg(rgb(t.bg_hover)))
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.text_secondary))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            open_url(&url);
                            this.hide_ci_checks(cx);
                        }))
                        .child("Open on GitHub \u{2197}"),
                )
        });

        deferred(
            anchored().position(position).snap_to_window().child(
                v_flex()
                    .id("ci-checks-popover")
                    .occlude()
                    .w(px(360.0))
                    .max_h(px(420.0))
                    .bg(rgb(t.bg_primary))
                    .border_1()
                    .border_color(rgb(t.border))
                    .rounded(px(8.0))
                    .shadow_lg()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.hide_ci_checks(cx);
                    }))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_scroll_wheel(|_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(header)
                    .child({
                        let body = v_flex()
                            .id("ci-checks-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .py(px(4.0));
                        if checks.is_empty() {
                            body.child(
                                div()
                                    .px(px(10.0))
                                    .py(px(8.0))
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child("No checks reported"),
                            )
                        } else {
                            body.children(
                                checks
                                    .into_iter()
                                    .enumerate()
                                    .map(|(i, c)| row(c, format!("ci-check-{}", i), cx)),
                            )
                        }
                    })
                    .when_some(footer, |d, f| d.child(f)),
            ),
        )
        .into_any_element()
    }
}
