use crate::git;
use crate::keybindings::Cancel;
use crate::settings::settings;
use crate::theme::theme;
use crate::views::components::{button, button_primary, modal_backdrop, modal_content};
use crate::workspace::hooks;
use crate::workspace::state::Workspace;
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use std::path::PathBuf;

/// Events emitted by the close worktree dialog
#[derive(Clone)]
pub enum CloseWorktreeDialogEvent {
    /// Dialog closed (either cancelled or worktree was removed)
    Closed,
}

impl EventEmitter<CloseWorktreeDialogEvent> for CloseWorktreeDialog {}

/// Processing state for async operations
#[derive(Clone, Debug, PartialEq)]
enum ProcessingState {
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
    workspace: Entity<Workspace>,
    focus_handle: FocusHandle,
    project_id: String,
    project_name: String,
    project_path: String,
    branch: Option<String>,
    default_branch: Option<String>,
    main_repo_path: Option<String>,
    is_dirty: bool,
    merge_enabled: bool,
    stash_enabled: bool,
    fetch_enabled: bool,
    delete_branch_enabled: bool,
    push_enabled: bool,
    unpushed_count: usize,
    error_message: Option<String>,
    processing: ProcessingState,
}

impl CloseWorktreeDialog {
    pub fn new(
        workspace: Entity<Workspace>,
        project_id: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let ws = workspace.read(cx);
        let project = ws.project(&project_id);

        let project_name = project.map(|p| p.name.clone()).unwrap_or_default();
        let project_path = project.map(|p| p.path.clone()).unwrap_or_default();
        let main_repo_path = project
            .and_then(|p| p.worktree_info.as_ref())
            .map(|wt| wt.main_repo_path.clone());

        let path = PathBuf::from(&project_path);
        let is_dirty = git::has_uncommitted_changes(&path);
        let branch = git::get_current_branch(&path);
        let default_branch = main_repo_path
            .as_ref()
            .and_then(|p| git::get_default_branch(&PathBuf::from(p)));
        let unpushed_count = git::count_unpushed_commits(&path);
        let wt_config = &settings(cx).worktree;

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
            merge_enabled: wt_config.default_merge,
            stash_enabled: wt_config.default_stash,
            fetch_enabled: wt_config.default_fetch,
            delete_branch_enabled: wt_config.default_delete_branch,
            push_enabled: wt_config.default_push,
            unpushed_count,
            error_message: None,
            processing: ProcessingState::Idle,
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(CloseWorktreeDialogEvent::Closed);
    }

    fn can_merge(&self) -> bool {
        (!self.is_dirty || self.stash_enabled)
            && self.branch.is_some()
            && self.default_branch.is_some()
    }

    fn confirm_label(&self) -> &'static str {
        if self.merge_enabled && self.can_merge() {
            "Merge & Close"
        } else {
            "Close Worktree"
        }
    }

    fn execute(&mut self, cx: &mut Context<Self>) {
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

        // Read hooks config before spawning
        let ws = workspace.read(cx);
        let project_hooks = ws
            .project(&project_id)
            .map(|p| p.hooks.clone())
            .unwrap_or_default();
        let global_hooks = settings(cx).hooks;

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
                    move || {
                        hooks::fire_pre_merge(
                            &project_hooks,
                            &global_hooks,
                            &project_id,
                            &project_name,
                            &project_path,
                            &branch,
                            &default_branch,
                            &main_repo_path,
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
                    let terminal_actions = hooks::fire_on_rebase_conflict(
                        &project_hooks,
                        &global_hooks,
                        &project_id,
                        &project_name,
                        &project_path,
                        &branch,
                        &default_branch,
                        &main_repo_path,
                        &e,
                    );
                    for (cmd, env) in terminal_actions {
                        let project_id = project_id.clone();
                        let _ = cx.update(|cx| {
                            workspace.update(cx, |ws, cx| {
                                ws.add_terminal_with_command(&project_id, &cmd, &env, cx);
                            })
                        });
                    }

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
                hooks::fire_post_merge(
                    &project_hooks,
                    &global_hooks,
                    &project_id,
                    &project_name,
                    &project_path,
                    &branch,
                    &default_branch,
                    &main_repo_path,
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

            // Step 2: before_worktree_remove hook (sync)
            let before_remove_result = smol::unblock({
                let project_hooks = project_hooks.clone();
                let global_hooks = global_hooks.clone();
                let project_id = project_id.clone();
                let project_name = project_name.clone();
                let project_path = project_path.clone();
                let branch = branch.clone();
                let main_repo_path = main_repo_path.clone();
                move || {
                    hooks::fire_before_worktree_remove(
                        &project_hooks,
                        &global_hooks,
                        &project_id,
                        &project_name,
                        &project_path,
                        &branch,
                        &main_repo_path,
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

            // Step 3: Remove worktree
            let _ = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    this.processing = ProcessingState::Removing;
                    cx.notify();
                })
            });

            let force_remove = is_dirty && !did_stash;

            // Fire on_dirty_worktree_close hook when closing dirty worktree without stash
            if force_remove {
                let terminal_actions = hooks::fire_on_dirty_worktree_close(
                    &project_hooks,
                    &global_hooks,
                    &project_id,
                    &project_name,
                    &project_path,
                    &branch,
                );
                for (cmd, env) in terminal_actions {
                    let project_id = project_id.clone();
                    let _ = cx.update(|cx| {
                        workspace.update(cx, |ws, cx| {
                            ws.add_terminal_with_command(&project_id, &cmd, &env, cx);
                        })
                    });
                }
            }

            cx.update(|cx| {
                let result = workspace.update(cx, |ws, cx| {
                    ws.remove_worktree_project(&project_id, force_remove, cx)
                });

                match result {
                    Ok(()) => {
                        // Step 4: worktree_removed hook (async)
                        hooks::fire_worktree_removed(
                            &project_hooks,
                            &global_hooks,
                            &project_id,
                            &project_name,
                            &project_path,
                            &branch,
                            &main_repo_path,
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
        })
        .detach();
    }
}

impl_focusable!(CloseWorktreeDialog);

impl Render for CloseWorktreeDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();

        if !focus_handle.contains_focused(window, cx) {
            window.focus(&focus_handle, cx);
        }

        let is_processing = self.processing != ProcessingState::Idle;

        let status_text = match &self.processing {
            ProcessingState::Stashing => Some("Stashing changes..."),
            ProcessingState::Fetching => Some("Fetching remote..."),
            ProcessingState::Rebasing => Some("Rebasing..."),
            ProcessingState::Merging => Some("Merging..."),
            ProcessingState::Pushing => Some("Pushing branch..."),
            ProcessingState::DeletingBranch => Some("Deleting branch..."),
            ProcessingState::Removing => Some("Removing worktree..."),
            ProcessingState::Idle => None,
        };

        let branch_display = self.branch.clone().unwrap_or_else(|| "detached".into());
        let default_branch_display = self.default_branch.clone().unwrap_or_else(|| "main".into());
        let can_merge = self.can_merge();
        let confirm_label = self.confirm_label();
        let error_msg = self.error_message.clone();

        modal_backdrop("close-worktree-dialog-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("CloseWorktreeDialog")
            .items_center()
            .on_action(cx.listener(|this, _: &Cancel, _, cx| {
                if this.processing == ProcessingState::Idle {
                    this.close(cx);
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                if this.processing == ProcessingState::Idle {
                    this.close(cx);
                }
            }))
            .child(
                modal_content("close-worktree-dialog", &t)
                    .w(px(450.0))
                    .max_h(px(600.0))
                    .overflow_y_scroll()
                    // Header
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .border_b_1()
                            .border_color(rgb(t.border))
                            .child(
                                h_flex()
                                    .gap(px(8.0))
                                    .child(
                                        svg()
                                            .path("icons/git-branch.svg")
                                            .size(px(16.0))
                                            .text_color(rgb(t.border_active)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(14.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(t.text_primary))
                                            .child("Close Worktree"),
                                    ),
                            )
                            .child(
                                div()
                                    .id("close-dialog-x-btn")
                                    .cursor_pointer()
                                    .w(px(24.0))
                                    .h(px(24.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.0))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .child(
                                        svg()
                                            .path("icons/close.svg")
                                            .size(px(14.0))
                                            .text_color(rgb(t.text_secondary)),
                                    )
                                    .when(!is_processing, |d| {
                                        d.on_click(cx.listener(|this, _, _, cx| {
                                            this.close(cx);
                                        }))
                                    }),
                            ),
                    )
                    // Content
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            // Project info
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .text_color(rgb(t.text_primary))
                                            .child(format!(
                                                "Project: {} ({})",
                                                self.project_name, branch_display
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(rgb(t.text_muted))
                                            .child(format!("Path: {}", self.project_path)),
                                    ),
                            )
                            // Dirty warning
                            .when(self.is_dirty, |d| {
                                d.child(
                                    div()
                                        .px(px(10.0))
                                        .py(px(8.0))
                                        .rounded(px(4.0))
                                        .bg(rgba(0xff990015))
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.0))
                                        .child(
                                            div()
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(0xffaa33))
                                                .child("This worktree has uncommitted changes."),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .text_color(rgb(0xffaa33))
                                                .child("They will be lost if you proceed."),
                                        ),
                                )
                            })
                            // Unpushed commits warning
                            .when(self.unpushed_count > 0 && !self.merge_enabled, |d| {
                                d.child(
                                    div()
                                        .px(px(10.0))
                                        .py(px(8.0))
                                        .rounded(px(4.0))
                                        .bg(rgba(0xff990015))
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.0))
                                        .child(
                                            div()
                                                .text_size(px(12.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(0xffaa33))
                                                .child(format!(
                                                    "{} unpushed commit(s) on this branch.",
                                                    self.unpushed_count
                                                )),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .text_color(rgb(0xffaa33))
                                                .child("Enable merge to preserve them, or they will remain on the unmerged branch."),
                                        ),
                                )
                            })
                            // Merge checkbox
                            .when(self.branch.is_some() && self.default_branch.is_some(), |d| {
                                let merge_label = format!(
                                    "Merge {} into {}",
                                    branch_display, default_branch_display
                                );
                                d.child(
                                    div()
                                        .id("merge-checkbox-row")
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        .py(px(4.0))
                                        .cursor(if can_merge && !is_processing {
                                            CursorStyle::PointingHand
                                        } else {
                                            CursorStyle::default()
                                        })
                                        .when(can_merge && !is_processing, |d| {
                                            d.on_click(cx.listener(|this, _, _, cx| {
                                                this.merge_enabled = !this.merge_enabled;
                                                this.error_message = None;
                                                cx.notify();
                                            }))
                                        })
                                        .child(
                                            div()
                                                .w(px(16.0))
                                                .h(px(16.0))
                                                .rounded(px(3.0))
                                                .border_1()
                                                .border_color(if can_merge {
                                                    rgb(t.border_active)
                                                } else {
                                                    rgb(t.border)
                                                })
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .when(self.merge_enabled && can_merge, |d| {
                                                    d.bg(rgb(t.border_active)).child(
                                                        svg()
                                                            .path("icons/check.svg")
                                                            .size(px(12.0))
                                                            .text_color(rgb(t.text_primary)),
                                                    )
                                                }),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap(px(1.0))
                                                .child(
                                                    div()
                                                        .text_size(px(12.0))
                                                        .text_color(if can_merge {
                                                            rgb(t.text_primary)
                                                        } else {
                                                            rgb(t.text_muted)
                                                        })
                                                        .child(merge_label),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(10.0))
                                                        .text_color(rgb(t.text_muted))
                                                        .child(if can_merge {
                                                            "rebase + merge commit"
                                                        } else {
                                                            "disabled: uncommitted changes"
                                                        }),
                                                ),
                                        ),
                                )
                            })
                            // Stash checkbox
                            .when(self.is_dirty && self.branch.is_some() && self.default_branch.is_some(), |d| {
                                d.child(
                                    div()
                                        .id("stash-checkbox-row")
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        .py(px(4.0))
                                        .cursor(if !is_processing {
                                            CursorStyle::PointingHand
                                        } else {
                                            CursorStyle::default()
                                        })
                                        .when(!is_processing, |d| {
                                            d.on_click(cx.listener(|this, _, _, cx| {
                                                this.stash_enabled = !this.stash_enabled;
                                                this.error_message = None;
                                                cx.notify();
                                            }))
                                        })
                                        .child(
                                            div()
                                                .w(px(16.0))
                                                .h(px(16.0))
                                                .rounded(px(3.0))
                                                .border_1()
                                                .border_color(rgb(t.border_active))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .when(self.stash_enabled, |d| {
                                                    d.bg(rgb(t.border_active)).child(
                                                        svg()
                                                            .path("icons/check.svg")
                                                            .size(px(12.0))
                                                            .text_color(rgb(t.text_primary)),
                                                    )
                                                }),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap(px(1.0))
                                                .child(
                                                    div()
                                                        .text_size(px(12.0))
                                                        .text_color(rgb(t.text_primary))
                                                        .child("Stash changes before merge"),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(10.0))
                                                        .text_color(rgb(t.text_muted))
                                                        .child("Auto-pop on failure"),
                                                ),
                                        ),
                                )
                            })
                            // Merge sub-options (fetch, delete branch, push)
                            .when(self.merge_enabled && can_merge, |d| {
                                d.child(
                                    div()
                                        .pl(px(8.0))
                                        .flex()
                                        .flex_col()
                                        .gap(px(4.0))
                                        // Fetch checkbox
                                        .child(
                                            div()
                                                .id("fetch-checkbox-row")
                                                .flex()
                                                .items_center()
                                                .gap(px(8.0))
                                                .py(px(4.0))
                                                .cursor(if !is_processing {
                                                    CursorStyle::PointingHand
                                                } else {
                                                    CursorStyle::default()
                                                })
                                                .when(!is_processing, |d| {
                                                    d.on_click(cx.listener(|this, _, _, cx| {
                                                        this.fetch_enabled = !this.fetch_enabled;
                                                        cx.notify();
                                                    }))
                                                })
                                                .child(
                                                    div()
                                                        .w(px(16.0))
                                                        .h(px(16.0))
                                                        .rounded(px(3.0))
                                                        .border_1()
                                                        .border_color(rgb(t.border_active))
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .when(self.fetch_enabled, |d| {
                                                            d.bg(rgb(t.border_active)).child(
                                                                svg()
                                                                    .path("icons/check.svg")
                                                                    .size(px(12.0))
                                                                    .text_color(rgb(t.text_primary)),
                                                            )
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .gap(px(1.0))
                                                        .child(
                                                            div()
                                                                .text_size(px(12.0))
                                                                .text_color(rgb(t.text_primary))
                                                                .child("Fetch remote before rebase"),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(px(10.0))
                                                                .text_color(rgb(t.text_muted))
                                                                .child("git fetch --all"),
                                                        ),
                                                ),
                                        )
                                        // Delete branch checkbox
                                        .child(
                                            div()
                                                .id("delete-branch-checkbox-row")
                                                .flex()
                                                .items_center()
                                                .gap(px(8.0))
                                                .py(px(4.0))
                                                .cursor(if !is_processing {
                                                    CursorStyle::PointingHand
                                                } else {
                                                    CursorStyle::default()
                                                })
                                                .when(!is_processing, |d| {
                                                    d.on_click(cx.listener(|this, _, _, cx| {
                                                        this.delete_branch_enabled =
                                                            !this.delete_branch_enabled;
                                                        cx.notify();
                                                    }))
                                                })
                                                .child(
                                                    div()
                                                        .w(px(16.0))
                                                        .h(px(16.0))
                                                        .rounded(px(3.0))
                                                        .border_1()
                                                        .border_color(rgb(t.border_active))
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .when(self.delete_branch_enabled, |d| {
                                                            d.bg(rgb(t.border_active)).child(
                                                                svg()
                                                                    .path("icons/check.svg")
                                                                    .size(px(12.0))
                                                                    .text_color(rgb(t.text_primary)),
                                                            )
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .gap(px(1.0))
                                                        .child(
                                                            div()
                                                                .text_size(px(12.0))
                                                                .text_color(rgb(t.text_primary))
                                                                .child("Delete branch after merge"),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(px(10.0))
                                                                .text_color(rgb(t.text_muted))
                                                                .child("local + remote"),
                                                        ),
                                                ),
                                        )
                                        // Push checkbox
                                        .child(
                                            div()
                                                .id("push-checkbox-row")
                                                .flex()
                                                .items_center()
                                                .gap(px(8.0))
                                                .py(px(4.0))
                                                .cursor(if !is_processing {
                                                    CursorStyle::PointingHand
                                                } else {
                                                    CursorStyle::default()
                                                })
                                                .when(!is_processing, |d| {
                                                    d.on_click(cx.listener(|this, _, _, cx| {
                                                        this.push_enabled = !this.push_enabled;
                                                        cx.notify();
                                                    }))
                                                })
                                                .child(
                                                    div()
                                                        .w(px(16.0))
                                                        .h(px(16.0))
                                                        .rounded(px(3.0))
                                                        .border_1()
                                                        .border_color(rgb(t.border_active))
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .when(self.push_enabled, |d| {
                                                            d.bg(rgb(t.border_active)).child(
                                                                svg()
                                                                    .path("icons/check.svg")
                                                                    .size(px(12.0))
                                                                    .text_color(rgb(t.text_primary)),
                                                            )
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .gap(px(1.0))
                                                        .child(
                                                            div()
                                                                .text_size(px(12.0))
                                                                .text_color(rgb(t.text_primary))
                                                                .child("Push target branch after merge"),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(px(10.0))
                                                                .text_color(rgb(t.text_muted))
                                                                .child(format!(
                                                                    "git push origin {}",
                                                                    self.default_branch
                                                                        .as_deref()
                                                                        .unwrap_or("main")
                                                                )),
                                                        ),
                                                ),
                                        ),
                                )
                            })
                            // Status message
                            .when_some(status_text, |d, text| {
                                d.child(
                                    div()
                                        .px(px(10.0))
                                        .py(px(6.0))
                                        .rounded(px(4.0))
                                        .bg(rgba(0x3399ff15))
                                        .text_size(px(12.0))
                                        .text_color(rgb(t.border_active))
                                        .child(text),
                                )
                            }),
                    )
                    // Error message
                    .when_some(error_msg, |d, msg| {
                        d.child(
                            div()
                                .px(px(16.0))
                                .py(px(8.0))
                                .bg(rgba(0xff00001a))
                                .text_size(px(12.0))
                                .text_color(rgb(t.error))
                                .child(msg),
                        )
                    })
                    // Footer
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .flex()
                            .justify_end()
                            .gap(px(8.0))
                            .border_t_1()
                            .border_color(rgb(t.border))
                            .child(
                                button("cancel-close-wt-btn", "Cancel", &t)
                                    .px(px(16.0))
                                    .py(px(8.0))
                                    .when(!is_processing, |d| {
                                        d.on_click(cx.listener(|this, _, _, cx| {
                                            this.close(cx);
                                        }))
                                    })
                                    .when(is_processing, |d| {
                                        d.opacity(0.5).cursor(CursorStyle::default())
                                    }),
                            )
                            .child(
                                button_primary("confirm-close-wt-btn", confirm_label, &t)
                                    .px(px(16.0))
                                    .py(px(8.0))
                                    .when(!is_processing, |d| {
                                        d.on_click(cx.listener(|this, _, _, cx| {
                                            this.execute(cx);
                                        }))
                                    })
                                    .when(is_processing, |d| {
                                        d.opacity(0.5).cursor(CursorStyle::default())
                                    }),
                            ),
                    ),
            )
    }
}
