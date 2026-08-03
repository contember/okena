//! Confirm path of CloseWorktreeDialog — dispatches the daemon-side
//! `CloseWorktree` action. The stash/fetch/rebase/merge/push/delete-branch
//! pipeline (and all hook integration) now runs on the daemon inside
//! `Workspace::close_worktree`; the dialog only forwards the raw checkbox flags
//! and reflects success/failure.

use super::{CloseWorktreeDialog, ProcessingState};

use gpui::Context;

impl CloseWorktreeDialog {
    pub(super) fn execute(&mut self, cx: &mut Context<Self>) {
        if self.processing != ProcessingState::Idle || self.loading_info {
            return;
        }
        self.error_message = None;
        // Single generic working state — the per-step pipeline now runs on the
        // daemon, so the dialog no longer drives stash/rebase/merge progress.
        self.processing = ProcessingState::Working;
        cx.notify();

        // Optimistically mark the project "closing" so the sidebar row shows a
        // busy/dimmed state immediately, covering the daemon-side before_remove
        // hook + removal window. On success the mirror drops the project (the
        // row vanishes); on dispatch failure we clear the flag below.
        let client_project_id = self.client_project_id.clone();
        self.workspace.update(cx, |ws, wcx| {
            ws.mark_closing_project(&client_project_id);
            wcx.notify();
        });

        let client = self.client.clone();
        let project_id = self.daemon_project_id.clone();
        let merge = self.merge_enabled;
        let stash = self.stash_enabled;
        let fetch = self.fetch_enabled;
        let push = self.push_enabled;
        let delete_branch = self.delete_branch_enabled;

        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client.post_action(okena_core::api::ActionRequest::CloseWorktree {
                    project_id,
                    merge,
                    stash,
                    fetch,
                    push,
                    delete_branch,
                })
            })
            .await;

            let _ = this.update(cx, |this, cx| match result {
                Ok(_) => {
                    // Daemon completed the close (or deferred it behind a visible
                    // before_remove hook PTY); the removal mirrors back.
                    this.close(cx);
                }
                Err(e) => {
                    // Dispatch never reached the daemon — clear the optimistic
                    // closing flag so the row isn't stuck busy.
                    let pid = this.client_project_id.clone();
                    this.workspace.update(cx, |ws, wcx| {
                        ws.finish_closing_project(&pid);
                        wcx.notify();
                    });
                    this.error_message = Some(e);
                    this.processing = ProcessingState::Idle;
                    cx.notify();
                }
            });
        })
        .detach();
    }
}
