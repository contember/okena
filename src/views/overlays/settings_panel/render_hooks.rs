use crate::theme::theme;
use gpui::*;

use super::components::*;
use super::SettingsPanel;

impl SettingsPanel {
    pub(super) fn render_hooks(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let is_project = self.selected_project_id.is_some();

        let (h1, h2, h3, h4, h5, h6, h7, h8, h9, h10, t1, t2, t3) = if is_project {
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
                self.project_hook_terminal_on_create.clone(),
                self.project_hook_terminal_on_close.clone(),
                self.project_hook_terminal_shell_wrapper.clone(),
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
                self.hook_terminal_on_create.clone(),
                self.hook_terminal_on_close.clone(),
                self.hook_terminal_shell_wrapper.clone(),
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
                        "Runs when a project is added to the workspace or on startup",
                        &h1, "", &t, true,
                    ))
                    .child(hook_input_row(
                        "hook-project-close", "On Project Close",
                        "Runs when a project is removed from the workspace",
                        &h2, "", &t, true,
                    ))
                    .child(hook_input_row(
                        "hook-worktree-create", "On Worktree Create",
                        "Runs in the new worktree directory after git worktree add completes",
                        &h3, "", &t, true,
                    ))
                    .child(hook_input_row(
                        "hook-worktree-close", "On Worktree Close",
                        "Runs after a worktree project is removed from the workspace",
                        &h4, "", &t, false,
                    )),
            )
            .child(section_header("Terminal Hooks", &t))
            .child(
                section_container(&t)
                    .child(hook_input_row(
                        "hook-terminal-on-create", "On Terminal Create",
                        "Runs inside each new terminal after the shell starts",
                        &t1, "", &t, true,
                    ))
                    .child(hook_input_row(
                        "hook-terminal-on-close", "On Terminal Close",
                        "Runs when a terminal process exits",
                        &t2, "", &t, true,
                    ))
                    .child(hook_input_row(
                        "hook-terminal-shell-wrapper", "Shell Wrapper",
                        "Wraps the shell invocation. Use {shell} as placeholder for the original command",
                        &t3, "", &t, false,
                    )),
            )
            .child(section_header("Worktree Close Flow", &t))
            .child(
                div()
                    .mx(px(16.0))
                    .mb(px(4.0))
                    .text_size(px(10.0))
                    .text_color(rgb(t.text_muted))
                    .child(merge_env_note),
            )
            .child(
                div()
                    .mx(px(16.0))
                    .mb(px(8.0))
                    .text_size(px(10.0))
                    .text_color(rgb(t.text_muted))
                    .child("Close flow: stash \u{2192} fetch \u{2192} pre merge \u{2192} rebase \u{2192} merge \u{2192} post merge \u{2192} remove"),
            )
            .child(
                section_container(&t)
                    .child(hook_input_row(
                        "hook-pre-merge", "Pre Merge",
                        "Runs before rebase + merge. Blocks the flow \u{2014} non-zero exit aborts the merge",
                        &h5, "", &t, true,
                    ))
                    .child(hook_input_row(
                        "hook-post-merge", "Post Merge",
                        "Runs after branch is merged into the default branch",
                        &h6, "", &t, true,
                    ))
                    .child(hook_input_row(
                        "hook-before-worktree-remove", "Before Worktree Remove",
                        "Runs before git worktree remove. Blocks \u{2014} non-zero exit aborts removal",
                        &h7, "", &t, true,
                    ))
                    .child(hook_input_row(
                        "hook-worktree-removed", "After Worktree Remove",
                        "Runs after the worktree directory is deleted from disk",
                        &h8, "", &t, true,
                    ))
                    .child(hook_input_row(
                        "hook-on-rebase-conflict", "On Rebase Conflict",
                        "Runs when rebase fails due to conflicts. Extra env: $OKENA_REBASE_ERROR",
                        &h9, "", &t, true,
                    ))
                    .child(hook_input_row(
                        "hook-on-dirty-worktree-close", "On Dirty Close",
                        "Runs when closing a worktree that has uncommitted changes without merging",
                        &h10, "", &t, false,
                    )),
            )
    }
}
