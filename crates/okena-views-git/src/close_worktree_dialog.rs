//! Confirmation dialog shown when closing a worktree. Checks for dirty
//! state and optionally rebases + merges the branch back before removing.
//!
//! Implementation is split across `close_worktree_dialog/` submodules:
//! `execute.rs` holds the async close pipeline; `view.rs` holds the
//! `Render` impl.

use okena_workspace::settings::{HooksConfig, WorktreeConfig};
use okena_workspace::state::Workspace;

use gpui::prelude::*;
use gpui::*;

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

/// Processing state for the close operation.
///
/// The per-step pipeline (stash/fetch/rebase/merge/push/delete-branch) runs
/// daemon-side inside `Workspace::close_worktree`, so the dialog only tracks
/// whether the single `CloseWorktree` action is in flight — it has no
/// per-step progress to surface.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum ProcessingState {
    Idle,
    Working,
}

/// Confirmation dialog shown when closing a worktree.
/// Checks for dirty state and optionally merges the branch back.
pub struct CloseWorktreeDialog {
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) token: String,
    pub(super) daemon_project_id: String,
    pub(super) focus_handle: FocusHandle,
    pub(super) project_name: String,
    pub(super) project_path: String,
    pub(super) branch: Option<String>,
    pub(super) default_branch: Option<String>,
    pub(super) is_dirty: bool,
    pub(super) merge_enabled: bool,
    pub(super) stash_enabled: bool,
    pub(super) fetch_enabled: bool,
    pub(super) delete_branch_enabled: bool,
    pub(super) push_enabled: bool,
    pub(super) unpushed_count: usize,
    pub(super) error_message: Option<String>,
    pub(super) processing: ProcessingState,
}

impl CloseWorktreeDialog {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        host: String,
        port: u16,
        token: String,
        daemon_project_id: String,
        workspace: Entity<Workspace>,
        // The daemon owns worktree removal; the dialog no longer scrubs focus
        // state itself, so this is unused (kept for call-site stability).
        _focus_manager: Entity<okena_workspace::focus::FocusManager>,
        project_id: String,
        worktree_config: WorktreeConfig,
        // Hooks now fire daemon-side inside `Workspace::close_worktree`; the
        // dialog no longer reads them (kept for call-site stability).
        _hooks_config: HooksConfig,
        cx: &mut Context<Self>,
    ) -> Self {
        let ws = workspace.read(cx);
        let project = ws.project(&project_id);

        let project_name = project.map(|p| p.name.clone()).unwrap_or_default();
        let project_path = project.map(|p| p.path.clone()).unwrap_or_default();

        let (is_dirty, branch, default_branch, unpushed_count) =
            Self::fetch_close_info(&host, port, &token, daemon_project_id.clone());

        Self {
            host,
            port,
            token,
            daemon_project_id,
            focus_handle: cx.focus_handle(),
            project_name,
            project_path,
            branch,
            default_branch,
            is_dirty,
            merge_enabled: worktree_config.default_merge,
            stash_enabled: worktree_config.default_stash,
            fetch_enabled: worktree_config.default_fetch,
            delete_branch_enabled: worktree_config.default_delete_branch,
            push_enabled: worktree_config.default_push,
            unpushed_count,
            error_message: None,
            processing: ProcessingState::Idle,
        }
    }

    /// Fetch the git-derived close info from the daemon. The repo lives on the
    /// daemon, so we post a `WorktreeCloseInfo` action rather than reading local
    /// git. Kept synchronous on purpose — the old code did blocking local git
    /// here, so a blocking HTTP call is no worse.
    fn fetch_close_info(host: &str, port: u16, token: &str, project_id: String)
        -> (bool, Option<String>, Option<String>, usize)
    {
        let action = okena_core::api::ActionRequest::WorktreeCloseInfo { project_id };
        match okena_transport::remote_action::post_action(host, port, token, action) {
            Ok(Some(v)) => {
                let is_dirty = v.get("is_dirty").and_then(|x| x.as_bool()).unwrap_or(false);
                let branch = v.get("branch").and_then(|x| x.as_str()).map(String::from);
                let default_branch = v.get("default_branch").and_then(|x| x.as_str()).map(String::from);
                let unpushed_count = v.get("unpushed_count").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
                (is_dirty, branch, default_branch, unpushed_count)
            }
            _ => (false, None, None, 0),
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
