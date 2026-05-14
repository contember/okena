//! Render impl for CloseWorktreeDialog — the modal form layout itself
//! (header, project info, dirty/unpushed warnings, merge/stash checkboxes,
//! merge sub-options, status banner, footer buttons).

use super::{CloseWorktreeDialog, ProcessingState};
use crate::Cancel;

use okena_files::theme::theme;
use okena_ui::button::{button, button_primary};
use okena_ui::modal::{modal_backdrop, modal_content};
use okena_ui::tokens::{ui_text, ui_text_md, ui_text_ms, ui_text_sm, ui_text_xl};

use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;

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
                                            .text_size(ui_text_xl(cx))
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
                                            .text_size(ui_text(13.0, cx))
                                            .text_color(rgb(t.text_primary))
                                            .child(format!(
                                                "Project: {} ({})",
                                                self.project_name, branch_display
                                            )),
                                    )
                                    .child(
                                        div()
                                            .text_size(ui_text_ms(cx))
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
                                                .text_size(ui_text_md(cx))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(0xffaa33))
                                                .child("This worktree has uncommitted changes."),
                                        )
                                        .child(
                                            div()
                                                .text_size(ui_text_ms(cx))
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
                                                .text_size(ui_text_md(cx))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(0xffaa33))
                                                .child(format!(
                                                    "{} unpushed commit(s) on this branch.",
                                                    self.unpushed_count
                                                )),
                                        )
                                        .child(
                                            div()
                                                .text_size(ui_text_ms(cx))
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
                                                        .text_size(ui_text_md(cx))
                                                        .text_color(if can_merge {
                                                            rgb(t.text_primary)
                                                        } else {
                                                            rgb(t.text_muted)
                                                        })
                                                        .child(merge_label),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(ui_text_sm(cx))
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
                                                        .text_size(ui_text_md(cx))
                                                        .text_color(rgb(t.text_primary))
                                                        .child("Stash changes before merge"),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(ui_text_sm(cx))
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
                                                                .text_size(ui_text_md(cx))
                                                                .text_color(rgb(t.text_primary))
                                                                .child("Fetch remote before rebase"),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(ui_text_sm(cx))
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
                                                                .text_size(ui_text_md(cx))
                                                                .text_color(rgb(t.text_primary))
                                                                .child("Delete branch after merge"),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(ui_text_sm(cx))
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
                                                                .text_size(ui_text_md(cx))
                                                                .text_color(rgb(t.text_primary))
                                                                .child("Push target branch after merge"),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(ui_text_sm(cx))
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
                                        .text_size(ui_text_md(cx))
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
                                .text_size(ui_text_md(cx))
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
