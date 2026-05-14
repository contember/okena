//! GitHeader — self-contained GPUI entity for git status display,
//! diff popover, commit log popover, branch switcher, and PR checks.
//!
//! Extracted from `ProjectColumn` to keep that view thin. Implementation
//! is split across the `git_header/` submodules — one per concern.

use okena_git::{BranchList, FileDiffSummary, GraphRow};
use okena_ui::simple_input::SimpleInputState;
use okena_workspace::request_broker::RequestBroker;
use okena_workspace::state::Workspace;

use crate::diff_viewer::provider::GitProvider;

use gpui::*;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

mod branch_picker;
mod commit_log;
mod diff_popover;
mod pr_checks;
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

    /// Open right-click context menu for a commit row in the graph.
    pub(super) commit_row_menu: Option<CommitRowContextMenu>,
}

impl GitHeader {
    pub fn new(
        project_id: String,
        request_broker: Entity<RequestBroker>,
        workspace: Entity<Workspace>,
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
            workspace,
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
            commit_row_menu: None,
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
}
