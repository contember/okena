//! Confirm path of CloseWorktreeDialog — dispatches the daemon-side
//! `CloseWorktree` action. The stash/fetch/rebase/merge/push/delete-branch
//! pipeline (and all hook integration) now runs on the daemon inside
//! `Workspace::close_worktree`; the dialog only forwards the raw checkbox flags
//! and reflects success/failure.

use super::{CloseWorktreeDialog, ProcessingState};

use gpui::Context;

impl CloseWorktreeDialog {
    pub(super) fn execute(&mut self, cx: &mut Context<Self>) {
        if self.processing != ProcessingState::Idle {
            return;
        }
        self.error_message = None;
        // Single generic working state — the per-step pipeline now runs on the
        // daemon, so the dialog no longer drives stash/rebase/merge progress.
        self.processing = ProcessingState::Working;
        cx.notify();

        let host = self.host.clone();
        let port = self.port;
        let token = self.token.clone();
        let local_endpoint = self.local_endpoint.clone();
        let project_id = self.daemon_project_id.clone();
        let merge = self.merge_enabled;
        let stash = self.stash_enabled;
        let fetch = self.fetch_enabled;
        let push = self.push_enabled;
        let delete_branch = self.delete_branch_enabled;

        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                okena_transport::remote_action::post_action_with_endpoint(
                    &host,
                    port,
                    &token,
                    local_endpoint.as_ref(),
                    okena_core::api::ActionRequest::CloseWorktree {
                        project_id, merge, stash, fetch, push, delete_branch,
                    },
                )
            })
            .await;

            let _ = this.update(cx, |this, cx| match result {
                Ok(_) => {
                    // Daemon completed the close (or deferred it behind a visible
                    // before_remove hook PTY); the removal mirrors back.
                    this.close(cx);
                }
                Err(e) => {
                    this.error_message = Some(e);
                    this.processing = ProcessingState::Idle;
                    cx.notify();
                }
            });
        })
        .detach();
    }
}
