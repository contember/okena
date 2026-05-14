//! Diff summary popover — shown on hover over the diff stats badge in the
//! project header. Lists changed files with click-to-open.

use super::GitHeader;
use crate::project_header;

use okena_core::theme::ThemeColors;
use okena_workspace::requests::{OverlayRequest, ProjectOverlay, ProjectOverlayKind};

use gpui::prelude::*;
use gpui::*;

use std::sync::atomic::Ordering;
use std::time::Duration;

/// Delay before showing diff summary popover (ms)
pub(super) const HOVER_DELAY_MS: u64 = 400;

impl GitHeader {
    pub(super) fn show_diff_popover(&mut self, cx: &mut Context<Self>) {
        if self.diff_popover_visible {
            return;
        }

        let token = self.hover_token.fetch_add(1, Ordering::SeqCst) + 1;
        let hover_token = self.hover_token.clone();
        let provider = self.git_provider.clone();

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            smol::Timer::after(Duration::from_millis(HOVER_DELAY_MS)).await;

            if hover_token.load(Ordering::SeqCst) != token {
                return;
            }

            let summaries = smol::unblock(move || provider.get_diff_file_summary()).await;

            let _ = this.update(cx, |this, cx| {
                if hover_token.load(Ordering::SeqCst) == token && !summaries.is_empty() {
                    this.diff_file_summaries = summaries;
                    this.diff_popover_visible = true;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn hide_diff_popover(&mut self, cx: &mut Context<Self>) {
        let token = self.hover_token.fetch_add(1, Ordering::SeqCst) + 1;

        if !self.diff_popover_visible {
            return;
        }

        let hover_token = self.hover_token.clone();

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            smol::Timer::after(Duration::from_millis(100)).await;

            if hover_token.load(Ordering::SeqCst) != token {
                return;
            }

            let _ = this.update(cx, |this, cx| {
                if hover_token.load(Ordering::SeqCst) == token && this.diff_popover_visible {
                    this.diff_popover_visible = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Render the diff summary popover (anchored below the diff stats badge).
    pub fn render_diff_popover(&self, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        if !self.diff_popover_visible || self.diff_file_summaries.is_empty() {
            return div().size_0().into_any_element();
        }

        let entity_handle = cx.entity().clone();
        let request_broker = self.request_broker.clone();
        let project_id = self.project_id.clone();
        let tree_elements = project_header::render_diff_file_list_interactive(
            &self.diff_file_summaries,
            move |file_path, _window, cx| {
                let file_path = file_path.to_string();
                let pid = project_id.clone();
                let _ = entity_handle.update(cx, |this: &mut GitHeader, cx| {
                    this.hide_diff_popover(cx);
                });
                request_broker.update(cx, |broker, cx| {
                    broker.push_overlay_request(OverlayRequest::Project(ProjectOverlay {
                        project_id: pid,
                        kind: ProjectOverlayKind::DiffViewer {
                            file: Some(file_path),
                            mode: None,
                            commit_message: None,
                            commits: None,
                            commit_index: None,
                        },
                    }), cx);
                });
            },
            t,
            cx,
        );

        let bounds = self.diff_stats_bounds;
        let position = point(
            bounds.origin.x,
            bounds.origin.y + bounds.size.height + px(4.0),
        );

        deferred(
            anchored()
                .position(position)
                .snap_to_window()
                .child(
                    div()
                        .id("diff-summary-popover")
                        .occlude()
                        .min_w(px(280.0))
                        .max_w(px(400.0))
                        .max_h(px(300.0))
                        .overflow_y_scroll()
                        .bg(rgb(t.bg_primary))
                        .border_1()
                        .border_color(rgb(t.border))
                        .rounded(px(6.0))
                        .shadow_lg()
                        .py(px(6.0))
                        .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                            if *hovered {
                                this.hover_token.fetch_add(1, Ordering::SeqCst);
                            } else {
                                this.hide_diff_popover(cx);
                            }
                        }))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_scroll_wheel(|_, _, cx| {
                            cx.stop_propagation();
                        })
                        .children(tree_elements),
                ),
        )
        .into_any_element()
    }
}
