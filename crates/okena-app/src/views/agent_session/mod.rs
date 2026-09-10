//! The agent-session panel — one view of a coding-agent session, shown wherever
//! a session appears.
//!
//! There used to be two of these: a sidebar beside a session's terminal and a
//! card in the Agents overview, separately written and already diverging. This
//! is the single one, rendered at two densities so it fits both a full-height
//! sidebar and a narrow lane.
//!
//! It is an entity rather than a render helper because its hosts are different
//! types — the window and the harness pane — and the actions it offers (focus a
//! worktree, restart the agent, delete the workspace) need a workspace, a focus
//! manager and a daemon client of their own.

mod detect;
mod model;
mod render;

pub use detect::{AGENT_COMMANDS, detect_agent};
pub use model::{AgentSessionInfo, AgentSessionKind, RelatedWorkspace, session_kind};

use crate::workspace::focus::FocusManager;
use crate::workspace::state::{WindowId, Workspace};
use gpui::*;
use okena_terminal::TerminalsRegistry;

/// Where the panel is being shown, and so how much it draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelDensity {
    /// A lane in the Agents overview, where several sessions sit side by side:
    /// its own header with Info/Terminal tabs, and no destructive action —
    /// tearing down a workspace from a card you are scanning past is too easy
    /// to do by accident.
    Compact,
    /// Inside a project column, in place of its terminal. No header of its own:
    /// the column already names the session and owns the toggle that got you
    /// here, and a second header would just repeat it.
    Embedded,
}

impl PanelDensity {
    /// Whether the panel draws its own header. Only false where the host
    /// already has one.
    fn has_header(self) -> bool {
        !matches!(self, PanelDensity::Embedded)
    }

    /// Whether the destructive teardown is offered here.
    ///
    /// The column is the one place a session is shown on its own, full height,
    /// having been deliberately opened — the overview's lanes are for scanning
    /// across sessions, which is the wrong place to offer deleting one.
    fn allows_delete(self) -> bool {
        matches!(self, PanelDensity::Embedded)
    }
}

/// What a compact panel is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PanelTab {
    #[default]
    Info,
    Terminal,
}

/// Everything the panel needs from its host.
#[derive(Clone)]
pub struct AgentPanelContext {
    pub client: okena_transport::remote_action::RemoteActionClient,
    /// Lets the panel open a worktree's diff without knowing which window it
    /// is in.
    pub request_broker: Entity<okena_workspace::request_broker::RequestBroker>,
    pub workspace: Entity<Workspace>,
    pub focus_manager: Entity<FocusManager>,
    pub window_id: WindowId,
    pub terminals: TerminalsRegistry,
}

pub struct AgentSessionPanel {
    client: okena_transport::remote_action::RemoteActionClient,
    request_broker: Entity<okena_workspace::request_broker::RequestBroker>,
    workspace: Entity<Workspace>,
    focus_manager: Entity<FocusManager>,
    window_id: WindowId,
    terminals: TerminalsRegistry,
    /// The session this panel describes.
    project_id: String,
    density: PanelDensity,
    /// Info or the live terminal. Only meaningful at compact density; the full
    /// sidebar sits beside a terminal already.
    tab: PanelTab,
    /// Two-step delete: the first click arms, the second confirms. `Some(force)`
    /// while armed.
    pending_delete: Option<bool>,
    /// The embedded terminal, keyed by id so it is rebuilt only when the
    /// session's terminal actually changes — rebuilding each frame would drop
    /// scrollback and selection.
    terminal: Option<EmbeddedTerminal>,
    /// Resolves the transport a session's terminal rides on. `None` in hosts
    /// that never show the terminal tab.
    remote_manager: Option<Entity<okena_remote_client::RemoteConnectionManager>>,
    terminal_focus: FocusHandle,
}

struct EmbeddedTerminal {
    terminal_id: String,
    terminal: std::sync::Arc<okena_terminal::terminal::Terminal>,
    content: Entity<okena_views_terminal::layout::terminal_pane::TerminalContent>,
}

impl AgentSessionPanel {
    pub fn new(
        project_id: String,
        density: PanelDensity,
        ctx: AgentPanelContext,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            client: ctx.client,
            request_broker: ctx.request_broker,
            workspace: ctx.workspace,
            focus_manager: ctx.focus_manager,
            window_id: ctx.window_id,
            terminals: ctx.terminals,
            project_id,
            density,
            tab: PanelTab::default(),
            pending_delete: None,
            terminal: None,
            remote_manager: None,
            terminal_focus: cx.focus_handle(),
        }
    }

    /// Let this panel show the session's live terminal.
    pub fn with_terminals(
        mut self,
        remote_manager: Option<Entity<okena_remote_client::RemoteConnectionManager>>,
    ) -> Self {
        self.remote_manager = remote_manager;
        self
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    /// Point an existing panel at a different session.
    ///
    /// Reused rather than rebuilt so the sidebar keeps its scroll position when
    /// focus moves between sessions; the per-session state that must not carry
    /// over is reset here.
    pub fn set_project(&mut self, project_id: String, cx: &mut Context<Self>) {
        if self.project_id == project_id {
            return;
        }
        self.project_id = project_id;
        // Both belong to the session that just left: an armed delete would
        // otherwise point at the newly-shown one.
        self.pending_delete = None;
        self.terminal = None;
        cx.notify();
    }

    /// Everything shown, read fresh from the workspace mirror each frame.
    fn info(&self, cx: &App) -> Option<AgentSessionInfo> {
        AgentSessionInfo::collect(self.workspace.read(cx), &self.terminals, &self.project_id)
    }

    /// Show what changed in a worktree.
    fn open_diff(&mut self, project_id: &str, cx: &mut Context<Self>) {
        self.request_broker.update(cx, |broker, cx| {
            broker.push_overlay_request(
                okena_workspace::requests::OverlayRequest::Project(
                    okena_workspace::requests::ProjectOverlay {
                        project_id: project_id.to_string(),
                        kind: okena_workspace::requests::ProjectOverlayKind::DiffViewer {
                            file: None,
                            mode: None,
                            commit_message: None,
                            commits: None,
                            commit_index: None,
                        },
                    },
                ),
                cx,
            );
        });
    }

    /// Focus a project in the terminal workspace, leaving any harness view.
    fn open_project(&mut self, project_id: String, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| {
                ws.set_focused_project_individual(fm, Some(project_id.clone()), cx);
            });
            cx.notify();
        });
        okena_workspace::harness_state::set_active_harness(self.window_id, None, cx);
        cx.notify();
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
    /// chosen when the session was started.
    fn restart_agent(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let daemon_id =
            okena_transport::client::strip_prefix(&self.project_id, client.connection_id());
        let existing: Vec<String> = self
            .workspace
            .read(cx)
            .project(&self.project_id)
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

    /// Stop the agent without tearing anything down.
    ///
    /// Closes the session's terminals, which ends their tmux sessions and with
    /// them the agent processes. The session project, its worktrees and any
    /// work on disk all stay — "Start agent" brings it back.
    ///
    /// Separate from deleting because they are different intents that were
    /// previously only reachable as one: stopping a runaway agent should not
    /// require destroying the checkouts it was working in.
    fn stop_agent(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let daemon_id =
            okena_transport::client::strip_prefix(&self.project_id, client.connection_id());
        let terminals: Vec<String> = self
            .workspace
            .read(cx)
            .project(&self.project_id)
            .and_then(|p| p.layout.as_ref())
            .map(|l| l.collect_terminal_ids())
            .unwrap_or_default()
            .iter()
            .map(|id| okena_transport::client::strip_prefix(id, client.connection_id()))
            .collect();
        if terminals.is_empty() {
            return;
        }

        cx.spawn(async move |_this, cx| {
            let result = smol::unblock(move || {
                client.post_action(okena_core::api::ActionRequest::CloseTerminals {
                    project_id: daemon_id,
                    terminal_ids: terminals,
                })
            })
            .await;

            if let Err(error) = result {
                cx.update(|cx| {
                    crate::views::panels::toast::ToastManager::error(
                        format!("Could not stop the agent: {error}"),
                        cx,
                    );
                });
            }
        })
        .detach();
    }

    /// Tear down the whole session workspace.
    ///
    /// Only ever called from the confirmation step: it deletes checkouts, and
    /// with `force` it discards uncommitted work in them.
    fn delete_workspace(&mut self, force: bool, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let daemon_id =
            okena_transport::client::strip_prefix(&self.project_id, client.connection_id());
        self.pending_delete = None;
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
}

// ── The terminal tab ─────────────────────────────────────────────────────────
//
// Only reachable at compact density, where the panel replaces a terminal the
// user would otherwise have to leave the overview to see.

impl AgentSessionPanel {
    /// Resolve the transport carrying this session's terminals.
    fn transport(
        &self,
        cx: &App,
    ) -> Option<std::sync::Arc<dyn okena_terminal::terminal::TerminalTransport>> {
        let connection_id = self
            .workspace
            .read(cx)
            .project(&self.project_id)?
            .connection_id
            .clone()?;
        let backend = self
            .remote_manager
            .as_ref()?
            .read(cx)
            .backend_for(&connection_id)?;
        Some(backend.transport())
    }

    /// Build the embedded terminal, or drop it when the session's terminal has.
    ///
    /// Keyed by terminal id: rebuilding on every frame would throw away the
    /// scrollback and any selection the user is part-way through.
    pub(super) fn sync_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(terminal_id) =
            AgentSessionInfo::visible_terminal_id(self.workspace.read(cx), &self.project_id)
        else {
            self.terminal = None;
            return;
        };
        if self
            .terminal
            .as_ref()
            .is_some_and(|t| t.terminal_id == terminal_id)
        {
            return;
        }
        let Some(transport) = self.transport(cx) else {
            self.terminal = None;
            return;
        };

        let (layout_path, project_path) = {
            let ws = self.workspace.read(cx);
            match ws.project(&self.project_id) {
                Some(p) => (
                    p.layout
                        .as_ref()
                        .and_then(|l| l.find_terminal_path(&terminal_id))
                        .unwrap_or_default(),
                    p.path.clone(),
                ),
                None => (Vec::new(), String::new()),
            }
        };

        let terminal =
            okena_views_terminal::overlays::terminal_overlay_utils::get_or_create_terminal(
                &terminal_id,
                &transport,
                &self.terminals,
                &project_path,
            );
        // A local broker keeps TerminalContent self-contained: this panel has
        // no overlay manager observing requests, so a file-viewer request from
        // a Ctrl+click would otherwise reach nothing.
        let request_broker = cx.new(|_| okena_workspace::request_broker::RequestBroker::new());
        let content =
            okena_views_terminal::overlays::terminal_overlay_utils::create_terminal_content(
                cx,
                self.terminal_focus.clone(),
                self.project_id.clone(),
                layout_path,
                self.workspace.clone(),
                request_broker,
                terminal.clone(),
            );

        self.terminal = Some(EmbeddedTerminal {
            terminal_id,
            terminal,
            content,
        });
    }

    pub(super) fn render_terminal(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        use gpui::prelude::*;
        let t = crate::theme::theme(cx);
        let Some(term) = &self.terminal else {
            return gpui_component::v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(crate::ui::tokens::ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child("No terminal in this session."),
                )
                .into_any_element();
        };
        let terminal = term.terminal.clone();
        div()
            .id(SharedString::from(format!(
                "agent-term-{}",
                self.project_id
            )))
            .flex_1()
            .min_h_0()
            .bg(rgb(t.bg_primary))
            .track_focus(&self.terminal_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.terminal_focus, cx);
                }),
            )
            // Forwarded here rather than relying on the panel's own handling:
            // without this the agent cannot be answered, which is the point of
            // showing its terminal instead of a link to it.
            // A listener rather than a bare closure: key encoding reads the
            // terminal view settings (Option-as-Meta among them), which need a
            // context to resolve.
            .on_key_down(cx.listener(move |_this, event, _window, cx| {
                okena_views_terminal::overlays::terminal_overlay_utils::handle_terminal_key_input(
                    &terminal, event, cx,
                );
            }))
            .child(
                AnyView::from(term.content.clone()).cached(StyleRefinement::default().size_full()),
            )
            .into_any_element()
    }
}
