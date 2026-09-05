//! Task workspace panel — the right-hand side of an agent session's view.
//!
//! An agent session is a project whose work spans several repos, so the
//! interesting context isn't in its own directory: it's the task it serves, the
//! assets the agent produced, and the worktrees it was handed. This panel shows
//! those beside the agent's terminal, which the ordinary project column already
//! renders on the left.
//!
//! Pure display from the workspace mirror — the same snapshot the sidebar and
//! project columns read, so it needs no fetching of its own.

use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_ms};
use gpui::prelude::*;
use gpui::*;
use gpui_component::v_flex;

use super::WindowView;

/// Width of the task panel. Fixed for now, like the harness panes.
const PANEL_WIDTH: f32 = 340.0;

/// Whether `candidate` belongs to the same task as the session, and isn't the
/// session itself.
///
/// Matched on the provider's own task id rather than the display key, which
/// changes when an issue moves team and would silently drop the worktrees.
fn is_related(
    session_id: &str,
    task_external_id: &str,
    candidate_id: &str,
    candidate_task: Option<&okena_core::tasks::TaskRef>,
) -> bool {
    candidate_id != session_id
        && candidate_task.is_some_and(|t| t.id.external_id == task_external_id)
}

/// One related checkout: the repo it lives in and its branch.
struct RelatedWorktree {
    name: String,
    branch: Option<String>,
    project_id: String,
}

impl WindowView {
    /// The focused project, when it is an agent session.
    ///
    /// Returns `None` for ordinary projects and worktrees, so the panel appears
    /// only where it has something to say.
    pub(crate) fn focused_agent_session(&self, cx: &App) -> Option<String> {
        let id = self
            .focus_manager
            .read(cx)
            .focused_project_id()?
            .to_string();
        let ws = self.workspace.read(cx);
        ws.project(&id)
            .filter(|p| p.is_agent_session())
            .map(|p| p.id.clone())
    }

    fn section(&self, title: &str, rows: Vec<AnyElement>, cx: &App) -> AnyElement {
        let t = theme(cx);
        v_flex()
            .gap(px(4.0))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(title.to_string()),
            )
            .children(rows)
            .into_any_element()
    }

    fn empty_row(&self, text: &str, cx: &App) -> AnyElement {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(text.to_string())
            .into_any_element()
    }

    /// Whether an agent is actually running in this session.
    ///
    /// Deliberately the same detection the Agents view uses, so the two can
    /// never disagree: if the session shows up there, no button here; if it
    /// doesn't, the button is the way to fix that.
    ///
    /// "A terminal is alive" is not the same question — a session left on a
    /// bare shell has a live terminal and no agent, which is exactly the case
    /// worth offering a restart for.
    fn agent_is_running(&self, project_id: &str, cx: &App) -> bool {
        let ws = self.workspace.read(cx);
        let Some(project) = ws.project(project_id) else {
            return false;
        };
        let Some(layout) = project.layout.as_ref() else {
            return false;
        };
        let terminals = self.terminals.lock();

        layout.collect_terminal_ids().iter().any(|id| {
            let Some(terminal) = terminals.get(id) else {
                return false;
            };
            // Resolve the pane's shell the way the spawn does: an unset pane
            // inherits the project's configured agent.
            let node_shell = layout
                .find_terminal_path(id)
                .and_then(|path| layout.get_at_path(&path).cloned())
                .and_then(|node| match node {
                    okena_workspace::state::LayoutNode::Terminal { shell_type, .. } => {
                        Some(shell_type)
                    }
                    _ => None,
                })
                .unwrap_or_default();
            let shell = match node_shell {
                okena_terminal::shell_config::ShellType::Default => {
                    project.default_shell.clone().unwrap_or_default()
                }
                explicit => explicit,
            };
            crate::views::harness::agents_view::detect_agent(&shell, terminal.title().as_deref())
                .is_some()
        })
    }

    /// Restart the agent for this session.
    ///
    /// Closes whatever is running first, then creates a fresh terminal. Closing
    /// matters: session persistence keeps a tmux session per terminal id and
    /// re-attaches to it, so reusing an id would reattach to the old process
    /// instead of launching the agent. A new terminal means a new session.
    ///
    /// The new terminal carries no shell override, so the daemon resolves the
    /// project's own `default_shell` — the agent command, prompt and MCP config
    /// chosen when the task was started.
    fn restart_session(&mut self, project_id: String, cx: &mut Context<Self>) {
        let client = match self.local_daemon_action_client(cx) {
            Ok(c) => c,
            Err(error) => {
                crate::views::panels::toast::ToastManager::error(error, cx);
                return;
            }
        };
        let daemon_id = okena_transport::client::strip_prefix(&project_id, client.connection_id());
        let existing: Vec<String> = self
            .workspace
            .read(cx)
            .project(&project_id)
            .and_then(|p| p.layout.as_ref())
            .map(|l| l.collect_terminal_ids())
            .unwrap_or_default()
            .iter()
            .map(|id| okena_transport::client::strip_prefix(id, client.connection_id()))
            .collect();

        cx.spawn(async move |_this, cx| {
            let result = smol::unblock(move || {
                if !existing.is_empty() {
                    // Best-effort: a terminal that has already gone should not
                    // block the restart the user asked for.
                    let _ = client.post_action(okena_core::api::ActionRequest::CloseTerminals {
                        project_id: daemon_id.clone(),
                        terminal_ids: existing,
                    });
                }
                client.post_action(okena_core::api::ActionRequest::CreateTerminal {
                    project_id: daemon_id,
                })
            })
            .await;

            if let Err(error) = result {
                cx.update(|cx| {
                    crate::views::panels::toast::ToastManager::error(
                        format!("Could not restart the agent: {error}"),
                        cx,
                    );
                });
            }
        })
        .detach();
    }

    /// Tear down the whole task workspace.
    ///
    /// Only ever called from the confirmation step: it deletes checkouts, and
    /// with `force` it discards uncommitted work in them.
    fn delete_workspace(&mut self, project_id: String, force: bool, cx: &mut Context<Self>) {
        let client = match self.local_daemon_action_client(cx) {
            Ok(c) => c,
            Err(error) => {
                crate::views::panels::toast::ToastManager::error(error, cx);
                return;
            }
        };
        let daemon_id = okena_transport::client::strip_prefix(&project_id, client.connection_id());
        self.pending_workspace_delete = None;
        cx.notify();

        cx.spawn(async move |_this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(okena_core::api::ActionRequest::TaskDeleteWorkspace {
                        project_id: daemon_id,
                        force,
                    })
                    .and_then(|v| v.ok_or_else(|| "Missing delete result".to_string()))
            })
            .await;

            cx.update(|cx| match result {
                // A partial delete must be surfaced: git refuses to remove a
                // dirty checkout, and silently leaving it would look like the
                // workspace was fully torn down.
                Ok(value) => {
                    let failures: Vec<String> = value
                        .get("failed")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|f| {
                                    let name = f.get("project")?.as_str()?;
                                    let err = f.get("error")?.as_str()?;
                                    Some(format!("{name}: {err}"))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    if !failures.is_empty() {
                        crate::views::panels::toast::ToastManager::error(
                            format!("Some parts were kept — {}", failures.join(" · ")),
                            cx,
                        );
                    }
                }
                Err(error) => crate::views::panels::toast::ToastManager::error(
                    format!("Could not delete the workspace: {error}"),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// Confirmation block shown in place of the delete button.
    fn render_delete_confirm(
        &self,
        project_id: &str,
        worktree_count: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let force = self
            .pending_workspace_delete
            .as_ref()
            .map(|(_, f)| *f)
            .unwrap_or(false);
        let id_force = project_id.to_string();
        let id_go = project_id.to_string();

        v_flex()
            .gap(px(6.0))
            .p(px(8.0))
            .rounded(px(4.0))
            .bg(with_alpha(t.error, 0.1))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_primary))
                    // Names the count so the blast radius is explicit rather
                    // than "are you sure?".
                    .child(format!(
                        "Delete this agent session and {worktree_count} worktree(s)?                          Checkouts are removed from disk and running agents are killed."
                    )),
            )
            .child(
                div()
                    .id("task-delete-force")
                    .cursor_pointer()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(if force { t.error } else { t.text_secondary }))
                    .child(if force {
                        "☑ Discard uncommitted changes"
                    } else {
                        "☐ Discard uncommitted changes"
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            this.pending_workspace_delete = Some((id_force.clone(), !force));
                            cx.notify();
                        }),
                    ),
            )
            .child(
                gpui_component::h_flex()
                    .gap(px(6.0))
                    .child(
                        div()
                            .id("task-delete-go")
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
                                    this.delete_workspace(id_go.clone(), force, cx);
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id("task-delete-cancel")
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
                                cx.listener(|this, _, _window, cx| {
                                    this.pending_workspace_delete = None;
                                    cx.notify();
                                }),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// The task workspace panel for `project_id`.
    pub(crate) fn render_task_panel(
        &mut self,
        project_id: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let ws = self.workspace.read(cx);

        let Some(project) = ws.project(&project_id) else {
            return div().into_any_element();
        };
        let task = project.task_ref.clone();
        let agent = project.agent.clone();

        // Everything else linked to the same task: the worktrees this agent was
        // handed, which live in other repos and would otherwise be invisible
        // from here.
        let related: Vec<RelatedWorktree> = match task.as_ref() {
            Some(task) => ws
                .projects()
                .iter()
                .filter(|p| {
                    is_related(
                        &project_id,
                        &task.id.external_id,
                        &p.id,
                        p.task_ref.as_ref(),
                    )
                })
                .map(|p| RelatedWorktree {
                    name: p.name.clone(),
                    branch: ws
                        .remote_snapshot(&p.id)
                        .and_then(|s| s.git_status.as_ref())
                        .and_then(|g| g.branch.clone()),
                    project_id: p.id.clone(),
                })
                .collect(),
            None => Vec::new(),
        };

        // Assets the agent registered over okena's MCP server.
        let asset_rows: Vec<AnyElement> = agent
            .as_ref()
            .map(|a| {
                a.assets
                    .iter()
                    .map(|asset| {
                        let label = match &asset.project {
                            Some(p) => format!("{} · {} ({p})", asset.kind.label(), asset.title),
                            None => format!("{} · {}", asset.kind.label(), asset.title),
                        };
                        v_flex()
                            .gap(px(1.0))
                            .child(
                                div()
                                    .w_full()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(t.text_primary))
                                    .child(label),
                            )
                            .children(asset.url.as_ref().map(|url| {
                                div()
                                    .w_full()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(url.clone())
                                    .into_any_element()
                            }))
                            .into_any_element()
                    })
                    .collect()
            })
            .unwrap_or_default();

        let worktree_count = related.len();
        let worktree_rows: Vec<AnyElement> = related
            .into_iter()
            .map(|w| {
                let id = w.project_id.clone();
                let label = match &w.branch {
                    Some(b) => format!("{} · {b}", w.name),
                    None => w.name.clone(),
                };
                div()
                    .id(SharedString::from(format!("task-wt-{id}")))
                    .cursor_pointer()
                    .w_full()
                    .px(px(6.0))
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(label)
                    // Jumping to a worktree is the common move from here: the
                    // agent reports a PR, you go read the diff.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            let workspace = this.workspace.clone();
                            let id = id.clone();
                            this.focus_manager.update(cx, |fm, cx| {
                                workspace.update(cx, |ws, cx| {
                                    ws.set_focused_project_individual(fm, Some(id.clone()), cx);
                                });
                                cx.notify();
                            });
                        }),
                    )
                    .into_any_element()
            })
            .collect();

        let status = agent.as_ref().and_then(|a| a.status.clone());
        let running = self.agent_is_running(&project_id, cx);
        let confirming = self
            .pending_workspace_delete
            .as_ref()
            .is_some_and(|(id, _)| *id == project_id);

        v_flex()
            .w(px(PANEL_WIDTH))
            .h_full()
            .flex_shrink_0()
            .border_l_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_primary))
            .child(
                // Header mirrors the project column's, so the two read as one
                // workspace rather than two stacked panels.
                v_flex()
                    .h(crate::ui::tokens::HEADER_HEIGHT)
                    .justify_center()
                    .px(px(12.0))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .bg(rgb(t.bg_header))
                    .child(
                        div()
                            .text_size(ui_text(13.0, cx))
                            .text_color(rgb(t.text_primary))
                            .child(
                                task.as_ref()
                                    .map(|t| t.display_key.clone())
                                    .unwrap_or_else(|| "Task".to_string()),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .id("task-panel-body")
                    .flex_1()
                    .overflow_y_scroll()
                    .p(px(12.0))
                    .gap(px(14.0))
                    .children(task.as_ref().map(|task| {
                        v_flex()
                            .gap(px(3.0))
                            .child(
                                div()
                                    .text_size(ui_text(13.0, cx))
                                    .text_color(rgb(t.text_primary))
                                    .child(task.title.clone()),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(task.url.clone()),
                            )
                            .into_any_element()
                    }))
                    .children((!running).then(|| {
                        // Hidden while the agent is running: there is nothing
                        // to fix, and a restart button on healthy state is
                        // noise that invites an accidental kill.
                        div()
                            .id("task-restart-agent")
                            .cursor_pointer()
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
                                cx.listener({
                                    let id = project_id.clone();
                                    move |this, _, _window, cx| {
                                        this.restart_session(id.clone(), cx);
                                    }
                                }),
                            )
                            .into_any_element()
                    }))
                    .children(status.map(|s| {
                        div()
                            .px(px(8.0))
                            .py(px(5.0))
                            .rounded(px(4.0))
                            .bg(with_alpha(t.success, 0.1))
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_primary))
                            .child(format!("“{s}”"))
                            .into_any_element()
                    }))
                    .child(self.section(
                        "ASSETS",
                        if asset_rows.is_empty() {
                            // Says why it's empty rather than just showing
                            // nothing: assets arrive from the agent, not okena.
                            vec![self.empty_row(
                                "None yet — agents register these over okena's MCP server.",
                                cx,
                            )]
                        } else {
                            asset_rows
                        },
                        cx,
                    ))
                    .child(self.section(
                        "WORKTREES",
                        if worktree_rows.is_empty() {
                            vec![self.empty_row("No related worktrees.", cx)]
                        } else {
                            worktree_rows
                        },
                        cx,
                    ))
                    .child(if confirming {
                        self.render_delete_confirm(&project_id, worktree_count, cx)
                    } else {
                        let id = project_id.clone();
                        div()
                            .id("task-delete-workspace")
                            .cursor_pointer()
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
                                    // Two steps on purpose: this removes
                                    // checkouts from disk.
                                    this.pending_workspace_delete = Some((id.clone(), false));
                                    cx.notify();
                                }),
                            )
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Explicit imports: the `gpui::*` glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself.
    use super::is_related;
    use okena_core::tasks::{TaskId, TaskRef};

    fn task(external: &str, key: &str) -> TaskRef {
        TaskRef {
            id: TaskId::new("linear", external),
            display_key: key.into(),
            title: "t".into(),
            url: "u".into(),
        }
    }

    #[test]
    fn a_worktree_on_the_same_task_is_related() {
        let t = task("u1", "LIN-1");
        assert!(is_related("session", "u1", "worktree", Some(&t)));
    }

    #[test]
    fn the_session_is_not_related_to_itself() {
        // Otherwise the panel would list the session among its own worktrees.
        let t = task("u1", "LIN-1");
        assert!(!is_related("session", "u1", "session", Some(&t)));
    }

    #[test]
    fn a_different_task_is_not_related() {
        let t = task("u2", "LIN-2");
        assert!(!is_related("session", "u1", "other", Some(&t)));
    }

    #[test]
    fn an_unlinked_project_is_not_related() {
        assert!(!is_related("session", "u1", "plain", None));
    }

    #[test]
    fn matching_is_by_provider_id_not_display_key() {
        // A display key changes when an issue moves team; matching on it would
        // silently drop every worktree the agent was handed.
        let renamed = task("u1", "ENG-9");
        assert!(is_related("session", "u1", "worktree", Some(&renamed)));
    }
}
