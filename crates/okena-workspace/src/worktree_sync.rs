use crate::state::Workspace;
use gpui::prelude::*;
use gpui::*;
use std::path::Path;
use std::time::Duration;

/// Periodically removes stale worktree projects whose directories no longer exist.
///
/// Worktrees are only added as projects explicitly by the user (via the worktree
/// list popover or the create worktree dialog). This watcher only cleans up
/// worktree projects that have become stale (directory deleted externally).
pub struct WorktreeSyncWatcher {
    workspace: Entity<Workspace>,
}

impl WorktreeSyncWatcher {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let mut watcher = Self { workspace };
        watcher.spawn_sync_loop(cx);
        watcher
    }

    fn spawn_sync_loop(&mut self, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                smol::Timer::after(Duration::from_secs(30)).await;

                // Collect current worktree projects, skipping those being actively managed
                let current_worktrees: Vec<(String, String)> = cx.update(|cx| {
                    let ws = workspace.read(cx);
                    ws.data().projects.iter()
                        .filter(|p| p.worktree_info.is_some())
                        .filter(|p| !ws.closing_projects.contains(&p.id))
                        .filter(|p| !ws.creating_projects.contains(&p.id))
                        .filter(|p| !ws.removing_worktree_paths.contains(&p.path))
                        .map(|p| (p.id.clone(), p.path.clone()))
                        .collect()
                });

                // Check for stale worktrees on blocking thread
                let stale_ids = smol::unblock({
                    move || {
                        current_worktrees.iter()
                            .filter(|(_, path)| !Path::new(path).exists())
                            .map(|(id, _)| id.clone())
                            .collect::<Vec<_>>()
                    }
                }).await;

                // Remove stale worktrees
                if !stale_ids.is_empty() {
                    cx.update(|cx| {
                        workspace.update(cx, |ws, cx| {
                            for id in &stale_ids {
                                ws.remove_stale_worktree(id);
                            }
                            ws.notify_data(cx);
                        });
                    });
                }

                // Check if entity is still alive
                let alive = this.update(cx, |_, _| true).unwrap_or(false);
                if !alive {
                    break;
                }
            }
        }).detach();
    }
}
