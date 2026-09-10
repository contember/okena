//! Create a task.
//!
//! Shown in place of the board, the way drafting a spec replaces the Specs
//! view: filling this in is a different job from scanning the list, and
//! splitting the width between them served neither.
//!
//! okena models a breakdown — epic, feature, story, task, defect — that no
//! provider has natively. Linear carries it as a label, and the daemon resolves
//! or creates that label; the form only has to say which kind is meant.

use super::HarnessPane;
use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms};
use crate::views::components::SimpleInput;
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::tasks::{Task, TaskKind};

/// A team or project a task can be filed in.
#[derive(Clone, Debug, serde::Deserialize)]
pub(crate) struct TaskContainer {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub key: String,
}

/// State of the "New task" form.
///
/// Top-level only. Sub-tasks are made by an agent through okena's MCP server,
/// which is the whole point of the breakdown flow — a form for typing them one
/// at a time was the slower half of the same job.
pub(crate) struct NewTaskForm {
    pub kind: TaskKind,
    /// `None` until the teams have loaded, or when there is only one.
    pub container_id: Option<String>,
    pub containers: Vec<TaskContainer>,
    pub creating: bool,
    pub error: Option<String>,
}

impl HarnessPane {
    /// Open the form.
    pub(super) fn open_new_task(&mut self, cx: &mut Context<Self>) {
        self.tasks.new_task = Some(NewTaskForm {
            kind: TaskKind::Task,
            container_id: None,
            containers: Vec::new(),
            creating: false,
            error: None,
        });
        self.tasks
            .new_task_title
            .update(cx, |i, cx| i.set_value("", cx));
        self.tasks
            .new_task_body
            .update(cx, |i, cx| i.set_value("", cx));
        self.load_containers(cx);
        cx.notify();
    }

    pub(super) fn close_new_task(&mut self, cx: &mut Context<Self>) {
        self.tasks.new_task = None;
        cx.notify();
    }

    /// Read the teams the user can file in.
    fn load_containers(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let provider = self.tasks.provider.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::TaskContainers { provider })
                    .and_then(|v| v.ok_or_else(|| "Missing teams".to_string()))
                    .map(|v| {
                        v.get("containers")
                            .and_then(|c| {
                                serde_json::from_value::<Vec<TaskContainer>>(c.clone()).ok()
                            })
                            .unwrap_or_default()
                    })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    let Some(form) = this.tasks.new_task.as_mut() else {
                        return;
                    };
                    match result {
                        Ok(containers) => {
                            // With one team there is no choice to make, so it
                            // is chosen rather than presented.
                            if containers.len() == 1 {
                                form.container_id = Some(containers[0].id.clone());
                            }
                            form.containers = containers;
                        }
                        // Not fatal: a provider that cannot enumerate teams can
                        // still create sub-tasks, and the form says so.
                        Err(e) => form.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Create the task, then close the form and refresh the board.
    pub(super) fn submit_new_task(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.tasks.new_task.as_ref() else {
            return;
        };
        if form.creating {
            return;
        }
        let title = self
            .tasks
            .new_task_title
            .read(cx)
            .value()
            .trim()
            .to_string();
        if title.is_empty() {
            if let Some(form) = self.tasks.new_task.as_mut() {
                form.error = Some("Give the task a title first.".into());
            }
            cx.notify();
            return;
        }
        let container_id = form.container_id.clone();
        if container_id.is_none() {
            if let Some(form) = self.tasks.new_task.as_mut() {
                form.error = Some("Choose a team for the new task.".into());
            }
            cx.notify();
            return;
        }
        let kind = form.kind;
        let description = self.tasks.new_task_body.read(cx).value().trim().to_string();
        let provider = self.tasks.provider.clone();

        if let Some(form) = self.tasks.new_task.as_mut() {
            form.creating = true;
            form.error = None;
        }
        cx.notify();

        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::TaskCreate {
                        provider,
                        title,
                        description,
                        kind: kind.wire_name().to_string(),
                        parent_external_id: None,
                        container_id,
                    })
                    .and_then(|v| v.ok_or_else(|| "Missing created task".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<Task>(v)
                            .map_err(|e| format!("Unexpected task: {e}"))
                    })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(task) => {
                            // The parent's cached children no longer include
                            // everything; drop it so the detail refetches.
                            if let Some(parent) = task.parent_id.as_ref() {
                                this.tasks.children.remove(parent);
                            }
                            this.tasks.new_task = None;
                            // Select it: you almost always want to look at what
                            // you just made, and it may not be assigned to you,
                            // in which case the refresh below will not list it.
                            this.tasks.selected = Some(task.id.external_id.clone());
                            this.refresh_tasks(cx);
                        }
                        Err(e) => {
                            if let Some(form) = this.tasks.new_task.as_mut() {
                                form.creating = false;
                                form.error = Some(e);
                            }
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Start an agent whose job is to break `task` into sub-tasks.
    ///
    /// A free-form session rather than a task session: it is not doing the
    /// work, it is deciding what the work is, so it gets no worktrees. okena's
    /// MCP server is what makes it useful — the agent reads the existing
    /// children and writes new ones back through `okena_create_subtask`,
    /// rather than handing the user a list to retype.
    pub(super) fn break_down_with_agent(&mut self, task: &Task, cx: &mut Context<Self>) {
        let goal = format!(
            "Break {} down into sub-tasks.\n\n\
             Title: {}\n\
             Kind: {}\n\
             Link: {}\n\n\
             {}\n\n\
             Work through okena's MCP tools:\n\
             - `okena_list_subtasks` first, so you do not duplicate a child \
             that already exists.\n\
             - `okena_create_subtask` once per child, with `parent` set to \
             `{}`.\n\n\
             Prefer several small children over one large one. Give each a \
             short imperative title and say in its description what \"done\" \
             means. A child is normally one step narrower than its parent — \
             a {} under a {}. Ask me before inventing scope that is not \
             implied by the parent.",
            task.display_key,
            task.title,
            task.kind.label(),
            task.url,
            task.description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(|d| format!("Description:\n{d}"))
                .unwrap_or_else(|| "It has no description.".to_string()),
            task.id.external_id,
            narrower_than(task.kind).label(),
            task.kind.label(),
        );

        if self.tasks.breaking_down.is_some() {
            return;
        }
        self.tasks.breaking_down = Some(task.id.external_id.clone());
        self.tasks.error = None;
        cx.notify();

        let client = self.client.clone();
        let name = format!("{} breakdown", task.display_key);
        // The session is linked to the task so the agent's `okena_whoami`
        // resolves it, and so the MCP tools default to the right parent.
        let agent = self.tasks.default_agent.clone().unwrap_or_default();
        let link = okena_core::tasks::TaskRef::from(task);
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::AgentStartSession {
                        goal,
                        name,
                        root: String::new(),
                        project_ids: Vec::new(),
                        agent_command: Some(agent),
                        task: Some(link),
                    })
                    .and_then(|v| v.ok_or_else(|| "Missing session result".to_string()))
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.tasks.breaking_down = None;
                    // Deliberately not opening it: you asked for a breakdown
                    // while reading the task, and yanking you into a terminal
                    // loses the place you were reading. The button becomes the
                    // way in once the session appears.
                    if let Err(e) = result {
                        this.tasks.error = Some(e);
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(super) fn render_new_task_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let Some(form) = self.tasks.new_task.as_ref() else {
            return div().into_any_element();
        };

        let mut kinds = h_flex().gap(px(6.0)).flex_wrap();
        for kind in TaskKind::all() {
            let selected = form.kind == kind;
            kinds = kinds.child(
                div()
                    .id(SharedString::from(format!(
                        "new-task-kind-{}",
                        kind.wire_name()
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
                    .child(kind.label())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            if let Some(form) = this.tasks.new_task.as_mut() {
                                form.kind = kind;
                            }
                            cx.notify();
                        }),
                    ),
            );
        }

        let mut body = v_flex()
            .w_full()
            .max_w(px(720.0))
            .gap(px(16.0))
            .child(
                v_flex().gap(px(4.0)).child(
                    div()
                        .text_size(ui_text(15.0, cx))
                        .text_color(rgb(t.text_primary))
                        .child("New task"),
                ),
            )
            .child(
                v_flex()
                    .gap(px(5.0))
                    .child(self.task_form_label("Title", cx))
                    .child(
                        okena_ui::input::input_container(&t, None)
                            .w_full()
                            .px(px(8.0))
                            .py(px(6.0))
                            .child(
                                SimpleInput::new(&self.tasks.new_task_title)
                                    .text_size(ui_text(13.0, cx)),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .gap(px(5.0))
                    .child(self.task_form_label("Description", cx))
                    .child(
                        okena_ui::input::input_container(&t, None)
                            .w_full()
                            .h(px(140.0))
                            .px(px(8.0))
                            .py(px(6.0))
                            .child(
                                SimpleInput::new(&self.tasks.new_task_body)
                                    .text_size(ui_text(13.0, cx)),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .gap(px(6.0))
                    .child(self.task_form_label("Kind", cx))
                    .child(kinds),
            );

        // Only where there is a choice: a single team is chosen rather than
        // presented.
        if form.containers.len() > 1 {
            let mut teams = h_flex().gap(px(6.0)).flex_wrap();
            for container in &form.containers {
                let selected = form.container_id.as_deref() == Some(container.id.as_str());
                let id = container.id.clone();
                let label = if container.key.is_empty() {
                    container.name.clone()
                } else {
                    format!("{} · {}", container.key, container.name)
                };
                teams = teams.child(
                    div()
                        .id(SharedString::from(format!(
                            "new-task-team-{}",
                            container.id
                        )))
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
                        .child(label)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| {
                                if let Some(form) = this.tasks.new_task.as_mut() {
                                    form.container_id = Some(id.clone());
                                }
                                cx.notify();
                            }),
                        ),
                );
            }
            body = body.child(
                v_flex()
                    .gap(px(6.0))
                    .child(self.task_form_label("Team", cx))
                    .child(teams),
            );
        }

        if let Some(err) = form.error.clone() {
            body = body.child(self.error_banner(err, cx));
        }

        let creating = form.creating;
        body = body.child(
            h_flex()
                .gap(px(8.0))
                .child(
                    div()
                        .id("new-task-create")
                        .cursor_pointer()
                        .px(px(16.0))
                        .py(px(7.0))
                        .rounded(px(4.0))
                        .bg(rgb(t.button_primary_bg))
                        .hover(|s| s.bg(rgb(t.button_primary_hover)))
                        .text_size(ui_text_md(cx))
                        .text_color(rgb(t.button_primary_fg))
                        .child(if creating { "Creating…" } else { "Create" })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _window, cx| {
                                this.submit_new_task(cx);
                            }),
                        ),
                )
                .child(self.small_button(
                    "new-task-cancel",
                    "Cancel",
                    cx.listener(move |this, _, _window, cx| {
                        this.close_new_task(cx);
                    }),
                    cx,
                )),
        );

        v_flex()
            .id("new-task-form")
            .size_full()
            .overflow_y_scroll()
            .items_center()
            .px(px(24.0))
            .py(px(24.0))
            .child(body)
            .into_any_element()
    }

    fn task_form_label(&self, label: &'static str, cx: &App) -> AnyElement {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_secondary))
            .child(label)
            .into_any_element()
    }
}

/// The kind a child of `parent` most likely is.
///
/// One step down the breakdown, because that is what breaking something down
/// means. A task's children are tasks — there is nothing narrower — and a
/// defect's children are tasks rather than more defects.
pub(super) fn narrower_than(parent: TaskKind) -> TaskKind {
    match parent {
        TaskKind::Epic => TaskKind::Feature,
        TaskKind::Feature => TaskKind::Story,
        TaskKind::Story | TaskKind::Task | TaskKind::Defect => TaskKind::Task,
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::narrower_than;
    use okena_core::tasks::TaskKind;

    #[test]
    fn a_child_defaults_one_step_down_the_breakdown() {
        assert_eq!(narrower_than(TaskKind::Epic), TaskKind::Feature);
        assert_eq!(narrower_than(TaskKind::Feature), TaskKind::Story);
        assert_eq!(narrower_than(TaskKind::Story), TaskKind::Task);
    }

    #[test]
    fn the_narrowest_kinds_stay_where_they_are() {
        // There is nothing below a task, and a defect's children are work to
        // do rather than more defects.
        assert_eq!(narrower_than(TaskKind::Task), TaskKind::Task);
        assert_eq!(narrower_than(TaskKind::Defect), TaskKind::Task);
    }
}
