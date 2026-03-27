//! Worktree list popover — standalone overlay entity.
//!
//! Shows all git worktrees for a project with checkboxes to toggle sidebar visibility.
//! Rendered at RootView level via OverlayManager, like context menus.

use okena_ui::overlay::CloseEvent;
use okena_ui::theme::theme;
use okena_ui::tokens::{ui_text_ms, ui_text_md};
use okena_workspace::settings::HooksConfig;
use okena_workspace::state::Workspace;
use gpui::*;
use gpui::prelude::*;

use crate::Cancel;

/// Event emitted by WorktreeListPopover.
pub enum WorktreeListPopoverEvent {
    Close,
}

impl CloseEvent for WorktreeListPopoverEvent {
    fn is_close(&self) -> bool { matches!(self, Self::Close) }
}

impl EventEmitter<WorktreeListPopoverEvent> for WorktreeListPopover {}

/// Standalone worktree list popover entity.
pub struct WorktreeListPopover {
    workspace: Entity<Workspace>,
    project_id: String,
    entries: Vec<(String, String)>,
    position: Point<Pixels>,
    hooks: HooksConfig,
    focus_handle: FocusHandle,
    /// Git repository root for the parent project (may differ from project path in monorepos).
    git_root: std::path::PathBuf,
    /// Subdirectory within the git repo (empty for non-monorepo projects).
    subdir: std::path::PathBuf,
}

impl WorktreeListPopover {
    pub fn new(
        workspace: Entity<Workspace>,
        project_id: String,
        position: Point<Pixels>,
        hooks: HooksConfig,
        cx: &mut Context<Self>,
    ) -> Self {
        let project_path = workspace.read(cx).project(&project_id)
            .map(|p| p.path.clone())
            .unwrap_or_default();
        let project_pathbuf = std::path::PathBuf::from(&project_path);
        let git_root = okena_git::get_repo_root(&project_pathbuf)
            .unwrap_or_else(|| project_pathbuf.clone());
        let subdir = project_pathbuf.strip_prefix(&git_root)
            .unwrap_or(std::path::Path::new(""))
            .to_path_buf();
        let entries = okena_git::repository::list_git_worktrees(&git_root);
        let focus_handle = cx.focus_handle();
        Self { workspace, project_id, entries, position, hooks, focus_handle, git_root, subdir }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(WorktreeListPopoverEvent::Close);
    }
}

impl Render for WorktreeListPopover {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        if !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }

        let ws = self.workspace.read(cx);
        let project_id = &self.project_id;
        let subdir = &self.subdir;

        // Build set of project paths already tracked in workspace for this parent.
        // This contains the full project path (which may include a monorepo subdir).
        let tracked_project_paths: std::collections::HashSet<String> = ws.data().projects.iter()
            .filter(|p| p.worktree_info.as_ref()
                .map_or(false, |wt| wt.parent_project_id == *project_id))
            .map(|p| p.path.clone())
            .collect();

        // Filter: skip the main repo itself (compare worktree root against git root,
        // not against the project path which may be a subdirectory in monorepos).
        let norm_git_root = okena_git::repository::normalize_path(&self.git_root);
        let worktrees: Vec<(String, String, bool)> = self.entries.iter()
            .filter(|(wt_path, _)| {
                let norm_wt = okena_git::repository::normalize_path(std::path::Path::new(wt_path));
                norm_wt != norm_git_root
            })
            .map(|(wt_path, branch)| {
                // Compute the expected project path (worktree root + monorepo subdir)
                let expected_path = okena_git::repository::project_path_in_worktree(wt_path, subdir);
                // Check both the expected path and bare worktree root for backwards compat
                let is_tracked = tracked_project_paths.contains(&expected_path)
                    || tracked_project_paths.contains(wt_path);
                (wt_path.clone(), branch.clone(), is_tracked)
            })
            .collect();

        let panel = okena_ui::popover::popover_panel("worktree-list-panel", &t)
            .w(px(280.0))
            .max_h(px(400.0))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_secondary))
                    .pb(px(6.0))
                    .child("WORKTREES")
            )
            .when(worktrees.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(ui_text_md(cx))
                        .text_color(rgb(t.text_muted))
                        .py(px(8.0))
                        .child("No worktrees found")
                )
            })
            .children(worktrees.into_iter().map(|(wt_path, branch, is_tracked)| {
                let project_id = self.project_id.clone();
                let wt_path_clone = wt_path.clone();
                let branch_clone = branch.clone();
                let hooks = self.hooks.clone();

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
                            // Find the tracked project by matching both the expected
                            // project path (with subdir) and the bare worktree root.
                            let expected_path = okena_git::repository::project_path_in_worktree(
                                &wt_path_clone, &this.subdir,
                            );
                            let ws = this.workspace.read(cx);
                            let wt_project_id = ws.data().projects.iter()
                                .find(|p| (p.path == expected_path || p.path == wt_path_clone)
                                    && p.worktree_info.as_ref()
                                        .map_or(false, |wt| wt.parent_project_id == project_id))
                                .map(|p| p.id.clone());
                            if let Some(id) = wt_project_id {
                                this.workspace.update(cx, |ws, cx| {
                                    ws.delete_project(&id, &hooks, cx);
                                });
                            }
                        } else {
                            // add_discovered_worktree computes the correct project
                            // path (worktree root + monorepo subdir) internally.
                            let expected_path = okena_git::repository::project_path_in_worktree(
                                &wt_path_clone, &this.subdir,
                            );
                            this.workspace.update(cx, |ws, cx| {
                                ws.add_discovered_worktree(
                                    &wt_path_clone,
                                    &branch_clone,
                                    &project_id,
                                    "",
                                );
                                let new_id = ws.data().projects.iter()
                                    .find(|p| p.path == expected_path || p.path == wt_path_clone)
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
                                    .text_size(ui_text_md(cx))
                                    .text_color(rgb(t.text_primary))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(branch.clone())
                            )
                    )
            }));

        let position = self.position;

        div()
            .track_focus(&self.focus_handle)
            .key_context("WorktreeListPopover")
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            .absolute()
            .inset_0()
            .occlude()
            .id("worktree-list-backdrop")
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                this.close(cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _window, cx| {
                this.close(cx);
            }))
            .on_scroll_wheel(|_, _, cx| { cx.stop_propagation(); })
            .child(deferred(
                anchored()
                    .position(position)
                    .snap_to_window()
                    .child(panel)
            ))
    }
}
