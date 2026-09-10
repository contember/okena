//! AGENTS section — agent sessions, separated from the repos.
//!
//! An agent session is a project rooted at the configured projects directory
//! rather than a repo, created when a task spans several of them. Listing it
//! among the repos makes it look like one; it isn't, so it gets its own
//! section with the task it belongs to.
//!
//! Both kinds — sessions working a task and sessions writing a spec — share one
//! list, told apart by a coloured kind badge on each row. Separate sections
//! made the list taller than it needed to be and forced a heading onto a group
//! that was often a single row; the badge carries the same information without
//! spending a line on it.

use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_ui::theme::theme;
use okena_ui::tokens::ui_text_ms;
use okena_workspace::state::AgentSortMode;

use super::{Sidebar, SidebarProjectInfo};

/// What a session was started to do, for the row's badge.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Task,
    Spec,
    Custom,
}

impl RowKind {
    fn badge(self) -> &'static str {
        match self {
            RowKind::Task => "task",
            RowKind::Spec => "spec",
            RowKind::Custom => "agent",
        }
    }

    /// Colour for the badge. Distinct hues rather than shades of one, so the
    /// kinds are told apart at a glance in a mixed list.
    fn color(self, t: &okena_ui::theme::ThemeColors) -> u32 {
        match self {
            RowKind::Task => t.button_primary_bg,
            RowKind::Spec => t.success,
            RowKind::Custom => t.warning,
        }
    }
}

/// A session row: the project, its kind, and what it is working on.
struct SessionRow {
    info: SidebarProjectInfo,
    kind: RowKind,
    /// Whether this is the session currently open in the main area.
    focused: bool,
    /// Task key for a task session, change name for a spec session.
    subtitle: Option<String>,
    /// Last time anything ran in this session, for activity ordering. `None`
    /// for a session that has not run anything yet.
    last_activity_at: Option<u64>,
}

impl Sidebar {
    fn render_session_row(&self, row: &SessionRow, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let id = row.info.id.clone();
        let name = row.info.name.clone();
        let subtitle = row.subtitle.clone();
        let kind = row.kind;
        let kind_color = kind.color(&t);
        let focused = row.focused;
        // Two lines rather than one: the name and what the session is working
        // on are both long enough to truncate, and squeezing them onto a row
        // together left neither readable.
        v_flex()
            .id(SharedString::from(format!("agent-session-{id}")))
            .cursor_pointer()
            .w_full()
            .min_w_0()
            .gap(px(1.0))
            .px(px(12.0))
            .py(px(5.0))
            // Same treatment a focused project row gets, so "what am I looking
            // at" reads the same way on both tabs.
            .when(focused, |d| d.bg(rgb(t.bg_hover)))
            .when(!focused, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
            // An accent edge as well as the fill: several sessions can look
            // alike at a glance, and the fill alone is easy to miss in a list
            // where the row under the pointer is also filled.
            .when(focused, |d| d.border_l_2().border_color(rgb(kind_color)))
            .when(!focused, |d| d.border_l_2().border_color(rgba(0)))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex_shrink_0()
                            .px(px(5.0))
                            .rounded(px(3.0))
                            .bg(okena_ui::theme::with_alpha(kind_color, 0.15))
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(kind_color))
                            .child(kind.badge()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(okena_ui::tokens::ui_text(13.0, cx))
                            .text_color(rgb(t.text_primary))
                            .child(name),
                    ),
            )
            .children(subtitle.map(|key| {
                div()
                    .w_full()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(key)
                    .into_any_element()
            }))
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.focus_project_from_sidebar(id.clone(), true, cx);
            }))
            .into_any_element()
    }

    /// The agent sessions in this workspace, ordered by the header's sort.
    pub(super) fn render_agents_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.read(cx);

        let focused_id = self.focus_manager.read(cx).focused_project_id().cloned();

        let mut sessions: Vec<SessionRow> = Vec::new();
        for p in workspace.data().projects.iter() {
            // Spec first: a session can only be one kind, and checking the
            // narrower marker first keeps that obvious.
            let (kind, subtitle) = if let Some(change) = p.spec_change.clone() {
                (RowKind::Spec, Some(change))
            } else if let Some(goal) = p.custom_session.clone() {
                (RowKind::Custom, Some(goal))
            } else if p.is_agent_session() {
                (
                    RowKind::Task,
                    p.task_ref.as_ref().map(|t| t.display_key.clone()),
                )
            } else {
                continue;
            };
            sessions.push(SessionRow {
                info: SidebarProjectInfo::from_project(p, workspace, self.window_id),
                kind,
                focused: focused_id.as_deref() == Some(p.id.as_str()),
                subtitle,
                last_activity_at: p.last_activity_at,
            });
        }

        let sort_mode = workspace
            .data()
            .window(self.window_id)
            .map(|w| w.agent_sort_mode)
            .unwrap_or_default();
        match sort_mode {
            // Most recent first. A session that has never run anything has no
            // activity stamp and sorts last rather than first, which is where a
            // just-created-but-idle session belongs.
            AgentSortMode::Activity => sessions.sort_by(|a, b| {
                b.last_activity_at
                    .cmp(&a.last_activity_at)
                    .then_with(|| a.info.name.cmp(&b.info.name))
            }),
            AgentSortMode::Name => sessions.sort_by_key(|r| r.info.name.to_lowercase()),
        }

        // The tab header already says AGENTS, so an empty list says why it is
        // empty rather than showing nothing at all.
        if sessions.is_empty() {
            return v_flex()
                .child(
                    div()
                        .px(px(12.0))
                        .py(px(10.0))
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child(
                            "No agent sessions. Start work on a task, draft a change \
                             in Specs, or use + above.",
                        ),
                )
                .into_any_element();
        }

        v_flex()
            .children(
                sessions
                    .iter()
                    .map(|row| self.render_session_row(row, cx))
                    .collect::<Vec<_>>(),
            )
            .into_any_element()
    }
}
