use crate::git::list_git_worktrees;
use crate::workspace::state::Workspace;
use gpui::prelude::*;
use gpui::*;
use std::path::Path;
use std::time::Duration;

/// Periodically discovers new git worktrees and removes stale worktree projects.
///
/// Follows the `GitStatusWatcher` pattern: a GPUI entity with a `cx.spawn` async loop.
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

                // Collect project info from workspace
                let (parent_projects, current_worktrees, existing_paths) = cx.update(|cx| {
                    let ws = workspace.read(cx);
                    let parents: Vec<(String, String)> = ws.data().projects.iter()
                        .filter(|p| p.worktree_info.is_none() && !p.is_remote)
                        .map(|p| (p.id.clone(), p.path.clone()))
                        .collect();
                    let worktrees: Vec<(String, String)> = ws.data().projects.iter()
                        .filter(|p| p.worktree_info.is_some())
                        .map(|p| (p.id.clone(), p.path.clone()))
                        .collect();
                    let paths: std::collections::HashSet<String> = ws.data().projects.iter()
                        .map(|p| p.path.clone())
                        .collect();
                    (parents, worktrees, paths)
                });

                // Run git worktree discovery on blocking thread
                let discovered = smol::unblock({
                    let parent_projects = parent_projects.clone();
                    let existing_paths = existing_paths.clone();
                    move || {
                        // Build canonical existing paths for dedup with git output
                        let canonical_existing: std::collections::HashSet<String> = existing_paths.iter()
                            .map(|p| Path::new(p).canonicalize()
                                .map(|c| c.to_string_lossy().to_string())
                                .unwrap_or_else(|_| p.clone()))
                            .collect();

                        let mut new_worktrees = Vec::new();
                        for (parent_id, parent_path) in &parent_projects {
                            if !Path::new(parent_path).exists() {
                                continue;
                            }
                            let canonical_parent = Path::new(parent_path)
                                .canonicalize()
                                .map(|c| c.to_string_lossy().to_string())
                                .unwrap_or_else(|_| parent_path.clone());
                            let worktrees = list_git_worktrees(Path::new(parent_path));
                            for (wt_path, branch) in worktrees {
                                if wt_path == *parent_path || wt_path == canonical_parent {
                                    continue;
                                }
                                if existing_paths.contains(&wt_path) || canonical_existing.contains(&wt_path) {
                                    continue;
                                }
                                if !Path::new(&wt_path).exists() {
                                    continue;
                                }
                                new_worktrees.push((parent_id.clone(), parent_path.clone(), wt_path, branch));
                            }
                        }
                        new_worktrees
                    }
                }).await;

                // Check for stale worktrees on blocking thread
                let stale_ids = smol::unblock({
                    let current_worktrees = current_worktrees;
                    move || {
                        current_worktrees.iter()
                            .filter(|(_, path)| !Path::new(path).exists())
                            .map(|(id, _)| id.clone())
                            .collect::<Vec<_>>()
                    }
                }).await;

                // Apply changes if any
                if !discovered.is_empty() || !stale_ids.is_empty() {
                    cx.update(|cx| {
                        workspace.update(cx, |ws, cx| {
                            for (parent_id, main_repo_path, wt_path, branch) in &discovered {
                                ws.add_discovered_worktree(
                                    wt_path,
                                    branch,
                                    parent_id,
                                    main_repo_path,
                                );
                            }

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
