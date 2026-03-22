//! Worktree list popover for showing/hiding worktrees in the sidebar

use crate::settings::settings;
use crate::theme::theme;
use gpui::*;
use gpui::prelude::*;

use super::Sidebar;

impl Sidebar {
    /// Render the worktree list popover for a project.
    /// Shows all git worktrees with checkboxes to toggle sidebar visibility.
    /// Uses cached entries from `self.worktree_list_entries` (computed in `show_worktree_list`).
    pub(super) fn render_worktree_list(&self, project_id: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let ws = self.workspace.read(cx);

        // Get parent project path (for normalization comparison)
        let project_path = ws.project(project_id)
            .map(|p| p.path.clone())
            .unwrap_or_default();

        // Use cached git worktrees
        let git_worktrees = &self.worktree_list_entries;

        // Build set of worktree paths already tracked in workspace
        let tracked_wt_paths: std::collections::HashSet<String> = ws.data().projects.iter()
            .filter(|p| p.worktree_info.as_ref()
                .map_or(false, |wt| wt.parent_project_id == project_id))
            .map(|p| p.path.clone())
            .collect();

        // Filter: skip the main repo itself (first entry usually matches project_path)
        let worktrees: Vec<(String, String, bool)> = git_worktrees.iter()
            .filter(|(wt_path, _)| {
                // Skip the main worktree (same path as parent project)
                let norm_wt = crate::git::repository::normalize_path(std::path::Path::new(wt_path));
                let norm_proj = crate::git::repository::normalize_path(std::path::Path::new(&project_path));
                norm_wt != norm_proj
            })
            .map(|(wt_path, branch)| {
                let is_tracked = tracked_wt_paths.contains(wt_path);
                (wt_path.clone(), branch.clone(), is_tracked)
            })
            .collect();

        okena_ui::popover::popover_panel("worktree-list-panel", &t)
            .w(px(280.0))
            .max_h(px(400.0))
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_secondary))
                    .pb(px(6.0))
                    .child("WORKTREES")
            )
            .when(worktrees.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(t.text_muted))
                        .py(px(8.0))
                        .child("No worktrees found")
                )
            })
            .children(worktrees.into_iter().map(|(wt_path, branch, is_tracked)| {
                let project_id = project_id.to_string();
                let wt_path_clone = wt_path.clone();
                let branch_clone = branch.clone();

                div()
                    .id(ElementId::Name(format!("wt-list-{}", wt_path).into()))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(4.0))
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        if is_tracked {
                            // Remove: find the project by path and delete it
                            let ws = this.workspace.read(cx);
                            let wt_project_id = ws.data().projects.iter()
                                .find(|p| p.path == wt_path_clone && p.worktree_info.as_ref()
                                    .map_or(false, |wt| wt.parent_project_id == project_id))
                                .map(|p| p.id.clone());
                            if let Some(id) = wt_project_id {
                                this.workspace.update(cx, |ws, cx| {
                                    ws.delete_project(&id, &settings(cx).hooks, cx);
                                });
                            }
                        } else {
                            // Add: register as a discovered worktree and link to parent
                            this.workspace.update(cx, |ws, cx| {
                                ws.add_discovered_worktree(
                                    &wt_path_clone,
                                    &branch_clone,
                                    &project_id,
                                    "",
                                );
                                // Find the newly added project ID and add to parent's worktree_ids
                                let new_id = ws.data().projects.iter()
                                    .find(|p| p.path == wt_path_clone)
                                    .map(|p| p.id.clone());
                                if let Some(new_id) = new_id {
                                    ws.add_to_worktree_ids(&project_id, &new_id);
                                }
                                ws.notify_data(cx);
                            });
                        }
                        cx.notify();
                    }))
                    .child(
                        // Checkbox indicator
                        div()
                            .flex_shrink_0()
                            .w(px(14.0))
                            .h(px(14.0))
                            .rounded(px(3.0))
                            .border_1()
                            .border_color(rgb(if is_tracked { t.border_active } else { t.border }))
                            .when(is_tracked, |d| d.bg(rgb(t.border_active)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(is_tracked, |d| {
                                d.child(
                                    svg()
                                        .path("icons/check.svg")
                                        .size(px(10.0))
                                        .text_color(rgb(t.bg_primary))
                                )
                            })
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(t.text_primary))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(branch.clone())
                            )
                    )
            }))
    }
}
