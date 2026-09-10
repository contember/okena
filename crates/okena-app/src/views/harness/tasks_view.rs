//! Tasks view — assigned work from the task manager, and a worktree per task.
//!
//! Every call goes to the daemon, which owns the provider credential. The
//! client never holds a task-manager token and never talks to Linear directly.

use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms, ui_text_sm};
use crate::views::components::{SimpleInput, SimpleInputState};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::tasks::{Task, TaskAuthState, TaskAuthStatusResponse, TaskKind, TaskState};
use okena_ui::resize_handle::ResizeHandle;
use okena_views_terminal::layout::split_pane::DragState;

use super::HarnessPane;

/// Agents offered in the "Start work" dialog. Mirrors the detection list in
/// `agents_view`, so anything okena can launch is also something it recognizes
/// afterwards.
const AGENT_CHOICES: &[&str] = &["claude", "copilot"];

/// Accent colour for a workflow-state category.
///
/// Keyed on the normalized category, not the provider's state name, so a team
/// that renames "In Progress" to "Cooking" still gets the right colour.
fn state_color(state: TaskState, t: &crate::theme::ThemeColors) -> u32 {
    match state {
        TaskState::InProgress => t.success,
        TaskState::InReview => t.warning,
        TaskState::Todo => t.button_primary_bg,
        TaskState::Backlog | TaskState::Canceled | TaskState::Unknown => t.text_secondary,
        TaskState::Done => t.success,
    }
}

/// Order a lane's tasks depth-first so each subtree stays together, returning
/// the nesting depth alongside each task.
///
/// Depth-first matters: a one-level pass puts a feature under its epic but
/// leaves that feature's own stories stranded at the end of the list, which
/// reads as no hierarchy at all.
///
/// Parents keep their incoming order (already newest-activity-first). Anything
/// whose parent isn't in this lane becomes a root so it can never be dropped,
/// and a parent cycle cannot loop forever — each task is emitted at most once.
pub(super) fn order_by_hierarchy(
    tasks: Vec<Task>,
    collapsed: &std::collections::HashSet<String>,
) -> Vec<TaskRow> {
    use std::collections::{HashMap, HashSet};

    let present: HashSet<String> = tasks.iter().map(|t| t.id.external_id.clone()).collect();
    let mut children: HashMap<String, Vec<usize>> = HashMap::new();
    let mut roots: Vec<usize> = Vec::new();
    for (i, task) in tasks.iter().enumerate() {
        match task
            .parent_id
            .as_ref()
            .filter(|parent| present.contains(*parent))
        {
            Some(parent) => children.entry(parent.clone()).or_default().push(i),
            None => roots.push(i),
        }
    }

    let mut emitted = vec![false; tasks.len()];
    let mut ordered: Vec<(usize, usize)> = Vec::new();
    // Explicit stack, not recursion: a malformed parent chain from a provider
    // must not be able to blow the render thread's stack.
    let mut stack: Vec<(usize, usize)> = roots.into_iter().rev().map(|i| (i, 0)).collect();
    while let Some((index, depth)) = stack.pop() {
        if emitted[index] {
            continue;
        }
        emitted[index] = true;
        ordered.push((index, depth));
        if collapsed.contains(&tasks[index].id.external_id) {
            // A collapsed parent still renders; its subtree is hidden. Mark the
            // whole subtree as accounted for — otherwise the unreachable sweep
            // below, which exists to rescue tasks caught in a parent cycle,
            // would re-emit every hidden descendant as a flat row.
            let mut hidden: Vec<usize> = children
                .get(&tasks[index].id.external_id)
                .cloned()
                .unwrap_or_default();
            while let Some(node) = hidden.pop() {
                if emitted[node] {
                    continue;
                }
                emitted[node] = true;
                if let Some(kids) = children.get(&tasks[node].id.external_id) {
                    hidden.extend(kids.iter().copied());
                }
            }
        } else if let Some(kids) = children.get(&tasks[index].id.external_id) {
            for kid in kids.iter().rev() {
                stack.push((*kid, depth + 1));
            }
        }
    }
    // A task inside a parent cycle is reachable from no root; show it flat
    // rather than silently losing it.
    for (i, done) in emitted.iter().enumerate() {
        if !done {
            ordered.push((i, 0));
        }
    }

    // Recorded before the tasks are consumed: the chevron must show on a
    // collapsed parent too, and by then its children are no longer walked.
    let has_children: Vec<bool> = (0..slots_len(&tasks))
        .map(|i| {
            children
                .get(&tasks[i].id.external_id)
                .is_some_and(|kids| !kids.is_empty())
        })
        .collect();

    let mut slots: Vec<Option<Task>> = tasks.into_iter().map(Some).collect();
    ordered
        .into_iter()
        .filter_map(|(i, depth)| {
            slots[i].take().map(|task| TaskRow {
                task,
                depth,
                has_children: has_children[i],
            })
        })
        .collect()
}

fn slots_len(tasks: &[Task]) -> usize {
    tasks.len()
}

/// A task as it appears in a lane: its nesting depth and whether it can be
/// expanded.
pub(super) struct TaskRow {
    pub task: Task,
    pub depth: usize,
    pub has_children: bool,
}

/// Colour for a breakdown level.
///
/// Defects are the one level that must stand out at a glance; the rest shade
/// from broad to narrow so the hierarchy reads without being loud.
fn kind_color(kind: TaskKind, t: &crate::theme::ThemeColors) -> u32 {
    match kind {
        TaskKind::Defect => t.error,
        TaskKind::Epic => t.button_primary_bg,
        TaskKind::Feature => t.success,
        TaskKind::Story => t.text_secondary,
        TaskKind::Task => t.text_muted,
    }
}

/// Where a task sits on the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Lane {
    /// Not started: nothing of okena's is running for it yet.
    Todo,
    InProgress(Stage),
}

/// Sub-state within the in-progress lane.
///
/// These come from okena's own view of the work, not the task manager's — the
/// provider cannot know that an agent is sitting at a prompt. "Needs attention"
/// is the same signal (and the same word) the sidebar's activity tiers use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    /// A session for this task is blocked waiting for input.
    NeedsAttention,
    /// Work is out for review: a pull request exists.
    ReadyForTesting,
    /// Being worked on.
    Active,
}

impl Stage {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Stage::NeedsAttention => "Needs attention",
            Stage::ReadyForTesting => "Ready for testing",
            Stage::Active => "Active",
        }
    }

    /// Ordered most-urgent first — a blocked agent is what the user must act on.
    pub(super) const fn all() -> [Stage; 3] {
        [Stage::NeedsAttention, Stage::ReadyForTesting, Stage::Active]
    }
}

/// Projects linked to a task, and what to do about them.
#[derive(Clone, Debug, Default)]
pub(super) struct TaskLinks {
    pub signals: TaskSignals,
    /// Project to focus when opening the task's existing session. Prefers the
    /// agent session (which spans the repos) over any single worktree.
    pub open_target: Option<String>,
    /// The agent session itself, when one exists. Distinct from `open_target`,
    /// which falls back to a worktree: only a session has session facts —
    /// a reported status, produced assets, the checkouts it was handed.
    pub session: Option<String>,
    /// Sessions running a detected coding agent.
    pub agents_running: usize,
    /// Sessions started *about* this task rather than to do it — an agent
    /// breaking it down, say. `(project_id, name)`.
    ///
    /// Kept apart from `session` because they answer different questions:
    /// one is where the work is happening, the other is who is helping think
    /// about it. Counting a helper as work would move the task into "in
    /// progress" the moment you asked an agent for a breakdown.
    pub helpers: Vec<(String, String)>,
}

/// What okena knows about the sessions linked to one task.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct TaskSignals {
    /// A worktree or agent session exists for this task.
    pub linked: bool,
    /// One of those sessions is waiting for input.
    pub waiting: bool,
    /// One of those projects has an open pull request.
    pub open_pr: bool,
}

/// Decide which lane a task belongs in.
///
/// okena's own signals win over the provider's state: a task Linear still calls
/// "Todo" is genuinely in progress once a worktree and an agent exist for it,
/// and that is the state the user cares about.
pub(super) fn lane_for(provider_in_progress: bool, signals: TaskSignals) -> Lane {
    if !signals.linked && !provider_in_progress {
        return Lane::Todo;
    }
    // Blocked beats everything: it is the only state needing a human right now.
    if signals.waiting {
        return Lane::InProgress(Stage::NeedsAttention);
    }
    if signals.open_pr {
        return Lane::InProgress(Stage::ReadyForTesting);
    }
    Lane::InProgress(Stage::Active)
}

impl HarnessPane {
    // ─── Data ────────────────────────────────────────────────────────────────

    /// Which providers are connected. Local to the daemon — no network call.
    pub(super) fn refresh_auth(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let provider = self.tasks.provider.clone();

        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::TasksAuthStatus)
                    .and_then(|v| v.ok_or_else(|| "Missing auth status".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<TaskAuthStatusResponse>(v)
                            .map_err(|e| format!("Invalid auth status: {e}"))
                    })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(status) => match status.provider(&this.tasks.provider) {
                            Some(entry) => {
                                this.tasks.provider_display_name = entry.display_name.clone();
                                this.tasks.connection = entry.auth.clone();
                                // Only fetch once a credential is known to
                                // exist — otherwise every open costs a
                                // guaranteed-failing round trip.
                                if entry.auth.is_connected() {
                                    this.refresh_tasks(cx);
                                }
                            }
                            None => {
                                this.tasks.error =
                                    Some(format!("This daemon doesn't know `{provider}`"));
                                this.tasks.connection = TaskAuthState::Disconnected;
                            }
                        },
                        Err(e) => {
                            this.tasks.error = Some(e);
                            this.tasks.connection = TaskAuthState::Disconnected;
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(super) fn refresh_tasks(&mut self, cx: &mut Context<Self>) {
        self.tasks.loading = true;
        self.tasks.error = None;
        cx.notify();

        let client = self.client.clone();
        let provider = self.tasks.provider.clone();

        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::TasksList { provider })
                    .and_then(|v| v.ok_or_else(|| "Missing task list".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<Vec<Task>>(v["tasks"].clone())
                            .map_err(|e| format!("Invalid task list: {e}"))
                    })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(tasks) => {
                            this.tasks.tasks = tasks;
                            this.tasks.error = None;
                            // Children may have been created since — by an
                            // agent through MCP, or by someone else entirely —
                            // so a refresh drops what was cached rather than
                            // showing a breakdown that is missing its newest
                            // half.
                            this.tasks.children.clear();
                            // Drop a selection whose task is gone — closed,
                            // reassigned, or filtered out by the provider.
                            // Leaving it would show detail for a task no
                            // longer in the list.
                            if let Some(id) = this.tasks.selected.clone()
                                && !this.tasks.tasks.iter().any(|t| t.id.external_id == id)
                            {
                                this.tasks.selected = None;
                            }
                        }
                        Err(e) => {
                            // A rejected credential is the one failure with a
                            // specific fix, so flip to the connect form rather
                            // than showing a bare error.
                            if e.contains("rejected the stored credential") {
                                this.tasks.connection = TaskAuthState::Expired;
                            }
                            this.tasks.error = Some(e);
                        }
                    }
                    this.tasks.loading = false;
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(super) fn connect(&mut self, cx: &mut Context<Self>) {
        let api_key = self.tasks.api_key_input.read(cx).value().trim().to_string();
        if api_key.is_empty() {
            self.tasks.error = Some("Enter an API key first".to_string());
            cx.notify();
            return;
        }

        self.tasks.loading = true;
        self.tasks.error = None;
        self.tasks.status = Some("Verifying key…".to_string());
        cx.notify();

        let client = self.client.clone();
        let provider = self.tasks.provider.clone();

        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::TasksConnectApiKey { provider, api_key })
                    .and_then(|v| v.ok_or_else(|| "Missing connect result".to_string()))
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.tasks.loading = false;
                    this.tasks.status = None;
                    match result {
                        Ok(_) => {
                            // Clear the key from the field as soon as it's
                            // stored — no reason to leave a secret on screen.
                            this.tasks.api_key_input.update(cx, |input, cx| {
                                input.set_value(String::new(), cx);
                            });
                            this.refresh_auth(cx);
                        }
                        Err(e) => this.tasks.error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Open the "Start work" dialog for `task`.
    ///
    /// Pre-filled rather than blank: the provider's branch name (which keeps
    /// its branch-to-issue linking working), the first top-level project, and
    /// the daemon's configured agent.
    pub(super) fn open_start_form(&mut self, task: &Task, cx: &mut Context<Self>) {
        let branch = task.branch_name.clone();
        let branch_input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Branch / worktree name")
                .default_value(branch)
        });
        let project_ids = self
            .workspace
            .read(cx)
            .projects()
            .iter()
            .find(|p| p.worktree_info.is_none())
            .map(|p| vec![p.id.clone()])
            .unwrap_or_default();

        self.tasks.start_form = Some(super::StartWorkForm {
            task: task.clone(),
            project_ids,
            branch_input,
            agent: self.tasks.default_agent.clone(),
        });
        self.tasks.error = None;
        cx.notify();
    }

    /// Focus an existing session for a task and leave the harness view.
    pub(super) fn open_session(&mut self, project_id: String, cx: &mut Context<Self>) {
        crate::views::components::project_nav::focus_project(
            &self.workspace,
            &self.focus_manager,
            self.window_id,
            &project_id,
            cx,
        );
        cx.notify();
    }

    pub(super) fn close_start_form(&mut self, cx: &mut Context<Self>) {
        self.tasks.start_form = None;
        cx.notify();
    }

    /// Read the daemon's configured agent so the dialog can default to it.
    pub(super) fn refresh_default_agent(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::GetSettings)
                    .and_then(|v| v.ok_or_else(|| "Missing settings".to_string()))
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    if let Ok(v) = result {
                        this.tasks.default_agent = v
                            .get("harness")
                            .and_then(|h| h.get("agent_command"))
                            .and_then(|c| c.as_str())
                            .filter(|c| !c.trim().is_empty())
                            .map(str::to_string);
                        // The Specs picker follows the same default until the
                        // user chooses for themselves.
                        if !this.specs.agent_picked {
                            this.specs.agent = this.tasks.default_agent.clone();
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Dispatch the configured run.
    pub(super) fn confirm_start(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.tasks.start_form.as_ref() else {
            return;
        };
        if form.project_ids.is_empty() {
            self.tasks.error = Some("Pick at least one project to work in".to_string());
            cx.notify();
            return;
        }
        if self.tasks.starting.is_some() {
            return;
        }

        // Harness panes post straight through `RemoteActionClient`, bypassing
        // the dispatcher's id stripping — see `HarnessPane::daemon_id`.
        let project_ids: Vec<String> = form
            .project_ids
            .iter()
            .map(|id| self.daemon_id(id))
            .collect();
        let branch = form.branch_input.read(cx).value().trim().to_string();
        // An empty string tells the daemon "no agent" explicitly, which is not
        // the same as `None` (fall back to the configured default).
        let agent_command = Some(form.agent.clone().unwrap_or_default());
        let external_id = form.task.id.external_id.clone();
        let display_key = form.task.display_key.clone();

        self.tasks.starting = Some(external_id.clone());
        self.tasks.error = None;
        self.tasks.status = Some(format!("Creating worktrees for {display_key}…"));
        self.tasks.start_form = None;
        cx.notify();

        let client = self.client.clone();
        let provider = self.tasks.provider.clone();

        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::TaskStartWork {
                        provider,
                        task_external_id: external_id,
                        project_ids,
                        agent_root: None,
                        branch: (!branch.is_empty()).then_some(branch),
                        agent_command,
                    })
                    .and_then(|v| v.ok_or_else(|| "Missing start-work result".to_string()))
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.tasks.starting = None;
                    match result {
                        Ok(value) => {
                            let branch = value
                                .get("branch")
                                .and_then(|v| v.as_str())
                                .unwrap_or("worktree");
                            let made = value
                                .get("created")
                                .and_then(|v| v.as_array())
                                .map(|a| a.len())
                                .unwrap_or(0);
                            let session = value
                                .get("agent_session")
                                .and_then(|v| v.get("root"))
                                .and_then(|v| v.as_str())
                                .map(|root| format!(", agent session in {root}"))
                                .unwrap_or_default();
                            this.tasks.status = Some(format!(
                                "{display_key} → {branch} · {made} worktree(s){session}"
                            ));
                            // Partial success is still a failure worth showing:
                            // dropping it silently would leave the user thinking
                            // every repo got a checkout.
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
                            this.tasks.error = (!failures.is_empty()).then(|| failures.join(" · "));
                        }
                        Err(e) => {
                            this.tasks.status = None;
                            this.tasks.error = Some(e);
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Collect okena's signals for one task from the projects linked to it.
    ///
    /// A task can have several linked projects (one worktree per repo, plus an
    /// agent session), so any blocked session marks the whole task blocked.
    fn links_for(&self, task: &Task, cx: &Context<Self>) -> TaskLinks {
        let ws = self.workspace.read(cx);
        let terminals = self.terminals.lock();
        let mut links = TaskLinks::default();

        for project in ws.projects() {
            let linked = project
                .task_ref
                .as_ref()
                .is_some_and(|t| t.id.external_id == task.id.external_id);
            if !linked {
                continue;
            }
            // A free-form session carries a task link so the task can list it,
            // but it is not work on the task: no worktrees, and it must not
            // move the task out of Todo.
            if project.is_custom_session() {
                links
                    .helpers
                    .push((project.id.clone(), project.name.clone()));
                continue;
            }

            links.signals.linked = true;

            // The agent session spans every repo, so it is the better landing
            // place than an arbitrary one of the task's worktrees.
            if project.is_agent_session() {
                links.session = Some(project.id.clone());
                links.open_target = Some(project.id.clone());
            } else if links.open_target.is_none() {
                links.open_target = Some(project.id.clone());
            }

            if ws
                .remote_snapshot(&project.id)
                .and_then(|s| s.git_status.as_ref())
                .and_then(|g| g.pr_info.as_ref())
                .is_some_and(|pr| pr.state == okena_core::api::PrState::Open)
            {
                links.signals.open_pr = true;
            }

            if let Some(layout) = project.layout.as_ref() {
                let mut has_agent = false;
                for id in layout.collect_terminal_ids() {
                    let Some(terminal) = terminals.get(&id) else {
                        continue;
                    };
                    if terminal.is_waiting_for_input() {
                        links.signals.waiting = true;
                    }
                    // Same detection the Agents view uses, resolved the same
                    // way, so the two never disagree about what is running.
                    let node_shell = layout
                        .find_terminal_path(&id)
                        .and_then(|path| layout.get_at_path(&path).cloned())
                        .and_then(|node| match node {
                            crate::workspace::state::LayoutNode::Terminal {
                                shell_type, ..
                            } => Some(shell_type),
                            _ => None,
                        })
                        .unwrap_or_default();
                    let shell = match node_shell {
                        okena_terminal::shell_config::ShellType::Default => {
                            project.default_shell.clone().unwrap_or_default()
                        }
                        explicit => explicit,
                    };
                    if crate::views::agent_session::detect_agent(
                        &shell,
                        terminal.title().as_deref(),
                    )
                    .is_some()
                    {
                        has_agent = true;
                    }
                }
                if has_agent {
                    links.agents_running += 1;
                }
            }
        }
        links
    }

    /// Select a task and make sure its children are on the way.
    ///
    /// Every path into the detail pane goes through here, so a task reached by
    /// clicking its parent gets the same treatment as one clicked in the list.
    pub(super) fn select_task(&mut self, external_id: String, cx: &mut Context<Self>) {
        self.tasks.selected = Some(external_id.clone());
        self.fetch_children(external_id, cx);
        cx.notify();
    }

    /// Any task okena currently knows about, by provider id.
    ///
    /// Looks past the user's own queue into the fetched children: a sub-task
    /// assigned to somebody else is still one you can open from its parent,
    /// and refusing to show it would make the hierarchy a dead end.
    pub(super) fn known_task(&self, external_id: &str) -> Option<Task> {
        self.tasks
            .tasks
            .iter()
            .chain(self.tasks.children.values().flatten())
            .find(|t| t.id.external_id == external_id)
            .cloned()
    }

    /// Read a task's sub-tasks, once.
    fn fetch_children(&mut self, external_id: String, cx: &mut Context<Self>) {
        if self.tasks.children.contains_key(&external_id)
            || self.tasks.children_loading.as_deref() == Some(external_id.as_str())
        {
            return;
        }
        self.tasks.children_loading = Some(external_id.clone());

        let client = self.client.clone();
        let provider = self.tasks.provider.clone();
        let key = external_id.clone();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::TaskChildren {
                        provider,
                        task_external_id: external_id,
                    })
                    .and_then(|v| v.ok_or_else(|| "Missing sub-tasks".to_string()))
                    .map(|v| {
                        v.get("tasks")
                            .and_then(|t| serde_json::from_value::<Vec<Task>>(t.clone()).ok())
                            .unwrap_or_default()
                    })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.tasks.children_loading = None;
                    // Cache the empty answer too: a leaf task should not be
                    // asked about again every time it is selected.
                    this.tasks.children.insert(key, result.unwrap_or_default());
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// A clickable row for a task in the hierarchy section.
    fn render_related_task(&self, task: &Task, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let id = task.id.external_id.clone();
        let state_label = if task.state_name.is_empty() {
            "—".to_string()
        } else {
            task.state_name.clone()
        };
        v_flex()
            .id(SharedString::from(format!(
                "related-{}",
                task.id.external_id
            )))
            .cursor_pointer()
            .w_full()
            .min_w_0()
            .gap(px(3.0))
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(4.0))
            .bg(rgb(t.bg_secondary))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .child(
                h_flex()
                    .items_center()
                    .gap(px(6.0))
                    .flex_wrap()
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_secondary))
                            .child(task.display_key.clone()),
                    )
                    .child(self.chip(task.kind.label().to_string(), kind_color(task.kind, &t), cx))
                    .child(self.chip(state_label, state_color(task.state, &t), cx)),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_primary))
                    .child(task.title.clone()),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.select_task(id.clone(), cx);
                }),
            )
            .into_any_element()
    }

    /// Group the loaded tasks into board lanes.
    fn board(&self, cx: &Context<Self>) -> (Vec<Task>, Vec<(Stage, Vec<Task>)>) {
        let mut todo = Vec::new();
        let mut staged: Vec<(Stage, Vec<Task>)> =
            Stage::all().into_iter().map(|s| (s, Vec::new())).collect();

        for task in &self.tasks.tasks {
            let provider_in_progress =
                matches!(task.state, TaskState::InProgress | TaskState::InReview);
            match lane_for(provider_in_progress, self.links_for(task, cx).signals) {
                Lane::Todo => todo.push(task.clone()),
                Lane::InProgress(stage) => {
                    if let Some((_, list)) = staged.iter_mut().find(|(s, _)| *s == stage) {
                        list.push(task.clone());
                    }
                }
            }
        }
        (todo, staged)
    }

    /// A collapsible section heading in the task list.
    fn render_section_header(
        &self,
        id: &'static str,
        label: &'static str,
        count: usize,
        accent: u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let collapsed = self.tasks.sections_collapsed.contains(id);
        h_flex()
            .id(SharedString::from(format!("task-section-{id}")))
            .cursor_pointer()
            .w_full()
            .items_center()
            .gap(px(6.0))
            .px(px(12.0))
            .py(px(6.0))
            .bg(with_alpha(accent, 0.08))
            .border_b_1()
            .border_color(rgb(t.border))
            .hover(|s| s.bg(with_alpha(accent, 0.14)))
            .child(
                div()
                    .w(px(10.0))
                    .flex_shrink_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(accent))
                    .child(if collapsed { "›" } else { "⌄" }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(ui_text(13.0, cx))
                    .text_color(rgb(accent))
                    .child(label),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(accent))
                    .child(format!("{count}")),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    if !this.tasks.sections_collapsed.remove(id) {
                        this.tasks.sections_collapsed.insert(id.to_string());
                    }
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// The task list: in progress above todo, each foldable.
    ///
    /// One column rather than two side-by-side lanes. The lanes forced every
    /// row into half the width, which a task's key, kind, state and title do
    /// not fit into, and left one column idle whenever the work was all in one
    /// state. In progress sits on top because it is what needs acting on.
    fn render_task_list(
        &self,
        todo: Vec<Task>,
        staged: Vec<(Stage, Vec<Task>)>,
        share: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let in_progress_total: usize = staged.iter().map(|(_, v)| v.len()).sum();

        let mut body = v_flex()
            .id("tasks-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();

        body = body.child(self.render_section_header(
            "in-progress",
            "In progress",
            in_progress_total,
            t.success,
            cx,
        ));
        if !self.tasks.sections_collapsed.contains("in-progress") {
            if in_progress_total == 0 {
                body = body.child(self.list_note("Nothing in flight.", cx));
            }
            for (stage, tasks) in staged {
                if tasks.is_empty() {
                    continue;
                }
                let accent = match stage {
                    Stage::NeedsAttention => t.warning,
                    Stage::ReadyForTesting => t.success,
                    Stage::Active => t.text_secondary,
                };
                body = body.child(
                    div()
                        .px(px(12.0))
                        .py(px(4.0))
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(accent))
                        .child(format!("{} · {}", stage.label(), tasks.len())),
                );
                for row in order_by_hierarchy(tasks, &self.tasks.collapsed) {
                    body = body.child(self.render_task_row(&row, cx));
                }
            }
        }

        body = body.child(self.render_section_header(
            "todo",
            "Todo",
            todo.len(),
            t.text_secondary,
            cx,
        ));
        if !self.tasks.sections_collapsed.contains("todo") {
            if todo.is_empty() {
                body = body.child(self.list_note("Nothing waiting.", cx));
            }
            for row in order_by_hierarchy(todo, &self.tasks.collapsed) {
                body = body.child(self.render_task_row(&row, cx));
            }
        }

        v_flex()
            .w(relative(share))
            .min_w_0()
            .h_full()
            .child(body)
            .into_any_element()
    }

    fn list_note(&self, text: &'static str, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .px(px(12.0))
            .py(px(10.0))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(text)
            .into_any_element()
    }

    /// The detail pane: everything about the selected task, and what to do
    /// with it.
    ///
    /// The actions live here rather than on every row: one row's worth of
    /// buttons repeated down a list is noise, and a task you are deciding
    /// about is one you have selected.
    fn render_task_detail(&self, share: f32, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let selected = self
            .tasks
            .selected
            .as_ref()
            .and_then(|id| self.known_task(id));

        let Some(task) = selected else {
            return v_flex()
                .w(relative(share))
                .min_w_0()
                .h_full()
                .items_center()
                .justify_center()
                .border_l_1()
                .border_color(rgb(t.border))
                .child(
                    div()
                        .text_size(ui_text_md(cx))
                        .text_color(rgb(t.text_muted))
                        .child("Select a task."),
                )
                .into_any_element();
        };

        let links = self.links_for(&task, cx);
        let busy = self.tasks.starting.is_some();
        let is_starting = self.tasks.starting.as_deref() == Some(task.id.external_id.as_str());
        let state_label = if task.state_name.is_empty() {
            "—".to_string()
        } else {
            task.state_name.clone()
        };
        let task_for_start = task.clone();

        let action = match links.open_target.clone() {
            // A session already exists for this task: starting another would
            // create a second set of worktrees on the same branch.
            Some(project_id) => div()
                .id("detail-open")
                .cursor_pointer()
                .px(px(14.0))
                .py(px(6.0))
                .rounded(px(4.0))
                .bg(rgb(t.bg_secondary))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .text_size(ui_text_md(cx))
                .text_color(rgb(t.text_primary))
                .child("Open session")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        this.open_session(project_id.clone(), cx);
                    }),
                ),
            None => div()
                .id("detail-start")
                .cursor_pointer()
                .px(px(14.0))
                .py(px(6.0))
                .rounded(px(4.0))
                .bg(if busy {
                    rgb(t.bg_secondary)
                } else {
                    rgb(t.button_primary_bg)
                })
                .when(!busy, |d| d.hover(|s| s.bg(rgb(t.button_primary_hover))))
                .text_size(ui_text_md(cx))
                .text_color(if busy {
                    rgb(t.text_secondary)
                } else {
                    rgb(t.button_primary_fg)
                })
                .child(if is_starting {
                    "Starting…"
                } else {
                    "Start work"
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _window, cx| {
                        this.open_start_form(&task_for_start, cx);
                    }),
                ),
        };

        let task_for_agent = task.clone();
        let starting_breakdown =
            self.tasks.breaking_down.as_deref() == Some(task.id.external_id.as_str());

        let mut body = v_flex()
            .id("task-detail-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .gap(px(10.0))
            .px(px(16.0))
            .py(px(14.0))
            .child(
                h_flex()
                    .gap(px(6.0))
                    .flex_wrap()
                    .items_center()
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_secondary))
                            .child(task.display_key.clone()),
                    )
                    .child(self.chip(task.kind.label().to_string(), kind_color(task.kind, &t), cx))
                    .child(self.chip(state_label, state_color(task.state, &t), cx))
                    // Pushed to the right edge: these act on the task as a
                    // whole, not on the chips beside them.
                    .child(div().flex_1().min_w_0())
                    .child({
                        let url = task.url.clone();
                        self.link_button(
                            format!("task-copy-{}", task.id.external_id),
                            "icons/copy.svg",
                            "Copy link",
                            move |_this, _, _window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                            },
                            cx,
                        )
                    })
                    .child({
                        let url = task.url.clone();
                        self.link_button(
                            format!("task-open-{}", task.id.external_id),
                            "icons/external-link.svg",
                            "Open in browser",
                            move |_this, _, _window, _cx| {
                                okena_core::process::open_url(&url);
                            },
                            cx,
                        )
                    }),
            )
            .child(
                div()
                    .text_size(ui_text(15.0, cx))
                    .text_color(rgb(t.text_primary))
                    .child(task.title.clone()),
            )
            .child(action);

        // Where it sits in the breakdown. The parent is clickable when okena
        // knows it; when it does not — a parent assigned to somebody else and
        // never fetched — the key still says what it is.
        if let Some(parent_id) = task.parent_id.as_ref() {
            body = body.child(self.detail_label("PARENT", cx));
            match self.known_task(parent_id) {
                Some(parent) => body = body.child(self.render_related_task(&parent, cx)),
                None => {
                    body = body.child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_secondary))
                            .child(
                                task.parent_key
                                    .clone()
                                    .unwrap_or_else(|| "A parent task".to_string()),
                            ),
                    );
                }
            }
        }

        let children = self.tasks.children.get(&task.id.external_id);
        let loading_children =
            self.tasks.children_loading.as_deref() == Some(task.id.external_id.as_str());
        body = body.child(self.detail_label_with_count("SUB-TASKS", children.map(|c| c.len()), cx));
        match children {
            Some(list) if !list.is_empty() => {
                for child in list {
                    body = body.child(self.render_related_task(child, cx));
                }
            }
            Some(_) => {
                body = body.child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child("None yet — break it down, or add one."),
                );
            }
            None => {
                body = body.child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child(if loading_children {
                            "Loading…"
                        } else {
                            "Not loaded."
                        }),
                );
            }
        }

        // The breakdown control belongs to the list it acts on, so it sits at
        // the foot of it. One slot, four states: open the agent that exists,
        // wait for the one starting, or start one — worded for whether there
        // is anything to refine.
        let has_children = children.is_some_and(|c| !c.is_empty());
        body = body.child(match links.helpers.first() {
            Some((id, _)) => {
                let open_id = id.clone();
                self.small_button(
                    "detail-open-breakdown",
                    "Open breakdown agent",
                    cx.listener(move |this, _, _window, cx| {
                        this.open_session(open_id.clone(), cx);
                    }),
                    cx,
                )
            }
            None if starting_breakdown => {
                self.small_button("detail-break-down", "Starting…", |_, _, _| {}, cx)
            }
            None => self.small_button(
                "detail-break-down",
                if has_children {
                    // Same agent, same tools: it lists what exists before
                    // adding, so refining is breaking down again with the
                    // children already there.
                    "Refine sub-tasks"
                } else {
                    "Break down with agent"
                },
                cx.listener(move |this, _, _window, cx| {
                    this.break_down_with_agent(&task_for_agent, cx);
                }),
                cx,
            ),
        });

        body = body.child(self.detail_label("BRANCH", cx)).child(
            div()
                .w_full()
                .min_w_0()
                .truncate()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_secondary))
                .child(task.branch_name.clone()),
        );

        if !task.labels.is_empty() {
            body = body.child(self.detail_label("LABELS", cx)).child(
                h_flex().gap(px(4.0)).flex_wrap().children(
                    task.labels
                        .iter()
                        .map(|l| self.chip(l.clone(), t.text_muted, cx))
                        .collect::<Vec<_>>(),
                ),
            );
        }

        // What okena itself has running for this task, which the provider
        // cannot know. With a session there is a great deal to say and the
        // block below says it; without one there is a single line, and the two
        // must not both appear.
        let session_info = links.session.as_ref().and_then(|id| {
            crate::views::agent_session::AgentSessionInfo::collect(
                self.workspace.read(cx),
                &self.terminals,
                id,
            )
        });
        // One list for every agent on this task, whatever it is doing. They
        // were two sections — the helper's name in one, the work session's
        // chips in another — which made the same question ("what is running
        // for this task?") take two places to answer.
        let mut agents: Vec<(String, bool)> = Vec::new();
        if let Some(id) = links.session.clone() {
            agents.push((id, true));
        }
        agents.extend(links.helpers.iter().map(|(id, _)| (id.clone(), false)));

        if agents.is_empty() {
            body = body.child(self.detail_label("IN OKENA", cx)).child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(if links.signals.linked {
                        "A worktree exists, with no agent session."
                    } else {
                        "No worktree or session yet."
                    }),
            );
        } else {
            body = body.child(self.detail_label("AGENTS", cx));
            for (id, is_work) in &agents {
                let Some(info) = crate::views::agent_session::AgentSessionInfo::collect(
                    self.workspace.read(cx),
                    &self.terminals,
                    id,
                ) else {
                    continue;
                };
                body = body.child(self.render_agent_row(&info, *is_work, cx));
            }
        }

        // Checkouts and output belong to the session doing the work; a
        // breakdown agent has neither.
        if let Some(info) = session_info.as_ref() {
            body = body.child(self.render_session_facts(info, cx));
        }

        if let Some(description) = task
            .description
            .as_ref()
            .map(|d| d.trim())
            .filter(|d| !d.is_empty())
        {
            body = body.child(self.detail_label("DESCRIPTION", cx)).child(
                div()
                    .w_full()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(description.to_string()),
            );
        }

        v_flex()
            .w(relative(share))
            .min_w_0()
            .h_full()
            .border_l_1()
            .border_color(rgb(t.border))
            .child(body)
            .into_any_element()
    }

    /// One agent working on this task: what it is, how it is doing, and a way
    /// into it.
    ///
    /// `is_work` separates the agent doing the task from one started about it —
    /// a breakdown, say. Both belong in the list, but conflating them would
    /// make "is anyone working on this?" unanswerable.
    fn render_agent_row(
        &self,
        info: &crate::views::agent_session::AgentSessionInfo,
        is_work: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);

        // Terminal state, not the agent's claim: one that stopped reporting
        // still shows as waiting when its prompt is waiting.
        let activity = info.activity();
        let mut chips = vec![
            self.chip(
                if is_work { "work" } else { "breakdown" }.to_string(),
                if is_work {
                    t.button_primary_bg
                } else {
                    t.warning
                },
                cx,
            ),
            self.chip(activity.label(), activity.color(&t), cx),
        ];
        if let Some(agent) = &info.agent {
            chips.push(self.chip(agent.clone(), t.text_secondary, cx));
        }
        if !info.mcp {
            // Explains a permanently empty PRODUCED list, and for a breakdown
            // agent it explains why no sub-tasks are appearing.
            chips.push(self.chip("no okena mcp".to_string(), t.text_muted, cx));
        }

        let open_id = info.project_id.clone();
        v_flex()
            .id(SharedString::from(format!(
                "task-agent-{}",
                info.project_id
            )))
            .cursor_pointer()
            .w_full()
            .min_w_0()
            .gap(px(3.0))
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(4.0))
            .bg(rgb(t.bg_secondary))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_primary))
                    .child(info.name.clone()),
            )
            .child(h_flex().gap(px(4.0)).flex_wrap().children(chips))
            .children(info.status.clone().map(|reported| {
                div()
                    .w_full()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(format!("\u{201c}{reported}\u{201d}"))
                    .into_any_element()
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.open_session(open_id.clone(), cx);
                }),
            )
            .into_any_element()
    }

    /// What okena's own session for this task is doing.
    ///
    /// Read through the shared `AgentSessionInfo`, so this says exactly what
    /// the session's own panel says rather than a second derivation of it.
    /// The session's own state is in the AGENTS list above; this is what the
    /// work left behind — its checkouts and its output.
    fn render_session_facts(
        &self,
        info: &crate::views::agent_session::AgentSessionInfo,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let mut out = v_flex().gap(px(6.0));

        out = out.child(self.detail_label("WORKTREES", cx));
        if info.workspaces.is_empty() {
            out = out.child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child("Runs directly in its project."),
            );
        }
        for w in &info.workspaces {
            let Some(summary) = crate::views::components::WorktreeSummary::collect(
                self.workspace.read(cx),
                &w.project_id,
            ) else {
                continue;
            };
            out = out.child(crate::views::components::render_worktree_card(
                &summary,
                |this, id, cx| this.open_session(id.to_string(), cx),
                |this, id, cx| this.open_diff(id, cx),
                cx,
            ));
        }

        out = out.child(self.detail_label("PRODUCED", cx));
        if info.assets.is_empty() {
            out = out.child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(if info.mcp {
                        "Nothing registered yet."
                    } else {
                        "Cannot report — no okena MCP."
                    }),
            );
        }
        for asset in &info.assets {
            out = out.child(
                v_flex()
                    .w_full()
                    .min_w_0()
                    .gap(px(1.0))
                    .px(px(8.0))
                    .py(px(5.0))
                    .rounded(px(4.0))
                    .bg(rgb(t.bg_secondary))
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
                    ),
            );
        }

        out.into_any_element()
    }

    /// A small icon button for a task's link actions.
    ///
    /// The permalink used to be printed in full: a long opaque string nobody
    /// reads and could not click. Two icons do what the text only hinted at.
    fn link_button(
        &self,
        id: String,
        icon: &'static str,
        tooltip: &'static str,
        on_click: impl Fn(&mut Self, &MouseDownEvent, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        div()
            .id(SharedString::from(id))
            .cursor_pointer()
            .flex_shrink_0()
            .size(px(22.0))
            .rounded(px(4.0))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path(icon)
                    .size(px(13.0))
                    .text_color(rgb(t.text_secondary)),
            )
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(tooltip).build(window, cx)
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event, window, cx| {
                    // These sit on rows that select or open a task; the button
                    // must do its own thing and nothing else.
                    cx.stop_propagation();
                    on_click(this, event, window, cx);
                }),
            )
            .into_any_element()
    }

    /// A heading with a count beside it, when there is one to give.
    fn detail_label_with_count(
        &self,
        label: &'static str,
        count: Option<usize>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        h_flex()
            .items_center()
            .justify_between()
            .pt(px(6.0))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(label),
            )
            .children(count.map(|n| {
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(format!("{n}"))
                    .into_any_element()
            }))
            .into_any_element()
    }

    fn detail_label(&self, label: &'static str, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .pt(px(6.0))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .child(label)
            .into_any_element()
    }

    // ─── Render ──────────────────────────────────────────────────────────────

    fn render_connect(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);
        let expired = self.tasks.connection == TaskAuthState::Expired;

        v_flex()
            .gap(px(10.0))
            .p(px(16.0))
            .max_w(px(560.0))
            .child(
                div()
                    .text_size(ui_text(13.0, cx))
                    .text_color(rgb(t.text_primary))
                    .child(if expired {
                        format!(
                            "Your {} credential was rejected. Paste a new key to reconnect.",
                            self.tasks.provider_display_name
                        )
                    } else {
                        format!(
                            "Connect {} to see your assigned tasks.",
                            self.tasks.provider_display_name
                        )
                    }),
            )
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child("Linear → Settings → Security & access → Personal API keys"),
            )
            .child(
                okena_ui::input::input_container(&t, None)
                    .w_full()
                    .px(px(8.0))
                    .py(px(5.0))
                    .child(
                        SimpleInput::new(&self.tasks.api_key_input).text_size(ui_text(13.0, cx)),
                    ),
            )
            .child(
                div()
                    .id("tasks-connect")
                    .cursor_pointer()
                    .w(px(96.0))
                    .px(px(12.0))
                    .py(px(5.0))
                    .rounded(px(4.0))
                    .bg(rgb(t.button_primary_bg))
                    .hover(|s| s.bg(rgb(t.button_primary_hover)))
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(t.button_primary_fg))
                    .child(if self.tasks.loading {
                        "Verifying…"
                    } else {
                        "Connect"
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _window, cx| this.connect(cx)),
                    ),
            )
    }

    fn render_task_row(&self, row: &TaskRow, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let task = &row.task;
        let depth = row.depth;
        let t = theme(cx);
        let links = self.links_for(task, cx);
        let collapsed = self.tasks.collapsed.contains(&task.id.external_id);
        let state_label = if task.state_name.is_empty() {
            "—".to_string()
        } else {
            task.state_name.clone()
        };

        // Indent per level, with a rail on nested rows so containment is
        // visible rather than implied by a few pixels of whitespace.
        let indent = 16.0 * depth as f32;
        let selected = self.tasks.selected.as_deref() == Some(task.id.external_id.as_str());
        let select_id = task.id.external_id.clone();
        h_flex()
            .id(SharedString::from(format!(
                "task-row-{}",
                task.id.external_id
            )))
            .cursor_pointer()
            .justify_between()
            .items_start()
            .gap(px(12.0))
            .pl(px(12.0 + indent))
            .pr(px(12.0))
            .py(px(8.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .when(selected, |d| d.bg(with_alpha(t.button_primary_bg, 0.14)))
            .when(!selected, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.select_task(select_id.clone(), cx);
                }),
            )
            .when(depth > 0, |d| {
                d.border_l_2()
                    .border_color(with_alpha(t.border_active, 0.5))
            })
            .child(
                // `min_w_0` is load-bearing: a flex child defaults to a minimum
                // width of its content, so a long title would widen this column
                // past the lane and push the row's controls out of view instead
                // of wrapping or truncating.
                v_flex()
                    .gap(px(3.0))
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(
                        h_flex()
                            .gap(px(8.0))
                            .items_center()
                            .flex_shrink_0()
                            .child(if row.has_children {
                                let id = task.id.external_id.clone();
                                div()
                                    .id(SharedString::from(format!("fold-{id}")))
                                    .cursor_pointer()
                                    .w(px(12.0))
                                    .flex_shrink_0()
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(if collapsed { "▸" } else { "▾" })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _window, cx| {
                                            // Folding a subtree is not selecting
                                            // the row it hangs off.
                                            cx.stop_propagation();
                                            if !this.tasks.collapsed.remove(&id) {
                                                this.tasks.collapsed.insert(id.clone());
                                            }
                                            cx.notify();
                                        }),
                                    )
                                    .into_any_element()
                            } else {
                                // Reserve the same width so keys stay aligned
                                // whether or not a row can fold.
                                div().w(px(12.0)).flex_shrink_0().into_any_element()
                            })
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(t.text_secondary))
                                    .child(task.display_key.clone()),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .bg(with_alpha(kind_color(task.kind, &t), 0.15))
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(kind_color(task.kind, &t)))
                                    .child(task.kind.label()),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .bg(with_alpha(state_color(task.state, &t), 0.15))
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(state_color(task.state, &t)))
                                    .child(state_label),
                            )
                            // The parent's key, so a sub-task is readable on its
                            // own row even when the parent sits in another lane
                            // or isn't assigned to you at all.
                            .children((links.agents_running > 0).then(|| {
                                div()
                                    .flex_shrink_0()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .bg(with_alpha(t.success, 0.15))
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(t.success))
                                    .child(format!("{} agent(s)", links.agents_running))
                                    .into_any_element()
                            }))
                            .children(task.parent_key.as_ref().map(|key| {
                                div()
                                    .flex_shrink_0()
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(format!("↳ {key}"))
                                    .into_any_element()
                            })),
                    )
                    .child(
                        div()
                            .w_full()
                            .overflow_hidden()
                            .text_ellipsis()
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
                            .child(format!("branch: {}", task.branch_name)),
                    ),
            )
            .child({
                let url = task.url.clone();
                self.link_button(
                    format!("row-open-{}", task.id.external_id),
                    "icons/external-link.svg",
                    "Open in browser",
                    move |_this, _, _window, _cx| {
                        okena_core::process::open_url(&url);
                    },
                    cx,
                )
            })
    }

    /// The "Start work" dialog: projects, branch name, agent.
    fn render_start_form(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let form = self.tasks.start_form.as_ref()?;
        let t = theme(cx);
        let selected = form.project_ids.clone();
        let chosen_agent = form.agent.clone();

        // Worktree children can't parent another worktree, so only top-level
        // projects are offered.
        let projects: Vec<(String, String)> = self
            .workspace
            .read(cx)
            .projects()
            .iter()
            .filter(|p| p.worktree_info.is_none())
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();

        let project_chips: Vec<AnyElement> = projects
            .into_iter()
            .map(|(id, name)| {
                let is_selected = selected.contains(&id);
                let id_for_click = id.clone();
                div()
                    .id(SharedString::from(format!("sw-proj-{id}")))
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(rgb(if is_selected {
                        t.border_active
                    } else {
                        t.border
                    }))
                    .when(is_selected, |d| d.bg(with_alpha(t.button_primary_bg, 0.15)))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(if is_selected {
                        t.text_primary
                    } else {
                        t.text_secondary
                    }))
                    .child(name)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            if let Some(form) = this.tasks.start_form.as_mut() {
                                if let Some(pos) =
                                    form.project_ids.iter().position(|p| *p == id_for_click)
                                {
                                    form.project_ids.remove(pos);
                                } else {
                                    form.project_ids.push(id_for_click.clone());
                                }
                                cx.notify();
                            }
                        }),
                    )
                    .into_any_element()
            })
            .collect();

        // "None" first so creating worktrees without an agent stays one click.
        let mut agent_options: Vec<Option<String>> = vec![None];
        for name in AGENT_CHOICES {
            agent_options.push(Some((*name).to_string()));
        }
        // A configured agent that isn't in the built-in list must still be
        // selectable, or the daemon's own default would be unreachable here.
        if let Some(default) = self.tasks.default_agent.as_ref()
            && !AGENT_CHOICES.contains(&default.as_str())
        {
            agent_options.push(Some(default.clone()));
        }

        let agent_chips: Vec<AnyElement> = agent_options
            .into_iter()
            .map(|option| {
                let is_selected = chosen_agent == option;
                let label = option.clone().unwrap_or_else(|| "No agent".to_string());
                let for_click = option.clone();
                div()
                    .id(SharedString::from(format!("sw-agent-{label}")))
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(rgb(if is_selected {
                        t.border_active
                    } else {
                        t.border
                    }))
                    .when(is_selected, |d| d.bg(with_alpha(t.button_primary_bg, 0.15)))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(if is_selected {
                        t.text_primary
                    } else {
                        t.text_secondary
                    }))
                    .child(label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _window, cx| {
                            if let Some(form) = this.tasks.start_form.as_mut() {
                                form.agent = for_click.clone();
                                cx.notify();
                            }
                        }),
                    )
                    .into_any_element()
            })
            .collect();

        let count = selected.len();
        let summary = match count {
            0 => "No projects selected".to_string(),
            1 => "1 worktree".to_string(),
            n => format!("{n} worktrees · agent session rooted above them"),
        };

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(with_alpha(0x000000, 0.45))
                // Swallow clicks on the backdrop so they can't reach the board
                // behind it.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    v_flex()
                        .w(px(520.0))
                        .max_h(px(560.0))
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(rgb(t.border))
                        .bg(rgb(t.bg_primary))
                        .child(
                            v_flex()
                                .px(px(16.0))
                                .py(px(12.0))
                                .gap(px(2.0))
                                .border_b_1()
                                .border_color(rgb(t.border))
                                .child(
                                    div()
                                        .text_size(ui_text(14.0, cx))
                                        .text_color(rgb(t.text_primary))
                                        .child(format!("Start work on {}", form.task.display_key)),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .text_size(ui_text_ms(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child(form.task.title.clone()),
                                ),
                        )
                        .child(
                            v_flex()
                                .id("start-form-body")
                                .flex_1()
                                .overflow_y_scroll()
                                .p(px(16.0))
                                .gap(px(14.0))
                                .child(
                                    v_flex()
                                        .gap(px(5.0))
                                        .child(self.form_label("Projects", cx))
                                        .child(
                                            h_flex()
                                                .gap(px(6.0))
                                                .flex_wrap()
                                                .children(project_chips),
                                        )
                                        .child(
                                            div()
                                                .text_size(ui_text_ms(cx))
                                                .text_color(rgb(t.text_muted))
                                                .child(summary),
                                        ),
                                )
                                .child(
                                    v_flex()
                                        .gap(px(5.0))
                                        .child(self.form_label("Branch / worktree name", cx))
                                        // Wrapped in `input_container` so it
                                        // reads as an editable field; a bare
                                        // SimpleInput draws no border or
                                        // background and looks like static text.
                                        .child(
                                            okena_ui::input::input_container(&t, None)
                                                .w_full()
                                                .px(px(8.0))
                                                .py(px(5.0))
                                                .child(
                                                    SimpleInput::new(&form.branch_input)
                                                        .text_size(ui_text(13.0, cx)),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .text_size(ui_text_ms(cx))
                                                .text_color(rgb(t.text_muted))
                                                .child(
                                                    "Used for every selected project. \
                                                     The provider's own name keeps its \
                                                     branch-to-issue link working.",
                                                ),
                                        ),
                                )
                                .child(
                                    v_flex()
                                        .gap(px(5.0))
                                        .child(self.form_label("Agent", cx))
                                        .child(
                                            h_flex().gap(px(6.0)).flex_wrap().children(agent_chips),
                                        ),
                                ),
                        )
                        .child(
                            h_flex()
                                .justify_end()
                                .gap(px(8.0))
                                .px(px(16.0))
                                .py(px(12.0))
                                .border_t_1()
                                .border_color(rgb(t.border))
                                .child(self.small_button(
                                    "sw-cancel",
                                    "Cancel",
                                    cx.listener(|this, _, _window, cx| this.close_start_form(cx)),
                                    cx,
                                ))
                                .child(
                                    div()
                                        .id("sw-confirm")
                                        .cursor_pointer()
                                        .px(px(12.0))
                                        .py(px(4.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(t.button_primary_bg))
                                        .hover(|s| s.bg(rgb(t.button_primary_hover)))
                                        .text_size(ui_text_md(cx))
                                        .text_color(rgb(t.button_primary_fg))
                                        .child("Start")
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _window, cx| {
                                                this.confirm_start(cx)
                                            }),
                                        ),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn form_label(&self, text: &str, cx: &Context<Self>) -> AnyElement {
        let t = theme(cx);
        div()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_secondary))
            .child(text.to_string())
            .into_any_element()
    }

    pub(super) fn render_tasks_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let connected = self.tasks.connection.is_connected();

        if !connected {
            return v_flex()
                .size_full()
                .children(self.tasks.error.clone().map(|e| self.error_banner(e, cx)))
                .child(self.render_connect(cx))
                .into_any_element();
        }

        let account = match &self.tasks.connection {
            TaskAuthState::Connected { account } => account.clone(),
            _ => None,
        };
        // The board renders the rows itself; only emptiness matters here.
        // Building a throwaway element per task would render every row twice.
        let has_rows = !self.tasks.tasks.is_empty();
        let loading = self.tasks.loading;

        let account_label = match &account {
            Some(name) => format!("{} · {name}", self.tasks.provider_display_name),
            None => self.tasks.provider_display_name.clone(),
        };
        let actions: Vec<AnyElement> = vec![
            div()
                .flex_shrink_0()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child(account_label)
                .into_any_element(),
            self.small_button(
                "tasks-new",
                "New task",
                cx.listener(|this, _, _window, cx| this.open_new_task(cx)),
                cx,
            ),
            self.small_button(
                "tasks-refresh",
                if loading { "Refreshing…" } else { "Refresh" },
                cx.listener(|this, _, _window, cx| this.refresh_tasks(cx)),
                cx,
            ),
            // Connecting and disconnecting live in Settings now: they are
            // configuration, and having them here as well meant two
            // implementations of the same thing.
            self.toolbar_icon(
                "tasks-settings",
                "icons/settings.svg",
                "Task manager settings",
                cx.listener(|this, _, _window, cx| this.open_settings("tasks", cx)),
                cx,
            ),
        ];

        // The form takes the whole view while it is open, the way drafting a
        // spec replaces the Specs view: filling it in is a different job from
        // scanning the board.
        if self.tasks.new_task.is_some() {
            return v_flex()
                .size_full()
                .child(self.render_toolbar(actions, cx))
                .child(self.render_new_task_form(cx))
                .into_any_element();
        }

        v_flex()
            .size_full()
            .child(self.render_toolbar(actions, cx))
            .children(self.tasks.status.clone().map(|m| self.info_banner(m, cx)))
            .children(self.tasks.error.clone().map(|e| self.error_banner(e, cx)))
            .child(if has_rows {
                let (todo, staged) = self.board(cx);
                let fraction = self.tasks.lane_fraction;
                let board_width = self.board_width.clone();
                let active_drag = self.active_drag.clone();

                h_flex()
                    .id("tasks-board")
                    .flex_1()
                    .min_h_0()
                    // Measure the board so a drag can be converted into a
                    // fraction; without the width, a pixel delta means nothing.
                    .child(
                        canvas(
                            move |bounds, _window, _cx| {
                                *board_width.borrow_mut() = f32::from(bounds.size.width);
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(self.render_task_list(todo, staged, fraction, cx))
                    .child({
                        let width = self.board_width.clone();
                        ResizeHandle::new(false, t.border, t.border_active, move |pos, _cx| {
                            *active_drag.borrow_mut() = Some(DragState::HarnessLane {
                                initial_mouse_x: f32::from(pos.x),
                                initial_fraction: fraction,
                                total_width: *width.borrow(),
                            });
                        })
                    })
                    .child(self.render_task_detail(1.0 - fraction, cx))
                    .into_any_element()
            } else {
                div()
                    .px(px(12.0))
                    .py(px(20.0))
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(if loading {
                        "Loading tasks…"
                    } else {
                        "No open tasks assigned to you."
                    })
                    .into_any_element()
            })
            .children(self.render_start_form(cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod lane_tests {
    // Explicit imports: `use super::*` would pull in the `gpui::*` glob, whose
    // `test` macro shadows the built-in one and recurses forever.
    use super::{Lane, Stage, TaskSignals, lane_for};

    #[test]
    fn untouched_task_is_a_todo() {
        assert_eq!(lane_for(false, TaskSignals::default()), Lane::Todo);
    }

    #[test]
    fn provider_in_progress_counts_even_with_no_session() {
        // Someone may be working on it outside okena.
        assert_eq!(
            lane_for(true, TaskSignals::default()),
            Lane::InProgress(Stage::Active)
        );
    }

    #[test]
    fn a_linked_session_moves_a_todo_into_progress() {
        // okena's own signal wins: a worktree exists, so it is underway even if
        // Linear still says Todo.
        let s = TaskSignals {
            linked: true,
            ..Default::default()
        };
        assert_eq!(lane_for(false, s), Lane::InProgress(Stage::Active));
    }

    #[test]
    fn waiting_for_input_needs_attention() {
        let s = TaskSignals {
            linked: true,
            waiting: true,
            open_pr: false,
        };
        assert_eq!(lane_for(false, s), Lane::InProgress(Stage::NeedsAttention));
    }

    #[test]
    fn an_open_pr_is_ready_for_testing() {
        let s = TaskSignals {
            linked: true,
            waiting: false,
            open_pr: true,
        };
        assert_eq!(lane_for(false, s), Lane::InProgress(Stage::ReadyForTesting));
    }

    #[test]
    fn blocked_beats_having_a_pr() {
        // A raised PR does not matter while the agent is stuck at a prompt.
        let s = TaskSignals {
            linked: true,
            waiting: true,
            open_pr: true,
        };
        assert_eq!(lane_for(false, s), Lane::InProgress(Stage::NeedsAttention));
    }

    #[test]
    fn stage_order_is_most_urgent_first() {
        assert_eq!(
            Stage::all(),
            [Stage::NeedsAttention, Stage::ReadyForTesting, Stage::Active]
        );
    }
}

#[cfg(test)]
mod lane_size_tests {
    use super::super::MIN_LANE_FRACTION;

    /// Mirrors `HarnessPane::set_lane_fraction`'s clamp, which needs a GPUI
    /// context and so can't be called directly here.
    fn clamp(f: f32) -> f32 {
        f.clamp(MIN_LANE_FRACTION, 1.0 - MIN_LANE_FRACTION)
    }

    #[test]
    fn a_lane_cannot_be_dragged_shut() {
        // Collapsing a lane to zero would hide its tasks with no way back.
        assert_eq!(clamp(0.0), MIN_LANE_FRACTION);
        assert_eq!(clamp(-3.0), MIN_LANE_FRACTION);
    }

    #[test]
    fn the_other_lane_cannot_be_dragged_shut_either() {
        assert_eq!(clamp(1.0), 1.0 - MIN_LANE_FRACTION);
        assert_eq!(clamp(4.2), 1.0 - MIN_LANE_FRACTION);
    }

    #[test]
    fn ordinary_positions_pass_through() {
        assert_eq!(clamp(0.5), 0.5);
        assert_eq!(clamp(0.25), 0.25);
    }

    #[test]
    fn both_lanes_always_fit() {
        // The lanes are sized `fraction` and `1 - fraction`, so any clamped
        // value must leave both above the minimum.
        for step in 0..=100 {
            let f = clamp(step as f32 / 100.0);
            assert!(f >= MIN_LANE_FRACTION, "todo lane too small at {f}");
            assert!(
                1.0 - f >= MIN_LANE_FRACTION - f32::EPSILON,
                "other lane too small at {f}"
            );
        }
    }
}

#[cfg(test)]
mod hierarchy_tests {
    // Explicit imports: the `gpui::*` glob shadows `#[test]` with `gpui::test`.
    use super::order_by_hierarchy;
    use okena_core::tasks::{Task, TaskId, TaskKind, TaskState};

    fn task(id: &str, parent: Option<&str>) -> Task {
        Task {
            id: TaskId::new("linear", id),
            display_key: id.to_uppercase(),
            title: id.into(),
            description: None,
            state: TaskState::Todo,
            state_name: "Todo".into(),
            url: String::new(),
            branch_name: String::new(),
            updated_at: String::new(),
            kind: TaskKind::Task,
            parent_id: parent.map(str::to_string),
            parent_key: parent.map(str::to_uppercase),
            labels: Vec::new(),
        }
    }

    use std::collections::HashSet;

    fn ordered(tasks: Vec<Task>) -> Vec<super::TaskRow> {
        order_by_hierarchy(tasks, &HashSet::new())
    }

    fn ids(rows: &[super::TaskRow]) -> Vec<String> {
        rows.iter().map(|r| r.task.id.external_id.clone()).collect()
    }

    fn depths(rows: &[super::TaskRow]) -> Vec<usize> {
        rows.iter().map(|r| r.depth).collect()
    }

    #[test]
    fn children_follow_their_parent() {
        let ordered = ordered(vec![
            task("a", None),
            task("b", None),
            task("a1", Some("a")),
        ]);
        assert_eq!(ids(&ordered), ["a", "a1", "b"]);
    }

    #[test]
    fn parents_keep_their_incoming_order() {
        // The list arrives newest-activity-first; grouping must not resort it.
        let ordered = ordered(vec![task("b", None), task("a", None)]);
        assert_eq!(ids(&ordered), ["b", "a"]);
    }

    #[test]
    fn a_child_whose_parent_is_absent_stays_top_level() {
        // The parent may be in another lane, or not assigned to you at all.
        let ordered = ordered(vec![task("x", Some("missing"))]);
        assert_eq!(ids(&ordered), ["x"]);
    }

    #[test]
    fn no_task_is_ever_dropped() {
        let input = vec![
            task("a", None),
            task("a1", Some("a")),
            task("a2", Some("a")),
            task("orphan", Some("elsewhere")),
        ];
        let n = input.len();
        assert_eq!(ordered(input).len(), n);
    }

    #[test]
    fn a_whole_subtree_stays_together() {
        // The bug this replaced: a one-level pass emitted the epic and its
        // features, then dumped the features' stories at the very end.
        let ordered = ordered(vec![
            task("epic", None),
            task("feat", Some("epic")),
            task("story", Some("feat")),
            task("other", None),
        ]);
        assert_eq!(ids(&ordered), ["epic", "feat", "story", "other"]);
        assert_eq!(depths(&ordered), [0, 1, 2, 0]);
    }

    #[test]
    fn a_parent_cycle_terminates_and_keeps_every_task() {
        // Provider data could name each other as parent; the render must not
        // hang or drop rows.
        let mut a = task("a", Some("b"));
        let b = task("b", Some("a"));
        a.parent_id = Some("b".into());
        let ordered = ordered(vec![a, b]);
        assert_eq!(ordered.len(), 2);
    }

    #[test]
    fn a_collapsed_parent_hides_its_subtree_but_stays_visible() {
        let mut collapsed = HashSet::new();
        collapsed.insert("epic".to_string());
        let rows = order_by_hierarchy(
            vec![
                task("epic", None),
                task("feat", Some("epic")),
                task("story", Some("feat")),
                task("other", None),
            ],
            &collapsed,
        );
        // The epic itself remains — collapsing hides descendants, not the row.
        assert_eq!(ids(&rows), ["epic", "other"]);
    }

    #[test]
    fn collapsing_a_middle_level_keeps_its_ancestors() {
        let mut collapsed = HashSet::new();
        collapsed.insert("feat".to_string());
        let rows = order_by_hierarchy(
            vec![
                task("epic", None),
                task("feat", Some("epic")),
                task("story", Some("feat")),
            ],
            &collapsed,
        );
        assert_eq!(ids(&rows), ["epic", "feat"]);
    }

    #[test]
    fn only_parents_are_foldable() {
        let rows = ordered(vec![task("epic", None), task("feat", Some("epic"))]);
        assert!(rows[0].has_children, "an epic with a child folds");
        assert!(!rows[1].has_children, "a leaf does not");
    }

    #[test]
    fn a_collapsed_parent_still_reports_children() {
        // Otherwise its chevron would vanish once collapsed, with no way back.
        let mut collapsed = HashSet::new();
        collapsed.insert("epic".to_string());
        let rows = order_by_hierarchy(
            vec![task("epic", None), task("feat", Some("epic"))],
            &collapsed,
        );
        assert!(rows[0].has_children);
    }

    #[test]
    fn siblings_stay_in_order_under_their_parent() {
        let ordered = ordered(vec![
            task("a", None),
            task("a1", Some("a")),
            task("a2", Some("a")),
        ]);
        assert_eq!(ids(&ordered), ["a", "a1", "a2"]);
    }
}
