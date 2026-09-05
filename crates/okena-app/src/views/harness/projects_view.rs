//! Projects view — every project with its branch, PR, pipeline and worktrees.
//!
//! Reads the same workspace mirror the terminal grid renders from, so it needs
//! no fetching of its own: git status, PR info and CI checks all arrive on the
//! daemon snapshot that already feeds the project columns.

use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_ms, ui_text_sm};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::{CiStatus, PrState};

use super::HarnessPane;

/// One project's rendered facts, snapshotted out of the workspace so the render
/// tree doesn't hold a borrow across `cx.listener` calls.
struct ProjectRow {
    id: String,
    name: String,
    path: String,
    branch: Option<String>,
    lines_added: usize,
    lines_removed: usize,
    ahead: Option<usize>,
    behind: Option<usize>,
    pr: Option<(u32, PrState, String)>,
    ci: Option<(CiStatus, usize, usize, usize)>,
    task: Option<(String, String)>,
    worktrees: Vec<(String, String)>,
    terminal_count: usize,
}

impl HarnessPane {
    fn project_rows(&self, cx: &Context<Self>) -> Vec<ProjectRow> {
        let ws = self.workspace.read(cx);
        ws.projects()
            .iter()
            // Worktrees appear nested under their parent, not as top-level rows.
            .filter(|p| p.worktree_info.is_none())
            .map(|p| {
                let git = ws
                    .remote_snapshot(&p.id)
                    .and_then(|snap| snap.git_status.as_ref());
                let worktrees = p
                    .worktree_ids
                    .iter()
                    .filter_map(|id| ws.project(id))
                    .map(|w| {
                        let branch = ws
                            .remote_snapshot(&w.id)
                            .and_then(|s| s.git_status.as_ref())
                            .and_then(|g| g.branch.clone())
                            .unwrap_or_default();
                        (w.name.clone(), branch)
                    })
                    .collect();

                ProjectRow {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    path: p.path.clone(),
                    branch: git.and_then(|g| g.branch.clone()),
                    lines_added: git.map(|g| g.lines_added).unwrap_or(0),
                    lines_removed: git.map(|g| g.lines_removed).unwrap_or(0),
                    ahead: git.and_then(|g| g.ahead),
                    behind: git.and_then(|g| g.behind),
                    pr: git.and_then(|g| {
                        g.pr_info
                            .as_ref()
                            .map(|pr| (pr.number, pr.state.clone(), pr.url.clone()))
                    }),
                    ci: git.and_then(|g| {
                        g.ci_checks
                            .as_ref()
                            .map(|c| (c.status.clone(), c.passed, c.failed, c.pending))
                    }),
                    task: p
                        .task_ref
                        .as_ref()
                        .map(|t| (t.display_key.clone(), t.title.clone())),
                    worktrees,
                    terminal_count: p
                        .layout
                        .as_ref()
                        .map(|l| l.collect_terminal_ids().len())
                        .unwrap_or(0),
                }
            })
            .collect()
    }

    /// Show this project in the terminal workspace.
    ///
    /// Leaving the harness view is the point — focusing a project while the
    /// view still covered the main area would look like nothing happened.
    fn open_project(&mut self, project_id: String, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| {
                ws.set_focused_project_individual(fm, Some(project_id.clone()), cx);
            });
            cx.notify();
        });
        okena_workspace::harness_state::set_active_harness(self.window_id, None, cx);
        cx.notify();
    }

    pub(super) fn chip(&self, text: String, color: u32, cx: &Context<Self>) -> AnyElement {
        div()
            .px(px(6.0))
            .py(px(1.0))
            .rounded(px(3.0))
            .bg(with_alpha(color, 0.15))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(color))
            .child(text)
            .into_any_element()
    }

    fn render_project_card(&self, row: &ProjectRow, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let id_for_open = row.id.clone();

        let mut chips: Vec<AnyElement> = Vec::new();
        if let Some(branch) = &row.branch {
            chips.push(self.chip(branch.clone(), t.text_secondary, cx));
        }
        if row.lines_added > 0 || row.lines_removed > 0 {
            chips.push(self.chip(
                format!("+{} −{}", row.lines_added, row.lines_removed),
                t.text_muted,
                cx,
            ));
        }
        match (row.ahead, row.behind) {
            (Some(a), Some(b)) if a > 0 || b > 0 => {
                chips.push(self.chip(format!("↑{a} ↓{b}"), t.warning, cx));
            }
            (Some(a), _) if a > 0 => chips.push(self.chip(format!("↑{a}"), t.warning, cx)),
            _ => {}
        }
        if let Some((number, state, _url)) = &row.pr {
            // Colour by PR state so a merged or closed PR doesn't read as live.
            let color = match state {
                PrState::Open => t.success,
                PrState::Merged => t.button_primary_bg,
                PrState::Closed | PrState::Draft => t.text_muted,
            };
            chips.push(self.chip(format!("PR #{number}"), color, cx));
        }
        if let Some((status, passed, failed, pending)) = &row.ci {
            let (color, label) = match status {
                CiStatus::Success => (t.success, format!("checks {passed}/{}", passed + failed)),
                CiStatus::Failure => (t.error, format!("{failed} failing")),
                CiStatus::Pending => (t.warning, format!("{pending} pending")),
            };
            chips.push(self.chip(label, color, cx));
        }
        chips.push(self.chip(
            format!("{} terminal(s)", row.terminal_count),
            t.text_muted,
            cx,
        ));

        let worktrees: Vec<AnyElement> = row
            .worktrees
            .iter()
            .map(|(name, branch)| {
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(if branch.is_empty() {
                        format!("  ↳ {name}")
                    } else {
                        format!("  ↳ {name} · {branch}")
                    })
                    .into_any_element()
            })
            .collect();

        v_flex()
            .rounded(px(6.0))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_secondary))
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .px(px(10.0))
                    .py(px(7.0))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .child(
                        v_flex()
                            .gap(px(1.0))
                            .child(
                                div()
                                    .text_size(ui_text(13.0, cx))
                                    .text_color(rgb(t.text_primary))
                                    .child(row.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(row.path.clone()),
                            ),
                    )
                    .child(self.small_button(
                        "open-project",
                        "Open",
                        cx.listener(move |this, _, _window, cx| {
                            this.open_project(id_for_open.clone(), cx);
                        }),
                        cx,
                    )),
            )
            .child(
                v_flex()
                    .p(px(10.0))
                    .gap(px(6.0))
                    .child(h_flex().gap(px(6.0)).flex_wrap().children(chips))
                    .children(row.task.as_ref().map(|(key, title)| {
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_secondary))
                            .child(format!("task: {key} — {title}"))
                            .into_any_element()
                    }))
                    .children(worktrees),
            )
            .into_any_element()
    }

    pub(super) fn render_projects_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let rows = self.project_rows(cx);

        if rows.is_empty() {
            return div()
                .p(px(20.0))
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_secondary))
                .child("No projects in this workspace yet.")
                .into_any_element();
        }

        let cards: Vec<AnyElement> = rows
            .iter()
            .map(|row| self.render_project_card(row, cx))
            .collect();

        v_flex()
            .id("projects-view-list")
            .size_full()
            .overflow_y_scroll()
            .p(px(12.0))
            .gap(px(10.0))
            .children(cards)
            .into_any_element()
    }
}
