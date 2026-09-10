//! Rendering for the agent-session panel.
//!
//! One implementation at two densities. The sections are the same in both and
//! in the same order — status, subject, where the work lands, what came out —
//! so a session reads identically whether you meet it in the sidebar or in the
//! overview.

use super::{AgentSessionInfo, AgentSessionKind, AgentSessionPanel, PanelTab};
use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_ms};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};

impl AgentSessionPanel {
    fn chip(&self, text: String, color: u32, cx: &App) -> AnyElement {
        div()
            .flex_shrink_0()
            .px(px(6.0))
            .py(px(1.0))
            .rounded(px(3.0))
            .bg(with_alpha(color, 0.15))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(color))
            .child(text)
            .into_any_element()
    }

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
            // The count is optional: a bare "0" beside a heading that is not a
            // list reads as "none found" rather than "not countable".
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

    /// The status pill: what the terminal says, not what the agent claims.
    ///
    /// An agent that stopped reporting still shows as waiting when its prompt
    /// is waiting, which is the state you actually need to act on.
    fn status_pill(&self, info: &AgentSessionInfo, cx: &App) -> AnyElement {
        let t = theme(cx);
        let (color, label) = if !info.running {
            (t.text_muted, "stopped".to_string())
        } else if info.waiting {
            (
                t.warning,
                if info.idle.is_empty() {
                    "waiting".to_string()
                } else {
                    format!("waiting · {}", info.idle)
                },
            )
        } else {
            (t.success, "running".to_string())
        };
        self.chip(label, color, cx)
    }

    /// A clickable row for a checkout this session's work lands in.
    fn workspace_row(&self, w: &super::RelatedWorkspace, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let id = w.project_id.clone();
        let mut chips = Vec::new();
        if let Some(branch) = &w.branch {
            chips.push(self.chip(branch.clone(), t.text_secondary, cx));
        }
        v_flex()
            .id(SharedString::from(format!("agent-ws-{}", w.project_id)))
            .cursor_pointer()
            .w_full()
            .min_w_0()
            .gap(px(2.0))
            .px(px(8.0))
            .py(px(5.0))
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
                    .child(w.name.clone()),
            )
            .children(w.repo.as_ref().map(|repo| {
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(repo.clone())
                    .into_any_element()
            }))
            .when(!chips.is_empty(), |d| {
                d.child(h_flex().gap(px(4.0)).flex_wrap().children(chips))
            })
            // Jumping to a checkout is the common move from here: the agent
            // reports a PR, you go read the diff.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.open_project(id.clone(), cx);
                }),
            )
            .into_any_element()
    }

    fn asset_row(&self, asset: &okena_core::harness::AgentAsset, cx: &App) -> AnyElement {
        let t = theme(cx);
        v_flex()
            .w_full()
            .min_w_0()
            .gap(px(1.0))
            .px(px(8.0))
            .py(px(5.0))
            .rounded(px(4.0))
            .bg(rgb(t.bg_primary))
            .border_1()
            .border_color(rgb(t.border))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_primary))
                    .child(asset.title.clone()),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(match (&asset.project, &asset.url) {
                        (Some(p), _) => format!("{} · {p}", asset.kind.label()),
                        (None, Some(url)) => format!("{} · {url}", asset.kind.label()),
                        (None, None) => asset.kind.label().to_string(),
                    }),
            )
            .into_any_element()
    }

    /// The scrolling body: every section, in the same order at both densities.
    fn render_info(&self, info: &AgentSessionInfo, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);

        let mut body = v_flex()
            .id(SharedString::from(format!(
                "agent-panel-body-{}",
                info.project_id
            )))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(10.0))
            .pb(px(12.0))
            .gap(px(4.0));

        // ── Session ──────────────────────────────────────────────────────────
        let mut facts = vec![self.chip(info.kind.label().to_string(), t.button_primary_bg, cx)];
        if let Some(agent) = &info.agent {
            facts.push(self.chip(agent.clone(), t.text_secondary, cx));
        }
        facts.push(if info.mcp {
            self.chip("okena mcp".to_string(), t.success, cx)
        } else {
            // Not an error: an agent started outside okena simply was not
            // handed the config, so it cannot report back.
            self.chip("no okena mcp".to_string(), t.text_muted, cx)
        });

        body = body
            .child(self.section_heading("SESSION", None, cx))
            .child(h_flex().gap(px(4.0)).flex_wrap().children(facts))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(info.root.clone()),
            );

        // What the agent says it is doing, as opposed to what the terminal
        // state implies — the two disagree often enough to show both.
        if let Some(status) = &info.status {
            body = body.child(
                div()
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded(px(4.0))
                    .bg(with_alpha(t.success, 0.1))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_primary))
                    .child(format!("“{status}”")),
            );
        }

        // Stop while it runs, start when it does not — the same slot, since
        // only one of them ever applies.
        if info.running {
            body = body.child(
                div()
                    .id("agent-panel-stop")
                    .cursor_pointer()
                    .mt(px(4.0))
                    .px(px(10.0))
                    .py(px(5.0))
                    .rounded(px(4.0))
                    .bg(rgb(t.bg_secondary))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_primary))
                    .child("Stop agent")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            this.stop_agent(cx);
                        }),
                    ),
            );
        } else {
            body = body.child(
                div()
                    .id("agent-panel-restart")
                    .cursor_pointer()
                    .mt(px(4.0))
                    .px(px(10.0))
                    .py(px(5.0))
                    .rounded(px(4.0))
                    .bg(rgb(t.button_primary_bg))
                    .hover(|s| s.bg(rgb(t.button_primary_hover)))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.button_primary_fg))
                    .child("Start agent")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            this.restart_agent(cx);
                        }),
                    ),
            );
        }

        // ── What it is working on ────────────────────────────────────────────
        if let Some(subject) = info.kind.subject() {
            let heading = match info.kind {
                AgentSessionKind::Spec { .. } => "CHANGE",
                AgentSessionKind::Custom { .. } => "GOAL",
                _ => "TASK",
            };
            body = body.child(self.section_heading(heading, None, cx)).child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(subject),
            );
            if let AgentSessionKind::Task(task) = &info.kind {
                body = body.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .truncate()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child(task.url.clone()),
                );
            }
        }

        // ── Where the work lands ─────────────────────────────────────────────
        body = body.child(self.section_heading(
            info.kind.workspace_heading(),
            Some(info.workspaces.len()),
            cx,
        ));
        if info.workspaces.is_empty() {
            body = body.child(self.note(info.kind.empty_workspace_note(), cx));
        }
        for w in &info.workspaces {
            body = body.child(self.workspace_row(w, cx));
        }

        // ── What came out ────────────────────────────────────────────────────
        body = body.child(self.section_heading("PRODUCED", Some(info.assets.len()), cx));
        if info.assets.is_empty() {
            body = body.child(self.note(
                if info.mcp {
                    // Distinguishes "produced nothing" from "cannot tell us",
                    // which look identical without this.
                    "Nothing registered yet."
                } else {
                    "Cannot report — no okena MCP."
                },
                cx,
            ));
        }
        for asset in &info.assets {
            body = body.child(self.asset_row(asset, cx));
        }

        // ── Teardown ─────────────────────────────────────────────────────────
        //
        if self.density.allows_delete() {
            body = body.child(match self.pending_delete {
                Some(force) => self.render_delete_confirm(info, force, cx),
                None => div()
                    .id("agent-panel-delete")
                    .cursor_pointer()
                    .mt(px(10.0))
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .hover(|s| s.bg(with_alpha(t.error, 0.1)))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.error))
                    .child("Delete workspace…")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            // Two steps on purpose: this removes checkouts.
                            this.pending_delete = Some(false);
                            cx.notify();
                        }),
                    )
                    .into_any_element(),
            });
        }

        body.into_any_element()
    }

    /// The armed delete, shown in place of the button.
    fn render_delete_confirm(
        &self,
        info: &AgentSessionInfo,
        force: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let count = info.workspaces.len();
        v_flex()
            .mt(px(10.0))
            .gap(px(6.0))
            .p(px(8.0))
            .rounded(px(4.0))
            .border_1()
            .border_color(rgb(t.error))
            .bg(with_alpha(t.error, 0.06))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_primary))
                    .child(match count {
                        0 => "Delete this session? It has no checkouts.".to_string(),
                        1 => "Delete this session and its 1 checkout?".to_string(),
                        n => format!("Delete this session and its {n} checkouts?"),
                    }),
            )
            .child(
                h_flex()
                    .id("agent-panel-delete-force")
                    .cursor_pointer()
                    .gap(px(6.0))
                    .items_center()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(if force { "☑" } else { "☐" })
                    // Uncommitted work is the one thing git refuses to discard
                    // on its own, so discarding it has to be asked for.
                    .child("Discard uncommitted changes")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            this.pending_delete = Some(!force);
                            cx.notify();
                        }),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(6.0))
                    .child(
                        div()
                            .id("agent-panel-delete-go")
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .bg(rgb(t.error))
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.button_primary_fg))
                            .child("Delete")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _window, cx| {
                                    this.delete_workspace(force, cx);
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id("agent-panel-delete-cancel")
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .bg(rgb(t.bg_secondary))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_primary))
                            .child("Cancel")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _window, cx| {
                                    this.pending_delete = None;
                                    cx.notify();
                                }),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// Header: who this session is, how it is doing, and the way into it.
    fn render_header(&self, info: &AgentSessionInfo, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let open_id = info.project_id.clone();

        let mut header = v_flex()
            .gap(px(6.0))
            .px(px(10.0))
            .py(px(8.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .child(
                h_flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_text(13.0, cx))
                            .text_color(rgb(t.text_primary))
                            .child(info.name.clone()),
                    )
                    .child(self.status_pill(info, cx)),
            );

        // The tab strip only exists where the terminal isn't already on screen,
        // which is exactly where the panel draws its own header.
        {
            let tab = |this: &Self,
                       label: &'static str,
                       value: PanelTab,
                       cx: &mut Context<Self>|
             -> AnyElement {
                let selected = this.tab == value;
                div()
                    .id(ElementId::Name(
                        format!("agent-tab-{}-{label}", this.project_id).into(),
                    ))
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .when(selected, |d| {
                        d.bg(with_alpha(t.button_primary_bg, 0.2))
                            .text_color(rgb(t.text_primary))
                    })
                    .when(!selected, |d| {
                        d.text_color(rgb(t.text_muted))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                    })
                    .text_size(ui_text_ms(cx))
                    .child(label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            this.tab = value;
                            cx.notify();
                        }),
                    )
                    .into_any_element()
            };
            header = header.child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap(px(6.0))
                    .child(
                        h_flex()
                            .gap(px(2.0))
                            .child(tab(self, "Info", PanelTab::Info, cx))
                            .child(tab(self, "Terminal", PanelTab::Terminal, cx)),
                    )
                    .child(
                        div()
                            .id(ElementId::Name(
                                format!("agent-open-{}", info.project_id).into(),
                            ))
                            .cursor_pointer()
                            .px(px(8.0))
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .bg(rgb(t.bg_secondary))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_primary))
                            .child("Open")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _window, cx| {
                                    this.open_project(open_id.clone(), cx);
                                }),
                            ),
                    ),
            );
        }

        header.into_any_element()
    }
}

impl Render for AgentSessionPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let Some(info) = self.info(cx) else {
            // The session went away — deleted, or its window closed. Say so
            // rather than rendering an empty shell that looks like a loading
            // state that never resolves.
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .bg(rgb(t.bg_primary))
                .child(self.note("This session is gone.", cx));
        };

        // Bind the terminal here rather than only on the tab switch: a session
        // that has just restarted has no terminal in the snapshot yet, and its
        // layout arrives a frame or two later.
        if self.density.has_header() && self.tab == PanelTab::Terminal {
            self.sync_terminal(cx);
        }

        let body = if self.density.has_header() && self.tab == PanelTab::Terminal {
            self.render_terminal(cx)
        } else {
            self.render_info(&info, cx)
        };

        let mut root = v_flex().size_full().bg(rgb(t.bg_primary));
        if self.density.has_header() {
            root = root.child(self.render_header(&info, cx));
        }
        root.child(body)
    }
}
