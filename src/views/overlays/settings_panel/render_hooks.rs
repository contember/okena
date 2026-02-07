use crate::theme::theme;
use gpui::*;

use super::components::*;
use super::SettingsPanel;

impl SettingsPanel {
    pub(super) fn render_hooks(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let is_project = self.selected_project_id.is_some();

        let (h1, h2, h3, h4) = if is_project {
            (
                self.project_hook_project_open.clone(),
                self.project_hook_project_close.clone(),
                self.project_hook_worktree_create.clone(),
                self.project_hook_worktree_close.clone(),
            )
        } else {
            (
                self.hook_project_open.clone(),
                self.hook_project_close.clone(),
                self.hook_worktree_create.clone(),
                self.hook_worktree_close.clone(),
            )
        };

        let scope_label = if is_project { "Project Hooks (override global)" } else { "Global Hooks" };
        let env_note = "Available env: $TERM_MANAGER_PROJECT_ID, $TERM_MANAGER_PROJECT_NAME, $TERM_MANAGER_PROJECT_PATH";

        div()
            .child(section_header(scope_label, &t))
            .child(
                div()
                    .mx(px(16.0))
                    .mb(px(8.0))
                    .text_size(px(10.0))
                    .text_color(rgb(t.text_muted))
                    .child(env_note),
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
    }
}
