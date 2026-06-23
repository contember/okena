//! The sidebar "AGENTS" section — a flat, cross-project list of panes running
//! an AI coding agent that reports its status via `OSC 9001`.
//!
//! This is the multi-agent "mission control": every active agent at a glance,
//! sorted by attention (blocked → done → working → idle), regardless of which
//! project it lives in. Clicking a row jumps straight to that pane. The section
//! hides itself when no agent is active. Per-pane indicators still live on the
//! tab itself (see `okena-views-terminal`).

use okena_core::agent_status::AgentStatus;
use okena_ui::theme::theme;
use okena_ui::tokens::{ui_text_md, ui_text_ms, ui_text_sm};
use gpui::*;
use gpui_component::tooltip::Tooltip;

use crate::sidebar::Sidebar;

/// One running agent, projected for rendering (owned so we don't hold the
/// workspace / terminal-registry locks while building elements).
struct SidebarAgentInfo {
    terminal_id: String,
    project_id: String,
    display_name: String,
    project_name: String,
    status: AgentStatus,
}

impl Sidebar {
    /// Render the AGENTS section (header + one row per active agent), or an
    /// empty element when no agent is reporting a status.
    pub fn render_agents_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let agents = self.collect_agents(cx);
        if agents.is_empty() {
            return div().into_any_element();
        }

        let mut children: Vec<AnyElement> = Vec::new();
        children.push(self.render_agents_header(agents.len(), cx).into_any_element());
        for agent in &agents {
            children.push(self.render_agent_row(agent, cx).into_any_element());
        }
        div().children(children).into_any_element()
    }

    /// Collect every non-hook terminal across all projects that currently has an
    /// agent status, sorted by lifecycle priority (blocked first) then name.
    fn collect_agents(&self, cx: &mut Context<Self>) -> Vec<SidebarAgentInfo> {
        let workspace = self.workspace.read(cx);
        let terminals = self.terminals.lock();
        let mut agents: Vec<SidebarAgentInfo> = Vec::new();
        for project in workspace.projects() {
            let Some(layout) = project.layout.as_ref() else {
                continue;
            };
            for tid in layout.collect_terminal_ids() {
                // Hook terminals have their own panel.
                if project.hook_terminals.contains_key(&tid) {
                    continue;
                }
                let Some(term) = terminals.get(tid.as_str()) else {
                    continue;
                };
                let Some(status) = term.agent_status() else {
                    continue;
                };
                let display_name = project.terminal_display_name(&tid, term.title());
                agents.push(SidebarAgentInfo {
                    terminal_id: tid,
                    project_id: project.id.clone(),
                    display_name,
                    project_name: project.name.clone(),
                    status,
                });
            }
        }
        agents.sort_by(|a, b| {
            b.status
                .lifecycle
                .priority()
                .cmp(&a.status.lifecycle.priority())
                .then_with(|| a.display_name.cmp(&b.display_name))
        });
        agents
    }

    fn render_agents_header(&self, count: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        div()
            .h(px(28.0))
            .px(px(12.0))
            .mt(px(8.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(t.text_secondary))
                    .child("AGENTS"),
            )
            .child(
                div()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(count.to_string()),
            )
    }

    fn render_agent_row(&self, agent: &SidebarAgentInfo, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let dot_color = rgb(agent.status.lifecycle.theme_color(&t));
        let project_id = agent.project_id.clone();
        let terminal_id = agent.terminal_id.clone();
        let custom = agent.status.custom.clone().filter(|c| !c.is_empty());

        // Two-line layout: the terminal title (usually the agent's task) on top,
        // the project — plus any free-form status the agent reports — muted
        // underneath, so a long title can no longer crowd out the project the
        // pane belongs to. Both lines ellipsize independently.
        let title = agent.display_name.clone();
        // Hover reveals whatever the lines truncate.
        let tooltip = match &custom {
            Some(c) => format!("{title} · {c}"),
            None => title.clone(),
        };
        // Second line: "project" or "project · custom status".
        let subtitle = match &custom {
            Some(c) => format!("{} · {c}", agent.project_name),
            None => agent.project_name.clone(),
        };

        div()
            .id(ElementId::Name(format!("agent-row-{}", agent.terminal_id).into()))
            .px(px(12.0))
            .py(px(5.0))
            .flex()
            .items_start()
            .gap(px(6.0))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.cursor_index = None;
                let workspace = this.workspace.clone();
                this.focus_manager.update(cx, |fm, cx| {
                    workspace.update(cx, |ws, cx| {
                        ws.focus_terminal_by_id(fm, &project_id, &terminal_id, cx);
                    });
                    cx.notify();
                });
            }))
            // Lifecycle status dot, nudged down to sit on the title line.
            .child(
                div()
                    .mt(px(4.0))
                    .w(px(8.0))
                    .h(px(8.0))
                    .rounded_full()
                    .bg(dot_color)
                    .flex_shrink_0(),
            )
            // Text column: title over project, each ellipsized.
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    // Line 1 — terminal title / agent task.
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(ui_text_md(cx))
                            .text_color(rgb(t.text_primary))
                            .child(title),
                    )
                    // Line 2 — project (+ optional free-form status).
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_muted))
                            .child(subtitle),
                    ),
            )
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
    }
}
