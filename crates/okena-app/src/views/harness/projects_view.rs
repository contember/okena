//! Projects view — one swimlane per project, scrolled horizontally.
//!
//! A lane gathers everything in flight for a project: its own branch and diff,
//! the worktrees open against it with their PRs and pipelines, and the agent
//! sessions working its tasks. Lanes rather than a list because the interesting
//! comparison is *across* projects — which ones have work in flight — and a
//! vertical list buries that under the first project's detail.
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

/// Width of one lane. Fixed so lanes line up as columns to scan across; the
/// container scrolls when they overflow.
pub(super) const LANE_WIDTH: f32 = 300.0;

/// Git facts shared by a project and its worktrees.
///
/// The same shape for both because a worktree is a checkout like any other —
/// it has a branch, a diff, a PR and a pipeline, and the lane shows them the
/// same way wherever they come from.
#[derive(Default)]
struct GitFacts {
    branch: Option<String>,
    lines_added: usize,
    lines_removed: usize,
    ahead: Option<usize>,
    behind: Option<usize>,
    pr: Option<(u32, PrState)>,
    ci: Option<(CiStatus, usize, usize, usize)>,
}

/// An agent session working on this project.
struct AgentCard {
    id: String,
    name: String,
    /// Task key, or the change name for a spec session.
    subtitle: Option<String>,
    /// The last status the agent reported through okena's MCP server.
    status: Option<String>,
    /// How many things it has produced — PRs, branches, notes.
    assets: usize,
}

/// One project's lane, snapshotted out of the workspace so the render tree
/// doesn't hold a borrow across `cx.listener` calls.
struct Lane {
    id: String,
    name: String,
    git: GitFacts,
    terminal_count: usize,
    /// Ids of the worktrees open against it. Only ids: the shared worktree
    /// card reads everything it shows, including push and review state, which
    /// would otherwise be collected here and go stale differently.
    worktrees: Vec<String>,
    agents: Vec<AgentCard>,
}

/// Whether an agent session is working one of `task_ids`.
///
/// Matched by task rather than by directory: a session is rooted above the
/// repos precisely so one agent can span several, so it has no path that would
/// place it in a lane. A spec session matches nothing here — it belongs to the
/// spec repository, not to any project in this workspace.
fn session_belongs_to(
    session: &crate::workspace::state::ProjectData,
    task_ids: &std::collections::HashSet<String>,
) -> bool {
    session
        .task_ref
        .as_ref()
        .is_some_and(|t| task_ids.contains(&t.id.external_id))
}

/// Whether `name` matches a fuzzy `query`.
///
/// Subsequence rather than substring, so "okwt" finds "okena-worktrees" the way
/// a command palette would — typing the shape of a name is faster than typing a
/// prefix of it. An empty query matches everything.
pub(super) fn fuzzy_matches(name: &str, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let mut chars = name.to_lowercase().into_bytes().into_iter();
    query.bytes().all(|needle| chars.any(|c| c == needle))
}

/// Whether a lane has work in flight — a worktree open, or an agent on it.
pub(super) fn lane_is_active(worktrees: usize, agents: usize) -> bool {
    worktrees > 0 || agents > 0
}

impl HarnessPane {
    fn git_facts(ws: &crate::workspace::state::Workspace, project_id: &str) -> GitFacts {
        let Some(g) = ws
            .remote_snapshot(project_id)
            .and_then(|snap| snap.git_status.as_ref())
        else {
            return GitFacts::default();
        };
        GitFacts {
            branch: g.branch.clone(),
            lines_added: g.lines_added,
            lines_removed: g.lines_removed,
            ahead: g.ahead,
            behind: g.behind,
            pr: g.pr_info.as_ref().map(|pr| (pr.number, pr.state.clone())),
            ci: g
                .ci_checks
                .as_ref()
                .map(|c| (c.status.clone(), c.passed, c.failed, c.pending)),
        }
    }

    fn lanes(&self, cx: &Context<Self>) -> Vec<Lane> {
        let ws = self.workspace.read(cx);

        // Agent sessions are rooted above the repos, so they carry no link to a
        // project directory. They are matched to a lane by task instead: a
        // session and the worktrees it was given all carry the same task.
        let sessions: Vec<&crate::workspace::state::ProjectData> = ws
            .projects()
            .iter()
            .filter(|p| p.is_any_agent_session())
            .collect();

        ws.projects()
            .iter()
            // Worktrees and sessions appear inside a lane, never as one.
            .filter(|p| p.worktree_info.is_none() && !p.is_any_agent_session())
            .map(|p| {
                let worktrees: Vec<String> = p
                    .worktree_ids
                    .iter()
                    .filter(|id| ws.project(id).is_some())
                    .cloned()
                    .collect();

                // Tasks this project has work in flight for, including one
                // started on the project itself rather than in a worktree.
                let task_ids: std::collections::HashSet<String> = p
                    .task_ref
                    .iter()
                    .chain(
                        p.worktree_ids
                            .iter()
                            .filter_map(|id| ws.project(id).and_then(|w| w.task_ref.as_ref())),
                    )
                    .map(|t| t.id.external_id.clone())
                    .collect();

                let agents: Vec<AgentCard> = sessions
                    .iter()
                    .filter(|s| session_belongs_to(s, &task_ids))
                    .map(|s| AgentCard {
                        id: s.id.clone(),
                        name: s.name.clone(),
                        subtitle: s
                            .task_ref
                            .as_ref()
                            .map(|t| t.display_key.clone())
                            .or_else(|| s.spec_change.clone()),
                        status: s.agent.as_ref().and_then(|a| a.status.clone()),
                        assets: s.agent.as_ref().map(|a| a.assets.len()).unwrap_or(0),
                    })
                    .collect();

                Lane {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    git: Self::git_facts(ws, &p.id),
                    terminal_count: p
                        .layout
                        .as_ref()
                        .map(|l| l.collect_terminal_ids().len())
                        .unwrap_or(0),
                    worktrees,
                    agents,
                }
            })
            .collect()
    }

    /// Show a project (or worktree, or session) in the terminal workspace.
    ///
    /// Leaving the harness view is the point — focusing something while the
    /// view still covered the main area would look like nothing happened.
    pub(super) fn open_project(&mut self, project_id: String, cx: &mut Context<Self>) {
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

    /// Show what changed in a worktree.
    pub(super) fn open_diff(&self, project_id: &str, cx: &mut App) {
        self.request_broker.update(cx, |broker, cx| {
            broker.push_overlay_request(
                okena_workspace::requests::OverlayRequest::Project(
                    okena_workspace::requests::ProjectOverlay {
                        project_id: project_id.to_string(),
                        kind: okena_workspace::requests::ProjectOverlayKind::DiffViewer {
                            file: None,
                            mode: None,
                            commit_message: None,
                            commits: None,
                            commit_index: None,
                        },
                    },
                ),
                cx,
            );
        });
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

    /// Chips for one checkout: branch, diff, divergence, PR, pipeline.
    fn git_chips(&self, git: &GitFacts, cx: &Context<Self>) -> Vec<AnyElement> {
        let t = theme(cx);
        let mut chips: Vec<AnyElement> = Vec::new();
        if let Some(branch) = &git.branch {
            chips.push(self.chip(branch.clone(), t.text_secondary, cx));
        }
        if git.lines_added > 0 || git.lines_removed > 0 {
            chips.push(self.chip(
                format!("+{} −{}", git.lines_added, git.lines_removed),
                t.text_muted,
                cx,
            ));
        }
        match (git.ahead, git.behind) {
            (Some(a), Some(b)) if a > 0 && b > 0 => {
                chips.push(self.chip(format!("↑{a} ↓{b}"), t.warning, cx));
            }
            (Some(a), _) if a > 0 => chips.push(self.chip(format!("↑{a}"), t.warning, cx)),
            (_, Some(b)) if b > 0 => chips.push(self.chip(format!("↓{b}"), t.warning, cx)),
            _ => {}
        }
        if let Some((number, state)) = &git.pr {
            // Coloured by state so a merged or closed PR doesn't read as live
            // work still waiting on you.
            let (color, label) = match state {
                PrState::Open => (t.success, format!("PR #{number}")),
                PrState::Draft => (t.text_muted, format!("PR #{number} draft")),
                PrState::Merged => (t.button_primary_bg, format!("PR #{number} merged")),
                PrState::Closed => (t.text_muted, format!("PR #{number} closed")),
            };
            chips.push(self.chip(label, color, cx));
        }
        if let Some((status, passed, failed, pending)) = &git.ci {
            let (color, label) = match status {
                CiStatus::Success => (t.success, format!("checks {passed}/{}", passed + failed)),
                CiStatus::Failure => (t.error, format!("{failed} failing")),
                CiStatus::Pending => (t.warning, format!("{pending} pending")),
            };
            chips.push(self.chip(label, color, cx));
        }
        chips
    }

    /// A heading inside a lane, optionally with a count.
    ///
    /// The count is optional because not every section is a list — a bare "0"
    /// beside a heading reads as "none found" rather than "not countable".
    pub(super) fn lane_section(
        &self,
        label: &str,
        count: Option<usize>,
        cx: &Context<Self>,
    ) -> AnyElement {
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
            .children(count.map(|n| {
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(format!("{n}"))
                    .into_any_element()
            }))
            .into_any_element()
    }

    /// A clickable card nested inside a lane.
    pub(super) fn lane_card(
        &self,
        id: String,
        title: String,
        subtitle: Option<String>,
        chips: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let open_id = id.clone();
        v_flex()
            .id(SharedString::from(format!("lane-card-{id}")))
            .cursor_pointer()
            .w_full()
            .min_w_0()
            .gap(px(4.0))
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(4.0))
            .bg(rgb(t.bg_primary))
            .border_1()
            .border_color(rgb(t.border))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_primary))
                    .child(title),
            )
            .children(subtitle.map(|s| {
                div()
                    .w_full()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(s)
                    .into_any_element()
            }))
            .when(!chips.is_empty(), |d| {
                d.child(h_flex().gap(px(4.0)).flex_wrap().children(chips))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.open_project(open_id.clone(), cx);
                }),
            )
            .into_any_element()
    }

    fn render_project_lane(&self, lane: &Lane, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let open_id = lane.id.clone();
        let header_chips = self.git_chips(&lane.git, cx);

        let mut body = v_flex()
            .id(SharedString::from(format!("lane-body-{}", lane.id)))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(10.0))
            .pb(px(10.0))
            .gap(px(4.0));

        body = body.child(self.lane_section("WORKTREES", Some(lane.worktrees.len()), cx));
        if lane.worktrees.is_empty() {
            body = body.child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child("None open."),
            );
        }
        for w in &lane.worktrees {
            let Some(summary) =
                crate::views::components::WorktreeSummary::collect(self.workspace.read(cx), w)
            else {
                continue;
            };
            body = body.child(crate::views::components::render_worktree_card(
                &summary,
                |this, id, cx| this.open_project(id.to_string(), cx),
                |this, id, cx| this.open_diff(id, cx),
                cx,
            ));
        }

        body = body.child(self.lane_section("AGENTS", Some(lane.agents.len()), cx));
        if lane.agents.is_empty() {
            body = body.child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child("No sessions."),
            );
        }
        for a in &lane.agents {
            let mut chips = Vec::new();
            if a.assets > 0 {
                chips.push(self.chip(format!("{} produced", a.assets), t.success, cx));
            }
            if let Some(status) = &a.status {
                chips.push(self.chip(status.clone(), t.text_secondary, cx));
            }
            body = body.child(self.lane_card(
                a.id.clone(),
                a.name.clone(),
                a.subtitle.clone(),
                chips,
                cx,
            ));
        }

        v_flex()
            .w(px(LANE_WIDTH))
            .flex_shrink_0()
            .h_full()
            .rounded(px(6.0))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_secondary))
            .child(
                v_flex()
                    .id(SharedString::from(format!("lane-head-{}", lane.id)))
                    .cursor_pointer()
                    .gap(px(6.0))
                    .px(px(10.0))
                    .py(px(8.0))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(ui_text(13.0, cx))
                                    .text_color(rgb(t.text_primary))
                                    .child(lane.name.clone()),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(format!("{} term", lane.terminal_count)),
                            ),
                    )
                    .when(!header_chips.is_empty(), |d| {
                        d.child(h_flex().gap(px(4.0)).flex_wrap().children(header_chips))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            this.open_project(open_id.clone(), cx);
                        }),
                    ),
            )
            .child(body)
            .into_any_element()
    }

    /// Toolbar actions for the Projects view.
    fn project_actions(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme(cx);
        let active_only = self.projects.active_only;
        vec![
            okena_ui::input::input_container(&t, None)
                .w(px(200.0))
                .flex_shrink_0()
                .px(px(8.0))
                .py(px(3.0))
                .child(
                    crate::views::components::SimpleInput::new(&self.projects.search)
                        .text_size(ui_text(13.0, cx)),
                )
                .into_any_element(),
            div()
                .id("projects-active-filter")
                .cursor_pointer()
                .flex_shrink_0()
                .px(px(10.0))
                .py(px(4.0))
                .rounded(px(4.0))
                .when(active_only, |d| {
                    d.bg(with_alpha(t.button_primary_bg, 0.2))
                        .text_color(rgb(t.text_primary))
                })
                .when(!active_only, |d| {
                    d.bg(rgb(t.bg_secondary))
                        .text_color(rgb(t.text_secondary))
                        .hover(|s| s.bg(rgb(t.bg_hover)))
                })
                .text_size(ui_text_ms(cx))
                .child("In flight")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        this.projects.active_only = !active_only;
                        cx.notify();
                    }),
                )
                .into_any_element(),
            self.toolbar_icon(
                "projects-settings",
                "icons/settings.svg",
                "Project settings",
                cx.listener(|this, _, _window, cx| this.open_settings("harness", cx)),
                cx,
            ),
        ]
    }

    pub(super) fn render_projects_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let query = self.projects.search.read(cx).value().to_string();
        let active_only = self.projects.active_only;
        let lanes: Vec<Lane> = self
            .lanes(cx)
            .into_iter()
            .filter(|l| fuzzy_matches(&l.name, &query))
            .filter(|l| !active_only || lane_is_active(l.worktrees.len(), l.agents.len()))
            .collect();

        let actions = self.project_actions(cx);
        let toolbar = self.render_toolbar(actions, cx);

        if lanes.is_empty() {
            // Says which of the two reasons applies, so a filter that hides
            // everything does not read as an empty workspace.
            let message = if query.trim().is_empty() && !active_only {
                "No projects in this workspace yet."
            } else {
                "No projects match the current filter."
            };
            return v_flex()
                .size_full()
                .child(toolbar)
                .child(
                    div()
                        .p(px(20.0))
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.text_secondary))
                        .child(message),
                )
                .into_any_element();
        }

        let rendered: Vec<AnyElement> = lanes
            .iter()
            .map(|lane| self.render_project_lane(lane, cx))
            .collect();

        // Horizontal scroll, matching the projects grid: shift+wheel, or a
        // native horizontal wheel. The lanes themselves scroll vertically, so
        // the plain wheel belongs to whichever lane is under the cursor.
        v_flex()
            .size_full()
            .child(toolbar)
            .child(
                h_flex()
                    .id("projects-lanes")
                    .flex_1()
                    .min_h_0()
                    .overflow_x_scroll()
                    .items_start()
                    .gap(px(10.0))
                    .p(px(12.0))
                    .children(rendered),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::session_belongs_to;
    use std::collections::HashSet;

    fn project(json: serde_json::Value) -> crate::workspace::state::ProjectData {
        serde_json::from_value(json).unwrap()
    }

    fn task_session(external_id: &str) -> crate::workspace::state::ProjectData {
        project(serde_json::json!({
            "id": format!("s-{external_id}"),
            "name": "QBL-1 (agent)",
            "path": "/p",
            "task_ref": {
                "id": { "provider": "linear", "external_id": external_id },
                "display_key": "QBL-1", "title": "t", "url": "http://x",
            },
        }))
    }

    fn ids(v: &[&str]) -> HashSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    use super::{fuzzy_matches, lane_is_active};

    #[test]
    fn an_empty_query_matches_everything() {
        assert!(fuzzy_matches("okena", ""));
        assert!(fuzzy_matches("okena", "   "));
    }

    #[test]
    fn matching_is_a_subsequence_not_a_prefix() {
        // Typing the shape of a name is faster than typing the start of it.
        assert!(fuzzy_matches("okena-worktrees", "okwt"));
        assert!(fuzzy_matches("okena-worktrees", "trees"));
    }

    #[test]
    fn matching_ignores_case() {
        assert!(fuzzy_matches("Okena", "oke"));
        assert!(fuzzy_matches("okena", "OKE"));
    }

    #[test]
    fn out_of_order_letters_do_not_match() {
        // A subsequence is still ordered — otherwise every query with the right
        // letters matches every project, which is no filter at all.
        assert!(!fuzzy_matches("okena", "aneko"));
    }

    #[test]
    fn a_query_longer_than_the_name_does_not_match() {
        assert!(!fuzzy_matches("ok", "okena"));
    }

    #[test]
    fn a_lane_is_in_flight_when_anything_is_open_on_it() {
        assert!(lane_is_active(1, 0), "a worktree is work in flight");
        assert!(lane_is_active(0, 1), "so is an agent");
        assert!(!lane_is_active(0, 0), "an idle repo is not");
    }

    #[test]
    fn a_session_lands_in_the_lane_whose_task_it_works() {
        assert!(session_belongs_to(&task_session("u1"), &ids(&["u1", "u2"])));
    }

    #[test]
    fn a_session_for_another_task_stays_out() {
        // The session is rooted above the repos, so nothing but the task links
        // it to a lane — a loose match would put one agent in every lane.
        assert!(!session_belongs_to(&task_session("u9"), &ids(&["u1"])));
    }

    #[test]
    fn a_spec_session_belongs_to_no_project_lane() {
        // It works the spec repository, which is not a project here.
        let spec = project(serde_json::json!({
            "id": "s1", "name": "add-login (spec)", "path": "/specs",
            "spec_change": "add-login",
        }));
        assert!(!session_belongs_to(&spec, &ids(&["u1"])));
    }

    #[test]
    fn a_lane_with_no_tasks_claims_no_sessions() {
        assert!(!session_belongs_to(&task_session("u1"), &ids(&[])));
    }
}
