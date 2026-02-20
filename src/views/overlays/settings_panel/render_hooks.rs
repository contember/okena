use crate::theme::theme;
use gpui::*;

use super::components::*;
use super::SettingsPanel;

impl SettingsPanel {
    pub(super) fn render_hooks(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let is_project = self.selected_project_id.is_some();

        let (h1, h2, h3, h4, h5, h6, h7, h8, h9, h10) = if is_project {
            (
                self.project_hook_project_open.clone(),
                self.project_hook_project_close.clone(),
                self.project_hook_worktree_create.clone(),
                self.project_hook_worktree_close.clone(),
                self.project_hook_pre_merge.clone(),
                self.project_hook_post_merge.clone(),
                self.project_hook_before_worktree_remove.clone(),
                self.project_hook_worktree_removed.clone(),
                self.project_hook_on_rebase_conflict.clone(),
                self.project_hook_on_dirty_worktree_close.clone(),
            )
        } else {
            (
                self.hook_project_open.clone(),
                self.hook_project_close.clone(),
                self.hook_worktree_create.clone(),
                self.hook_worktree_close.clone(),
                self.hook_pre_merge.clone(),
                self.hook_post_merge.clone(),
                self.hook_before_worktree_remove.clone(),
                self.hook_worktree_removed.clone(),
                self.hook_on_rebase_conflict.clone(),
                self.hook_on_dirty_worktree_close.clone(),
            )
        };

        let scope_label = if is_project { "Project Hooks (override global)" } else { "Global Hooks" };
        let env_note = "Available env: $OKENA_PROJECT_ID, $OKENA_PROJECT_NAME, $OKENA_PROJECT_PATH";
        let merge_env_note = "Extra env: $OKENA_BRANCH, $OKENA_TARGET_BRANCH, $OKENA_MAIN_REPO_PATH";
        let multiline_hint = "Use multiple lines to chain actions. Prefix with terminal: to open in a terminal pane.";

        div()
            .child(section_header(scope_label, &t))
            .child(
                div()
                    .mx(px(16.0))
                    .mb(px(4.0))
                    .text_size(px(10.0))
                    .text_color(rgb(t.text_muted))
                    .child(env_note),
            )
            .child(
                div()
                    .mx(px(16.0))
                    .mb(px(8.0))
                    .text_size(px(10.0))
                    .text_color(rgb(t.text_muted))
                    .child(multiline_hint),
            )
            .child(
                section_container(&t)
                    .child(hook_input_row(
                        "hook-project-open", "On Project Open",
                        "Command to run when a project is opened",
                        &h1, "", &t, true,
                    ))
                    .child(hook_input_row(
                        "hook-project-close", "On Project Close",
                        "Command to run when a project is closed",
                        &h2, "", &t, true,
                    ))
                    .child(hook_input_row(
                        "hook-worktree-create", "On Worktree Create",
                        "Command to run after a git worktree is created",
                        &h3, "", &t, true,
                    ))
                    .child(hook_input_row(
                        "hook-worktree-close", "On Worktree Close",
                        "Command to run after a git worktree is removed",
                        &h4, "", &t, false,
                    )),
            )
            .child(section_header("Merge & Removal Hooks", &t))
            .child(
                div()
                    .mx(px(16.0))
                    .mb(px(8.0))
                    .text_size(px(10.0))
                    .text_color(rgb(t.text_muted))
                    .child(merge_env_note),
            )
            .child(
                section_container(&t)
                    .child(hook_input_row(
                        "hook-pre-merge", "Pre Merge",
                        "Sync hook before merge begins (abort on failure)",
                        &h5, "", &t, true,
                    ))
                    .child(hook_input_row(
                        "hook-post-merge", "Post Merge",
                        "Async hook after successful merge",
                        &h6, "", &t, true,
                    ))
                    .child(hook_input_row(
                        "hook-before-worktree-remove", "Before Worktree Remove",
                        "Sync hook before worktree is deleted (abort on failure)",
                        &h7, "", &t, true,
                    ))
                    .child(hook_input_row(
                        "hook-worktree-removed", "Worktree Removed",
                        "Async hook after worktree is deleted",
                        &h8, "", &t, true,
                    ))
                    .child(hook_input_row(
                        "hook-on-rebase-conflict", "On Rebase Conflict",
                        "Async hook when rebase fails (env: $OKENA_REBASE_ERROR)",
                        &h9, "", &t, true,
                    ))
                    .child(hook_input_row(
                        "hook-on-dirty-worktree-close", "On Dirty Worktree Close",
                        "Async hook when closing worktree with uncommitted changes",
                        &h10, "", &t, false,
                    )),
            )
    }
}
