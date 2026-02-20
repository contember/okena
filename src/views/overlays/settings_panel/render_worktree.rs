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

        div()
            .child(section_header("Path", &t))
            .child(
                section_container(&t)
                    .child(
                        hook_input_row(
                            "worktree-path-template",
                            "Path Template",
                            "relative to project dir. {repo} = repo name, {branch} = branch",
                            &self.worktree_dir_suffix_input,
                            "",
                            &t,
                            false,
                        ),
                    ),
            )
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
