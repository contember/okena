//! Rendering for the project info panel.
//!
//! Sections in the agent panel's order — what this is, where the work lands,
//! who is working on it — so a repo and a session read alike beside their
//! terminals.

use super::{ProjectInfo, ProjectInfoKind, ProjectInfoPanel};
use crate::theme::theme;
use crate::ui::tokens::ui_text_ms;
use crate::views::agent_session::AgentSessionInfo;
use crate::views::components::WorktreeSummary;
use crate::views::components::worktree_card::{chip, ci_chip_style, pr_chip_style};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};

impl ProjectInfoPanel {
    fn section_heading(&self, label: &str, count: Option<usize>, cx: &App) -> AnyElement {
        let t = theme(cx);
        h_flex()
            .items_center()
            .justify_between()
            .pt(px(8.0))
            .pb(px(2.0))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(label.to_string()),
            )
            // Optional: a bare "0" beside a heading that is not a list reads as
            // "none found" rather than "not countable".
            .children(count.map(|n| {
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(format!("{n}"))
                    .into_any_element()
            }))
            .into_any_element()
    }

    fn note(&self, text: impl Into<String>, cx: &App) -> AnyElement {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(text.into())
            .into_any_element()
    }

    /// Chips for the checkout itself: branch, diff, divergence, PR, pipeline.
    fn git_chips(&self, info: &ProjectInfo, cx: &App) -> Vec<AnyElement> {
        let t = theme(cx);
        let git = &info.git;
        let mut chips = Vec::new();
        if let Some(branch) = &git.branch {
            chips.push(chip(branch.clone(), t.text_secondary, cx));
        }
        if git.has_changes() {
            chips.push(chip(
                format!("+{} −{}", git.lines_added, git.lines_removed),
                t.text_muted,
                cx,
            ));
        }
        match (git.ahead, git.behind) {
            (Some(a), Some(b)) if a > 0 && b > 0 => {
                chips.push(chip(format!("↑{a} ↓{b}"), t.warning, cx));
            }
            (Some(a), _) if a > 0 => chips.push(chip(format!("↑{a}"), t.warning, cx)),
            (_, Some(b)) if b > 0 => chips.push(chip(format!("↓{b}"), t.warning, cx)),
            _ => {}
        }
        if let Some((number, state)) = &git.pr {
            let (color, label) = pr_chip_style(*number, state, &t);
            chips.push(chip(label, color, cx));
        }
        if let Some((status, passed, failed, pending)) = &git.ci {
            let (color, label) = ci_chip_style(status, *passed, *failed, *pending, &t);
            chips.push(chip(label, color, cx));
        }
        chips
    }

    /// One agent session working this project.
    ///
    /// Its own card rather than the session panel at a smaller size: here the
    /// question is only which agents are on this project and whether one wants
    /// you — the rest is a click away, in the session's own column.
    fn render_session_card(&self, s: &AgentSessionInfo, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let activity = s.activity();
        let mut chips = vec![chip(activity.label(), activity.color(&t), cx)];
        if let Some(agent) = &s.agent {
            chips.push(chip(agent.clone(), t.text_secondary, cx));
        }
        if !s.assets.is_empty() {
            chips.push(chip(format!("{} produced", s.assets.len()), t.success, cx));
        }

        let open_id = s.project_id.clone();
        v_flex()
            .id(SharedString::from(format!(
                "project-info-session-{}",
                s.project_id
            )))
            .cursor_pointer()
            .w_full()
            .min_w_0()
            .gap(px(3.0))
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(4.0))
            .bg(rgb(t.bg_primary))
            .border_1()
            .border_color(rgb(t.border))
            .hover(|style| style.bg(rgb(t.bg_hover)))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_primary))
                    .child(s.name.clone()),
            )
            .children(s.kind.subject().map(|subject| {
                div()
                    .w_full()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(subject)
            }))
            // What the agent says it is doing, beside what its terminal shows.
            .children(s.status.clone().map(|status| {
                div()
                    .w_full()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(format!("“{status}”"))
            }))
            .child(h_flex().gap(px(4.0)).flex_wrap().children(chips))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.open_project(open_id.clone(), cx);
                }),
            )
            .into_any_element()
    }
}

impl Render for ProjectInfoPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let Some(info) = self.info(cx) else {
            // The project went away. Say so rather than rendering an empty
            // shell that looks like a load that never finishes.
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .bg(rgb(t.bg_primary))
                .child(self.note("This project is gone.", cx))
                .into_any_element();
        };

        // Read out of the workspace up front: the cards below take listeners,
        // which need the context mutably.
        let (worktrees, sessions): (Vec<WorktreeSummary>, Vec<AgentSessionInfo>) = {
            let ws = self.workspace.read(cx);
            (
                info.worktrees
                    .iter()
                    .filter_map(|id| WorktreeSummary::collect(ws, id))
                    .collect(),
                info.sessions
                    .iter()
                    .filter_map(|id| AgentSessionInfo::collect(ws, &self.terminals, id))
                    .collect(),
            )
        };

        let mut body = v_flex()
            .id(SharedString::from(format!(
                "project-info-body-{}",
                info.project_id
            )))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(10.0))
            .pb(px(12.0))
            .gap(px(4.0));

        // ── What this is ─────────────────────────────────────────────────────
        let heading = match &info.kind {
            ProjectInfoKind::Repo => "REPOSITORY",
            ProjectInfoKind::Worktree { .. } => "WORKTREE",
        };
        body = body.child(self.section_heading(heading, None, cx));
        if let ProjectInfoKind::Worktree { repo: Some(repo) } = &info.kind {
            body = body.child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(format!("Worktree of {repo}")),
            );
        }
        let chips = self.git_chips(&info, cx);
        if !chips.is_empty() {
            body = body.child(h_flex().gap(px(4.0)).flex_wrap().children(chips));
        }
        body = body.child(
            div()
                .w_full()
                .min_w_0()
                .truncate()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child(info.path.clone()),
        );
        // Only when something changed: a button that opens an empty diff is
        // worse than no button.
        if info.git.has_changes() {
            let diff_id = info.project_id.clone();
            body = body.child(
                div()
                    .id("project-info-diff")
                    .cursor_pointer()
                    .mt(px(4.0))
                    .px(px(10.0))
                    .py(px(5.0))
                    .rounded(px(4.0))
                    .bg(rgb(t.bg_secondary))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_primary))
                    .child("Review changes")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            this.open_diff(&diff_id, cx);
                        }),
                    ),
            );
        }

        // ── Where the work lands ─────────────────────────────────────────────
        // A worktree is itself where work lands; only a repo has others.
        if info.kind == ProjectInfoKind::Repo {
            body = body.child(self.section_heading("WORKTREES", Some(worktrees.len()), cx));
            if worktrees.is_empty() {
                body = body.child(self.note("None open.", cx));
            }
            for summary in &worktrees {
                body = body.child(crate::views::components::render_worktree_card(
                    summary,
                    |this: &mut Self, id, cx| this.open_project(id.to_string(), cx),
                    |this: &mut Self, id, cx| this.open_diff(id, cx),
                    cx,
                ));
            }
        }

        // ── Who is working on it ─────────────────────────────────────────────
        body = body.child(self.section_heading("AGENTS", Some(sessions.len()), cx));
        if sessions.is_empty() {
            body = body.child(self.note("No sessions working its tasks.", cx));
        }
        for session in &sessions {
            body = body.child(self.render_session_card(session, cx));
        }

        v_flex()
            .size_full()
            .bg(rgb(t.bg_primary))
            .child(body)
            .into_any_element()
    }
}
