//! Specs view — OpenSpec roots, and the planning tree inside the one you pick.
//!
//! Roots are whatever the daemon discovered the way the `openspec` CLI would:
//! stores registered on this machine, projects with their own `openspec/` tree,
//! folders from settings. Reading and writing both go through the daemon, which
//! refuses any root it did not discover and any path that resolves outside a
//! root. The client never touches the filesystem itself.
//!
//! Documents render as plain text rather than formatted Markdown. OpenSpec is
//! deliberately plain Markdown, and showing the file as written is honest about
//! what an agent will read and edit — rich rendering can come later without
//! changing anything here.

use crate::theme::{ThemeColors, theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms};
use crate::views::components::SimpleInput;
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::specs::{
    SpecChange, SpecDiagnostic, SpecDoc, SpecRoot, SpecRootKind, SpecSeverity, SpecStores, SpecTree,
};

use super::HarnessPane;

/// Agents offered for drafting. Same list the Tasks view launches from, so
/// anything okena can start work with can also write a spec.
pub(super) const AGENT_CHOICES: &[&str] = &["claude", "copilot"];

/// Width of the root and document list. Fixed rather than draggable: the list
/// holds short names, and a second resizable divider in the harness would be
/// more chrome than it earns.
const TREE_WIDTH: f32 = 280.0;

fn kind_label(kind: SpecRootKind) -> &'static str {
    match kind {
        SpecRootKind::Store => "store",
        SpecRootKind::Project => "project",
        SpecRootKind::Folder => "folder",
    }
}

/// A problem, something worth a look, or fine.
fn health_color(root: &SpecRoot, t: &ThemeColors) -> u32 {
    let warned = root
        .status
        .iter()
        .any(|d| d.severity == SpecSeverity::Warning)
        || root.references.iter().any(|r| !r.status.is_empty());
    if !root.healthy {
        t.error
    } else if warned {
        t.warning
    } else {
        t.success
    }
}

/// How to reach this root from the CLI.
fn cli_hint(root: &SpecRoot) -> String {
    match (root.kind, root.store_id.as_deref()) {
        (SpecRootKind::Store, Some(id)) => format!("openspec list --store {id}"),
        _ => format!("cd {} && openspec list", root.path),
    }
}

type Loaded = (SpecStores, Option<String>, Option<Result<SpecTree, String>>);

impl HarnessPane {
    /// Load the discovered roots, then the tree of the open (or default) root.
    pub(super) fn refresh_specs(&mut self, cx: &mut Context<Self>) {
        self.specs.load_generation += 1;
        let generation = self.specs.load_generation;
        self.specs.loading = true;
        self.specs.error = None;
        cx.notify();

        let client = self.client.clone();
        let wanted = self.specs.root_key.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || -> Result<Loaded, String> {
                let stores = client
                    .post_action(ActionRequest::SpecStores)
                    .and_then(|v| v.ok_or_else(|| "Missing spec stores".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<SpecStores>(v)
                            .map_err(|e| format!("Unexpected spec stores: {e}"))
                    })?;
                // Stay on the open root while it still exists; otherwise open
                // the default one.
                let key = wanted
                    .filter(|k| stores.root(k).is_some())
                    .or_else(|| stores.default_root().map(|r| r.key.clone()));
                let tree = key.clone().map(|k| {
                    client
                        .post_action(ActionRequest::SpecsTree { root: Some(k) })
                        .and_then(|v| v.ok_or_else(|| "Missing spec tree".to_string()))
                        .and_then(|v| {
                            serde_json::from_value::<SpecTree>(v)
                                .map_err(|e| format!("Unexpected spec tree: {e}"))
                        })
                });
                Ok((stores, key, tree))
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.specs.load_generation != generation {
                        return;
                    }
                    this.specs.loading = false;
                    match result {
                        Ok((stores, key, tree)) => {
                            if key != this.specs.root_key {
                                this.specs.selected = None;
                                this.specs.content = None;
                                this.specs.content_error = None;
                            }
                            this.specs.root_key = key;
                            this.specs.stores = Some(stores);
                            match tree {
                                Some(Ok(tree)) => {
                                    this.specs.tree = Some(tree);
                                    // Drop a selection whose document no longer
                                    // exists, so a deleted or renamed file
                                    // doesn't leave stale content on screen
                                    // looking current.
                                    if let Some(path) = this.specs.selected.clone()
                                        && !this.specs.tree_contains(&path)
                                    {
                                        this.specs.selected = None;
                                        this.specs.content = None;
                                    }
                                }
                                Some(Err(e)) => {
                                    this.specs.tree = None;
                                    this.specs.error = Some(e);
                                }
                                None => this.specs.tree = None,
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

    fn select_spec_root(&mut self, key: String, cx: &mut Context<Self>) {
        if self.specs.root_key.as_deref() == Some(key.as_str()) {
            return;
        }
        self.specs.root_key = Some(key);
        self.specs.tree = None;
        self.specs.selected = None;
        self.specs.content = None;
        self.specs.content_error = None;
        self.specs.collapsed.clear();
        self.refresh_specs(cx);
    }

    /// Load one document's content.
    pub(super) fn open_spec_doc(&mut self, path: String, cx: &mut Context<Self>) {
        self.specs.selected = Some(path.clone());
        self.specs.content = None;
        self.specs.content_error = None;
        cx.notify();

        let client = self.client.clone();
        let root = self.specs.root_key.clone();
        let wanted = path.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::SpecRead { root, path })
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

    /// The root a new change goes into: the one picked in the form, else the
    /// open one.
    fn draft_target(&self) -> Option<String> {
        self.specs
            .draft_root
            .clone()
            .or_else(|| self.specs.root_key.clone())
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
        let root = self.draft_target();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::SpecDraftChange {
                        root,
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
                            this.specs.draft_root = None;
                            for input in [&this.specs.idea_input, &this.specs.name_input] {
                                input.update(cx, |i, cx| i.set_value("", cx));
                            }
                            // Open the root it went into, with the change
                            // expanded: the user just made it.
                            if let Some(root) = v.get("root").and_then(|r| r.as_str()) {
                                this.specs.root_key = Some(root.to_string());
                            }
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

    /// One selectable root in the root list.
    fn render_root_row(&self, root: &SpecRoot, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let selected = self.specs.root_key.as_deref() == Some(root.key.as_str());
        let key = root.key.clone();
        h_flex()
            .id(SharedString::from(format!("spec-root-{}", root.key)))
            .cursor_pointer()
            .w_full()
            .min_w_0()
            .items_center()
            .gap(px(6.0))
            .px(px(6.0))
            .py(px(3.0))
            .rounded(px(3.0))
            .when(selected, |d| d.bg(with_alpha(t.button_primary_bg, 0.18)))
            .when(!selected, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
            .child(
                div()
                    .size(px(6.0))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(rgb(health_color(root, &t))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(if selected {
                        t.text_primary
                    } else {
                        t.text_secondary
                    }))
                    .child(root.name.clone()),
            )
            .when(root.is_default, |d| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child("default"),
                )
            })
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(kind_label(root.kind)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.select_spec_root(key.clone(), cx);
                }),
            )
            .into_any_element()
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
        // Artifacts then the change's delta specs, which is the order they are
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
                .when_some(
                    change.created.clone().filter(|_| !change.archived),
                    |d, created| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.text_muted))
                                .child(created),
                        )
                    },
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
                        .child("no artifacts yet"),
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

    fn muted_line(&self, text: impl Into<SharedString>, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .px(px(6.0))
            .py(px(3.0))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(text.into())
            .into_any_element()
    }

    /// Left column: the roots, then the open root's changes, specs and archive.
    fn render_spec_tree(&self, stores: &SpecStores, cx: &mut Context<Self>) -> AnyElement {
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

        col = col.child(self.section_label("Roots", cx));
        for root in &stores.roots {
            col = col.child(self.render_root_row(root, cx));
        }

        let Some(tree) = self.specs.tree.clone() else {
            if self.specs.loading {
                col = col.child(self.muted_line("Loading…", cx));
            }
            return col.into_any_element();
        };
        if !tree.initialized {
            col = col.child(self.muted_line(
                "No openspec/ directory yet — drafting a change creates one.",
                cx,
            ));
        }

        col = col.child(self.section_label("Changes", cx));
        if tree.changes.is_empty() {
            col = col.child(self.muted_line("No changes in flight.", cx));
        }
        for change in &tree.changes {
            col = col.child(self.render_change(change, cx));
        }

        col = col.child(self.section_label("Specs", cx));
        if tree.specs.is_empty() {
            col = col.child(self.muted_line("No specs yet.", cx));
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

    fn fact_row(&self, label: &str, value: String, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        h_flex()
            .gap(px(10.0))
            .items_start()
            .min_w_0()
            .child(
                div()
                    .w(px(72.0))
                    .flex_shrink_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(label.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(t.text_primary))
                    .child(value),
            )
            .into_any_element()
    }

    fn diagnostic_row(&self, d: &SpecDiagnostic, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        let color = match d.severity {
            SpecSeverity::Error => t.error,
            SpecSeverity::Warning => t.warning,
        };
        v_flex()
            .gap(px(1.0))
            .child(
                div()
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(color))
                    .child(d.message.clone()),
            )
            .when_some(d.fix.clone(), |el, fix| {
                el.child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child(format!("Fix: {fix}")),
                )
            })
            .into_any_element()
    }

    /// Right column with no document selected: what this root is and how it is
    /// wired — the facts `openspec doctor` and `openspec context` report.
    fn render_root_overview(&self, root: &SpecRoot, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let mut col = v_flex()
            .id("spec-root-overview")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .px(px(20.0))
            .py(px(16.0))
            .gap(px(8.0))
            .child(
                h_flex()
                    .gap(px(8.0))
                    .items_center()
                    .child(
                        div()
                            .text_size(ui_text(15.0, cx))
                            .text_color(rgb(t.text_primary))
                            .child(root.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child(if root.is_default {
                                format!("{} · machine default", kind_label(root.kind))
                            } else {
                                kind_label(root.kind).to_string()
                            }),
                    ),
            )
            .child(self.fact_row("Path", root.path.clone(), cx));
        if let Some(remote) = root.remote.clone() {
            col = col.child(self.fact_row("Remote", remote, cx));
        }
        if let Some(schema) = root.schema.clone() {
            col = col.child(self.fact_row("Schema", schema, cx));
        }
        if !root.used_by.is_empty() {
            col = col.child(self.fact_row("Used by", root.used_by.join(", "), cx));
        }
        col = col.child(self.fact_row("CLI", cli_hint(root), cx));

        if !root.references.is_empty() {
            col = col.child(
                div()
                    .pt(px(8.0))
                    .child(self.section_label("References", cx)),
            );
            for r in &root.references {
                col = col.child(match &r.root {
                    Some(path) if r.status.is_empty() => {
                        self.fact_row(&r.id, format!("✓ {path}"), cx)
                    }
                    _ => v_flex()
                        .children(r.status.iter().map(|d| self.diagnostic_row(d, cx)))
                        .into_any_element(),
                });
            }
        }
        if !root.status.is_empty() {
            col = col.child(div().pt(px(8.0)).child(self.section_label("Problems", cx)));
            for d in &root.status {
                col = col.child(self.diagnostic_row(d, cx));
            }
        }
        col.child(
            div()
                .pt(px(12.0))
                .text_size(ui_text_md(cx))
                .text_color(rgb(t.text_muted))
                .child("Select a document to read it."),
        )
        .into_any_element()
    }

    /// Right column: the selected document, or the root's overview.
    fn render_spec_document(&self, root: Option<&SpecRoot>, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let Some(path) = self.specs.selected.clone() else {
            return match root {
                Some(root) => self.render_root_overview(root, cx),
                None => v_flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_size(ui_text_md(cx))
                            .text_color(rgb(t.text_muted))
                            .child("Select a root."),
                    )
                    .into_any_element(),
            };
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

        let header = match root {
            Some(root) => format!("{} · {path}", root.name),
            None => path,
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
                    .child(header),
            )
            .child(body)
            .into_any_element()
    }

    /// Toolbar actions for the Specs view.
    fn spec_actions(&self, stores: &SpecStores, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme(cx);
        let mut actions = vec![
            self.toolbar_icon(
                "specs-settings",
                "icons/settings.svg",
                "Spec settings",
                cx.listener(|this, _, _window, cx| this.open_settings("specs", cx)),
                cx,
            ),
            self.small_button(
                "spec-refresh",
                if self.specs.loading {
                    "Refreshing…"
                } else {
                    "Refresh"
                },
                cx.listener(|this, _, _window, cx| this.refresh_specs(cx)),
                cx,
            ),
        ];
        // Only offered where a change could go.
        if stores.roots.iter().any(|r| r.healthy) {
            actions.push(
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
            );
        }
        actions
    }

    /// A label over a form field.
    pub(super) fn field_label(&self, label: &str, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_secondary))
            .child(label.to_string())
            .into_any_element()
    }

    /// Explanatory line under a form field.
    pub(super) fn field_hint(&self, hint: &str, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(hint.to_string())
            .into_any_element()
    }

    pub(super) fn choice_chip(
        &self,
        id: String,
        label: String,
        selected: bool,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        div()
            .id(SharedString::from(id))
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
                    on_click(this, cx);
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// The full-view new-change form: where, name, prompt, agent.
    fn render_new_change_form(&self, stores: &SpecStores, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);

        let target = self.draft_target();
        let mut roots = h_flex().gap(px(6.0)).flex_wrap();
        for root in stores.roots.iter().filter(|r| r.healthy) {
            let key = root.key.clone();
            roots = roots.child(self.choice_chip(
                format!("spec-target-{}", root.key),
                format!("{} · {}", root.name, kind_label(root.kind)),
                target.as_deref() == Some(root.key.as_str()),
                move |this, _cx| this.specs.draft_root = Some(key.clone()),
                cx,
            ));
        }
        let target_hint = match target.as_deref().and_then(|k| stores.root(k)) {
            Some(root) => match (root.kind, root.store_id.as_deref()) {
                (SpecRootKind::Store, Some(id)) => format!(
                    "Drafted in store '{id}' at {}. The agent is told to pass --store {id} to the openspec CLI.",
                    root.path
                ),
                _ => format!("Drafted under openspec/changes/ in {}.", root.path),
            },
            None => "Pick where the change should live.".to_string(),
        };

        let mut agents = h_flex().gap(px(6.0)).flex_wrap();
        // "No agent" first: scaffolding a change without launching anything is
        // a legitimate choice, not a fallback.
        for choice in std::iter::once(None).chain(AGENT_CHOICES.iter().map(|a| Some(*a))) {
            let value = choice.map(str::to_string);
            agents = agents.child(self.choice_chip(
                format!("spec-agent-{}", choice.unwrap_or("none")),
                choice.unwrap_or("No agent").to_string(),
                self.specs.agent.as_deref() == choice,
                move |this, _cx| {
                    this.specs.agent = value.clone();
                    this.specs.agent_picked = true;
                },
                cx,
            ));
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
                        "okena scaffolds the change under openspec/changes/ — the \
                         .openspec.yaml openspec new change writes, plus a stub \
                         proposal — and starts an agent briefed on the OpenSpec \
                         conventions.",
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
                        this.specs.draft_root = None;
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

    /// Nothing discovered: say where okena looked and where to fix it.
    fn render_no_roots(&self, stores: &SpecStores, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        v_flex()
            .p(px(20.0))
            .gap(px(8.0))
            .max_w(px(720.0))
            .child(
                div()
                    .text_size(ui_text(15.0, cx))
                    .text_color(rgb(t.text_primary))
                    .child("No OpenSpec roots yet"),
            )
            .child(self.field_hint(
                "okena shows the stores registered on this machine (openspec store \
                 list), projects that hold an openspec/ tree or point at a store with \
                 `store:`, and folders you add. Register or create a store, or add a \
                 folder, in Settings → Specs.",
                cx,
            ))
            .children(stores.status.iter().map(|d| self.diagnostic_row(d, cx)))
            .child(self.field_hint(&format!("Store registry: {}", stores.registry_path), cx))
            .child(h_flex().pt(px(6.0)).child(self.small_button(
                "spec-open-settings",
                "Open spec settings",
                cx.listener(|this, _, _window, cx| this.open_settings("specs", cx)),
                cx,
            )))
            .into_any_element()
    }

    pub(super) fn render_specs_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let root = v_flex().size_full();

        let Some(stores) = self.specs.stores.clone() else {
            // An unreachable daemon is the whole story — an empty list next to
            // it would imply there is nothing to show.
            if let Some(err) = self.specs.error.clone() {
                return root
                    .child(self.error_banner(err, cx))
                    .child(div().px(px(12.0)).child(self.small_button(
                        "spec-retry",
                        "Retry",
                        cx.listener(|this, _, _window, cx| this.refresh_specs(cx)),
                        cx,
                    )))
                    .into_any_element();
            }
            return root
                .child(self.info_banner("Loading specs…".into(), cx))
                .into_any_element();
        };

        // The new-change form takes the whole view: configuring a session is a
        // separate task from reading specs, and splitting the space between
        // them served neither.
        if self.specs.composing {
            return root
                .child(self.render_new_change_form(&stores, cx))
                .into_any_element();
        }

        let actions = self.spec_actions(&stores, cx);
        let mut root = root.child(self.render_toolbar(actions, cx));
        if let Some(err) = self.specs.error.clone() {
            root = root.child(self.error_banner(err, cx));
        }
        if stores.roots.is_empty() {
            return root
                .child(self.render_no_roots(&stores, cx))
                .into_any_element();
        }

        let open_root = self
            .specs
            .root_key
            .as_deref()
            .and_then(|k| stores.root(k))
            .cloned();
        root.child(
            h_flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .bg(rgb(t.bg_primary))
                .child(self.render_spec_tree(&stores, cx))
                .child(self.render_spec_document(open_root.as_ref(), cx)),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::{AGENT_CHOICES, cli_hint};
    use okena_core::specs::{SpecChange, SpecDoc, SpecRoot, SpecRootKind, SpecTree};

    fn doc(path: &str, name: &str) -> SpecDoc {
        SpecDoc {
            path: path.into(),
            name: name.into(),
        }
    }

    fn tree() -> SpecTree {
        SpecTree {
            root_key: "store:team-plans".into(),
            root: "/repo".into(),
            store_id: Some("team-plans".into()),
            initialized: true,
            specs: vec![doc("openspec/specs/auth/spec.md", "auth")],
            changes: vec![SpecChange {
                name: "add-login".into(),
                path: "openspec/changes/add-login".into(),
                artifacts: vec![doc("openspec/changes/add-login/proposal.md", "proposal.md")],
                specs: vec![doc("openspec/changes/add-login/specs/auth/spec.md", "auth")],
                archived: false,
                schema: Some("spec-driven".into()),
                created: Some("2026-09-10".into()),
            }],
            archived: Vec::new(),
        }
    }

    #[test]
    fn a_listed_document_is_found_anywhere_in_the_tree() {
        let t = tree();
        for path in [
            "openspec/specs/auth/spec.md",
            "openspec/changes/add-login/proposal.md",
            "openspec/changes/add-login/specs/auth/spec.md",
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

    fn root(kind: SpecRootKind, store_id: Option<&str>) -> SpecRoot {
        SpecRoot {
            key: "k".into(),
            kind,
            name: "n".into(),
            path: "/work/app".into(),
            store_id: store_id.map(str::to_string),
            remote: None,
            schema: None,
            healthy: true,
            is_default: false,
            references: Vec::new(),
            used_by: Vec::new(),
            status: Vec::new(),
        }
    }

    #[test]
    fn the_cli_hint_selects_a_store_by_id_and_anything_else_by_directory() {
        assert_eq!(
            cli_hint(&root(SpecRootKind::Store, Some("team-plans"))),
            "openspec list --store team-plans"
        );
        // A folder holding store metadata is not registered, so --store would fail.
        assert_eq!(
            cli_hint(&root(SpecRootKind::Folder, Some("team-plans"))),
            "cd /work/app && openspec list"
        );
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
