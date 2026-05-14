//! GitHeader — self-contained GPUI entity for git status display,
//! diff popover, and commit log popover in the project column header.
//!
//! Extracted from `ProjectColumn` to keep that view thin.

use okena_core::process::open_url;
use okena_core::types::DiffMode;
use okena_git::{
    self as git, BranchList, CommitLogEntry, FileDiffSummary, GitStatus, GraphRow,
};
use okena_workspace::request_broker::RequestBroker;
use okena_workspace::requests::{OverlayRequest, ProjectOverlay, ProjectOverlayKind};
use okena_ui::simple_input::{SimpleInput, SimpleInputState};

use crate::diff_viewer::provider::GitProvider;
use crate::project_header::{self, CiStatusColor, PrStateColor};

use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use gpui_component::{h_flex, v_flex};
use okena_core::theme::ThemeColors;
use okena_ui::tokens::{ui_text_sm, ui_text_ms, ui_text_md};
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

    // ── Branch switcher state ───────────────────────────────────────
    branch_picker_visible: bool,
    branch_picker_bounds: Bounds<Pixels>,
    branch_picker_list: BranchList,
    branch_picker_filter: Entity<SimpleInputState>,
    branch_picker_create_mode: bool,
    branch_picker_create_name: Entity<SimpleInputState>,
    branch_picker_status: BranchPickerStatus,

    // ── PR checks popover state ─────────────────────────────────────
    pr_checks_visible: bool,
    pr_badge_bounds: Bounds<Pixels>,
}

/// Mutually-exclusive states of the branch switcher popover: idle (waiting
/// for input), loading the branch list, executing a checkout/create, or
/// surfacing a last-error banner. Reset to `Idle` on every show/hide.
#[derive(Clone, Debug)]
enum BranchPickerStatus {
    Idle,
    Loading,
    Working,
    Error(String),
}

/// Whether a picker row represents a local or remote branch. Drives whether
/// checkout creates a tracking branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BranchKind {
    Local,
    Remote,
}

const COMMIT_PAGE_SIZE: usize = 50;

impl GitHeader {
    pub fn new(
        project_id: String,
        request_broker: Entity<RequestBroker>,
        git_provider: Arc<dyn GitProvider>,
        cx: &mut Context<Self>,
    ) -> Self {
        let branch_picker_filter = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Filter branches\u{2026}")
                .icon("icons/search.svg")
        });
        let branch_picker_create_name = cx.new(|cx| {
            SimpleInputState::new(cx).placeholder("New branch name")
        });
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
            branch_picker_visible: false,
            branch_picker_bounds: Bounds::default(),
            branch_picker_list: BranchList::default(),
            branch_picker_filter,
            branch_picker_create_mode: false,
            branch_picker_create_name,
            branch_picker_status: BranchPickerStatus::Idle,
            pr_checks_visible: false,
            pr_badge_bounds: Bounds::default(),
        }
    }

    /// Update the current branch name (from the git status watcher).
    pub fn set_current_branch(&mut self, branch: Option<String>) {
        self.current_branch = branch;
    }

    /// Replace the git provider. Clears cached diff/commit data that belonged
    /// to the old provider so subsequent reads refetch from the new source.
    pub fn set_git_provider(&mut self, provider: Arc<dyn GitProvider>, cx: &mut Context<Self>) {
        self.git_provider = provider;
        self.diff_file_summaries.clear();
        self.commit_log_entries.clear();
        self.commit_log_count = 0;
        self.commit_log_has_more = false;
        self.commit_log_loading = false;
        self.commit_log_branches.clear();
        cx.notify();
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

    // ── Branch picker ───────────────────────────────────────────────

    /// Open the branch switcher popover and load branches asynchronously.
    /// No-op when the provider is read-only (remote-mirrored project).
    pub fn show_branch_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.git_provider.supports_mutations() {
            return;
        }
        if self.branch_picker_visible {
            // Already open — just refocus filter so a second hotkey press is harmless.
            let filter = self.branch_picker_filter.clone();
            filter.update(cx, |inp, cx| inp.focus(window, cx));
            return;
        }

        // Hide other popovers
        self.diff_popover_visible = false;
        self.commit_log_visible = false;

        self.branch_picker_visible = true;
        // Clear stale list so the previous repo's branches don't flash before
        // the async load completes.
        self.branch_picker_list = BranchList::default();
        self.branch_picker_status = BranchPickerStatus::Loading;
        self.branch_picker_create_mode = false;
        let filter = self.branch_picker_filter.clone();
        filter.update(cx, |inp, cx| {
            inp.set_value("", cx);
            inp.focus(window, cx);
        });
        let create_input = self.branch_picker_create_name.clone();
        create_input.update(cx, |inp, cx| inp.set_value("", cx));
        cx.notify();

        let provider = self.git_provider.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let list = smol::unblock(move || provider.list_branches_classified()).await;
            let _ = this.update(cx, |this, cx| {
                this.branch_picker_list = list;
                if matches!(this.branch_picker_status, BranchPickerStatus::Loading) {
                    this.branch_picker_status = BranchPickerStatus::Idle;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Close the branch switcher popover.
    pub fn hide_branch_picker(&mut self, cx: &mut Context<Self>) {
        if !self.branch_picker_visible {
            return;
        }
        self.branch_picker_visible = false;
        self.branch_picker_create_mode = false;
        self.branch_picker_status = BranchPickerStatus::Idle;
        cx.notify();
    }

    /// Record the on-screen bounds of the branch chip so the popover can
    /// anchor underneath it. Caller-side change detection avoids re-running
    /// this every frame.
    pub fn set_branch_chip_bounds(&mut self, bounds: Bounds<Pixels>) {
        if self.branch_picker_bounds != bounds {
            self.branch_picker_bounds = bounds;
        }
    }

    fn toggle_branch_create_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.branch_picker_create_mode = !self.branch_picker_create_mode;
        self.branch_picker_status = BranchPickerStatus::Idle;
        if self.branch_picker_create_mode {
            let input = self.branch_picker_create_name.clone();
            input.update(cx, |inp, cx| {
                inp.set_value("", cx);
                inp.focus(window, cx);
            });
        } else {
            let filter = self.branch_picker_filter.clone();
            filter.update(cx, |inp, cx| inp.focus(window, cx));
        }
        cx.notify();
    }

    fn checkout_branch(&mut self, branch: String, kind: BranchKind, cx: &mut Context<Self>) {
        if matches!(self.branch_picker_status, BranchPickerStatus::Working) {
            return;
        }
        self.branch_picker_status = BranchPickerStatus::Working;
        cx.notify();

        let provider = self.git_provider.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let result = smol::unblock(move || match kind {
                BranchKind::Local => provider.checkout_local_branch(&branch),
                BranchKind::Remote => provider.checkout_remote_branch(&branch),
            })
            .await;

            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => this.hide_branch_picker(cx),
                Err(e) => {
                    this.branch_picker_status = BranchPickerStatus::Error(e);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn create_branch_from_current(&mut self, cx: &mut Context<Self>) {
        if matches!(self.branch_picker_status, BranchPickerStatus::Working) {
            return;
        }
        let raw = self
            .branch_picker_create_name
            .read(cx)
            .value()
            .trim()
            .to_string();
        if raw.is_empty() {
            self.branch_picker_status =
                BranchPickerStatus::Error("Branch name cannot be empty".to_string());
            cx.notify();
            return;
        }
        if okena_git::validate_git_ref(&raw).is_err() {
            self.branch_picker_status =
                BranchPickerStatus::Error(format!("Invalid branch name: {}", raw));
            cx.notify();
            return;
        }

        self.branch_picker_status = BranchPickerStatus::Working;
        cx.notify();

        let provider = self.git_provider.clone();
        let name = raw.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let result = smol::unblock(move || {
                provider.create_and_checkout_branch(&name, None)
            })
            .await;

            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => this.hide_branch_picker(cx),
                Err(e) => {
                    this.branch_picker_status = BranchPickerStatus::Error(e);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    // ── PR checks popover ───────────────────────────────────────────

    /// Toggle the PR checks popover. Caller is responsible for ensuring
    /// the PR badge is actually rendered (otherwise the popover anchors
    /// to stale bounds).
    pub fn toggle_pr_checks(&mut self, cx: &mut Context<Self>) {
        self.pr_checks_visible = !self.pr_checks_visible;
        if self.pr_checks_visible {
            // Hide siblings so they don't overlap.
            self.diff_popover_visible = false;
            self.commit_log_visible = false;
            self.branch_picker_visible = false;
        }
        cx.notify();
    }

    fn hide_pr_checks(&mut self, cx: &mut Context<Self>) {
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
                    .child({
                        let entity_for_branch_bounds = entity_handle.clone();
                        let entity_for_branch_click = entity_handle.clone();
                        let entity_for_pr_bounds = entity_handle.clone();
                        let entity_for_pr_click = entity_handle.clone();
                        let supports_switch = self.git_provider.supports_mutations();
                        let has_pr = status.pr_info.is_some();
                        let on_branch_click: Option<Arc<dyn Fn(&mut Window, &mut App)>> =
                            if supports_switch {
                                Some(Arc::new(move |window, app| {
                                    let _ = entity_for_branch_click.update(app, |this, cx| {
                                        if this.branch_picker_visible {
                                            this.hide_branch_picker(cx);
                                        } else {
                                            this.show_branch_picker(window, cx);
                                        }
                                    });
                                }))
                            } else {
                                None
                            };
                        let on_pr_click: Option<Arc<dyn Fn(&mut Window, &mut App)>> =
                            if has_pr {
                                Some(Arc::new(move |_window, app| {
                                    let _ = entity_for_pr_click.update(app, |this, cx| {
                                        this.toggle_pr_checks(cx);
                                    });
                                }))
                            } else {
                                None
                            };
                        let on_branch_bounds: Option<Arc<dyn Fn(Bounds<Pixels>, &mut App)>> =
                            if supports_switch {
                                Some(Arc::new(move |bounds, app| {
                                    let _ = entity_for_branch_bounds.update(app, |this, _cx| {
                                        this.set_branch_chip_bounds(bounds);
                                    });
                                }))
                            } else {
                                None
                            };
                        let on_pr_bounds: Option<Arc<dyn Fn(Bounds<Pixels>, &mut App)>> =
                            if has_pr {
                                Some(Arc::new(move |bounds, app| {
                                    let _ = entity_for_pr_bounds.update(app, |this, _cx| {
                                        this.set_pr_badge_bounds(bounds);
                                    });
                                }))
                            } else {
                                None
                            };
                        project_header::render_branch_status(
                            &status,
                            project_header::BranchStatusCallbacks {
                                on_branch_click,
                                on_pr_click,
                                on_branch_bounds,
                                on_pr_bounds,
                            },
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
                                        broker.push_overlay_request(OverlayRequest::Project(ProjectOverlay {
                                            project_id: project_id_for_click.clone(),
                                            kind: ProjectOverlayKind::DiffViewer {
                                                file: None,
                                                mode: None,
                                                commit_message: None,
                                                commits: None,
                                                commit_index: None,
                                            },
                                        }), cx);
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
                    .when_some(
                        project_header::render_ahead_behind_badge(
                            (status.ahead, status.behind),
                            t,
                        ),
                        |d, badge| d.child(badge),
                    )
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

    /// Render the branch switcher popover anchored under the branch chip.
    /// Returns a zero-size element when the popover is hidden.
    pub fn render_branch_picker(
        &mut self,
        window: &mut Window,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.branch_picker_visible {
            return div().size_0().into_any_element();
        }

        // Keep the active input focused while the popover is open. This handles
        // the first render after `show_branch_picker` (which can't observe its
        // own popover) and any focus loss from re-rendering parents.
        let active = if self.branch_picker_create_mode {
            &self.branch_picker_create_name
        } else {
            &self.branch_picker_filter
        };
        let active_handle = active.read(cx).focus_handle(cx);
        if !active_handle.is_focused(window) {
            let active = active.clone();
            active.update(cx, |inp, cx| inp.focus(window, cx));
        }

        let bounds = self.branch_picker_bounds;
        let position = point(
            bounds.origin.x,
            bounds.origin.y + bounds.size.height + px(6.0),
        );

        let filter_text = self.branch_picker_filter.read(cx).value().to_lowercase();
        let current = self.branch_picker_list.current.clone();
        let local: Vec<&String> = self
            .branch_picker_list
            .local
            .iter()
            .filter(|b| filter_text.is_empty() || b.to_lowercase().contains(&filter_text))
            .collect();
        let remote: Vec<&String> = self
            .branch_picker_list
            .remote
            .iter()
            .filter(|b| filter_text.is_empty() || b.to_lowercase().contains(&filter_text))
            .collect();
        let is_create = self.branch_picker_create_mode;
        let is_working =
            matches!(self.branch_picker_status, BranchPickerStatus::Working);
        let is_loading =
            matches!(self.branch_picker_status, BranchPickerStatus::Loading);
        let error = match &self.branch_picker_status {
            BranchPickerStatus::Error(msg) => Some(msg.clone()),
            _ => None,
        };

        let row = |name: String,
                   is_current: bool,
                   kind: BranchKind,
                   key: String,
                   cx: &mut Context<Self>|
         -> AnyElement {
            let name_for_click = name.clone();
            let is_remote = kind == BranchKind::Remote;
            h_flex()
                .id(ElementId::Name(key.into()))
                .px(px(10.0))
                .py(px(4.0))
                .gap(px(6.0))
                .items_center()
                .cursor_pointer()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(if is_current { t.text_primary } else { t.text_secondary }))
                .when(is_current, |d| d.font_weight(FontWeight::SEMIBOLD))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .child(
                    svg()
                        .path("icons/git-branch.svg")
                        .size(px(10.0))
                        .text_color(rgb(if is_remote { t.term_green } else { t.text_muted })),
                )
                .child(div().flex_1().min_w_0().text_ellipsis().overflow_hidden().child(name))
                .when(is_current, |d| {
                    d.child(
                        div()
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.term_cyan))
                            .child("HEAD"),
                    )
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.checkout_branch(name_for_click.clone(), kind, cx);
                }))
                .into_any_element()
        };

        let section_header = |label: &'static str, cx: &App| -> Div {
            div()
                .px(px(10.0))
                .py(px(4.0))
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(label)
        };

        deferred(
                anchored()
                    .position(position)
                    .snap_to_window()
                    .child(
                        v_flex()
                            .id("branch-picker-popover")
                            .occlude()
                            .w(px(320.0))
                            .max_h(px(420.0))
                            .bg(rgb(t.bg_primary))
                            .border_1()
                            .border_color(rgb(t.border))
                            .rounded(px(8.0))
                            .shadow_lg()
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.hide_branch_picker(cx);
                            }))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                                .on_scroll_wheel(|_, _, cx| {
                                    cx.stop_propagation();
                                })
                                // Filter / create input
                                .child(
                                    div()
                                        .px(px(10.0))
                                        .py(px(8.0))
                                        .border_b_1()
                                        .border_color(rgb(t.border))
                                        .child(if is_create {
                                            v_flex()
                                                .gap(px(6.0))
                                                .child(
                                                    div()
                                                        .text_size(ui_text_sm(cx))
                                                        .text_color(rgb(t.text_muted))
                                                        .child(format!(
                                                            "New branch from {}",
                                                            current.clone().unwrap_or_else(|| "HEAD".to_string())
                                                        )),
                                                )
                                                .child(
                                                    SimpleInput::new(&self.branch_picker_create_name)
                                                        .text_size(ui_text_md(cx)),
                                                )
                                                .into_any_element()
                                        } else {
                                            SimpleInput::new(&self.branch_picker_filter)
                                                .text_size(ui_text_md(cx))
                                                .into_any_element()
                                        }),
                                )
                                // Error banner
                                .when_some(error, |d, msg| {
                                    d.child(
                                        div()
                                            .px(px(10.0))
                                            .py(px(4.0))
                                            .text_size(ui_text_sm(cx))
                                            .text_color(rgb(t.term_red))
                                            .child(msg),
                                    )
                                })
                                .when(!is_create, |d| {
                                    let total = local.len() + remote.len();
                                    let local_rows: Vec<AnyElement> = local
                                        .iter()
                                        .enumerate()
                                        .map(|(i, b)| {
                                            let is_current = current.as_deref() == Some(b.as_str());
                                            row(
                                                (*b).clone(),
                                                is_current,
                                                BranchKind::Local,
                                                format!("branch-picker-local-{}", i),
                                                cx,
                                            )
                                        })
                                        .collect();
                                    let remote_rows: Vec<AnyElement> = remote
                                        .iter()
                                        .enumerate()
                                        .map(|(i, b)| {
                                            row(
                                                (*b).clone(),
                                                false,
                                                BranchKind::Remote,
                                                format!("branch-picker-remote-{}", i),
                                                cx,
                                            )
                                        })
                                        .collect();
                                    d.child(
                                        v_flex()
                                            .id("branch-picker-list")
                                            .flex_1()
                                            .min_h_0()
                                            .overflow_y_scroll()
                                            .py(px(4.0))
                                            .when(is_loading && total == 0, |d| {
                                                d.child(
                                                    div()
                                                        .px(px(10.0))
                                                        .py(px(8.0))
                                                        .text_size(ui_text_sm(cx))
                                                        .text_color(rgb(t.text_muted))
                                                        .child("Loading\u{2026}"),
                                                )
                                            })
                                            .when(!is_loading && total == 0, |d| {
                                                d.child(
                                                    div()
                                                        .px(px(10.0))
                                                        .py(px(8.0))
                                                        .text_size(ui_text_sm(cx))
                                                        .text_color(rgb(t.text_muted))
                                                        .child(if filter_text.is_empty() {
                                                            "No branches".to_string()
                                                        } else {
                                                            format!("No matches for \"{}\"", filter_text)
                                                        }),
                                                )
                                            })
                                            .when(!local_rows.is_empty(), |d| {
                                                d.child(section_header("LOCAL", cx))
                                                    .children(local_rows)
                                            })
                                            .when(!remote_rows.is_empty(), |d| {
                                                d.child(section_header("REMOTE", cx))
                                                    .children(remote_rows)
                                            }),
                                    )
                                })
                                .child(
                                    h_flex()
                                        .px(px(10.0))
                                        .py(px(6.0))
                                        .gap(px(8.0))
                                        .border_t_1()
                                        .border_color(rgb(t.border))
                                        .items_center()
                                        .child({
                                            let label = if is_create { "Cancel" } else { "+ New branch" };
                                            div()
                                                .id("branch-picker-toggle-create")
                                                .cursor_pointer()
                                                .px(px(6.0))
                                                .py(px(3.0))
                                                .rounded(px(4.0))
                                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                                .text_size(ui_text_sm(cx))
                                                .text_color(rgb(t.text_secondary))
                                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                    cx.stop_propagation();
                                                })
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.toggle_branch_create_mode(window, cx);
                                                }))
                                                .child(label)
                                        })
                                        .when(is_create, |d| {
                                            d.child(
                                                div()
                                                    .id("branch-picker-create-confirm")
                                                    .cursor_pointer()
                                                    .px(px(8.0))
                                                    .py(px(3.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgb(t.term_cyan))
                                                    .text_size(ui_text_sm(cx))
                                                    .text_color(rgb(t.bg_primary))
                                                    .opacity(if is_working { 0.5 } else { 1.0 })
                                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                        cx.stop_propagation();
                                                    })
                                                    .on_click(cx.listener(|this, _, _window, cx| {
                                                        this.create_branch_from_current(cx);
                                                    }))
                                                    .child("Create & checkout"),
                                            )
                                        })
                                        .when(is_working, |d| {
                                            d.child(
                                                div()
                                                    .text_size(ui_text_sm(cx))
                                                    .text_color(rgb(t.text_muted))
                                                    .child("Working\u{2026}"),
                                            )
                                        }),
                                ),
                        ),
            )
            .into_any_element()
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
