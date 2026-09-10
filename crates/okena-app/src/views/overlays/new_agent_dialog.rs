//! Configure and start a free-form agent session.
//!
//! The third way into an agent session, alongside starting work on a task and
//! drafting a spec. Those two derive everything — goal, working directory,
//! context — from the thing they are about; this one asks, because there is
//! nothing to derive it from.
//!
//! Everything the daemon needs is decided here rather than inferred later, so
//! what the user sees is exactly what gets started.

use crate::keybindings::Cancel;
use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms};
use crate::views::components::{SimpleInput, SimpleInputState};
use crate::workspace::focus::FocusManager;
use crate::workspace::state::{WindowId, Workspace};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_ui::modal::{modal_backdrop, modal_content};

/// Agents offered here. The same list every other launch path uses, so anything
/// okena can start is also something it recognizes afterwards.
const AGENT_CHOICES: &[&str] = &["claude", "copilot"];

pub enum NewAgentDialogEvent {
    Close,
}

pub struct NewAgentDialog {
    client: okena_transport::remote_action::RemoteActionClient,
    workspace: Entity<Workspace>,
    focus_manager: Entity<FocusManager>,
    window_id: WindowId,
    focus_handle: FocusHandle,
    goal_input: Entity<SimpleInputState>,
    name_input: Entity<SimpleInputState>,
    root_input: Entity<SimpleInputState>,
    /// Projects the agent is pointed at. Order is the user's click order, which
    /// is also the order they appear in the brief.
    selected: Vec<String>,
    /// `None` opens the session on a plain shell.
    agent: Option<String>,
    starting: bool,
    error: Option<String>,
}

impl NewAgentDialog {
    pub fn new(
        client: okena_transport::remote_action::RemoteActionClient,
        workspace: Entity<Workspace>,
        focus_manager: Entity<FocusManager>,
        window_id: WindowId,
        default_agent: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let goal_input = cx.new(|cx| {
            SimpleInputState::new(cx).placeholder(
                "e.g. audit every crate for unwraps on user input and open a PR per crate",
            )
        });
        let name_input =
            cx.new(|cx| SimpleInputState::new(cx).placeholder("Optional — derived from the goal"));
        let root_input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Optional — the selected project, or your projects root")
        });
        Self {
            client,
            workspace,
            focus_manager,
            window_id,
            focus_handle: cx.focus_handle(),
            goal_input,
            name_input,
            root_input,
            selected: Vec::new(),
            agent: default_agent,
            starting: false,
            error: None,
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(NewAgentDialogEvent::Close);
    }

    /// Projects offerable as context: real repos, not sessions or worktrees.
    ///
    /// A worktree belongs to work already in flight and a session is an agent,
    /// so neither is something to point a new agent at.
    fn candidates(&self, cx: &App) -> Vec<(String, String)> {
        self.workspace
            .read(cx)
            .projects()
            .iter()
            .filter(|p| p.worktree_info.is_none() && !p.is_any_agent_session())
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect()
    }

    fn toggle_project(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some(pos) = self.selected.iter().position(|s| *s == id) {
            self.selected.remove(pos);
        } else {
            self.selected.push(id);
        }
        cx.notify();
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if self.starting {
            return;
        }
        let goal = self.goal_input.read(cx).value().trim().to_string();
        if goal.is_empty() {
            self.error = Some("Describe what the agent should do first.".into());
            cx.notify();
            return;
        }
        let name = self.name_input.read(cx).value().trim().to_string();
        let root = self.root_input.read(cx).value().trim().to_string();
        // An explicit empty string is the daemon's "no agent, just a shell".
        let agent_command = Some(self.agent.clone().unwrap_or_default());
        let project_ids = self.selected.clone();

        self.starting = true;
        self.error = None;
        cx.notify();

        let client = self.client.clone();
        let workspace = self.workspace.clone();
        let focus_manager = self.focus_manager.clone();
        let window_id = self.window_id;
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::AgentStartSession {
                        goal,
                        name,
                        root,
                        project_ids,
                        agent_command,
                    })
                    .and_then(|v| v.ok_or_else(|| "Missing session result".to_string()))
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.starting = false;
                    match result {
                        Ok(value) => {
                            // Focus the new session, so starting one lands you
                            // in it rather than back where you were.
                            if let Some(id) = value.get("project_id").and_then(|v| v.as_str()) {
                                let id = id.to_string();
                                okena_workspace::harness_state::set_active_harness(
                                    window_id, None, cx,
                                );
                                focus_manager.update(cx, |fm, cx| {
                                    workspace.update(cx, |ws, cx| {
                                        ws.set_focused_project_individual(fm, Some(id), cx);
                                    });
                                    cx.notify();
                                });
                            }
                            this.close(cx);
                        }
                        Err(e) => this.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn field_label(&self, label: &str, cx: &App) -> AnyElement {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_secondary))
            .child(label.to_string())
            .into_any_element()
    }

    fn field_hint(&self, hint: &str, cx: &App) -> AnyElement {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(hint.to_string())
            .into_any_element()
    }
}

impl EventEmitter<NewAgentDialogEvent> for NewAgentDialog {}

impl Focusable for NewAgentDialog {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for NewAgentDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        if !focus_handle.contains_focused(window, cx) {
            window.focus(&focus_handle, cx);
        }

        let mut agents = h_flex().gap(px(6.0)).flex_wrap();
        // "No agent" first: opening a session on a plain shell in a chosen
        // directory is a legitimate thing to want, not a fallback.
        for choice in std::iter::once(None).chain(AGENT_CHOICES.iter().map(|a| Some(*a))) {
            let selected = self.agent.as_deref() == choice;
            let label = choice.unwrap_or("No agent").to_string();
            let value = choice.map(str::to_string);
            agents = agents.child(
                div()
                    .id(SharedString::from(format!(
                        "new-agent-kind-{}",
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
                            this.agent = value.clone();
                            cx.notify();
                        }),
                    ),
            );
        }

        let mut projects = h_flex().gap(px(6.0)).flex_wrap();
        for (id, name) in self.candidates(cx) {
            let selected = self.selected.contains(&id);
            projects = projects.child(
                div()
                    .id(SharedString::from(format!("new-agent-project-{id}")))
                    .cursor_pointer()
                    .px(px(10.0))
                    .py(px(4.0))
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
                    .text_size(ui_text_ms(cx))
                    .child(name)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            this.toggle_project(id.clone(), cx);
                        }),
                    ),
            );
        }

        let starting = self.starting;

        modal_backdrop("new-agent-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("NewAgentDialog")
            .items_center()
            .on_action(cx.listener(|this, _: &Cancel, _, cx| this.close(cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.close(cx)),
            )
            .child(
                modal_content("new-agent-modal", &t)
                    .w(px(640.0))
                    .max_h(px(700.0))
                    .flex()
                    .flex_col()
                    .child(
                        v_flex()
                            .id("new-agent-body")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .gap(px(16.0))
                            .p(px(20.0))
                            .child(
                                v_flex()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .text_size(ui_text(15.0, cx))
                                            .text_color(rgb(t.text_primary))
                                            .child("New agent"),
                                    )
                                    .child(self.field_hint(
                                        "Starts an agent session with okena's MCP server \
                                         wired in, so it can report status and register \
                                         what it produces.",
                                        cx,
                                    )),
                            )
                            .child(
                                v_flex()
                                    .gap(px(5.0))
                                    .child(self.field_label("Goal", cx))
                                    .child(
                                        okena_ui::input::input_container(&t, None)
                                            .w_full()
                                            .h(px(120.0))
                                            .px(px(8.0))
                                            .py(px(6.0))
                                            .child(
                                                SimpleInput::new(&self.goal_input)
                                                    .text_size(ui_text(13.0, cx)),
                                            ),
                                    )
                                    .child(self.field_hint(
                                        "What the agent is asked to do. This is its opening \
                                         prompt, so context beats brevity.",
                                        cx,
                                    )),
                            )
                            .child(
                                v_flex()
                                    .gap(px(6.0))
                                    .child(self.field_label("Projects", cx))
                                    .child(projects)
                                    .child(self.field_hint(
                                        "Named in the brief with their paths. One project \
                                         runs the agent inside it; several run it above them.",
                                        cx,
                                    )),
                            )
                            .child(
                                v_flex()
                                    .gap(px(5.0))
                                    .child(self.field_label("Working directory", cx))
                                    .child(
                                        okena_ui::input::input_container(&t, None)
                                            .w_full()
                                            .px(px(8.0))
                                            .py(px(6.0))
                                            .child(
                                                SimpleInput::new(&self.root_input)
                                                    .text_size(ui_text(13.0, cx)),
                                            ),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap(px(5.0))
                                    .child(self.field_label("Session name", cx))
                                    .child(
                                        okena_ui::input::input_container(&t, None)
                                            .w_full()
                                            .px(px(8.0))
                                            .py(px(6.0))
                                            .child(
                                                SimpleInput::new(&self.name_input)
                                                    .text_size(ui_text(13.0, cx)),
                                            ),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap(px(6.0))
                                    .child(self.field_label("Agent", cx))
                                    .child(agents),
                            )
                            .children(self.error.clone().map(|e| {
                                div()
                                    .px(px(10.0))
                                    .py(px(6.0))
                                    .rounded(px(4.0))
                                    .bg(with_alpha(t.error, 0.1))
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(t.error))
                                    .child(e)
                                    .into_any_element()
                            })),
                    )
                    .child(
                        h_flex()
                            .gap(px(8.0))
                            .justify_end()
                            .px(px(20.0))
                            .py(px(14.0))
                            .border_t_1()
                            .border_color(rgb(t.border))
                            .child(
                                div()
                                    .id("new-agent-cancel")
                                    .cursor_pointer()
                                    .px(px(14.0))
                                    .py(px(6.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(t.bg_secondary))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .text_size(ui_text_md(cx))
                                    .text_color(rgb(t.text_primary))
                                    .child("Cancel")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _window, cx| {
                                            cx.stop_propagation();
                                            this.close(cx);
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .id("new-agent-start")
                                    .cursor_pointer()
                                    .px(px(16.0))
                                    .py(px(6.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(t.button_primary_bg))
                                    .hover(|s| s.bg(rgb(t.button_primary_hover)))
                                    .text_size(ui_text_md(cx))
                                    .text_color(rgb(t.button_primary_fg))
                                    .child(if starting {
                                        "Starting…"
                                    } else {
                                        "Start agent"
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _window, cx| {
                                            cx.stop_propagation();
                                            this.start(cx);
                                        }),
                                    ),
                            ),
                    )
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
            )
    }
}
