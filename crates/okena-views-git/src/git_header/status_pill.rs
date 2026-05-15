//! Git status pill — the branch chip + commit-log button + diff stats badge
//! shown in the project column header.

use super::GitHeader;
use crate::project_header;

use okena_core::theme::ThemeColors;
use okena_git::GitStatus;
use okena_ui::tokens::ui_text_sm;
use okena_workspace::requests::{OverlayRequest, ProjectOverlay, ProjectOverlayKind};

use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use gpui_component::tooltip::Tooltip;

use std::sync::Arc;

impl GitHeader {
    /// Render the git status bar (branch, commit log button, diff stats).
    ///
    /// `current_branch` is the branch name from the git status watcher
    /// (passed in because the watcher lives in the main app).
    pub fn render_git_status(
        &self,
        status: Option<GitStatus>,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entity_handle = cx.entity().clone();

        match status {
            Some(status) if status.branch.is_some() => {
                let has_changes = status.has_changes();
                let lines_added = status.lines_added;
                let lines_removed = status.lines_removed;
                let project_id = self.project_id.clone();

                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(4.0))
                    .text_size(ui_text_sm(cx))
                    .line_height(px(12.0))
                    .child({
                        let entity_for_branch_bounds = entity_handle.clone();
                        let entity_for_branch_click = entity_handle.clone();
                        let entity_for_pr_bounds = entity_handle.clone();
                        let entity_for_pr_click = entity_handle.clone();
                        let supports_switch = self.git_provider.supports_mutations();
                        let has_pr = status.pr_info.is_some();
                        let on_branch_click: Option<Arc<dyn Fn(&mut Window, &mut App)>> =
                            if supports_switch {
                                Some(Arc::new(move |window, app| {
                                    let _ = entity_for_branch_click.update(app, |this, cx| {
                                        if this.branch_picker_visible {
                                            this.hide_branch_picker(cx);
                                        } else {
                                            this.show_branch_picker(window, cx);
                                        }
                                    });
                                }))
                            } else {
                                None
                            };
                        let on_pr_click: Option<Arc<dyn Fn(&mut Window, &mut App)>> =
                            if has_pr {
                                Some(Arc::new(move |_window, app| {
                                    let _ = entity_for_pr_click.update(app, |this, cx| {
                                        this.toggle_pr_checks(cx);
                                    });
                                }))
                            } else {
                                None
                            };
                        let on_branch_bounds: Option<Arc<dyn Fn(Bounds<Pixels>, &mut App)>> =
                            if supports_switch {
                                Some(Arc::new(move |bounds, app| {
                                    let _ = entity_for_branch_bounds.update(app, |this, _cx| {
                                        this.set_branch_chip_bounds(bounds);
                                    });
                                }))
                            } else {
                                None
                            };
                        let on_pr_bounds: Option<Arc<dyn Fn(Bounds<Pixels>, &mut App)>> =
                            if has_pr {
                                Some(Arc::new(move |bounds, app| {
                                    let _ = entity_for_pr_bounds.update(app, |this, _cx| {
                                        this.set_pr_badge_bounds(bounds);
                                    });
                                }))
                            } else {
                                None
                            };
                        project_header::render_branch_status(
                            &status,
                            project_header::BranchStatusCallbacks {
                                on_branch_click,
                                on_pr_click,
                                on_branch_bounds,
                                on_pr_bounds,
                            },
                            t,
                        )
                    })
                    // Commit log button
                    .child({
                        let entity_for_bounds = entity_handle.clone();
                        div()
                            .id(ElementId::Name(format!("commit-log-btn-{}", project_id).into()))
                            .relative()
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(18.0))
                            .h(px(16.0))
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                cx.stop_propagation();
                                this.toggle_commit_log(cx);
                            }))
                            .child(
                                svg()
                                    .path("icons/git-commit.svg")
                                    .size(px(10.0))
                                    .text_color(rgb(t.text_muted)),
                            )
                            .child(
                                canvas(
                                    move |bounds, _window, app| {
                                        let _ = entity_for_bounds.update(app, |this: &mut GitHeader, _cx| {
                                            this.commit_log_bounds = bounds;
                                        });
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            )
                            .tooltip(move |_window, cx| Tooltip::new("Commit Log").build(_window, cx))
                    })
                    // Diff stats (clickable, only if there are changes)
                    .when(has_changes, |d: Div| {
                        let request_broker = self.request_broker.clone();
                        let project_id_for_click = self.project_id.clone();
                        d.child(
                            project_header::render_diff_stats_badge(lines_added, lines_removed, t)
                                .id(ElementId::Name(format!("git-diff-stats-{}", project_id).into()))
                                .relative()
                                .cursor_pointer()
                                .rounded(px(3.0))
                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                                    if *hovered {
                                        this.show_diff_popover(cx);
                                    } else {
                                        this.hide_diff_popover(cx);
                                    }
                                }))
                                .on_click(cx.listener(move |this, _, _window, cx| {
                                    cx.stop_propagation();
                                    this.hide_diff_popover(cx);
                                    request_broker.update(cx, |broker, cx| {
                                        broker.push_overlay_request(OverlayRequest::Project(ProjectOverlay {
                                            project_id: project_id_for_click.clone(),
                                            kind: ProjectOverlayKind::DiffViewer {
                                                file: None,
                                                mode: None,
                                                commit_message: None,
                                                commits: None,
                                                commit_index: None,
                                            },
                                        }), cx);
                                    });
                                }))
                                // Invisible canvas to capture bounds for popover positioning
                                .child(canvas(
                                    {
                                        let entity_handle = entity_handle.clone();
                                        move |bounds, _window, app| {
                                            entity_handle.update(app, |this, _cx| {
                                                this.diff_stats_bounds = bounds;
                                            });
                                        }
                                    },
                                    |_, _, _, _| {},
                                ).absolute().size_full())
                        )
                    })
                    .when_some(
                        project_header::render_ahead_behind_badge(
                            (status.ahead, status.behind),
                            t,
                        ),
                        |d, badge| d.child(badge),
                    )
                    .into_any_element()
            }
            _ => div().into_any_element(), // Not a git repo - show nothing
        }
    }
}
