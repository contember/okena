//! PR checks popover — list of CI checks for the current PR, anchored
//! under the PR badge in the project header.

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
    /// Toggle the PR checks popover. Caller is responsible for ensuring
    /// the PR badge is actually rendered (otherwise the popover anchors
    /// to stale bounds).
    pub fn toggle_pr_checks(&mut self, cx: &mut Context<Self>) {
        self.pr_checks_visible = !self.pr_checks_visible;
        if self.pr_checks_visible {
            // Hide siblings so they don't overlap. Route through hide_branch_picker
            // so the modal focus context is restored — otherwise the previously
            // focused terminal stays "stolen" by the picker.
            self.diff_popover_visible = false;
            self.commit_log_visible = false;
            self.hide_branch_picker(cx);
        }
        cx.notify();
    }

    pub(super) fn hide_pr_checks(&mut self, cx: &mut Context<Self>) {
        if !self.pr_checks_visible {
            return;
        }
        self.pr_checks_visible = false;
        cx.notify();
    }

    /// Record the on-screen bounds of the PR badge so the checks popover
    /// can anchor underneath it. Change-detected to avoid notify churn.
    pub fn set_pr_badge_bounds(&mut self, bounds: Bounds<Pixels>) {
        if self.pr_badge_bounds != bounds {
            self.pr_badge_bounds = bounds;
        }
    }

    /// Render the PR checks popover anchored under the PR badge. Returns a
    /// zero-size element when hidden or when there's no PR info.
    pub fn render_pr_checks_popover(
        &self,
        pr_info: Option<&git::PrInfo>,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.pr_checks_visible {
            return div().size_0().into_any_element();
        }
        let Some(pr) = pr_info else {
            return div().size_0().into_any_element();
        };

        let bounds = self.pr_badge_bounds;
        let position = point(
            bounds.origin.x,
            bounds.origin.y + bounds.size.height + px(6.0),
        );

        let pr_number = pr.number;
        let pr_url = pr.url.clone();
        let summary = pr.ci_checks.clone();
        let pr_state_label = pr.state.label();
        let pr_state_color = pr.state.color(t);
        let summary_tooltip = summary.as_ref().map(|s| s.tooltip_text());
        let checks: Vec<git::CiCheck> = summary
            .as_ref()
            .map(|s| s.checks.clone())
            .unwrap_or_default();

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

        deferred(
            anchored()
                .position(position)
                .snap_to_window()
                .child(
                    v_flex()
                        .id("pr-checks-popover")
                        .occlude()
                        .w(px(360.0))
                        .max_h(px(420.0))
                        .bg(rgb(t.bg_primary))
                        .border_1()
                        .border_color(rgb(t.border))
                        .rounded(px(8.0))
                        .shadow_lg()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.hide_pr_checks(cx);
                        }))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_scroll_wheel(|_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(
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
                                        .text_color(rgb(pr_state_color)),
                                )
                                .child(
                                    div()
                                        .text_size(ui_text_ms(cx))
                                        .text_color(rgb(t.text_secondary))
                                        .child(format!("#{} \u{2014} {}", pr_number, pr_state_label)),
                                )
                                .when_some(summary_tooltip, |d, label| {
                                    d.child(
                                        div()
                                            .flex_1()
                                            .text_size(ui_text_sm(cx))
                                            .text_color(rgb(t.text_muted))
                                            .text_ellipsis()
                                            .overflow_hidden()
                                            .child(label),
                                    )
                                }),
                        )
                        .child({
                            let body = v_flex()
                                .id("pr-checks-scroll")
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
                                    checks.into_iter().enumerate().map(|(i, c)| {
                                        row(c, format!("pr-check-{}", i), cx)
                                    }),
                                )
                            }
                        })
                        .child(
                            h_flex()
                                .px(px(10.0))
                                .py(px(6.0))
                                .justify_end()
                                .border_t_1()
                                .border_color(rgb(t.border))
                                .child(
                                    div()
                                        .id("pr-checks-open-github")
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
                                            open_url(&pr_url);
                                            this.hide_pr_checks(cx);
                                        }))
                                        .child("Open on GitHub \u{2197}"),
                                ),
                        ),
                ),
        )
        .into_any_element()
    }
}
