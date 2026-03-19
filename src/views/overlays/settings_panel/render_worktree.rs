use crate::settings::settings_entity;
use crate::theme::theme;
use gpui::*;

use super::components::*;
use super::SettingsPanel;

impl SettingsPanel {
    pub(super) fn render_worktree(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let s = settings_entity(cx).read(cx).settings.clone();
        let wt = &s.worktree;

        // Count worktrees that don't match the current template (migratable)
        let template = s.worktree.path_template.clone();
        let migratable_count = self.workspace.read(cx).data().projects.iter()
            .filter(|p| p.worktree_info.as_ref()
                .map_or(false, |wt| !wt.worktree_path.is_empty() && !wt.matches_template(&template)))
            .count();

        div()
            .child(section_header("Path", &t))
            .child({
                let container = section_container(&t)
                    .child(
                        hook_input_row(
                            "worktree-path-template",
                            "Path Template",
                            "relative to project dir. {repo} = repo name, {branch} = branch",
                            &self.worktree_dir_suffix_input,
                            "",
                            &t,
                            migratable_count > 0,
                        ),
                    );
                if migratable_count > 0 {
                    container.child(
                        div()
                            .id("migrate-all-worktrees")
                            .px(px(12.0))
                            .py(px(8.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .text_color(rgb(t.text_primary))
                                            .child(format!("Migrate {} worktree(s) to new path", migratable_count))
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.0))
                                            .text_color(rgb(t.text_muted))
                                            .child("Moves existing worktrees to match the template above")
                                    )
                            )
                            .child(
                                div()
                                    .id("migrate-all-btn")
                                    .cursor_pointer()
                                    .px(px(10.0))
                                    .py(px(4.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(t.bg_secondary))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .text_size(px(12.0))
                                    .text_color(rgb(t.text_primary))
                                    .child("Migrate")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        let count = this.workspace.update(cx, |ws, cx| {
                                            ws.migrate_all_worktrees_to_template(cx)
                                        });
                                        if count > 0 {
                                            crate::views::panels::toast::ToastManager::info(
                                                format!("Migrated {} worktree(s) to new path template", count), cx,
                                            );
                                        }
                                        cx.notify();
                                    }))
                            )
                    )
                } else {
                    container
                }
            })
            .child(section_header("Close Defaults", &t))
            .child(
                section_container(&t)
                    .child(self.render_toggle(
                        "wt-default-merge",
                        "Merge into default branch",
                        wt.default_merge,
                        true,
                        |state, val, cx| state.set_worktree_default_merge(val, cx),
                        cx,
                    ))
                    .child(self.render_toggle(
                        "wt-default-stash",
                        "Stash changes before merge",
                        wt.default_stash,
                        true,
                        |state, val, cx| state.set_worktree_default_stash(val, cx),
                        cx,
                    ))
                    .child(self.render_toggle(
                        "wt-default-fetch",
                        "Fetch remote before rebase",
                        wt.default_fetch,
                        true,
                        |state, val, cx| state.set_worktree_default_fetch(val, cx),
                        cx,
                    ))
                    .child(self.render_toggle(
                        "wt-default-push",
                        "Push target branch after merge",
                        wt.default_push,
                        true,
                        |state, val, cx| state.set_worktree_default_push(val, cx),
                        cx,
                    ))
                    .child(self.render_toggle(
                        "wt-default-delete-branch",
                        "Delete branch after merge",
                        wt.default_delete_branch,
                        false,
                        |state, val, cx| state.set_worktree_default_delete_branch(val, cx),
                        cx,
                    )),
            )
    }
}
