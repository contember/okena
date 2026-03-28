//! GitHeader — self-contained GPUI entity for git status display,
//! diff popover, and commit log popover in the project column header.
//!
//! Extracted from `ProjectColumn` to keep that view thin.

use okena_core::process::open_url;
use okena_core::types::DiffMode;
use okena_git::{
    self as git, CommitLogEntry, FileDiffSummary, GitStatus, GraphRow,
};
use okena_workspace::request_broker::RequestBroker;
use okena_workspace::requests::OverlayRequest;

use crate::diff_viewer::provider::GitProvider;
use crate::project_header;

use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use gpui_component::{h_flex, v_flex};
use okena_core::theme::ThemeColors;
use okena_ui::tokens::{ui_text_sm, ui_text_ms};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Delay before showing diff summary popover (ms)
const HOVER_DELAY_MS: u64 = 400;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum BranchPickerTarget {
    /// Picking branch to view graph for
    #[default]
    Graph,
    /// Picking base branch for compare
    CompareBase,
    /// Picking head branch for compare
    CompareHead,
}

/// Self-contained GPUI entity managing git status display, diff summary
/// popover, and commit log popover.
pub struct GitHeader {
    project_id: String,
    request_broker: Entity<RequestBroker>,
    git_provider: Arc<dyn GitProvider>,

    /// Current branch from git watcher (updated externally before rendering).
    current_branch: Option<String>,

    // ── Diff popover state ──────────────────────────────────────────
    diff_popover_visible: bool,
    diff_file_summaries: Vec<FileDiffSummary>,
    hover_token: Arc<AtomicU64>,
    diff_stats_bounds: Bounds<Pixels>,

    // ── Commit log state ────────────────────────────────────────────
    commit_log_visible: bool,
    commit_log_entries: Vec<GraphRow>,
    commit_log_loading: bool,
    commit_log_bounds: Bounds<Pixels>,
    commit_log_count: usize,
    commit_log_has_more: bool,
    commit_log_scroll: ScrollHandle,
    commit_log_branch: Option<String>,
    commit_log_branches: Vec<String>,
    commit_log_branch_picker: bool,
    commit_log_branch_filter: String,
    commit_log_compare_mode: bool,
    commit_log_compare_base: Option<String>,
    commit_log_compare_head: Option<String>,
    commit_log_picker_target: BranchPickerTarget,
}

const COMMIT_PAGE_SIZE: usize = 50;

impl GitHeader {
    pub fn new(
        project_id: String,
        request_broker: Entity<RequestBroker>,
        git_provider: Arc<dyn GitProvider>,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            project_id,
            request_broker,
            git_provider,
            current_branch: None,
            diff_popover_visible: false,
            diff_file_summaries: Vec::new(),
            hover_token: Arc::new(AtomicU64::new(0)),
            diff_stats_bounds: Bounds::default(),
            commit_log_visible: false,
            commit_log_entries: Vec::new(),
            commit_log_loading: false,
            commit_log_bounds: Bounds::default(),
            commit_log_count: 0,
            commit_log_has_more: false,
            commit_log_scroll: ScrollHandle::new(),
            commit_log_branch: None,
            commit_log_branches: Vec::new(),
            commit_log_branch_picker: false,
            commit_log_branch_filter: String::new(),
            commit_log_compare_mode: false,
            commit_log_compare_base: None,
            commit_log_compare_head: None,
            commit_log_picker_target: BranchPickerTarget::default(),
        }
    }

    /// Update the current branch name (from the git status watcher).
    pub fn set_current_branch(&mut self, branch: Option<String>) {
        self.current_branch = branch;
    }

    // ── Diff popover ────────────────────────────────────────────────

    fn show_diff_popover(&mut self, cx: &mut Context<Self>) {
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

    fn hide_diff_popover(&mut self, cx: &mut Context<Self>) {
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

    // ── Commit log ──────────────────────────────────────────────────

    fn toggle_commit_log(&mut self, cx: &mut Context<Self>) {
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

    fn hide_commit_log(&mut self, cx: &mut Context<Self>) {
        if self.commit_log_visible {
            self.commit_log_visible = false;
            cx.notify();
        }
    }

    // ── Rendering ───────────────────────────────────────────────────

    /// Render the git status bar (branch, commit log button, diff stats).
    ///
    /// `current_branch` is the branch name from the git status watcher
    /// (passed in because the watcher lives in the main app).
    pub fn render_git_status(
        &self,
        status: Option<GitStatus>,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entity_handle = cx.entity().clone();

        match status {
            Some(status) if status.branch.is_some() => {
                let has_changes = status.has_changes();
                let lines_added = status.lines_added;
                let lines_removed = status.lines_removed;
                let project_id = self.project_id.clone();

                h_flex()
                    .flex_shrink_0()
                    .gap(px(6.0))
                    .text_size(ui_text_sm(cx))
                    .line_height(px(12.0))
                    // Branch name + PR badge + CI status
                    .child({
                        let pr_url = status.pr_info.as_ref().map(|p| p.url.clone());
                        project_header::render_branch_status(
                            &status,
                            pr_url.map(|url| {
                                move |_: &mut Window, _: &mut App| {
                                    open_url(&url);
                                }
                            }),
                            t,
                        )
                    })
                    // Commit log button
                    .child({
                        let entity_for_bounds = entity_handle.clone();
                        div()
                            .id(ElementId::Name(format!("commit-log-btn-{}", project_id).into()))
                            .relative()
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(18.0))
                            .h(px(16.0))
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                cx.stop_propagation();
                                this.toggle_commit_log(cx);
                            }))
                            .child(
                                svg()
                                    .path("icons/git-commit.svg")
                                    .size(px(10.0))
                                    .text_color(rgb(t.text_muted)),
                            )
                            .child(
                                canvas(
                                    move |bounds, _window, app| {
                                        let _ = entity_for_bounds.update(app, |this: &mut GitHeader, _cx| {
                                            this.commit_log_bounds = bounds;
                                        });
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            )
                            .tooltip(move |_window, cx| Tooltip::new("Commit Log").build(_window, cx))
                    })
                    // Diff stats (clickable, only if there are changes)
                    .when(has_changes, |d: Div| {
                        let request_broker = self.request_broker.clone();
                        let project_id_for_click = self.project_id.clone();
                        d.child(
                            project_header::render_diff_stats_badge(lines_added, lines_removed, t)
                                .id(ElementId::Name(format!("git-diff-stats-{}", project_id).into()))
                                .relative()
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                                    if *hovered {
                                        this.show_diff_popover(cx);
                                    } else {
                                        this.hide_diff_popover(cx);
                                    }
                                }))
                                .on_click(cx.listener(move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.hide_diff_popover(cx);
                                    request_broker.update(cx, |broker, cx| {
                                        broker.push_overlay_request(OverlayRequest::DiffViewer {
                                            project_id: project_id_for_click.clone(),
                                            file: None,
                                            mode: None,
                                            commit_message: None,
                                            commits: None,
                                            commit_index: None,
                                        }, cx);
                                    });
                                }))
                                // Invisible canvas to capture bounds for popover positioning
                                .child(canvas(
                                    {
                                        let entity_handle = entity_handle.clone();
                                        move |bounds, _window, app| {
                                            entity_handle.update(app, |this, _cx| {
                                                this.diff_stats_bounds = bounds;
                                            });
                                        }
                                    },
                                    |_, _, _, _| {},
                                ).absolute().size_full())
                        )
                    })
                    .into_any_element()
            }
            _ => div().into_any_element(), // Not a git repo - show nothing
        }
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
                    broker.push_overlay_request(OverlayRequest::DiffViewer {
                        project_id: pid,
                        file: Some(file_path),
                        mode: None,
                        commit_message: None,
                        commits: None,
                        commit_index: None,
                    }, cx);
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
                            broker.push_overlay_request(OverlayRequest::DiffViewer {
                                project_id: project_id.clone(),
                                file: None,
                                mode: Some(DiffMode::Commit(commit_hash)),
                                commit_message: Some(commit_msg),
                                commits: Some(commits_vec),
                                commit_index: Some(commit_idx),
                            }, cx);
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

        div()
            .size_full()
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("commit-log-backdrop")
                    .absolute()
                    .inset_0()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.hide_commit_log(cx);
                    }))
                    .on_scroll_wheel(|_, _, cx| {
                        cx.stop_propagation();
                    }),
            )
            .child(
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
                                                                    let base = this.commit_log_compare_base.clone().unwrap();
                                                                    let head = this.commit_log_compare_head.clone().unwrap();
                                                                    this.hide_commit_log(cx);
                                                                    broker.update(cx, |broker, cx| {
                                                                        broker.push_overlay_request(OverlayRequest::DiffViewer {
                                                                            project_id: pid.clone(),
                                                                            file: None,
                                                                            mode: Some(DiffMode::BranchCompare {
                                                                                base,
                                                                                head,
                                                                            }),
                                                                            commit_message: None,
                                                                            commits: None,
                                                                            commit_index: None,
                                                                        }, cx);
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
                ),
            )
            .into_any_element()
    }
}
