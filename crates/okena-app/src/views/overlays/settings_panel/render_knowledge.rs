//! Knowledge settings — the stores on this machine, adding one, and where
//! okena looks.
//!
//! Stores are listed in okena's own registry (`knowledge/stores.yaml` in the
//! profile), not in these settings: cloning, adding, creating and forgetting a
//! store go through the daemon, which owns that file (ADR-0003). The project
//! discovery switch and the clone folder are ordinary settings.

use super::SettingsPanel;
use super::components::{hook_input_row, section_container, section_header, settings_input_row};
use super::render_specs::{
    badge, banner, diagnostic, labeled_input, muted, muted_row, path_line, text_input,
};
use crate::settings::settings_entity;
use crate::theme::{ThemeColors, theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_ms};
use crate::views::components::simple_input::{InputChangedEvent, SimpleInputState};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::knowledge::{
    KnowledgeGitStatus, KnowledgePointer, KnowledgeRoot, KnowledgeRootKind, KnowledgeStores,
    Severity,
};

/// Which "add a store" form is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AddMode {
    Clone,
    Register,
    Create,
}

/// State of the Knowledge page.
pub(super) struct KnowledgePage {
    stores: Option<KnowledgeStores>,
    loading: bool,
    /// The discovery settings the current snapshot was read with; a change
    /// triggers a re-read.
    fetched_for: Option<String>,
    refresh_pending: bool,
    busy: bool,
    error: Option<String>,
    notice: Option<String>,
    mode: AddMode,
    init_git: bool,
    clone_url_input: Entity<SimpleInputState>,
    clone_path_input: Entity<SimpleInputState>,
    register_path_input: Entity<SimpleInputState>,
    setup_id_input: Entity<SimpleInputState>,
    setup_name_input: Entity<SimpleInputState>,
    setup_path_input: Entity<SimpleInputState>,
    setup_remote_input: Entity<SimpleInputState>,
    clone_dir_input: Entity<SimpleInputState>,
}

impl KnowledgePage {
    pub(super) fn new(clone_dir: Option<String>, cx: &mut Context<SettingsPanel>) -> Self {
        let clone_dir_input = text_input(cx, "~/knowledge", clone_dir);
        cx.subscribe(
            &clone_dir_input,
            |_this, entity, _: &InputChangedEvent, cx| {
                let val = entity.read(cx).value().to_string();
                settings_entity(cx).update(cx, |state, cx| state.set_knowledge_clone_dir(val, cx));
            },
        )
        .detach();

        Self {
            stores: None,
            loading: false,
            fetched_for: None,
            refresh_pending: false,
            busy: false,
            error: None,
            notice: None,
            mode: AddMode::Clone,
            init_git: true,
            clone_url_input: text_input(cx, "e.g. git@github.com:acme/eng-knowledge.git", None),
            clone_path_input: text_input(cx, "Leave blank to use the clone folder below", None),
            register_path_input: text_input(cx, "e.g. ~/knowledge/eng-knowledge", None),
            setup_id_input: text_input(cx, "e.g. acme-eng", None),
            setup_name_input: text_input(cx, "e.g. Acme Engineering", None),
            setup_path_input: text_input(cx, "e.g. ~/knowledge/acme-eng", None),
            setup_remote_input: text_input(cx, "e.g. git@github.com:acme/eng-knowledge.git", None),
            clone_dir_input,
        }
    }
}

fn health(root: &KnowledgeRoot, t: &ThemeColors) -> (&'static str, u32) {
    if !root.healthy {
        ("problem", t.error)
    } else if root.status.iter().any(|d| d.severity == Severity::Warning) {
        ("check", t.warning)
    } else {
        ("ok", t.success)
    }
}

/// `main → origin/main · 3 to pull · uncommitted changes`
fn git_line(git: &KnowledgeGitStatus) -> String {
    let mut parts = vec![match (&git.branch, &git.upstream) {
        (Some(branch), Some(upstream)) => format!("{branch} → {upstream}"),
        (Some(branch), None) => format!("{branch}, no upstream"),
        (None, _) => "detached HEAD".to_string(),
    }];
    if git.behind > 0 {
        parts.push(format!("{} to pull", git.behind));
    }
    if git.ahead > 0 {
        parts.push(format!("{} to push", git.ahead));
    }
    if git.dirty {
        parts.push("uncommitted changes".to_string());
    }
    parts.join(" · ")
}

impl SettingsPanel {
    fn knowledge_fingerprint(cx: &App) -> String {
        format!(
            "{:?}",
            settings_entity(cx).read(cx).settings.harness.knowledge
        )
    }

    /// Read what the daemon discovers, with each store's local sync state.
    fn refresh_knowledge_stores(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.action_client.clone() else {
            return;
        };
        self.knowledge.fetched_for = Some(Self::knowledge_fingerprint(cx));
        self.knowledge.loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::KnowledgeStores)
                    .and_then(|v| v.ok_or_else(|| "Missing knowledge stores".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<KnowledgeStores>(v)
                            .map_err(|e| format!("Unexpected knowledge stores: {e}"))
                    })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.knowledge.loading = false;
                    match result {
                        Ok(stores) => this.knowledge.stores = Some(stores),
                        Err(e) => this.knowledge.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Re-read after a settings change, once it has reached the daemon —
    /// settings travel as a separate message.
    fn refresh_knowledge_stores_soon(&mut self, cx: &mut Context<Self>) {
        self.knowledge.refresh_pending = true;
        cx.spawn(async move |this, cx| {
            smol::Timer::after(std::time::Duration::from_millis(500)).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.knowledge.refresh_pending = false;
                    this.refresh_knowledge_stores(cx);
                });
            });
        })
        .detach();
    }

    /// Run a store change on the daemon, then say what happened and re-read.
    fn run_knowledge_action(
        &mut self,
        action: ActionRequest,
        describe: fn(&serde_json::Value) -> String,
        cx: &mut Context<Self>,
    ) {
        if self.knowledge.busy {
            return;
        }
        let Some(client) = self.action_client.clone() else {
            self.knowledge.error = Some("The local daemon is unavailable.".into());
            cx.notify();
            return;
        };
        self.knowledge.busy = true;
        self.knowledge.error = None;
        self.knowledge.notice = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || client.post_action(action)).await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.knowledge.busy = false;
                    match result {
                        Ok(v) => {
                            this.knowledge.notice =
                                Some(describe(&v.unwrap_or(serde_json::Value::Null)));
                            for input in [
                                &this.knowledge.clone_url_input,
                                &this.knowledge.clone_path_input,
                                &this.knowledge.register_path_input,
                                &this.knowledge.setup_id_input,
                                &this.knowledge.setup_name_input,
                                &this.knowledge.setup_path_input,
                                &this.knowledge.setup_remote_input,
                            ] {
                                input.update(cx, |i, cx| i.set_value("", cx));
                            }
                            this.refresh_knowledge_stores(cx);
                        }
                        Err(e) => this.knowledge.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn knowledge_input(&self, input: &Entity<SimpleInputState>, cx: &App) -> String {
        input.read(cx).value().trim().to_string()
    }

    fn clone_knowledge_store(&mut self, cx: &mut Context<Self>) {
        let url = self.knowledge_input(&self.knowledge.clone_url_input, cx);
        let path = self.knowledge_input(&self.knowledge.clone_path_input, cx);
        if url.is_empty() {
            self.knowledge.error = Some("Enter the repository URL to clone.".into());
            cx.notify();
            return;
        }
        self.run_knowledge_action(
            ActionRequest::KnowledgeStoreClone {
                url,
                path: (!path.is_empty()).then_some(path),
            },
            |v| {
                let id = v["id"].as_str().unwrap_or("store");
                let root = v["root"].as_str().unwrap_or("");
                if v["identity_missing"].as_bool() == Some(true) {
                    format!(
                        "Cloned '{id}' into {root}. It has no .okena-knowledge/store.yaml yet — commit one so every clone agrees on its id."
                    )
                } else {
                    format!("Cloned '{id}' into {root}.")
                }
            },
            cx,
        );
    }

    fn register_knowledge_store(&mut self, cx: &mut Context<Self>) {
        let path = self.knowledge_input(&self.knowledge.register_path_input, cx);
        if path.is_empty() {
            self.knowledge.error = Some("Choose the store checkout's folder.".into());
            cx.notify();
            return;
        }
        self.run_knowledge_action(
            ActionRequest::KnowledgeStoreRegister { path },
            |v| {
                let id = v["id"].as_str().unwrap_or("store");
                if v["already_registered"].as_bool() == Some(true) {
                    format!("'{id}' was already added from that folder.")
                } else if v["identity_missing"].as_bool() == Some(true) {
                    format!(
                        "Added '{id}', named after its folder. Commit a .okena-knowledge/store.yaml so every clone agrees on the id."
                    )
                } else {
                    format!("Added store '{id}'.")
                }
            },
            cx,
        );
    }

    fn create_knowledge_store(&mut self, cx: &mut Context<Self>) {
        let id = self.knowledge_input(&self.knowledge.setup_id_input, cx);
        let name = self.knowledge_input(&self.knowledge.setup_name_input, cx);
        let path = self.knowledge_input(&self.knowledge.setup_path_input, cx);
        let remote = self.knowledge_input(&self.knowledge.setup_remote_input, cx);
        if id.is_empty() || path.is_empty() {
            self.knowledge.error = Some("A new store needs an id and a folder.".into());
            cx.notify();
            return;
        }
        self.run_knowledge_action(
            ActionRequest::KnowledgeStoreSetup {
                id,
                path,
                name: (!name.is_empty()).then_some(name),
                description: None,
                remote: (!remote.is_empty()).then_some(remote),
                init_git: self.knowledge.init_git,
            },
            |v| {
                let committed = if v["committed"].as_bool() == Some(true) {
                    " with an initial commit"
                } else {
                    ""
                };
                format!(
                    "Created store '{}' at {}{committed}. Push it where your team can clone it.",
                    v["id"].as_str().unwrap_or("store"),
                    v["root"].as_str().unwrap_or("")
                )
            },
            cx,
        );
    }

    fn knowledge_button(
        &self,
        id: String,
        label: &str,
        primary: bool,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        div()
            .id(SharedString::from(id))
            .cursor_pointer()
            .flex_shrink_0()
            .px(px(10.0))
            .py(px(3.0))
            .rounded(px(4.0))
            .when(primary, |d| {
                d.bg(rgb(t.button_primary_bg))
                    .hover(|s| s.bg(rgb(t.button_primary_hover)))
                    .text_color(rgb(t.button_primary_fg))
            })
            .when(!primary, |d| {
                d.border_1()
                    .border_color(rgb(t.border))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .text_color(rgb(t.text_secondary))
            })
            .when(self.knowledge.busy, |d| d.opacity(0.6))
            .text_size(ui_text_ms(cx))
            .child(label.to_string())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| on_click(this, cx)),
            )
            .into_any_element()
    }

    fn render_knowledge_root_row(
        &self,
        root: &KnowledgeRoot,
        border_top: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let (label, color) = health(root, &t);

        let mut actions = h_flex().gap(px(6.0)).flex_shrink_0();
        if root.kind == KnowledgeRootKind::Store
            && let Some(id) = root.store_id.clone()
        {
            let target = id.clone();
            actions = actions.child(self.knowledge_button(
                format!("knowledge-unregister-{id}"),
                "Remove",
                false,
                move |this, cx| {
                    this.run_knowledge_action(
                        ActionRequest::KnowledgeStoreUnregister { id: target.clone() },
                        |v| {
                            format!(
                                "Removed '{}' from okena. Its checkout is still at {}.",
                                v["id"].as_str().unwrap_or(""),
                                v["left_on_disk"].as_str().unwrap_or("")
                            )
                        },
                        cx,
                    )
                },
                cx,
            ));
        }

        let c = &root.counts;
        let mut body = v_flex()
            .gap(px(3.0))
            .flex_1()
            .min_w_0()
            .child(
                h_flex()
                    .gap(px(6.0))
                    .items_center()
                    .flex_wrap()
                    .child(
                        div()
                            .text_size(ui_text(13.0, cx))
                            .text_color(rgb(t.text_primary))
                            .child(root.name.clone()),
                    )
                    .when_some(
                        root.store_id.clone().filter(|id| *id != root.name),
                        |d, id| d.child(badge(&id, t.border_active, cx)),
                    )
                    .child(badge(label, color, cx)),
            )
            .child(path_line(root.path.clone(), &t, cx))
            .when_some(root.description.clone(), |d, description| {
                d.child(muted(description, &t, cx))
            })
            .when_some(root.remote.clone(), |d, remote| {
                d.child(muted(format!("Remote: {remote}"), &t, cx))
            })
            .when_some(root.git.as_ref().map(git_line), |d, line| {
                d.child(muted(line, &t, cx))
            })
            .when(root.healthy, |d| {
                d.child(muted(
                    format!(
                        "{} docs · {} skills · {} agents · {} templates",
                        c.docs, c.skills, c.agents, c.templates
                    ),
                    &t,
                    cx,
                ))
            })
            .when(!root.used_by.is_empty(), |d| {
                d.child(muted(
                    format!("Used by: {}", root.used_by.join(", ")),
                    &t,
                    cx,
                ))
            });
        for d in &root.status {
            body = body.child(diagnostic(d, &t, cx));
        }

        h_flex()
            .items_start()
            .gap(px(12.0))
            .px(px(12.0))
            .py(px(10.0))
            .when(border_top, |d| d.border_t_1().border_color(rgb(t.border)))
            .child(body)
            .child(actions)
            .into_any_element()
    }

    fn render_knowledge_pointer_row(
        &self,
        pointer: &KnowledgePointer,
        border_top: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let resolved = pointer.root_key.is_some();
        v_flex()
            .gap(px(3.0))
            .px(px(12.0))
            .py(px(10.0))
            .when(border_top, |d| d.border_t_1().border_color(rgb(t.border)))
            .child(
                h_flex()
                    .gap(px(6.0))
                    .items_center()
                    .child(
                        div()
                            .text_size(ui_text(13.0, cx))
                            .text_color(rgb(t.text_primary))
                            .child(format!("{} follows {}", pointer.project, pointer.store_id)),
                    )
                    .when(resolved, |d| {
                        d.child(badge("on this machine", t.success, cx))
                    }),
            )
            .child(path_line(pointer.path.clone(), &t, cx))
            .children(pointer.status.iter().map(|d| diagnostic(d, &t, cx)))
            .into_any_element()
    }

    fn render_knowledge_mode_chip(
        &self,
        mode: AddMode,
        label: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let selected = self.knowledge.mode == mode;
        div()
            .id(SharedString::from(format!("knowledge-mode-{label}")))
            .cursor_pointer()
            .px(px(10.0))
            .py(px(3.0))
            .rounded(px(4.0))
            .border_1()
            .border_color(rgb(if selected { t.border_active } else { t.border }))
            .when(selected, |d| d.bg(with_alpha(t.button_primary_bg, 0.15)))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(if selected {
                t.text_primary
            } else {
                t.text_secondary
            }))
            .child(label.to_string())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.knowledge.mode = mode;
                    this.knowledge.error = None;
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    fn render_add_knowledge_store(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let busy = self.knowledge.busy;
        let tabs = h_flex()
            .gap(px(6.0))
            .flex_wrap()
            .child(self.render_knowledge_mode_chip(AddMode::Clone, "Clone a repository", cx))
            .child(self.render_knowledge_mode_chip(AddMode::Register, "Add an existing folder", cx))
            .child(self.render_knowledge_mode_chip(AddMode::Create, "Create a new store", cx));

        let form = match self.knowledge.mode {
            AddMode::Clone => v_flex()
                .gap(px(10.0))
                .child(labeled_input(
                    "Repository URL",
                    "Your team's knowledge repository. Git runs without prompts, so credentials must come from an SSH agent or a credential helper.",
                    &self.knowledge.clone_url_input,
                    &t,
                    cx,
                ))
                .child(labeled_input(
                    "Destination (optional)",
                    "Where to put the checkout. Blank clones into the clone folder, named the way git clone names it.",
                    &self.knowledge.clone_path_input,
                    &t,
                    cx,
                ))
                .child(h_flex().child(self.knowledge_button(
                    "knowledge-clone-submit".into(),
                    if busy { "Cloning…" } else { "Clone" },
                    true,
                    |this, cx| this.clone_knowledge_store(cx),
                    cx,
                ))),
            AddMode::Register => v_flex()
                .gap(px(10.0))
                .child(labeled_input(
                    "Checkout folder",
                    "The top of a knowledge repository you already have: one with .okena-knowledge/store.yaml, or docs/, skills/, agents/ or templates/ folders.",
                    &self.knowledge.register_path_input,
                    &t,
                    cx,
                ))
                .child(h_flex().child(self.knowledge_button(
                    "knowledge-register-submit".into(),
                    if busy { "Adding…" } else { "Add" },
                    true,
                    |this, cx| this.register_knowledge_store(cx),
                    cx,
                ))),
            AddMode::Create => {
                let init_git = self.knowledge.init_git;
                v_flex()
                    .gap(px(10.0))
                    .child(labeled_input(
                        "Store id",
                        "Kebab-case, e.g. acme-eng. Project repos follow it with `stores: [acme-eng]` in .okena/knowledge.yaml.",
                        &self.knowledge.setup_id_input,
                        &t,
                        cx,
                    ))
                    .child(labeled_input(
                        "Name (optional)",
                        "How the store is shown; the id when blank.",
                        &self.knowledge.setup_name_input,
                        &t,
                        cx,
                    ))
                    .child(labeled_input(
                        "Folder",
                        "An empty folder outside any other git repository, e.g. ~/knowledge/acme-eng.",
                        &self.knowledge.setup_path_input,
                        &t,
                        cx,
                    ))
                    .child(labeled_input(
                        "Remote (optional)",
                        "The clone URL, recorded in the store's identity.",
                        &self.knowledge.setup_remote_input,
                        &t,
                        cx,
                    ))
                    .child(
                        h_flex().child(
                            div()
                                .id("knowledge-init-git")
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(3.0))
                                .rounded(px(4.0))
                                .border_1()
                                .border_color(rgb(if init_git { t.border_active } else { t.border }))
                                .when(init_git, |d| d.bg(with_alpha(t.button_primary_bg, 0.15)))
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.text_primary))
                                .child(if init_git {
                                    "✓ Initialize Git with an initial commit"
                                } else {
                                    "Initialize Git with an initial commit"
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _window, cx| {
                                        this.knowledge.init_git = !this.knowledge.init_git;
                                        cx.notify();
                                    }),
                                ),
                        ),
                    )
                    .child(h_flex().child(self.knowledge_button(
                        "knowledge-create-submit".into(),
                        if busy { "Creating…" } else { "Create store" },
                        true,
                        |this, cx| this.create_knowledge_store(cx),
                        cx,
                    )))
            }
        };

        section_container(&t)
            .child(
                v_flex()
                    .px(px(12.0))
                    .py(px(10.0))
                    .gap(px(12.0))
                    .child(tabs)
                    .child(form),
            )
            .into_any_element()
    }

    pub(super) fn render_knowledge(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        // Read on first paint rather than when the panel opens, so a user who
        // never visits this page makes no call; re-read when discovery changes.
        let fingerprint = Self::knowledge_fingerprint(cx);
        if self.action_client.is_some()
            && !self.knowledge.loading
            && !self.knowledge.refresh_pending
            && self.knowledge.fetched_for.as_deref() != Some(fingerprint.as_str())
        {
            if self.knowledge.fetched_for.is_none() {
                self.refresh_knowledge_stores(cx);
            } else {
                self.refresh_knowledge_stores_soon(cx);
            }
        }

        let discovery = settings_entity(cx)
            .read(cx)
            .settings
            .harness
            .knowledge
            .clone();
        let stores = self.knowledge.stores.clone();
        let roots: Vec<KnowledgeRoot> =
            stores.as_ref().map(|s| s.roots.clone()).unwrap_or_default();
        let daemon = self.action_client.is_some();

        let mut page = v_flex();

        // ── Overview ───────────────────────────────────────────────────────
        let mut facts = v_flex()
            .flex_1()
            .min_w_0()
            .gap(px(2.0))
            .px(px(12.0))
            .py(px(8.0));
        if let Some(s) = &stores {
            facts = facts.child(path_line(
                format!("Store registry: {}", s.registry_path),
                &t,
                cx,
            ));
        }
        page = page.child(section_header("Knowledge", &t, cx)).child(
            section_container(&t)
                .child(settings_input_row(
                    "knowledge-intro",
                    "Knowledge stores",
                    "A knowledge store is a git repository of your team's engineering docs, \
                     skills, agents and prompt templates, in docs/, skills/, agents/ and \
                     templates/ folders. okena lists the stores you add here, plus what your \
                     projects keep under .okena/knowledge/, in the harness Knowledge view.",
                    &t,
                    cx,
                    false,
                ))
                .child(
                    h_flex()
                        .items_end()
                        .justify_between()
                        .border_t_1()
                        .border_color(rgb(t.border))
                        .child(facts)
                        .child(div().flex_shrink_0().px(px(12.0)).py(px(8.0)).child(
                            self.knowledge_button(
                                "knowledge-refresh".into(),
                                if self.knowledge.loading {
                                    "Reading…"
                                } else {
                                    "Refresh"
                                },
                                false,
                                |this, cx| this.refresh_knowledge_stores(cx),
                                cx,
                            ),
                        )),
                ),
        );
        if let Some(notice) = self.knowledge.notice.clone() {
            page = page.child(banner(notice, t.success, cx));
        }
        if let Some(error) = self.knowledge.error.clone() {
            page = page.child(banner(error, t.error, cx));
        }
        if let Some(s) = &stores {
            for d in &s.status {
                page = page.child(div().mx(px(12.0)).mt(px(8.0)).child(diagnostic(d, &t, cx)));
            }
        }

        // ── Stores ─────────────────────────────────────────────────────────
        page = page.child(section_header("Stores", &t, cx));
        let mut container = section_container(&t);
        let store_roots: Vec<&KnowledgeRoot> = roots
            .iter()
            .filter(|r| r.kind == KnowledgeRootKind::Store)
            .collect();
        if !daemon {
            container = container.child(muted_row(
                "The local daemon is unavailable, so stores cannot be read.",
                &t,
                cx,
            ));
        } else if stores.is_none() {
            container = container.child(muted_row("Reading stores…", &t, cx));
        } else if store_roots.is_empty() {
            container = container.child(muted_row(
                "No knowledge stores on this machine yet. Clone your team's below.",
                &t,
                cx,
            ));
        }
        for (i, root) in store_roots.iter().enumerate() {
            container = container.child(self.render_knowledge_root_row(root, i > 0, cx));
        }
        page = page
            .child(container)
            .child(section_header("Add a store", &t, cx))
            .child(self.render_add_knowledge_store(cx));

        // ── Discovery ──────────────────────────────────────────────────────
        page = page.child(section_header("Discovery", &t, cx)).child(
            section_container(&t)
                .child(self.render_toggle(
                    "knowledge-discover-projects",
                    "Find knowledge in projects",
                    discovery.projects,
                    true,
                    |state, val, cx| state.set_knowledge_discovery_projects(val, cx),
                    cx,
                ))
                .child(hook_input_row(
                    "knowledge-clone-dir",
                    "Clone folder",
                    "Where a clone goes when no destination is given. Empty is ~/knowledge.",
                    &self.knowledge.clone_dir_input,
                    &t,
                    false,
                    cx,
                )),
        );

        if discovery.projects && stores.is_some() {
            let project_roots: Vec<&KnowledgeRoot> = roots
                .iter()
                .filter(|r| r.kind == KnowledgeRootKind::Project)
                .collect();
            let pointers = stores
                .as_ref()
                .map(|s| s.pointers.clone())
                .unwrap_or_default();
            let mut container = section_container(&t);
            if project_roots.is_empty() && pointers.is_empty() {
                container = container.child(muted_row(
                    "No project follows a store in .okena/knowledge.yaml or keeps its own \
                     .okena/knowledge/ folder.",
                    &t,
                    cx,
                ));
            }
            let mut first = true;
            for root in project_roots {
                container = container.child(self.render_knowledge_root_row(root, !first, cx));
                first = false;
            }
            for pointer in &pointers {
                container = container.child(self.render_knowledge_pointer_row(pointer, !first, cx));
                first = false;
            }
            page = page
                .child(section_header("Found in projects", &t, cx))
                .child(container);
        }

        page.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::git_line;
    use okena_core::knowledge::KnowledgeGitStatus;

    #[test]
    fn the_git_line_names_the_branch_and_only_what_needs_doing() {
        let tracked = KnowledgeGitStatus {
            branch: Some("main".into()),
            upstream: Some("origin/main".into()),
            ..Default::default()
        };
        assert_eq!(git_line(&tracked), "main → origin/main");
        assert_eq!(
            git_line(&KnowledgeGitStatus {
                behind: 3,
                ahead: 1,
                dirty: true,
                ..tracked.clone()
            }),
            "main → origin/main · 3 to pull · 1 to push · uncommitted changes"
        );
        assert_eq!(
            git_line(&KnowledgeGitStatus {
                upstream: None,
                ..tracked
            }),
            "main, no upstream"
        );
        assert_eq!(git_line(&KnowledgeGitStatus::default()), "detached HEAD");
    }
}
