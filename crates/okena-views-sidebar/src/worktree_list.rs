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
        let entries = okena_git::repository::list_git_worktrees(
            std::path::Path::new(&project_path),
        );
        let focus_handle = cx.focus_handle();
        Self { workspace, project_id, entries, position, hooks, focus_handle }
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

        // Get parent project path (for normalization comparison)
        let project_path = ws.project(project_id)
            .map(|p| p.path.clone())
            .unwrap_or_default();

        // Build set of worktree paths already tracked in workspace
        let tracked_wt_paths: std::collections::HashSet<String> = ws.data().projects.iter()
            .filter(|p| p.worktree_info.as_ref()
                .map_or(false, |wt| wt.parent_project_id == *project_id))
            .map(|p| p.path.clone())
            .collect();

        // Filter: skip the main repo itself
        let worktrees: Vec<(String, String, bool)> = self.entries.iter()
            .filter(|(wt_path, _)| {
                let norm_wt = okena_git::repository::normalize_path(std::path::Path::new(wt_path));
                let norm_proj = okena_git::repository::normalize_path(std::path::Path::new(&project_path));
                norm_wt != norm_proj
            })
            .map(|(wt_path, branch)| {
                let is_tracked = tracked_wt_paths.contains(wt_path);
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
                            let ws = this.workspace.read(cx);
                            let wt_project_id = ws.data().projects.iter()
                                .find(|p| p.path == wt_path_clone && p.worktree_info.as_ref()
                                    .map_or(false, |wt| wt.parent_project_id == project_id))
                                .map(|p| p.id.clone());
                            if let Some(id) = wt_project_id {
                                this.workspace.update(cx, |ws, cx| {
                                    ws.delete_project(&id, &hooks, cx);
                                });
                            }
                        } else {
                            this.workspace.update(cx, |ws, cx| {
                                ws.add_discovered_worktree(
                                    &wt_path_clone,
                                    &branch_clone,
                                    &project_id,
                                    "",
                                );
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
