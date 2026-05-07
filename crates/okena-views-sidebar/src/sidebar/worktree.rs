//! Quick worktree creation — spawns a background task that generates a branch
//! name, registers the project optimistically, then runs `git fetch` + `git
//! worktree add` and fires configured hooks on success.

use super::Sidebar;
use gpui::*;

impl Sidebar {
    /// Spawn quick worktree creation on a background thread.
    /// All blocking git operations (branch name generation, worktree creation)
    /// run off the main thread to avoid UI jank.
    pub fn spawn_quick_create_worktree(&mut self, project_id: &str, cx: &mut Context<Self>) {
        // Debounce: prevent concurrent creation for the same parent
        if !self.creating_worktree.insert(project_id.to_string()) {
            return;
        }

        let workspace = self.workspace.clone();
        let focus_manager = self.focus_manager.clone();
        let parent_id = project_id.to_string();
        let parent_id_for_cleanup = parent_id.clone();

        // Collect data from workspace and settings (non-blocking reads)
        let prep = self.workspace.read(cx).prepare_quick_create(project_id);
        let settings = self.sidebar_settings(cx);
        let path_template = settings.worktree_path_template.clone();
        let hooks = settings.hooks.clone();
        let Some((parent_path, main_repo_path)) = prep else {
            log::error!("Quick worktree creation failed: parent project not found");
            self.creating_worktree.remove(project_id);
            return;
        };

        // Store hooks for later use in async block
        let hooks_for_register = hooks.clone();
        let hooks_for_fire = hooks.clone();
        let hooks_for_error = hooks.clone();

        cx.spawn(async move |sidebar_weak, cx| {
            // Phase 1 (fast): resolve git root, generate branch name, compute
            // paths — no network calls needed.
            let prep_result = smol::unblock(move || -> Result<(String, std::path::PathBuf, String, String, Option<String>), String> {
                let project_path = std::path::PathBuf::from(&parent_path);

                // Determine git root
                let git_root = main_repo_path
                    .map(std::path::PathBuf::from)
                    .or_else(|| okena_git::get_repo_root(&project_path))
                    .ok_or_else(|| "Not a git repository".to_string())?;

                // Compute subdir (project path relative to git root)
                let normalized_project = okena_git::repository::normalize_path(&project_path);
                let normalized_root = okena_git::repository::normalize_path(&git_root);
                let subdir = normalized_project.strip_prefix(&normalized_root)
                    .unwrap_or(std::path::Path::new(""))
                    .to_path_buf();

                // Generate branch name (username cached, branch listing is local)
                let branch = okena_git::branch_names::generate_branch_name(&git_root);

                // Fast local lookup for default branch (no network)
                let default_branch = okena_git::repository::get_default_branch(&git_root);

                // Compute target paths
                let (worktree_path, project_path) = okena_git::repository::compute_target_paths(
                    &git_root, &subdir, &path_template, &branch,
                );

                Ok((branch, git_root, worktree_path, project_path, default_branch))
            }).await;

            let (branch, git_root, worktree_path, project_path, default_branch) = match prep_result {
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

            // Register project in sidebar immediately so it appears instantly.
            // Hooks are deferred until the worktree directory exists on disk.
            let project_id = cx.update(|cx| {
                workspace.update(cx, |ws, cx| {
                    let id = ws.register_worktree_project_deferred_hooks(
                        &parent_id, &branch, &git_root,
                        &worktree_path, &project_path, &hooks_for_register, cx,
                    );
                    if let Ok(ref id) = id {
                        ws.mark_creating_project(id);
                    }
                    id
                })
            });

            let Ok(project_id) = project_id else {
                log::error!("Quick worktree creation failed: could not register project");
                let _ = sidebar_weak.update(cx, |sidebar, cx| {
                    sidebar.creating_worktree.remove(&parent_id_for_cleanup);
                    cx.notify();
                });
                return;
            };

            // Phase 2 (slow): fetch + git worktree add in background.
            // The project is already visible in the sidebar.
            let branch_clone = branch.clone();
            let worktree_path_clone = worktree_path.clone();
            let git_root_clone = git_root.clone();
            let create_result = smol::unblock(move || -> Result<(), String> {
                let target = std::path::PathBuf::from(&worktree_path_clone);

                // Fetch and create worktree — fetch runs first if we have a default branch
                if let Some(ref db) = default_branch {
                    if let Some(repo_str) = git_root_clone.to_str() {
                        let _ = okena_core::process::safe_output(
                            okena_core::process::command("git")
                                .args(["-C", repo_str, "fetch", "origin", db.as_str()]),
                        );
                    }
                }

                okena_git::repository::create_worktree_with_start_point(
                    &git_root_clone,
                    &branch_clone,
                    &target,
                    default_branch.as_deref(),
                ).map_err(|e| e.to_string())
            }).await;

            match create_result {
                Ok(()) => {
                    // Worktree directory exists — clear creating state and fire hooks
                    let _ = cx.update(|cx| {
                        workspace.update(cx, |ws, cx| {
                            ws.finish_creating_project(&project_id);
                            ws.fire_worktree_hooks(&project_id, &hooks_for_fire, cx);
                            ws.notify_data(cx);
                        });
                    });
                }
                Err(e) => {
                    log::error!("Quick worktree git operation failed: {}", e);
                    // Remove the optimistically-added project since git worktree add failed
                    let _ = cx.update(|cx| {
                        focus_manager.update(cx, |fm, cx| {
                            workspace.update(cx, |ws, cx| {
                                ws.finish_creating_project(&project_id);
                                ws.delete_project(fm, &project_id, &hooks_for_error, cx);
                            });
                        });
                    });
                }
            }

            // Clear debounce guard
            let _ = sidebar_weak.update(cx, |sidebar, cx| {
                sidebar.creating_worktree.remove(&parent_id_for_cleanup);
                cx.notify();
            });
        }).detach();
    }
}
