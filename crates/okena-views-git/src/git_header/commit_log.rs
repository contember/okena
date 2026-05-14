//! Commit log popover — git graph with optional compare mode and an inline
//! branch picker. Anchored under the commit log button.

use super::{BranchPickerTarget, GitHeader};
use crate::project_header;

use okena_core::theme::ThemeColors;
use okena_core::types::DiffMode;
use okena_git::{self as git, CommitLogEntry, GraphRow};
use okena_ui::tokens::{ui_text_ms, ui_text_sm};
use okena_workspace::requests::{OverlayRequest, ProjectOverlay, ProjectOverlayKind};

use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};

use std::sync::Arc;

pub(super) const COMMIT_PAGE_SIZE: usize = 50;

impl GitHeader {
    pub(super) fn toggle_commit_log(&mut self, cx: &mut Context<Self>) {
        if self.commit_log_visible {
            self.commit_log_visible = false;
            cx.notify();
            return;
        }
        self.diff_popover_visible = false;

        self.commit_log_visible = true;
        self.commit_log_loading = true;
        self.commit_log_entries.clear();
        self.commit_log_count = 0;
        self.commit_log_has_more = false;
        self.commit_log_branch = None;
        self.commit_log_branch_picker = false;
        self.commit_log_branch_filter.clear();
        self.commit_log_compare_mode = false;
        self.commit_log_compare_base = None;
        self.commit_log_compare_head = None;
        self.commit_log_picker_target = BranchPickerTarget::Graph;
        cx.notify();

        let page = COMMIT_PAGE_SIZE;
        let provider = self.git_provider.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let (entries, branches) = smol::unblock(move || {
                let entries = provider.get_commit_graph(page, None);
                let branches = provider.list_branches();
                (entries, branches)
            })
            .await;

            let _ = this.update(cx, |this, cx| {
                this.commit_log_loading = false;
                let commit_count = entries.iter().filter(|r| matches!(r, git::GraphRow::Commit(_))).count();
                this.commit_log_has_more = commit_count >= page;
                this.commit_log_count = commit_count;
                this.commit_log_entries = entries;
                this.commit_log_branches = branches;
                cx.notify();
            });
        })
        .detach();
    }

    fn switch_commit_log_branch(&mut self, branch: Option<String>, cx: &mut Context<Self>) {
        self.commit_log_branch = branch.clone();
        self.commit_log_branch_picker = false;
        self.commit_log_branch_filter.clear();
        self.commit_log_loading = true;
        self.commit_log_entries.clear();
        self.commit_log_count = 0;
        self.commit_log_has_more = false;
        cx.notify();

        let provider = self.git_provider.clone();
        let page = COMMIT_PAGE_SIZE;

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let entries = smol::unblock(move || {
                provider.get_commit_graph(page, branch.as_deref())
            })
            .await;

            let _ = this.update(cx, |this, cx| {
                this.commit_log_loading = false;
                let commit_count = entries.iter().filter(|r| matches!(r, git::GraphRow::Commit(_))).count();
                this.commit_log_has_more = commit_count >= page;
                this.commit_log_count = commit_count;
                this.commit_log_entries = entries;
                cx.notify();
            });
        })
        .detach();
    }

    fn load_more_commits(&mut self, cx: &mut Context<Self>) {
        if self.commit_log_loading || !self.commit_log_has_more {
            return;
        }

        self.commit_log_loading = true;
        cx.notify();

        let provider = self.git_provider.clone();
        let branch = self.commit_log_branch.clone();
        let already_loaded = self.commit_log_count;
        let page = COMMIT_PAGE_SIZE;
        let new_total = already_loaded + page;

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let entries = smol::unblock(move || {
                provider.get_commit_graph(new_total, branch.as_deref())
            })
            .await;

            let _ = this.update(cx, |this, cx| {
                this.commit_log_loading = false;
                let commit_count = entries.iter().filter(|r| matches!(r, git::GraphRow::Commit(_))).count();
                this.commit_log_has_more = commit_count >= new_total;
                this.commit_log_count = commit_count;
                this.commit_log_entries = entries;
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn hide_commit_log(&mut self, cx: &mut Context<Self>) {
        if self.commit_log_visible {
            self.commit_log_visible = false;
            cx.notify();
        }
    }

    /// Render the commit log popover (anchored below the commit log button).
    ///
    /// `current_branch` is the branch name from the git status watcher.
    pub fn render_commit_log_popover(
        &self,
        current_branch: Option<String>,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.commit_log_visible {
            return div().size_0().into_any_element();
        }

        let bounds = self.commit_log_bounds;
        let position = point(
            bounds.origin.x - px(8.0),
            bounds.origin.y + bounds.size.height + px(6.0),
        );

        let branch_name = current_branch;

        let content = {
            let entity_handle = cx.entity().clone();
            let project_id = self.project_id.clone();
            let request_broker = self.request_broker.clone();
            let on_commit_click: Option<Arc<dyn Fn(&str, &str, usize, &mut Window, &mut App)>> =
                if self.commit_log_entries.is_empty() {
                    None
                } else {
                    let all_commits: Vec<CommitLogEntry> = self.commit_log_entries.iter()
                        .filter_map(|r| match r { GraphRow::Commit(e) => Some(e.clone()), _ => None })
                        .collect();
                    Some(Arc::new(move |hash: &str, msg: &str, _commit_idx: usize, _window: &mut Window, cx: &mut App| {
                        let commit_hash = hash.to_string();
                        let commit_msg = msg.to_string();
                        let commits_vec = all_commits.clone();
                        let commit_idx = commits_vec.iter().position(|c| c.hash == commit_hash).unwrap_or(0);
                        let _ = entity_handle.update(cx, |this: &mut GitHeader, cx| {
                            this.hide_commit_log(cx);
                        });
                        request_broker.update(cx, |broker, cx| {
                            broker.push_overlay_request(OverlayRequest::Project(ProjectOverlay {
                                project_id: project_id.clone(),
                                kind: ProjectOverlayKind::DiffViewer {
                                    file: None,
                                    mode: Some(DiffMode::Commit(commit_hash)),
                                    commit_message: Some(commit_msg),
                                    commits: Some(commits_vec),
                                    commit_index: Some(commit_idx),
                                },
                            }), cx);
                        });
                    }))
                };
            project_header::render_commit_log_content(
                &self.commit_log_entries,
                self.commit_log_loading,
                on_commit_click,
                t,
                cx,
            )
        };

        deferred(
            anchored()
                .position(position)
                .snap_to_window()
                .child(
                    v_flex()
                        .id("commit-log-popover")
                        .occlude()
                        .w(px(520.0))
                        .max_h(px(420.0))
                        .bg(rgb(t.bg_primary))
                        .border_1()
                        .border_color(rgb(t.border))
                        .rounded(px(8.0))
                        .shadow_lg()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.hide_commit_log(cx);
                        }))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_scroll_wheel(|_, _, cx| {
                            cx.stop_propagation();
                        })
                        // Header
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
                                        .path("icons/git-commit.svg")
                                        .size(px(11.0))
                                        .text_color(rgb(t.text_muted)),
                                )
                                .child(
                                    div()
                                        .text_size(ui_text_ms(cx))
                                        .text_color(rgb(t.text_secondary))
                                        .child("GRAPH"),
                                )
                                // Right side: Compare toggle + branch selector
                                .child({
                                    let display_branch = self.commit_log_branch.clone()
                                        .or(branch_name);
                                    let is_compare = self.commit_log_compare_mode;
                                    h_flex()
                                        .flex_1()
                                        .justify_end()
                                        .gap(px(4.0))
                                        .items_center()
                                        // Compare toggle
                                        .child(
                                            div()
                                                .id("commit-log-compare-toggle")
                                                .cursor_pointer()
                                                .px(px(6.0))
                                                .py(px(2.0))
                                                .rounded(px(4.0))
                                                .bg(rgb(if is_compare { t.bg_selection } else { t.bg_hover }))
                                                .hover(|s| s.bg(rgb(t.bg_selection)))
                                                .text_size(ui_text_sm(cx))
                                                .text_color(rgb(if is_compare { t.term_cyan } else { t.text_muted }))
                                                .on_mouse_down(MouseButton::Left, |_, _, cx| { cx.stop_propagation(); })
                                                .on_click(cx.listener(|this, _, _window, cx| {
                                                    this.commit_log_compare_mode = !this.commit_log_compare_mode;
                                                    if this.commit_log_compare_mode {
                                                        // Pre-fill base with current branch
                                                        this.commit_log_compare_base = this.current_branch.clone();
                                                        this.commit_log_compare_head = this.commit_log_branch.clone();
                                                    }
                                                    this.commit_log_branch_picker = false;
                                                    cx.notify();
                                                }))
                                                .child("Compare"),
                                        )
                                        // Branch selector pill (only when not in compare mode)
                                        .when(!is_compare, |d| {
                                            d.when_some(display_branch, |d, name| {
                                                d.child(
                                                    h_flex()
                                                        .id("commit-log-branch-btn")
                                                        .gap(px(4.0))
                                                        .items_center()
                                                        .px(px(6.0))
                                                        .py(px(2.0))
                                                        .rounded(px(4.0))
                                                        .bg(rgb(t.bg_hover))
                                                        .cursor_pointer()
                                                        .hover(|s| s.bg(rgb(t.bg_selection)))
                                                        .on_mouse_down(MouseButton::Left, |_, _, cx| { cx.stop_propagation(); })
                                                        .on_click(cx.listener(|this, _, _window, cx| {
                                                            this.commit_log_picker_target = BranchPickerTarget::Graph;
                                                            this.commit_log_branch_picker = !this.commit_log_branch_picker;
                                                            this.commit_log_branch_filter.clear();
                                                            cx.notify();
                                                        }))
                                                        .child(svg().path("icons/git-branch.svg").size(px(10.0)).text_color(rgb(t.term_green)))
                                                        .child(
                                                            div().text_size(ui_text_sm(cx)).text_color(rgb(t.text_secondary))
                                                                .max_w(px(140.0)).text_ellipsis().overflow_hidden().child(name),
                                                        ),
                                                )
                                            })
                                        })
                                }),
                        )
                        // Compare bar — two branch selectors + view diff button
                        .when(self.commit_log_compare_mode, |d| {
                            let base = self.commit_log_compare_base.clone();
                            let head = self.commit_log_compare_head.clone();
                            let pid = self.project_id.clone();
                            let broker = self.request_broker.clone();
                            let both_selected = base.is_some() && head.is_some();
                            d.child(
                                h_flex()
                                    .px(px(10.0))
                                    .py(px(6.0))
                                    .gap(px(6.0))
                                    .items_center()
                                    .border_b_1()
                                    .border_color(rgb(t.border))
                                    // Base branch pill
                                    .child(
                                        div()
                                            .id("compare-base-btn")
                                            .cursor_pointer()
                                            .px(px(6.0))
                                            .py(px(2.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(t.bg_hover))
                                            .hover(|s| s.bg(rgb(t.bg_selection)))
                                            .text_size(ui_text_sm(cx))
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| { cx.stop_propagation(); })
                                            .on_click(cx.listener(|this, _, _window, cx| {
                                                this.commit_log_picker_target = BranchPickerTarget::CompareBase;
                                                this.commit_log_branch_picker = !this.commit_log_branch_picker;
                                                this.commit_log_branch_filter.clear();
                                                cx.notify();
                                            }))
                                            .child(
                                                h_flex().gap(px(3.0)).items_center()
                                                    .child(svg().path("icons/git-branch.svg").size(px(9.0)).text_color(rgb(t.term_green)))
                                                    .child(
                                                        div().text_color(rgb(t.text_secondary))
                                                            .max_w(px(120.0)).text_ellipsis().overflow_hidden()
                                                            .child(base.clone().unwrap_or_else(|| "base...".to_string())),
                                                    ),
                                            ),
                                    )
                                    // Arrow
                                    .child(div().text_size(ui_text_sm(cx)).text_color(rgb(t.text_muted)).child("\u{2192}"))
                                    // Head branch pill
                                    .child(
                                        div()
                                            .id("compare-head-btn")
                                            .cursor_pointer()
                                            .px(px(6.0))
                                            .py(px(2.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(t.bg_hover))
                                            .hover(|s| s.bg(rgb(t.bg_selection)))
                                            .text_size(ui_text_sm(cx))
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| { cx.stop_propagation(); })
                                            .on_click(cx.listener(|this, _, _window, cx| {
                                                this.commit_log_picker_target = BranchPickerTarget::CompareHead;
                                                this.commit_log_branch_picker = !this.commit_log_branch_picker;
                                                this.commit_log_branch_filter.clear();
                                                cx.notify();
                                            }))
                                            .child(
                                                h_flex().gap(px(3.0)).items_center()
                                                    .child(svg().path("icons/git-branch.svg").size(px(9.0)).text_color(rgb(t.term_cyan)))
                                                    .child(
                                                        div().text_color(rgb(t.text_secondary))
                                                            .max_w(px(120.0)).text_ellipsis().overflow_hidden()
                                                            .child(head.clone().unwrap_or_else(|| "head...".to_string())),
                                                    ),
                                            ),
                                    )
                                    // View Diff button
                                    .child(
                                        div()
                                            .flex_1()
                                            .flex()
                                            .justify_end()
                                            .child(
                                                div()
                                                    .id("compare-view-diff")
                                                    .cursor_pointer()
                                                    .px(px(8.0))
                                                    .py(px(3.0))
                                                    .rounded(px(4.0))
                                                    .when(both_selected, |d| {
                                                        d.bg(rgb(t.term_cyan))
                                                            .text_color(rgb(t.bg_primary))
                                                            .hover(|s| s.opacity(0.9))
                                                    })
                                                    .when(!both_selected, |d| {
                                                        d.bg(rgb(t.bg_hover))
                                                            .text_color(rgb(t.text_muted))
                                                    })
                                                    .text_size(ui_text_sm(cx))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .on_mouse_down(MouseButton::Left, |_, _, cx| { cx.stop_propagation(); })
                                                    .when(both_selected, |d| {
                                                        d.on_click(cx.listener(move |this, _, _window, cx| {
                                                            let (Some(base), Some(head)) = (
                                                                this.commit_log_compare_base.clone(),
                                                                this.commit_log_compare_head.clone(),
                                                            ) else {
                                                                return;
                                                            };
                                                            this.hide_commit_log(cx);
                                                            broker.update(cx, |broker, cx| {
                                                                broker.push_overlay_request(OverlayRequest::Project(ProjectOverlay {
                                                                    project_id: pid.clone(),
                                                                    kind: ProjectOverlayKind::DiffViewer {
                                                                        file: None,
                                                                        mode: Some(DiffMode::BranchCompare {
                                                                            base,
                                                                            head,
                                                                        }),
                                                                        commit_message: None,
                                                                        commits: None,
                                                                        commit_index: None,
                                                                    },
                                                                }), cx);
                                                            });
                                                        }))
                                                    })
                                                    .child("View Diff"),
                                            ),
                                    ),
                            )
                        })
                        // Branch picker (inline, between header and commit list)
                        .when(self.commit_log_branch_picker, |d| {
                            let filter = self.commit_log_branch_filter.to_lowercase();
                            let filtered: Vec<&String> = self.commit_log_branches.iter()
                                .filter(|b| filter.is_empty() || b.to_lowercase().contains(&filter))
                                .collect();
                            d.child(
                                v_flex()
                                    .border_b_1()
                                    .border_color(rgb(t.border))
                                    .max_h(px(200.0))
                                    // Filter input
                                    .child(
                                        div()
                                            .px(px(10.0))
                                            .py(px(6.0))
                                            .child(
                                                div()
                                                    .px(px(8.0))
                                                    .py(px(4.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(t.bg_secondary))
                                                    .text_size(ui_text_ms(cx))
                                                    .text_color(rgb(t.text_primary))
                                                    .child(
                                                        if filter.is_empty() {
                                                            format!("{} branches", self.commit_log_branches.len())
                                                        } else {
                                                            format!("\"{}\" \u{2014} {} matches", self.commit_log_branch_filter, filtered.len())
                                                        }
                                                    ),
                                            ),
                                    )
                                    // Branch list
                                    .child(
                                        div()
                                            .id("branch-picker-scroll")
                                            .flex_1()
                                            .min_h_0()
                                            .overflow_y_scroll()
                                            .children(
                                                filtered.iter().enumerate().map(|(i, branch)| {
                                                    let b = (*branch).clone();
                                                    let target = self.commit_log_picker_target;
                                                    let is_selected = match target {
                                                        BranchPickerTarget::Graph => self.commit_log_branch.as_ref() == Some(*branch),
                                                        BranchPickerTarget::CompareBase => self.commit_log_compare_base.as_ref() == Some(*branch),
                                                        BranchPickerTarget::CompareHead => self.commit_log_compare_head.as_ref() == Some(*branch),
                                                    };
                                                    div()
                                                        .id(ElementId::Name(format!("branch-{}-{}", i, branch).into()))
                                                        .px(px(10.0))
                                                        .py(px(3.0))
                                                        .cursor_pointer()
                                                        .text_size(ui_text_ms(cx))
                                                        .text_color(rgb(if is_selected { t.text_primary } else { t.text_secondary }))
                                                        .when(is_selected, |d| d.font_weight(FontWeight::SEMIBOLD))
                                                        .hover(|s| s.bg(rgb(t.bg_hover)))
                                                        .on_click(cx.listener(move |this, _, _window, cx| {
                                                            match target {
                                                                BranchPickerTarget::Graph => {
                                                                    this.switch_commit_log_branch(Some(b.clone()), cx);
                                                                }
                                                                BranchPickerTarget::CompareBase => {
                                                                    this.commit_log_compare_base = Some(b.clone());
                                                                    this.commit_log_branch_picker = false;
                                                                    cx.notify();
                                                                }
                                                                BranchPickerTarget::CompareHead => {
                                                                    this.commit_log_compare_head = Some(b.clone());
                                                                    this.commit_log_branch_picker = false;
                                                                    cx.notify();
                                                                }
                                                            }
                                                        }))
                                                        .child((*branch).clone())
                                                        .into_any_element()
                                                }),
                                            ),
                                    ),
                            )
                        })
                        // Scrollable commit list
                        .child(
                            div()
                                .id("commit-log-scroll")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .track_scroll(&self.commit_log_scroll)
                                .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                                    let delta_y = f32::from(event.delta.pixel_delta(px(1.0)).y);
                                    if delta_y >= 0.0 {
                                        return;
                                    }
                                    if !this.commit_log_has_more || this.commit_log_loading {
                                        return;
                                    }
                                    let row_count = this.commit_log_entries.len();
                                    let est_content_h = row_count as f32 * 20.0;
                                    let scroll_y = -f32::from(this.commit_log_scroll.offset().y);
                                    let viewport_h = 380.0;
                                    if scroll_y + viewport_h > est_content_h - 200.0 {
                                        this.load_more_commits(cx);
                                    }
                                }))
                                .py(px(4.0))
                                .child(content),
                        ),
                ),
        )
        .into_any_element()
    }
}
