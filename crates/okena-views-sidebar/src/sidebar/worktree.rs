//! Quick worktree creation — generates a branch name client-side, then
//! dispatches `ActionRequest::CreateWorktree` to the daemon. The daemon owns
//! the worktree creation (fetch + `git worktree add`), project registration,
//! terminal spawning and hooks; the new worktree project mirrors back into the
//! sidebar. The GUI never mutates its read-only mirror directly.

use super::Sidebar;
use gpui::*;
use okena_core::api::ActionRequest;

impl Sidebar {
    /// Spawn quick worktree creation. Branch-name generation reads the parent
    /// repo off the main thread (to avoid UI jank); the actual worktree
    /// creation is the daemon's job via `ActionRequest::CreateWorktree`.
    pub fn spawn_quick_create_worktree(&mut self, project_id: &str, cx: &mut Context<Self>) {
        // Debounce: prevent concurrent creation for the same parent
        if !self.creating_worktree.insert(project_id.to_string()) {
            return;
        }

        let parent_id = project_id.to_string();
        let parent_id_for_cleanup = parent_id.clone();

        // Collect data from workspace (non-blocking reads)
        let prep = self.workspace.read(cx).prepare_quick_create(project_id);
        let Some((parent_path, main_repo_path)) = prep else {
            log::error!("Quick worktree creation failed: parent project not found");
            self.creating_worktree.remove(project_id);
            return;
        };

        cx.spawn(async move |sidebar_weak, cx| {
            // Resolve git root and generate a branch name off the main thread.
            // The daemon computes paths / fetches / creates the worktree.
            let branch_result = smol::unblock(move || -> Result<String, String> {
                let project_path = std::path::PathBuf::from(&parent_path);

                // Determine git root
                let git_root = main_repo_path
                    .map(std::path::PathBuf::from)
                    .or_else(|| okena_git::get_repo_root(&project_path))
                    .ok_or_else(|| "Not a git repository".to_string())?;

                // Generate branch name (username cached, branch listing is local)
                Ok(okena_git::branch_names::generate_branch_name(&git_root))
            }).await;

            let branch = match branch_result {
                Ok(v) => v,
                Err(e) => {
                    log::error!("Quick worktree creation failed: {}", e);
                    let _ = sidebar_weak.update(cx, |sidebar, cx| {
                        sidebar.creating_worktree.remove(&parent_id_for_cleanup);
                        cx.notify();
                    });
                    return;
                }
            };

            // Dispatch CreateWorktree to the daemon. The dispatcher queues
            // pending visibility keyed by branch name and sends the action; the
            // new worktree project mirrors back when the daemon finishes.
            let _ = sidebar_weak.update(cx, |sidebar, cx| {
                sidebar.dispatch_action_for_project(
                    &parent_id,
                    ActionRequest::CreateWorktree {
                        project_id: parent_id.clone(),
                        branch,
                        create_branch: true,
                    },
                    cx,
                );
                sidebar.creating_worktree.remove(&parent_id_for_cleanup);
                cx.notify();
            });
        }).detach();
    }
}
