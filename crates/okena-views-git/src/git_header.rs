//! GitHeader — self-contained GPUI entity for git status display,
//! diff popover, commit log popover, branch switcher, and PR checks.
//!
//! Extracted from `ProjectColumn` to keep that view thin. Implementation
//! is split across the `git_header/` submodules — one per concern.

use okena_git::{BranchDetail, BranchList, CommitLogEntry, FileDiffSummary};
use okena_ui::simple_input::{InputChangedEvent, SimpleInputState};
use okena_workspace::request_broker::RequestBroker;
use okena_workspace::state::Workspace;

use crate::diff_viewer::provider::GitProvider;

use gpui::*;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

mod branch_picker;
mod ci_checks_popover;
mod commit_log;
mod diff_popover;
mod status_pill;

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

/// A single navigable entry in the branch switcher list, flattened across the
/// LOCAL and REMOTE sections (local-first) so a single `selected_index` can
/// drive keyboard navigation. Recomputed whenever the filter text or the loaded
/// branch list changes — see `recompute_branch_filtered`.
#[derive(Clone)]
struct BranchNavItem {
    name: String,
    kind: BranchKind,
    is_current: bool,
    /// Tip time, upstream state and holding worktree, as reported by
    /// `list_branches_classified`. Defaults (all unknown) when the host did
    /// not report metadata for this branch.
    detail: BranchDetail,
}

/// State for the context menu on a branch row in the picker. Captured when
/// the menu opens so it survives the list being refiltered underneath it.
pub(super) struct BranchRowContextMenu {
    pub(super) position: Point<Pixels>,
    pub(super) name: String,
    /// Comparing the current branch with itself has nothing to show, so the
    /// compare item is disabled for it.
    pub(super) is_current: bool,
    /// Keyboard-highlighted item, as an index into `BRANCH_ROW_ACTIONS`.
    pub(super) selected: usize,
}

/// State for the right-click context menu on a commit row in the graph.
/// Captured once at open time so the menu doesn't need a live reference
/// back to the underlying `CommitLogEntry`.
pub(super) struct CommitRowContextMenu {
    pub(super) position: Point<Pixels>,
    pub(super) hash: String,
    /// Formatted payload computed once at open time.
    pub(super) send_text: String,
}

/// Self-contained GPUI entity managing git status display, diff summary
/// popover, and commit log popover.
pub struct GitHeader {
    project_id: String,
    request_broker: Entity<RequestBroker>,
    workspace: Entity<Workspace>,
    focus_manager: Entity<okena_workspace::focus::FocusManager>,
    git_provider: Arc<dyn GitProvider>,
    /// Current branch, pushed in by the parent before rendering.
    current_branch: Option<String>,

    // ── Diff popover state ──────────────────────────────────────────
    diff_popover_visible: bool,
    diff_file_summaries: Vec<FileDiffSummary>,
    diff_popover_error: Option<String>,
    hover_token: Arc<AtomicU64>,
    diff_stats_bounds: Bounds<Pixels>,

    // ── Commit log state ────────────────────────────────────────────
    commit_log_visible: bool,
    commit_log_entries: Vec<CommitLogEntry>,
    commit_log_loading: bool,
    commit_log_error: Option<String>,
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
    /// Filtered branches in display order (local-first), derived from
    /// `branch_picker_list` + the filter text. Drives keyboard selection.
    branch_picker_filtered: Vec<BranchNavItem>,
    /// Index into `branch_picker_filtered` of the keyboard-highlighted row.
    branch_picker_selected: usize,
    /// Scroll handle for the branch list, so keyboard navigation can keep the
    /// selected row in view.
    branch_picker_scroll: ScrollHandle,
    branch_picker_create_mode: bool,
    /// Open context menu for one branch row, if any.
    pub(super) branch_row_menu: Option<BranchRowContextMenu>,
    /// On-screen bounds of the keyboard-selected row, so the menu key can
    /// anchor the menu under it the same way a right-click does.
    branch_row_bounds: Bounds<Pixels>,
    branch_picker_create_name: Entity<SimpleInputState>,
    branch_picker_status: BranchPickerStatus,

    // ── CI checks popover state ─────────────────────────────────────
    ci_checks_visible: bool,
    ci_badge_bounds: Bounds<Pixels>,

    /// Open right-click context menu for a commit row in the graph.
    pub(super) commit_row_menu: Option<CommitRowContextMenu>,
}

impl GitHeader {
    pub fn new(
        project_id: String,
        request_broker: Entity<RequestBroker>,
        workspace: Entity<Workspace>,
        focus_manager: Entity<okena_workspace::focus::FocusManager>,
        git_provider: Arc<dyn GitProvider>,
        cx: &mut Context<Self>,
    ) -> Self {
        let branch_picker_filter = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Filter branches\u{2026}")
                .icon("icons/search.svg")
        });
        let branch_picker_create_name =
            cx.new(|cx| SimpleInputState::new(cx).placeholder("New branch name"));
        // Re-filter the branch list (and reset the keyboard selection) as the
        // user types. Without this the parent `GitHeader` wouldn't re-run its
        // own filtering when only the child input entity notifies.
        cx.subscribe(
            &branch_picker_filter,
            |this: &mut Self, _, _: &InputChangedEvent, cx| {
                this.recompute_branch_filtered(cx);
                cx.notify();
            },
        )
        .detach();
        Self {
            project_id,
            request_broker,
            workspace,
            focus_manager,
            git_provider,
            current_branch: None,
            diff_popover_visible: false,
            diff_file_summaries: Vec::new(),
            diff_popover_error: None,
            hover_token: Arc::new(AtomicU64::new(0)),
            diff_stats_bounds: Bounds::default(),
            commit_log_visible: false,
            commit_log_entries: Vec::new(),
            commit_log_loading: false,
            commit_log_error: None,
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
            branch_picker_filtered: Vec::new(),
            branch_picker_selected: 0,
            branch_picker_scroll: ScrollHandle::new(),
            branch_picker_create_mode: false,
            branch_row_menu: None,
            branch_row_bounds: Bounds::default(),
            branch_picker_create_name,
            branch_picker_status: BranchPickerStatus::Idle,
            ci_checks_visible: false,
            ci_badge_bounds: Bounds::default(),
            commit_row_menu: None,
        }
    }

    /// Update the current branch name (from the daemon's git status).
    pub fn set_current_branch(&mut self, branch: Option<String>) {
        self.current_branch = branch;
    }

    /// Replace the git provider. Clears cached diff/commit data that belonged
    /// to the old provider so subsequent reads refetch from the new source.
    pub fn set_git_provider(&mut self, provider: Arc<dyn GitProvider>, cx: &mut Context<Self>) {
        self.git_provider = provider;
        self.diff_file_summaries.clear();
        self.diff_popover_error = None;
        self.commit_log_entries.clear();
        self.commit_log_count = 0;
        self.commit_log_has_more = false;
        self.commit_log_loading = false;
        self.commit_log_error = None;
        self.commit_log_branches.clear();
        cx.notify();
    }
}
