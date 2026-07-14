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
use serde::Deserialize;

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

#[derive(Deserialize)]
struct CloseInfo {
    is_dirty: bool,
    branch: Option<String>,
    default_branch: Option<String>,
    unpushed_count: usize,
}

/// Confirmation dialog shown when closing a worktree.
/// Checks for dirty state and optionally merges the branch back.
pub struct CloseWorktreeDialog {
    pub(super) client: okena_transport::remote_action::RemoteActionClient,
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
    pub(super) loading_info: bool,
    pub(super) error_message: Option<String>,
    pub(super) processing: ProcessingState,
}

impl CloseWorktreeDialog {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: okena_transport::remote_action::RemoteActionClient,
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

        let mut dialog = Self {
            client,
            daemon_project_id,
            focus_handle: cx.focus_handle(),
            project_name,
            project_path,
            branch: None,
            default_branch: None,
            is_dirty: false,
            merge_enabled: worktree_config.default_merge,
            stash_enabled: worktree_config.default_stash,
            fetch_enabled: worktree_config.default_fetch,
            delete_branch_enabled: worktree_config.default_delete_branch,
            push_enabled: worktree_config.default_push,
            unpushed_count: 0,
            loading_info: true,
            error_message: None,
            processing: ProcessingState::Idle,
        };
        dialog.load_close_info(cx);
        dialog
    }

    fn load_close_info(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let project_id = self.daemon_project_id.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || Self::fetch_close_info(&client, project_id)).await;
            let _ = this.update(cx, |this, cx| {
                this.loading_info = false;
                match result {
                    Ok(info) => {
                        this.is_dirty = info.is_dirty;
                        this.branch = info.branch;
                        this.default_branch = info.default_branch;
                        this.unpushed_count = info.unpushed_count;
                    }
                    Err(error) => this.error_message = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn fetch_close_info(
        client: &okena_transport::remote_action::RemoteActionClient,
        project_id: String,
    ) -> Result<CloseInfo, String> {
        let action = okena_core::api::ActionRequest::WorktreeCloseInfo { project_id };
        let value = client
            .post_action(action)?
            .ok_or_else(|| "Missing worktree close info response".to_string())?;
        serde_json::from_value(value)
            .map_err(|error| format!("Invalid worktree close info response: {error}"))
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
