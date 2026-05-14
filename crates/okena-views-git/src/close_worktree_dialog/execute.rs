//! Confirm path of CloseWorktreeDialog — the async pipeline that optionally
//! stashes/fetches/rebases/merges and then removes the worktree. Hook
//! integration runs before the merge step and before the actual removal.

use super::{CloseWorktreeDialog, ProcessingState};

use okena_git as git;
use okena_workspace::hooks;
use okena_workspace::state::PendingWorktreeClose;

use gpui::Context;
use std::path::PathBuf;

impl CloseWorktreeDialog {
    pub(super) fn execute(&mut self, cx: &mut Context<Self>) {
        if self.processing != ProcessingState::Idle {
            return;
        }

        self.error_message = None;

        let project_id = self.project_id.clone();
        let project_name = self.project_name.clone();
        let project_path = self.project_path.clone();
        let branch = self.branch.clone().unwrap_or_default();
        let default_branch = self.default_branch.clone().unwrap_or_default();
        let main_repo_path = self.main_repo_path.clone().unwrap_or_default();
        let merge_enabled = self.merge_enabled && self.can_merge();
        let stash_enabled = self.stash_enabled && self.is_dirty;
        let fetch_enabled = self.fetch_enabled;
        let push_enabled = self.push_enabled;
        let delete_branch_enabled = self.delete_branch_enabled;
        let is_dirty = self.is_dirty;
        let workspace = self.workspace.clone();

        // Read hooks config and monitor before spawning
        let ws = workspace.read(cx);
        let project_hooks = ws
            .project(&project_id)
            .map(|p| p.hooks.clone())
            .unwrap_or_default();
        let global_hooks = self.hooks_config.clone();
        let folder = ws.folder_for_project_or_parent(&project_id);
        let folder_id = folder.map(|f| f.id.clone());
        let folder_name = folder.map(|f| f.name.clone());
        let monitor = hooks::try_monitor(cx);
        let runner = hooks::try_runner(cx);

        cx.spawn(async move |this, cx| {
            let mut did_stash = false;

            // Step 1: If merge enabled, run merge flow
            if merge_enabled {
                // Stash (if stash_enabled and is_dirty)
                if stash_enabled {
                    let _ = cx.update(|cx| {
                        this.update(cx, |this, cx| {
                            this.processing = ProcessingState::Stashing;
                            cx.notify();
                        })
                    });

                    let stash_path = PathBuf::from(&project_path);
                    let stash_result =
                        smol::unblock(move || git::stash_changes(&stash_path)).await;

                    if let Err(e) = stash_result {
                        let _ = cx.update(|cx| {
                            this.update(cx, |this, cx| {
                                this.error_message =
                                    Some(format!("Stash failed: {}", e));
                                this.processing = ProcessingState::Idle;
                                cx.notify();
                            })
                        });
                        return;
                    }

                    did_stash = true;
                }

                // Fetch (if fetch_enabled)
                if fetch_enabled {
                    let _ = cx.update(|cx| {
                        this.update(cx, |this, cx| {
                            this.processing = ProcessingState::Fetching;
                            cx.notify();
                        })
                    });

                    let fetch_path = PathBuf::from(&project_path);
                    let fetch_result =
                        smol::unblock(move || git::fetch_all(&fetch_path)).await;

                    if let Err(e) = fetch_result {
                        if did_stash {
                            let pop_path = PathBuf::from(&project_path);
                            let _ = smol::unblock(move || git::stash_pop(&pop_path)).await;
                        }
                        let _ = cx.update(|cx| {
                            this.update(cx, |this, cx| {
                                this.error_message =
                                    Some(format!("Fetch failed: {}", e));
                                this.processing = ProcessingState::Idle;
                                cx.notify();
                            })
                        });
                        return;
                    }
                }

                // pre_merge hook (sync)
                let pre_merge_result = smol::unblock({
                    let project_hooks = project_hooks.clone();
                    let global_hooks = global_hooks.clone();
                    let project_id = project_id.clone();
                    let project_name = project_name.clone();
                    let project_path = project_path.clone();
                    let branch = branch.clone();
                    let default_branch = default_branch.clone();
                    let main_repo_path = main_repo_path.clone();
                    let folder_id = folder_id.clone();
                    let folder_name = folder_name.clone();
                    let monitor = monitor.clone();
                    move || {
                        // Sync hooks run headlessly (no PTY) — they block the flow
                        // and can't be shown in the UI anyway
                        hooks::fire_pre_merge(
                            &project_hooks,
                            &global_hooks,
                            &project_id,
                            &project_name,
                            &project_path,
                            &branch,
                            &default_branch,
                            &main_repo_path,
                            folder_id.as_deref(),
                            folder_name.as_deref(),
                            monitor.as_ref(),
                            None,
                        )
                    }
                })
                .await;

                if let Err(e) = pre_merge_result {
                    if did_stash {
                        let pop_path = PathBuf::from(&project_path);
                        let _ = smol::unblock(move || git::stash_pop(&pop_path)).await;
                    }
                    let _ = cx.update(|cx| {
                        this.update(cx, |this, cx| {
                            this.error_message = Some(format!("pre_merge hook failed: {}", e));
                            this.processing = ProcessingState::Idle;
                            cx.notify();
                        })
                    });
                    return;
                }

                // Rebase
                let _ = cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        this.processing = ProcessingState::Rebasing;
                        cx.notify();
                    })
                });

                let worktree_path = PathBuf::from(&project_path);
                let rebase_target = default_branch.clone();
                let rebase_result = smol::unblock(move || {
                    git::rebase_onto(&worktree_path, &rebase_target)
                })
                .await;

                if let Err(e) = rebase_result {
                    // Fire on_rebase_conflict hook
                    let error_msg = e.to_string();
                    let (terminal_actions, hook_results) = hooks::fire_on_rebase_conflict(
                        &project_hooks,
                        &global_hooks,
                        &project_id,
                        &project_name,
                        &project_path,
                        &branch,
                        &default_branch,
                        &main_repo_path,
                        &error_msg,
                        folder_id.as_deref(),
                        folder_name.as_deref(),
                        monitor.as_ref(),
                        runner.as_ref(),
                    );
                    let _ = cx.update(|cx| {
                        workspace.update(cx, |ws, cx| {
                            for (cmd, env) in terminal_actions {
                                ws.add_terminal_with_command(&project_id, &cmd, &env, cx);
                            }
                            ws.register_hook_results(hook_results, cx);
                        })
                    });

                    if did_stash {
                        let pop_path = PathBuf::from(&project_path);
                        let _ = smol::unblock(move || git::stash_pop(&pop_path)).await;
                    }
                    let _ = cx.update(|cx| {
                        this.update(cx, |this, cx| {
                            this.error_message = Some(format!("Rebase failed: {}", e));
                            this.processing = ProcessingState::Idle;
                            cx.notify();
                        })
                    });
                    return;
                }

                // Merge (ff-only) in the main repo
                let _ = cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        this.processing = ProcessingState::Merging;
                        cx.notify();
                    })
                });

                let main_path = PathBuf::from(&main_repo_path);
                let merge_branch = branch.clone();
                let merge_result = smol::unblock(move || {
                    git::merge_branch(&main_path, &merge_branch, true)
                })
                .await;

                if let Err(e) = merge_result {
                    if did_stash {
                        let pop_path = PathBuf::from(&project_path);
                        let _ = smol::unblock(move || git::stash_pop(&pop_path)).await;
                    }
                    let _ = cx.update(|cx| {
                        this.update(cx, |this, cx| {
                            this.error_message = Some(format!("Merge failed: {}", e));
                            this.processing = ProcessingState::Idle;
                            cx.notify();
                        })
                    });
                    return;
                }

                // post_merge hook (async)
                let _ = hooks::fire_post_merge(
                    &project_hooks,
                    &global_hooks,
                    &project_id,
                    &project_name,
                    &project_path,
                    &branch,
                    &default_branch,
                    &main_repo_path,
                    folder_id.as_deref(),
                    folder_name.as_deref(),
                    monitor.as_ref(),
                    runner.as_ref(),
                );

                // Push default branch (if push_enabled)
                if push_enabled {
                    let _ = cx.update(|cx| {
                        this.update(cx, |this, cx| {
                            this.processing = ProcessingState::Pushing;
                            cx.notify();
                        })
                    });

                    let push_path = PathBuf::from(&main_repo_path);
                    let push_branch = default_branch.clone();
                    let push_result = smol::unblock(move || {
                        git::push_branch(&push_path, &push_branch)
                    })
                    .await;

                    if let Err(e) = push_result {
                        log::warn!("Push failed (continuing): {}", e);
                    }
                }

                // Delete branch (if delete_branch_enabled)
                if delete_branch_enabled {
                    let _ = cx.update(|cx| {
                        this.update(cx, |this, cx| {
                            this.processing = ProcessingState::DeletingBranch;
                            cx.notify();
                        })
                    });

                    let del_local_path = PathBuf::from(&main_repo_path);
                    let del_local_branch = branch.clone();
                    let del_local_result = smol::unblock(move || {
                        git::delete_local_branch(&del_local_path, &del_local_branch)
                    })
                    .await;

                    if let Err(e) = del_local_result {
                        log::warn!("Delete local branch failed (continuing): {}", e);
                    }

                    let del_remote_path = PathBuf::from(&main_repo_path);
                    let del_remote_branch = branch.clone();
                    let del_remote_result = smol::unblock(move || {
                        git::delete_remote_branch(&del_remote_path, &del_remote_branch)
                    })
                    .await;

                    if let Err(e) = del_remote_result {
                        log::warn!("Delete remote branch failed (continuing): {}", e);
                    }
                }
            }

            let force_remove = is_dirty && !did_stash;

            // Step 2: before_worktree_remove hook
            // If the hook exists and we have a runner, fire it as a visible PTY terminal
            // and register a pending close — the actual removal happens when the hook exits.
            // If no hook or no runner, proceed with immediate removal.
            let has_before_remove_hook =
                project_hooks.worktree.before_remove.is_some() || global_hooks.worktree.before_remove.is_some();

            if has_before_remove_hook && runner.is_some() {
                // Fire hook as visible PTY terminal and defer removal
                let ok = cx.update(|cx| {
                    let hook_results = hooks::fire_before_worktree_remove_async(
                        &project_hooks,
                        &global_hooks,
                        &project_id,
                        &project_name,
                        &project_path,
                        &branch,
                        &main_repo_path,
                        folder_id.as_deref(),
                        folder_name.as_deref(),
                        monitor.as_ref(),
                        runner.as_ref(),
                    );

                    let pending_terminal_id = hook_results.first().map(|r| r.terminal_id.clone());

                    if pending_terminal_id.is_some() {
                        workspace.update(cx, |ws, cx| {
                            ws.register_hook_results(hook_results, cx);

                            // Register pending close — PTY exit handler will complete it
                            if let Some(hook_terminal_id) = pending_terminal_id {
                                ws.register_pending_worktree_close(PendingWorktreeClose {
                                    project_id: project_id.clone(),
                                    hook_terminal_id,
                                    branch: branch.clone(),
                                    main_repo_path: main_repo_path.clone(),
                                });
                            }
                        });

                        // Close dialog — removal will happen when hook exits
                        let _ = this.update(cx, |this, cx| {
                            this.close(cx);
                        });
                        true
                    } else {
                        // Hook terminal failed to spawn — abort, don't remove
                        let _ = this.update(cx, |this, cx| {
                            this.error_message = Some("before_worktree_remove hook failed to start".into());
                            this.processing = ProcessingState::Idle;
                            cx.notify();
                        });
                        false
                    }
                });
                if !ok {
                    return;
                }
            } else {
                // No hook or no runner — run headlessly then remove immediately
                if has_before_remove_hook {
                    let before_remove_result = smol::unblock({
                        let project_hooks = project_hooks.clone();
                        let global_hooks = global_hooks.clone();
                        let project_id = project_id.clone();
                        let project_name = project_name.clone();
                        let project_path = project_path.clone();
                        let branch = branch.clone();
                        let main_repo_path = main_repo_path.clone();
                        let folder_id = folder_id.clone();
                        let folder_name = folder_name.clone();
                        let monitor = monitor.clone();
                        move || {
                            hooks::fire_before_worktree_remove(
                                &project_hooks,
                                &global_hooks,
                                &project_id,
                                &project_name,
                                &project_path,
                                &branch,
                                &main_repo_path,
                                folder_id.as_deref(),
                                folder_name.as_deref(),
                                monitor.as_ref(),
                                None,
                            )
                        }
                    })
                    .await;

                    if let Err(e) = before_remove_result {
                        let _ = cx.update(|cx| {
                            this.update(cx, |this, cx| {
                                this.error_message =
                                    Some(format!("before_worktree_remove hook failed: {}", e));
                                this.processing = ProcessingState::Idle;
                                cx.notify();
                            })
                        });
                        return;
                    }
                }

                // Fire on_dirty_worktree_close hook when closing dirty worktree without stash
                if force_remove {
                    let (terminal_actions, hook_results) = hooks::fire_on_dirty_worktree_close(
                        &project_hooks,
                        &global_hooks,
                        &project_id,
                        &project_name,
                        &project_path,
                        &branch,
                        folder_id.as_deref(),
                        folder_name.as_deref(),
                        monitor.as_ref(),
                        runner.as_ref(),
                    );
                    let _ = cx.update(|cx| {
                        workspace.update(cx, |ws, cx| {
                            for (cmd, env) in terminal_actions {
                                ws.add_terminal_with_command(&project_id, &cmd, &env, cx);
                            }
                            ws.register_hook_results(hook_results, cx);
                        })
                    });
                }

                let _ = cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        this.processing = ProcessingState::Removing;
                        cx.notify();
                    })
                });

                cx.update(|cx| {
                    let result = workspace.update(cx, |ws, cx| {
                        ws.remove_worktree_project(&project_id, force_remove, &global_hooks, cx)
                    });

                    match result {
                        Ok(()) => {
                            let _ = hooks::fire_worktree_removed(
                                &project_hooks,
                                &global_hooks,
                                &project_id,
                                &project_name,
                                &project_path,
                                &branch,
                                &main_repo_path,
                                folder_id.as_deref(),
                                folder_name.as_deref(),
                                monitor.as_ref(),
                                runner.as_ref(),
                            );

                            let _ = this.update(cx, |this, cx| {
                                this.close(cx);
                            });
                        }
                        Err(e) => {
                            let _ = this.update(cx, |this, cx| {
                                this.error_message = Some(format!("Failed to remove worktree: {}", e));
                                this.processing = ProcessingState::Idle;
                                cx.notify();
                            });
                        }
                    }
                });
            }
        })
        .detach();
    }
}
