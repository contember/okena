//! Worktree list popover — standalone overlay entity.
//!
//! Shows all git worktrees for a project with checkboxes to toggle sidebar visibility.
//! Rendered at WindowView level via OverlayManager, like context menus.

use gpui::prelude::*;
use gpui::*;
use okena_ui::overlay::CloseEvent;
use okena_ui::theme::theme;
use okena_ui::tokens::{ui_text_md, ui_text_ms};
use okena_workspace::state::Workspace;

use crate::Cancel;
use okena_core::api::ApiWorktreeEntry;

/// Event emitted by WorktreeListPopover.
pub enum WorktreeListPopoverEvent {
    Close,
    /// Untrack (delete) a tracked worktree project. The daemon owns the
    /// project, so the host routes this to `ActionRequest::DeleteProject`
    /// rather than mutating the read-only mirror here.
    DeleteProject {
        project_id: String,
    },
    /// Track an already-on-disk worktree as a project. The daemon owns the
    /// project list, so the host routes this to
    /// `ActionRequest::AddDiscoveredWorktree` rather than mutating the mirror.
    AddDiscoveredWorktree {
        parent_project_id: String,
        worktree_path: String,
        branch: String,
    },
}

impl CloseEvent for WorktreeListPopoverEvent {
    fn is_close(&self) -> bool {
        matches!(self, Self::Close)
    }
}

impl EventEmitter<WorktreeListPopoverEvent> for WorktreeListPopover {}

/// Standalone worktree list popover entity.
pub struct WorktreeListPopover {
    workspace: Entity<Workspace>,
    project_id: String,
    entries: Vec<ApiWorktreeEntry>,
    loading: bool,
    error_message: Option<String>,
    position: Point<Pixels>,
    focus_handle: FocusHandle,
}

impl WorktreeListPopover {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: okena_transport::remote_action::RemoteActionClient,
        daemon_project_id: String,
        workspace: Entity<Workspace>,
        project_id: String,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let mut popover = Self {
            workspace,
            project_id,
            entries: Vec::new(),
            loading: true,
            error_message: None,
            position,
            focus_handle,
        };
        popover.load_worktrees(client, daemon_project_id, cx);
        popover
    }

    fn load_worktrees(
        &mut self,
        client: okena_transport::remote_action::RemoteActionClient,
        project_id: String,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || Self::fetch_worktrees(&client, project_id)).await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(entries) => this.entries = entries,
                    Err(error) => this.error_message = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Fetch the worktree listing from the daemon. The git repo lives on the
    /// daemon, so we post a `GitListWorktrees` action rather than scanning the
    fn fetch_worktrees(
        client: &okena_transport::remote_action::RemoteActionClient,
        project_id: String,
    ) -> Result<Vec<ApiWorktreeEntry>, String> {
        let action = okena_core::api::ActionRequest::GitListWorktrees { project_id };
        let value = client
            .post_action(action)?
            .ok_or_else(|| "Missing worktree list response".to_string())?;
        let entries = value
            .get("entries")
            .cloned()
            .ok_or_else(|| "Invalid worktree list response".to_string())?;
        serde_json::from_value(entries)
            .map_err(|error| format!("Invalid worktree list response: {error}"))
    }

    /// Find a tracked worktree project by its worktree root path.
    /// Checks both the expected project path (with monorepo subdir) and the
    /// bare worktree root for backwards compatibility with older workspace files.
    fn find_tracked_project_id(&self, entry: &ApiWorktreeEntry, cx: &App) -> Option<String> {
        let ws = self.workspace.read(cx);
        ws.data()
            .projects
            .iter()
            .find(|p| {
                (p.path == entry.project_path || p.path == entry.worktree_path)
                    && p.worktree_info
                        .as_ref()
                        .is_some_and(|wt| wt.parent_project_id == self.project_id)
            })
            .map(|p| p.id.clone())
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(WorktreeListPopoverEvent::Close);
    }
}

impl Render for WorktreeListPopover {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let loading = self.loading;
        let error_message = self.error_message.clone();

        if !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }

        let ws = self.workspace.read(cx);
        let project_id = &self.project_id;

        let tracked_project_paths: std::collections::HashSet<String> = ws
            .data()
            .projects
            .iter()
            .filter(|p| {
                p.worktree_info
                    .as_ref()
                    .is_some_and(|wt| wt.parent_project_id == *project_id)
            })
            .map(|p| p.path.clone())
            .collect();

        let worktrees: Vec<(ApiWorktreeEntry, bool)> = self
            .entries
            .iter()
            .filter(|entry| !entry.is_main)
            .map(|entry| {
                let is_tracked = tracked_project_paths.contains(&entry.project_path)
                    || tracked_project_paths.contains(&entry.worktree_path);
                (entry.clone(), is_tracked)
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
                    .child("WORKTREES"),
            )
            .child(
                div()
                    .id("worktree-list-scroll")
                    .max_h(scroll_max_h)
                    .overflow_y_scroll()
                    .when(loading, |d| {
                        d.child(
                            div()
                                .text_size(ui_text_md(cx))
                                .text_color(rgb(t.text_muted))
                                .py(px(8.0))
                                .child("Loading\u{2026}"),
                        )
                    })
                    .when_some(error_message, |d, error| {
                        d.child(
                            div()
                                .text_size(ui_text_md(cx))
                                .text_color(rgb(t.error))
                                .py(px(8.0))
                                .child(error),
                        )
                    })
                    .when(
                        worktrees.is_empty() && !loading && self.error_message.is_none(),
                        |d| {
                            d.child(
                                div()
                                    .text_size(ui_text_md(cx))
                                    .text_color(rgb(t.text_muted))
                                    .py(px(8.0))
                                    .child("No worktrees found"),
                            )
                        },
                    )
                    .children(worktrees.into_iter().map(|(entry, is_tracked)| {
                        let project_id = self.project_id.clone();
                        let entry_for_click = entry.clone();

                        div()
                            .id(ElementId::Name(
                                format!("wt-list-{}", entry.worktree_path).into(),
                            ))
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
                                    if let Some(id) =
                                        this.find_tracked_project_id(&entry_for_click, cx)
                                    {
                                        // Daemon owns the project — emit an event so the
                                        // host dispatches DeleteProject; the removal
                                        // mirrors back. No direct mirror mutation here.
                                        cx.emit(WorktreeListPopoverEvent::DeleteProject {
                                            project_id: id,
                                        });
                                    }
                                } else {
                                    // The daemon owns the project list — emit an event so
                                    // the host dispatches AddDiscoveredWorktree; the new
                                    // worktree project mirrors back. No direct mirror
                                    // mutation here.
                                    cx.emit(WorktreeListPopoverEvent::AddDiscoveredWorktree {
                                        parent_project_id: project_id.clone(),
                                        worktree_path: entry_for_click.worktree_path.clone(),
                                        branch: entry_for_click.branch.clone(),
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
                                    .border_color(rgb(if is_tracked {
                                        t.border_active
                                    } else {
                                        t.border
                                    }))
                                    .when(is_tracked, |d| d.bg(rgb(t.border_active)))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(is_tracked, |d| {
                                        d.child(
                                            svg()
                                                .path("icons/check.svg")
                                                .size(px(10.0))
                                                .text_color(rgb(t.bg_primary)),
                                        )
                                    }),
                            )
                            .child(
                                div().flex_1().min_w_0().child(
                                    div()
                                        .text_size(ui_text_md(cx))
                                        .text_color(rgb(t.text_primary))
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(entry.branch.clone()),
                                ),
                            )
                    })),
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
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    this.close(cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _window, cx| {
                    this.close(cx);
                }),
            )
            .on_scroll_wheel(|_, _, cx| {
                cx.stop_propagation();
            })
            .child(deferred(
                anchored().position(position).snap_to_window().child(panel),
            ))
    }
}
