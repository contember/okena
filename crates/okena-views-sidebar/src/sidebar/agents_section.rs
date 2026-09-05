//! AGENTS section — agent sessions, separated from the repos.
//!
//! An agent session is a project rooted at the configured projects directory
//! rather than a repo, created when a task spans several of them. Listing it
//! among the repos makes it look like one; it isn't, so it gets its own
//! section with the task it belongs to.
//!
//! Two kinds live here and are grouped apart: sessions working a task, and
//! sessions writing a spec. They are started from different views, act on
//! different repositories, and get closed on different schedules, so a single
//! flat list would make you read every row to find the one you meant.

use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_ui::theme::theme;
use okena_ui::tokens::ui_text_ms;

use super::{Sidebar, SidebarList, SidebarProjectInfo};

/// A session row: the project, and the task key or change name it belongs to.
struct SessionRow {
    info: SidebarProjectInfo,
    /// Task key for a task session, change name for a spec session.
    subtitle: Option<String>,
}

impl Sidebar {
    /// Heading over a group of sessions.
    ///
    /// Only rendered when both groups have rows: with one kind present the
    /// heading says nothing the rows don't, and the AGENTS tab header already
    /// names what the list is.
    fn session_group_label(&self, label: &str, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .px(px(12.0))
            .pt(px(10.0))
            .pb(px(2.0))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(label.to_string())
            .into_any_element()
    }

    fn render_session_row(&self, row: &SessionRow, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let id = row.info.id.clone();
        let name = row.info.name.clone();
        let subtitle = row.subtitle.clone();
        div()
            .id(SharedString::from(format!("agent-session-{id}")))
            .cursor_pointer()
            .h(px(28.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(okena_ui::tokens::ui_text(13.0, cx))
                    .text_color(rgb(t.text_primary))
                    .child(name),
            )
            .children(subtitle.map(|key| {
                div()
                    .flex_shrink_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(key)
                    .into_any_element()
            }))
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.leave_harness_view(cx);
                let workspace = this.workspace.clone();
                let id = id.clone();
                this.focus_manager.update(cx, |fm, cx| {
                    workspace.update(cx, |ws, cx| {
                        ws.set_focused_project_individual(fm, Some(id.clone()), cx);
                    });
                    cx.notify();
                });
            }))
            .into_any_element()
    }

    /// PROJECTS / AGENTS tabs.
    ///
    /// Two tabs rather than stacking agent sessions under the repos: they are a
    /// different kind of thing, and stacking them pushed the list the user was
    /// reading off-screen. Its own row rather than part of the overview row,
    /// which already carries the ordering and create menus.
    pub(super) fn render_list_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let list = self.list;
        let tab = |label: &'static str, mode: SidebarList, cx: &mut Context<Self>| {
            let selected = list == mode;
            div()
                .id(ElementId::Name(label.into()))
                .cursor_pointer()
                .px(px(6.0))
                .py(px(2.0))
                .rounded(px(4.0))
                .when(!selected, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
                .text_size(ui_text_ms(cx))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(if selected {
                    t.text_primary
                } else {
                    t.text_muted
                }))
                .child(label)
                .on_click(cx.listener(move |this, _, _window, cx| {
                    if this.list != mode {
                        this.list = mode;
                        // The cursor indexes into the list that is going away.
                        this.cursor_index = None;
                        cx.notify();
                    }
                }))
        };
        h_flex()
            .h(px(24.0))
            .w_full()
            .items_center()
            .gap(px(2.0))
            .pl(px(20.0))
            .pr(px(12.0))
            .child(tab("PROJECTS", SidebarList::Projects, cx))
            .child(tab("AGENTS", SidebarList::Agents, cx))
    }

    pub(super) fn render_agents_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let workspace = self.workspace.read(cx);

        let mut task_sessions: Vec<SessionRow> = Vec::new();
        let mut spec_sessions: Vec<SessionRow> = Vec::new();
        for p in workspace.data().projects.iter() {
            let info = SidebarProjectInfo::from_project(p, workspace, self.window_id);
            // Spec first: a session can only be one kind, and checking the
            // narrower marker first keeps that obvious.
            if let Some(change) = p.spec_change.clone() {
                spec_sessions.push(SessionRow {
                    info,
                    subtitle: Some(change),
                });
            } else if p.is_agent_session() {
                let subtitle = p.task_ref.as_ref().map(|t| t.display_key.clone());
                task_sessions.push(SessionRow { info, subtitle });
            }
        }

        // The tab header already says AGENTS, so an empty list says why it is
        // empty rather than showing nothing at all.
        if task_sessions.is_empty() && spec_sessions.is_empty() {
            return div()
                .px(px(12.0))
                .py(px(12.0))
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child(
                    "No agent sessions. Start work on a task, or draft a change \
                     in Specs.",
                )
                .into_any_element();
        }

        let label_groups = !task_sessions.is_empty() && !spec_sessions.is_empty();
        let mut children: Vec<AnyElement> = Vec::new();

        if !task_sessions.is_empty() {
            if label_groups {
                children.push(self.session_group_label("TASKS", cx));
            }
            for row in &task_sessions {
                children.push(self.render_session_row(row, cx));
            }
        }

        if !spec_sessions.is_empty() {
            if label_groups {
                // A divider as well as a heading: the two groups look alike at
                // a glance, and the heading alone reads as part of the list
                // above it.
                children.push(
                    div()
                        .mt(px(6.0))
                        .mx(px(12.0))
                        .h(px(1.0))
                        .bg(rgb(t.border))
                        .into_any_element(),
                );
                children.push(self.session_group_label("SPEC WRITERS", cx));
            }
            for row in &spec_sessions {
                children.push(self.render_session_row(row, cx));
            }
        }

        v_flex().children(children).into_any_element()
    }
}
