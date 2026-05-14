//! Confirmation dialog shown when closing a worktree. Checks for dirty
//! state and optionally rebases + merges the branch back before removing.
//!
//! Implementation is split across `close_worktree_dialog/` submodules:
//! `execute.rs` holds the async close pipeline; `view.rs` holds the
//! `Render` impl.

use okena_git as git;
use okena_workspace::settings::{HooksConfig, WorktreeConfig};
use okena_workspace::state::Workspace;

use gpui::prelude::*;
use gpui::*;
use std::path::PathBuf;

mod execute;
mod view;

/// Events emitted by the close worktree dialog
#[derive(Clone)]
pub enum CloseWorktreeDialogEvent {
    /// Dialog closed (either cancelled or worktree was removed)
    Closed,
}

impl EventEmitter<CloseWorktreeDialogEvent> for CloseWorktreeDialog {}

impl okena_ui::overlay::CloseEvent for CloseWorktreeDialogEvent {
    fn is_close(&self) -> bool { matches!(self, Self::Closed) }
}

/// Processing state for async operations
#[derive(Clone, Debug, PartialEq)]
pub(super) enum ProcessingState {
    Idle,
    Stashing,
    Fetching,
    Rebasing,
    Merging,
    Pushing,
    DeletingBranch,
    Removing,
}

/// Confirmation dialog shown when closing a worktree.
/// Checks for dirty state and optionally merges the branch back.
pub struct CloseWorktreeDialog {
    pub(super) workspace: Entity<Workspace>,
    pub(super) focus_handle: FocusHandle,
    pub(super) project_id: String,
    pub(super) project_name: String,
    pub(super) project_path: String,
    pub(super) branch: Option<String>,
    pub(super) default_branch: Option<String>,
    pub(super) main_repo_path: Option<String>,
    pub(super) is_dirty: bool,
    pub(super) merge_enabled: bool,
    pub(super) stash_enabled: bool,
    pub(super) fetch_enabled: bool,
    pub(super) delete_branch_enabled: bool,
    pub(super) push_enabled: bool,
    pub(super) unpushed_count: usize,
    pub(super) error_message: Option<String>,
    pub(super) processing: ProcessingState,
    pub(super) hooks_config: HooksConfig,
}

impl CloseWorktreeDialog {
    pub fn new(
        workspace: Entity<Workspace>,
        project_id: String,
        worktree_config: WorktreeConfig,
        hooks_config: HooksConfig,
        cx: &mut Context<Self>,
    ) -> Self {
        let ws = workspace.read(cx);
        let project = ws.project(&project_id);

        let project_name = project.map(|p| p.name.clone()).unwrap_or_default();
        let project_path = project.map(|p| p.path.clone()).unwrap_or_default();
        let main_repo_path = ws.worktree_parent_path(&project_id);

        let path = PathBuf::from(&project_path);
        let is_dirty = git::has_uncommitted_changes(&path);
        let branch = git::get_current_branch(&path);
        let default_branch = main_repo_path
            .as_ref()
            .and_then(|p| git::get_default_branch(&PathBuf::from(p)));
        let unpushed_count = git::count_unpushed_commits(&path);

        Self {
            workspace,
            focus_handle: cx.focus_handle(),
            project_id,
            project_name,
            project_path,
            branch,
            default_branch,
            main_repo_path,
            is_dirty,
            merge_enabled: worktree_config.default_merge,
            stash_enabled: worktree_config.default_stash,
            fetch_enabled: worktree_config.default_fetch,
            delete_branch_enabled: worktree_config.default_delete_branch,
            push_enabled: worktree_config.default_push,
            unpushed_count,
            error_message: None,
            processing: ProcessingState::Idle,
            hooks_config,
        }
    }

    pub(super) fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(CloseWorktreeDialogEvent::Closed);
    }

    pub(super) fn can_merge(&self) -> bool {
        (!self.is_dirty || self.stash_enabled)
            && self.branch.is_some()
            && self.default_branch.is_some()
    }

    pub(super) fn confirm_label(&self) -> &'static str {
        if self.merge_enabled && self.can_merge() {
            "Merge & Close"
        } else {
            "Close Worktree"
        }
    }
}

impl gpui::Focusable for CloseWorktreeDialog {
    fn focus_handle(&self, _cx: &gpui::App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}
