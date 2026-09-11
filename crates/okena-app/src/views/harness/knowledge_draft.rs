//! "New with agent" in the Knowledge view: say what to write, pick where and
//! with which agent, and okena opens an agent session there briefed on the
//! store layout.
//!
//! A full-view form rather than a pane, like drafting a spec change:
//! configuring a session and reading knowledge are separate tasks.

use super::HarnessPane;
use crate::theme::theme;
use crate::ui::tokens::ui_text;
use crate::views::components::{SimpleInput, SimpleInputState};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::knowledge::{KnowledgeRootKind, KnowledgeStores};

/// State of the "New with agent" form.
pub(crate) struct DraftForm {
    pub(crate) open: bool,
    pub(crate) request: Entity<SimpleInputState>,
    /// Root to write in. Follows the open root until picked in the form.
    pub(crate) root: Option<String>,
    pub(crate) starting: bool,
    pub(crate) error: Option<String>,
    /// What the last start did, shown above the view once the form closes.
    pub(crate) notice: Option<String>,
}

impl DraftForm {
    pub(crate) fn new(cx: &mut Context<HarnessPane>) -> Self {
        let request = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("e.g. document how CI caches dependencies, and when to bust the cache")
                .multiline()
        });
        Self {
            open: false,
            request,
            root: None,
            starting: false,
            error: None,
            notice: None,
        }
    }
}

impl HarnessPane {
    pub(super) fn open_knowledge_draft(&mut self, cx: &mut Context<Self>) {
        self.knowledge_draft.open = true;
        self.knowledge_draft.root = None;
        self.knowledge_draft.error = None;
        self.knowledge_draft.notice = None;
        cx.notify();
    }

    /// Where the draft goes: the root picked in the form, else the open one.
    fn knowledge_draft_target(&self) -> Option<String> {
        self.knowledge_draft
            .root
            .clone()
            .or_else(|| self.knowledge.root_key.clone())
    }

    fn start_knowledge_draft(&mut self, agent: String, cx: &mut Context<Self>) {
        if self.knowledge_draft.starting {
            return;
        }
        let request = self
            .knowledge_draft
            .request
            .read(cx)
            .value()
            .trim()
            .to_string();
        if request.is_empty() {
            self.knowledge_draft.error = Some("Say what to write first.".into());
            cx.notify();
            return;
        }
        self.knowledge_draft.starting = true;
        self.knowledge_draft.error = None;
        cx.notify();

        let client = self.client.clone();
        let root = self.knowledge_draft_target();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::KnowledgeDraft {
                        root,
                        request,
                        agent_command: Some(agent),
                    })
                    .and_then(|v| v.ok_or_else(|| "Missing draft result".to_string()))
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.knowledge_draft.starting = false;
                    match result {
                        Ok(v) => {
                            this.knowledge_draft.open = false;
                            this.knowledge_draft.root = None;
                            this.knowledge_draft
                                .request
                                .update(cx, |i, cx| i.set_value("", cx));
                            let name = v
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("the session");
                            // The session runs in its own terminal, reachable
                            // from the sidebar; what it writes shows up here on
                            // the next refresh.
                            this.knowledge_draft.notice = Some(format!(
                                "Started {name} — it is in the sidebar. Refresh to see what it writes."
                            ));
                        }
                        Err(e) => this.knowledge_draft.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(super) fn render_knowledge_draft_form(
        &self,
        stores: &KnowledgeStores,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);

        let target = self.knowledge_draft_target();
        let mut roots = h_flex().gap(px(6.0)).flex_wrap();
        for root in stores.roots.iter().filter(|r| r.healthy) {
            let key = root.key.clone();
            let kind = match root.kind {
                KnowledgeRootKind::Store => "store",
                KnowledgeRootKind::Project => "project",
            };
            roots = roots.child(self.choice_chip(
                format!("knowledge-draft-root-{}", root.key),
                format!("{} · {kind}", root.name),
                target.as_deref() == Some(root.key.as_str()),
                move |this, _cx| this.knowledge_draft.root = Some(key.clone()),
                cx,
            ));
        }
        let target_hint = match target.as_deref().and_then(|k| stores.root(k)) {
            Some(root) if root.kind == KnowledgeRootKind::Store => format!(
                "The agent works in {} on a new knowledge/… branch, commits when done, and does not push unless you ask.",
                root.path
            ),
            Some(root) => format!(
                "The agent works in {}. Committing is left to you.",
                root.path
            ),
            None => "Pick where the knowledge should live.".to_string(),
        };

        let starting = self.knowledge_draft.starting;
        let mut body = v_flex()
            .w_full()
            .max_w(px(720.0))
            .gap(px(18.0))
            .child(
                v_flex()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(ui_text(15.0, cx))
                            .text_color(rgb(t.text_primary))
                            .child("Write with an agent"),
                    )
                    .child(self.field_hint(
                        "okena opens an agent session in the knowledge root, briefed on its \
                         layout — docs/, skills/, agents/ and templates/ — and on the \
                         frontmatter people and agents pick entries by.",
                        cx,
                    )),
            )
            .child(
                v_flex()
                    .gap(px(6.0))
                    .child(self.field_label("Where", cx))
                    .child(roots)
                    .child(self.field_hint(&target_hint, cx)),
            )
            .child(
                v_flex()
                    .gap(px(5.0))
                    .child(self.field_label("What to write", cx))
                    .child(
                        okena_ui::input::input_container(&t, None)
                            .w_full()
                            .h(px(140.0))
                            .px(px(8.0))
                            .py(px(6.0))
                            .child(
                                SimpleInput::new(&self.knowledge_draft.request)
                                    .text_size(ui_text(13.0, cx)),
                            ),
                    )
                    .child(self.field_hint(
                        "A new doc, a skill, a subagent or a template — or what to change in \
                         one. This is the agent's brief, so context beats brevity.",
                        cx,
                    )),
            );

        if let Some(err) = self.knowledge_draft.error.clone() {
            body = body.child(self.error_banner(err, cx));
        }

        // No "without an agent" here: writing is the agent's whole job, and
        // an empty entry is something you can make without okena.
        let launcher = okena_ui::agent_launcher::AgentLauncher::new(
            "knowledge-draft-launcher",
            match target.as_deref().and_then(|k| stores.root(k)) {
                Some(root) => format!("Write in {}", root.name),
                None => "Write with an agent".to_string(),
            },
        )
        .options(crate::views::agent_session::launch_options(
            self.tasks.default_agent.as_deref(),
            &t,
        ))
        .preferred(self.tasks.default_agent.clone())
        .busy(starting.then_some("Starting…"))
        .on_launch(cx.listener(|this, command: &SharedString, _window, cx| {
            this.start_knowledge_draft(command.to_string(), cx);
        }));

        body = body.child(
            h_flex()
                .items_center()
                .gap(px(12.0))
                .child(self.small_button(
                    "knowledge-draft-cancel",
                    "Cancel",
                    cx.listener(|this, _, _window, cx| {
                        this.knowledge_draft.open = false;
                        this.knowledge_draft.root = None;
                        this.knowledge_draft.error = None;
                        cx.notify();
                    }),
                    cx,
                ))
                .child(div().flex_1().min_w_0().child(launcher)),
        );

        v_flex()
            .id("knowledge-draft-form")
            .size_full()
            .overflow_y_scroll()
            .items_center()
            .px(px(24.0))
            .py(px(24.0))
            .child(body)
            .into_any_element()
    }
}
