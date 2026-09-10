//! Specs view — the OpenSpec documents in the configured spec repository.
//!
//! Reading and writing both go through the daemon, which owns the repository
//! path and refuses any path that resolves outside it. The client never touches
//! the filesystem itself.
//!
//! Documents render as plain text rather than formatted Markdown. OpenSpec is
//! deliberately plain Markdown, and showing the file as written is honest about
//! what an agent will read and edit — rich rendering can come later without
//! changing anything here.

use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms};
use crate::views::components::SimpleInput;
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::specs::{SpecChange, SpecDoc, SpecTree};

use super::HarnessPane;

/// Agents offered for drafting. Same list the Tasks view launches from, so
/// anything okena can start work with can also write a spec.
const AGENT_CHOICES: &[&str] = &["claude", "copilot"];

/// Width of the document list. Fixed rather than draggable: the list holds
/// short file names, and a second resizable divider in the harness would be
/// more chrome than it earns.
const TREE_WIDTH: f32 = 260.0;

impl HarnessPane {
    /// Load the spec tree from the daemon.
    pub(super) fn refresh_specs(&mut self, cx: &mut Context<Self>) {
        if self.specs.loading {
            return;
        }
        self.specs.loading = true;
        self.specs.error = None;
        cx.notify();

        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::SpecsTree)
                    .and_then(|v| v.ok_or_else(|| "Missing spec tree".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<SpecTree>(v)
                            .map_err(|e| format!("Unexpected spec tree: {e}"))
                    })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.specs.loading = false;
                    match result {
                        Ok(tree) => {
                            this.specs.tree = Some(tree);
                            // Drop a selection whose document no longer exists,
                            // so a deleted or renamed file doesn't leave stale
                            // content on screen looking current.
                            if let Some(path) = this.specs.selected.clone()
                                && !this.specs.tree_contains(&path)
                            {
                                this.specs.selected = None;
                                this.specs.content = None;
                            }
                        }
                        Err(e) => this.specs.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Load one document's content.
    pub(super) fn open_spec_doc(&mut self, path: String, cx: &mut Context<Self>) {
        self.specs.selected = Some(path.clone());
        self.specs.content = None;
        self.specs.content_error = None;
        cx.notify();

        let client = self.client.clone();
        let wanted = path.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::SpecRead { path })
                    .and_then(|v| v.ok_or_else(|| "Missing document".to_string()))
                    .map(|v| {
                        v.get("content")
                            .and_then(|c| c.as_str())
                            .unwrap_or_default()
                            .to_string()
                    })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    // Ignore a response for a document the user has already
                    // navigated away from, or a slow read would overwrite a
                    // faster one selected afterwards.
                    if this.specs.selected.as_deref() != Some(wanted.as_str()) {
                        return;
                    }
                    match result {
                        Ok(content) => this.specs.content = Some(content),
                        Err(e) => this.specs.content_error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Scaffold the configured change, start its agent, and return to the specs.
    pub(super) fn draft_spec_change(&mut self, cx: &mut Context<Self>) {
        if self.specs.drafting {
            return;
        }
        let idea = self.specs.idea_input.read(cx).value().trim().to_string();
        if idea.is_empty() {
            self.specs.error = Some("Describe the change first.".into());
            cx.notify();
            return;
        }
        let name = self.specs.name_input.read(cx).value().trim().to_string();
        self.specs.drafting = true;
        self.specs.error = None;
        cx.notify();

        let client = self.client.clone();
        // An explicit empty string is the daemon's "scaffold only, no agent".
        let agent_command = Some(self.specs.agent.clone().unwrap_or_default());
        let name = (!name.is_empty()).then_some(name);
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::SpecDraftChange {
                        idea,
                        name,
                        agent_command,
                    })
                    .and_then(|v| v.ok_or_else(|| "Missing draft result".to_string()))
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.specs.drafting = false;
                    match result {
                        Ok(v) => {
                            let change = v
                                .get("change")
                                .and_then(|c| c.as_str())
                                .unwrap_or("change")
                                .to_string();
                            // Back to the specs: the session runs in its own
                            // terminal, reachable from the sidebar, and the
                            // thing worth looking at here is the change itself.
                            this.specs.composing = false;
                            for input in [&this.specs.idea_input, &this.specs.name_input] {
                                input.update(cx, |i, cx| i.set_value("", cx));
                            }
                            // Expand it: the user just made it, so its
                            // artifacts are what they want to see next.
                            this.specs.collapsed.remove(&change);
                            this.refresh_specs(cx);
                            // Jump straight to the stub okena wrote, so there
                            // is something to read while the agent works.
                            if let Some(path) = v.get("path").and_then(|p| p.as_str()) {
                                this.open_spec_doc(format!("{path}/proposal.md"), cx);
                            }
                        }
                        Err(e) => this.specs.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn toggle_change(&mut self, name: String, cx: &mut Context<Self>) {
        if !self.specs.collapsed.remove(&name) {
            self.specs.collapsed.insert(name);
        }
        cx.notify();
    }

    /// One selectable document row.
    fn render_doc_row(&self, doc: &SpecDoc, indent: f32, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let selected = self.specs.selected.as_deref() == Some(doc.path.as_str());
        let path = doc.path.clone();
        div()
            .id(SharedString::from(format!("spec-doc-{}", doc.path)))
            .cursor_pointer()
            .w_full()
            .min_w_0()
            .pl(px(10.0 + indent))
            .pr(px(8.0))
            .py(px(3.0))
            .rounded(px(3.0))
            .when(selected, |d| d.bg(with_alpha(t.button_primary_bg, 0.18)))
            .when(!selected, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
            .text_size(ui_text_md(cx))
            .text_color(rgb(if selected {
                t.text_primary
            } else {
                t.text_secondary
            }))
            .truncate()
            .child(doc.name.clone())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.open_spec_doc(path.clone(), cx);
                }),
            )
            .into_any_element()
    }

    /// One change directory: a disclosure row over its documents.
    fn render_change(&self, change: &SpecChange, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let collapsed = self.specs.collapsed.contains(&change.name);
        let name = change.name.clone();
        // Artifacts then the change's own specs, which is the order they are
        // written in and the order they are read in.
        let docs: Vec<SpecDoc> = change
            .artifacts
            .iter()
            .chain(change.specs.iter())
            .cloned()
            .collect();

        let mut col = v_flex().w_full().min_w_0().child(
            h_flex()
                .id(SharedString::from(format!("spec-change-{}", change.name)))
                .cursor_pointer()
                .w_full()
                .min_w_0()
                .items_center()
                .gap(px(4.0))
                .px(px(6.0))
                .py(px(3.0))
                .rounded(px(3.0))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .child(
                    div()
                        .w(px(10.0))
                        .flex_shrink_0()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child(if collapsed { "›" } else { "⌄" }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(ui_text_md(cx))
                        .text_color(rgb(t.text_primary))
                        .child(change.name.clone()),
                )
                // Count rather than a spinner: it says at a glance whether the
                // agent has written anything yet.
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child(format!("{}", docs.len())),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        this.toggle_change(name.clone(), cx);
                    }),
                ),
        );
        if !collapsed {
            for doc in &docs {
                col = col.child(self.render_doc_row(doc, 12.0, cx));
            }
            if docs.is_empty() {
                col = col.child(
                    div()
                        .pl(px(22.0))
                        .py(px(2.0))
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child("empty"),
                );
            }
        }
        col.into_any_element()
    }

    fn section_label(&self, label: &str, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .px(px(6.0))
            .pt(px(10.0))
            .pb(px(3.0))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(label.to_uppercase())
            .into_any_element()
    }

    /// Left column: changes, stable specs, and the archive.
    fn render_spec_tree(&self, tree: &SpecTree, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let mut col = v_flex()
            .id("spec-tree")
            .w(px(TREE_WIDTH))
            .flex_shrink_0()
            .h_full()
            .overflow_y_scroll()
            .px(px(6.0))
            .pb(px(10.0))
            .border_r_1()
            .border_color(rgb(t.border));

        col = col.child(self.section_label("Changes", cx));
        if tree.changes.is_empty() {
            col = col.child(
                div()
                    .px(px(6.0))
                    .py(px(3.0))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child("No changes in flight."),
            );
        }
        for change in &tree.changes {
            col = col.child(self.render_change(change, cx));
        }

        col = col.child(self.section_label("Specs", cx));
        if tree.specs.is_empty() {
            col = col.child(
                div()
                    .px(px(6.0))
                    .py(px(3.0))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child("No specs yet."),
            );
        }
        for doc in &tree.specs {
            col = col.child(self.render_doc_row(doc, 0.0, cx));
        }

        // Archived changes are history, so they are listed but never in the
        // way: the section only appears once something has been archived.
        if !tree.archived.is_empty() {
            col = col.child(self.section_label("Archive", cx));
            for change in &tree.archived {
                col = col.child(self.render_change(change, cx));
            }
        }
        col.into_any_element()
    }

    /// Right column: the selected document.
    fn render_spec_document(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let Some(path) = self.specs.selected.clone() else {
            return v_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(ui_text_md(cx))
                        .text_color(rgb(t.text_muted))
                        .child("Select a document."),
                )
                .into_any_element();
        };

        let body: AnyElement = if let Some(err) = &self.specs.content_error {
            self.error_banner(err.clone(), cx)
        } else if let Some(content) = &self.specs.content {
            let mut doc = v_flex()
                .id("spec-document-body")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(16.0))
                .py(px(10.0));
            // Rendered line by line so long documents wrap and select the way
            // the file viewer's do; a single text node would collapse blank
            // lines and lose the document's shape.
            for (i, line) in content.lines().enumerate() {
                doc = doc.child(
                    div()
                        .id(SharedString::from(format!("spec-line-{i}")))
                        .w_full()
                        .min_h(px(16.0))
                        .text_size(ui_text(12.5, cx))
                        .text_color(rgb(t.text_primary))
                        .child(line.to_string()),
                );
            }
            doc.into_any_element()
        } else {
            self.info_banner("Loading…".into(), cx)
        };

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                div()
                    .w_full()
                    .px(px(16.0))
                    .py(px(6.0))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .truncate()
                    .child(path),
            )
            .child(body)
            .into_any_element()
    }

    /// Toolbar actions for the Specs view.
    fn spec_actions(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme(cx);
        vec![
            self.toolbar_icon(
                "specs-settings",
                "icons/settings.svg",
                "Spec settings",
                cx.listener(|this, _, _window, cx| this.open_settings("harness", cx)),
                cx,
            ),
            div()
                .id("spec-new-change")
                .cursor_pointer()
                .flex_shrink_0()
                .px(px(12.0))
                .py(px(4.0))
                .rounded(px(4.0))
                .bg(rgb(t.button_primary_bg))
                .hover(|s| s.bg(rgb(t.button_primary_hover)))
                .text_size(ui_text_md(cx))
                .text_color(rgb(t.button_primary_fg))
                .child("New change")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        this.specs.composing = true;
                        this.specs.error = None;
                        cx.notify();
                    }),
                )
                .into_any_element(),
        ]
    }

    /// A label over a form field.
    fn field_label(&self, label: &str, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_secondary))
            .child(label.to_string())
            .into_any_element()
    }

    /// Explanatory line under a form field.
    fn field_hint(&self, hint: &str, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(hint.to_string())
            .into_any_element()
    }

    /// The full-view new-change form: name, prompt, agent.
    fn render_new_change_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);

        let mut agents = h_flex().gap(px(6.0)).flex_wrap();
        // "No agent" first: scaffolding a change without launching anything is
        // a legitimate choice, not a fallback.
        for choice in std::iter::once(None).chain(AGENT_CHOICES.iter().map(|a| Some(*a))) {
            let selected = self.specs.agent.as_deref() == choice;
            let label = choice.unwrap_or("No agent").to_string();
            let value = choice.map(str::to_string);
            agents = agents.child(
                div()
                    .id(SharedString::from(format!(
                        "spec-agent-{}",
                        choice.unwrap_or("none")
                    )))
                    .cursor_pointer()
                    .px(px(12.0))
                    .py(px(5.0))
                    .rounded(px(4.0))
                    .when(selected, |d| {
                        d.bg(with_alpha(t.button_primary_bg, 0.2))
                            .text_color(rgb(t.text_primary))
                    })
                    .when(!selected, |d| {
                        d.bg(rgb(t.bg_secondary))
                            .text_color(rgb(t.text_secondary))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                    })
                    .text_size(ui_text_md(cx))
                    .child(label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            this.specs.agent = value.clone();
                            this.specs.agent_picked = true;
                            cx.notify();
                        }),
                    ),
            );
        }

        let drafting = self.specs.drafting;
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
                            .child("New change"),
                    )
                    .child(self.field_hint(
                        "okena scaffolds the change under openspec/changes/ and \
                         starts an agent briefed on the OpenSpec conventions.",
                        cx,
                    )),
            )
            .child(
                v_flex()
                    .gap(px(5.0))
                    .child(self.field_label("Change name", cx))
                    .child(
                        okena_ui::input::input_container(&t, None)
                            .w_full()
                            .px(px(8.0))
                            .py(px(6.0))
                            .child(
                                SimpleInput::new(&self.specs.name_input)
                                    .text_size(ui_text(13.0, cx)),
                            ),
                    )
                    .child(self.field_hint(
                        "Becomes the directory name. Leave blank to derive one \
                         from the prompt.",
                        cx,
                    )),
            )
            .child(
                v_flex()
                    .gap(px(5.0))
                    .child(self.field_label("Prompt", cx))
                    .child(
                        okena_ui::input::input_container(&t, None)
                            .w_full()
                            .h(px(140.0))
                            .px(px(8.0))
                            .py(px(6.0))
                            .child(
                                SimpleInput::new(&self.specs.idea_input)
                                    .text_size(ui_text(13.0, cx)),
                            ),
                    )
                    .child(self.field_hint(
                        "What the change is for. This is what the agent is \
                         briefed with, so context beats brevity.",
                        cx,
                    )),
            )
            .child(
                v_flex()
                    .gap(px(6.0))
                    .child(self.field_label("Agent", cx))
                    .child(agents),
            );

        if let Some(err) = self.specs.error.clone() {
            body = body.child(self.error_banner(err, cx));
        }

        body = body.child(
            h_flex()
                .gap(px(8.0))
                .child(
                    div()
                        .id("spec-start")
                        .cursor_pointer()
                        .px(px(16.0))
                        .py(px(7.0))
                        .rounded(px(4.0))
                        .bg(rgb(t.button_primary_bg))
                        .hover(|s| s.bg(rgb(t.button_primary_hover)))
                        .text_size(ui_text_md(cx))
                        .text_color(rgb(t.button_primary_fg))
                        .child(if drafting {
                            "Starting…"
                        } else {
                            "Start session"
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| {
                                this.draft_spec_change(cx);
                            }),
                        ),
                )
                .child(self.small_button(
                    "spec-cancel",
                    "Cancel",
                    cx.listener(move |this, _, _window, cx| {
                        this.specs.composing = false;
                        this.specs.error = None;
                        cx.notify();
                    }),
                    cx,
                )),
        );

        v_flex()
            .id("spec-new-change-form")
            .size_full()
            .overflow_y_scroll()
            .items_center()
            .px(px(24.0))
            .py(px(24.0))
            .child(body)
            .into_any_element()
    }

    pub(super) fn render_specs_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let mut root = v_flex().size_full();

        // An unreadable or unset repository is the whole story — showing an
        // empty tree next to it would imply there is nothing in the repo.
        if let Some(err) = self.specs.error.clone()
            && self.specs.tree.is_none()
        {
            return root
                .child(self.error_banner(err, cx))
                .child(self.info_banner(
                    "Set a spec repository in Settings → Harness, then reopen this view.".into(),
                    cx,
                ))
                .into_any_element();
        }

        let Some(tree) = self.specs.tree.clone() else {
            return root
                .child(self.info_banner("Loading specs…".into(), cx))
                .into_any_element();
        };

        // The new-change form takes the whole view: configuring a session is a
        // separate task from reading specs, and splitting the space between
        // them served neither.
        if self.specs.composing {
            return root
                .child(self.render_new_change_form(cx))
                .into_any_element();
        }

        let actions = self.spec_actions(cx);
        root = root.child(self.render_toolbar(actions, cx));
        if let Some(err) = self.specs.error.clone() {
            root = root.child(self.error_banner(err, cx));
        }
        if !tree.initialized {
            root = root.child(self.info_banner(
                format!(
                    "{} has no openspec/ directory yet — drafting a change creates one.",
                    tree.root
                ),
                cx,
            ));
        }

        root.child(
            h_flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .bg(rgb(t.bg_primary))
                .child(self.render_spec_tree(&tree, cx))
                .child(self.render_spec_document(cx)),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::AGENT_CHOICES;
    use okena_core::specs::{SpecChange, SpecDoc, SpecTree};

    fn doc(path: &str) -> SpecDoc {
        SpecDoc {
            path: path.into(),
            name: path.rsplit('/').next().unwrap().into(),
        }
    }

    fn tree() -> SpecTree {
        SpecTree {
            root: "/repo".into(),
            initialized: true,
            specs: vec![doc("openspec/specs/auth.md")],
            changes: vec![SpecChange {
                name: "add-login".into(),
                path: "openspec/changes/add-login".into(),
                artifacts: vec![doc("openspec/changes/add-login/proposal.md")],
                specs: vec![doc("openspec/changes/add-login/specs/auth.md")],
                archived: false,
            }],
            archived: Vec::new(),
        }
    }

    #[test]
    fn a_listed_document_is_found_anywhere_in_the_tree() {
        let t = tree();
        for path in [
            "openspec/specs/auth.md",
            "openspec/changes/add-login/proposal.md",
            "openspec/changes/add-login/specs/auth.md",
        ] {
            assert!(
                super::super::SpecsState::contains(&t, path),
                "missed {path}"
            );
        }
    }

    #[test]
    fn a_vanished_document_is_not_found() {
        // This is what clears a stale selection after a refresh.
        assert!(!super::super::SpecsState::contains(
            &tree(),
            "openspec/changes/add-login/design.md"
        ));
    }

    #[test]
    fn archived_changes_are_searched_too() {
        // An archived document stays selected while you read it; dropping the
        // selection the moment a change is archived would yank it away.
        let mut t = tree();
        let mut old = t.changes.remove(0);
        old.archived = true;
        t.archived.push(old);
        assert!(super::super::SpecsState::contains(
            &t,
            "openspec/changes/add-login/proposal.md"
        ));
    }

    /// Group projects the way the sidebar and the sessions pane both do.
    fn split_sessions(projects: &[crate::workspace::state::ProjectData]) -> (Vec<&str>, Vec<&str>) {
        let mut specs = Vec::new();
        let mut tasks = Vec::new();
        for p in projects {
            if p.is_spec_session() {
                specs.push(p.id.as_str());
            } else if p.is_agent_session() {
                tasks.push(p.id.as_str());
            }
        }
        (tasks, specs)
    }

    fn project(json: serde_json::Value) -> crate::workspace::state::ProjectData {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn spec_sessions_are_grouped_apart_from_task_sessions() {
        // The sidebar lists the two kinds under separate headings, so a session
        // must land in exactly one group and a plain repo in neither.
        let projects = vec![
            project(serde_json::json!({
                "id": "spec1", "name": "add-login (spec)", "path": "/specs",
                "spec_change": "add-login",
            })),
            project(serde_json::json!({
                "id": "task1", "name": "QBL-1 (agent)", "path": "/p",
                "task_ref": {
                    "id": { "provider": "linear", "external_id": "u1" },
                    "display_key": "QBL-1", "title": "t", "url": "http://x",
                },
            })),
            project(serde_json::json!({
                "id": "repo1", "name": "okena", "path": "/p/okena",
            })),
        ];
        let (tasks, specs) = split_sessions(&projects);
        assert_eq!(tasks, ["task1"]);
        assert_eq!(specs, ["spec1"]);
    }

    #[test]
    fn a_spec_session_never_lands_in_the_task_group() {
        // It has no task link, so the task branch must not claim it even
        // though both are sessions rooted above the repos.
        let p = project(serde_json::json!({
            "id": "spec1", "name": "x (spec)", "path": "/specs",
            "spec_change": "x",
        }));
        assert!(p.is_spec_session() && !p.is_agent_session());
    }

    #[test]
    fn the_draft_agents_are_the_ones_okena_can_launch() {
        assert_eq!(AGENT_CHOICES, ["claude", "copilot"]);
    }
}
