//! Worktree list popover — standalone overlay entity.
//!
//! Shows all git worktrees for a project with checkboxes to toggle sidebar visibility.
//! Rendered at WindowView level via OverlayManager, like context menus.

use okena_ui::overlay::CloseEvent;
use okena_ui::theme::theme;
use okena_ui::tokens::{ui_text_ms, ui_text_md};
use okena_workspace::state::{WindowId, Workspace};
use gpui::*;
use gpui::prelude::*;

use crate::Cancel;

/// Event emitted by WorktreeListPopover.
pub enum WorktreeListPopoverEvent {
    Close,
    /// Untrack (delete) a tracked worktree project. The daemon owns the
    /// project, so the host routes this to `ActionRequest::DeleteProject`
    /// rather than mutating the read-only mirror here.
    DeleteProject { project_id: String },
}

impl CloseEvent for WorktreeListPopoverEvent {
    fn is_close(&self) -> bool { matches!(self, Self::Close) }
}

impl EventEmitter<WorktreeListPopoverEvent> for WorktreeListPopover {}

/// Standalone worktree list popover entity.
pub struct WorktreeListPopover {
    workspace: Entity<Workspace>,
    /// Spawning window for the multi-window new-project visibility rule
    /// (PRD user story 14): a click that adds a discovered worktree
    /// makes the new project visible in this window only, hidden in
    /// every other window. Threaded from the originating `WindowView`
    /// through `OverlayManager::show_worktree_list`.
    window_id: WindowId,
    project_id: String,
    entries: Vec<(String, String)>,
    position: Point<Pixels>,
    focus_handle: FocusHandle,
    /// Normalized git root (for filtering out the main repo entry).
    norm_git_root: std::path::PathBuf,
    /// Subdirectory within the git repo (empty for non-monorepo projects).
    subdir: std::path::PathBuf,
}

impl WorktreeListPopover {
    pub fn new(
        workspace: Entity<Workspace>,
        project_id: String,
        position: Point<Pixels>,
        window_id: WindowId,
        cx: &mut Context<Self>,
    ) -> Self {
        let project_path = workspace.read(cx).project(&project_id)
            .map(|p| p.path.clone())
            .unwrap_or_default();
        let (git_root, subdir) = okena_git::resolve_git_root_and_subdir(
            std::path::Path::new(&project_path),
        );
        let norm_git_root = okena_git::repository::normalize_path(&git_root);
        let entries = okena_git::repository::list_git_worktrees(&git_root);
        let focus_handle = cx.focus_handle();
        Self { workspace, window_id, project_id, entries, position, focus_handle, norm_git_root, subdir }
    }

    /// Find a tracked worktree project by its worktree root path.
    /// Checks both the expected project path (with monorepo subdir) and the
    /// bare worktree root for backwards compatibility with older workspace files.
    fn find_tracked_project_id(&self, wt_path: &str, cx: &App) -> Option<String> {
        let expected_path = okena_git::repository::project_path_in_worktree(wt_path, &self.subdir);
        let ws = self.workspace.read(cx);
        ws.data().projects.iter()
            .find(|p| (p.path == expected_path || p.path == wt_path)
                && p.worktree_info.as_ref()
                    .is_some_and(|wt| wt.parent_project_id == self.project_id))
            .map(|p| p.id.clone())
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

        let tracked_project_paths: std::collections::HashSet<String> = ws.data().projects.iter()
            .filter(|p| p.worktree_info.as_ref()
                .is_some_and(|wt| wt.parent_project_id == *project_id))
            .map(|p| p.path.clone())
            .collect();

        let worktrees: Vec<(String, String, bool)> = self.entries.iter()
            .filter(|(wt_path, _)| {
                let norm_wt = okena_git::repository::normalize_path(std::path::Path::new(wt_path));
                norm_wt != self.norm_git_root
            })
            .map(|(wt_path, branch)| {
                let expected_path = okena_git::repository::project_path_in_worktree(wt_path, subdir);
                let is_tracked = tracked_project_paths.contains(&expected_path)
                    || tracked_project_paths.contains(wt_path);
                (wt_path.clone(), branch.clone(), is_tracked)
            })
            .collect();

        let viewport_h = window.viewport_size().height;
        let available = (viewport_h - px(120.0)).max(px(0.0));
        let scroll_max_h = available.min(px(500.0));

        let panel = okena_ui::popover::popover_panel("worktree-list-panel", &t)
            .w(px(280.0))
            .flex()
            .flex_col()
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_secondary))
                    .pb(px(6.0))
                    .child("WORKTREES")
            )
            .child(
                div()
                    .id("worktree-list-scroll")
                    .max_h(scroll_max_h)
                    .overflow_y_scroll()
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
                            if let Some(id) = this.find_tracked_project_id(&wt_path_clone, cx) {
                                // Daemon owns the project — emit an event so the
                                // host dispatches DeleteProject; the removal
                                // mirrors back. No direct mirror mutation here.
                                cx.emit(WorktreeListPopoverEvent::DeleteProject { project_id: id });
                            }
                        } else {
                            // TODO(daemon): tracking a discovered worktree mutates
                            // the read-only mirror directly. There is no single
                            // ActionRequest to attach an already-on-disk worktree
                            // to the daemon's project list yet — needs a daemon-side
                            // action (Batch B). Left as-is for now.
                            let window_id = this.window_id;
                            this.workspace.update(cx, |ws, cx| {
                                if let Some(new_id) = ws.add_discovered_worktree(
                                    &wt_path_clone,
                                    &branch_clone,
                                    &project_id,
                                    window_id,
                                ) {
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
            }))
            );

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
