//! Terminal pane view - composition of child entity views.

pub mod url_detector;
mod scrollbar;
mod search_bar;
mod content;
mod actions;
mod zoom;
mod navigation;
mod render;

use content::TerminalContentEvent;
use search_bar::{SearchBar, SearchBarEvent};

pub use content::TerminalContent;

use crate::ActionDispatch;
use crate::terminal_view_settings;
use okena_terminal::backend::TerminalBackend;
use okena_terminal::shell_config::ShellType;
use okena_terminal::terminal::{Terminal, TerminalSize};
use okena_terminal::TerminalsRegistry;
use okena_workspace::focus::FocusManager;
use okena_workspace::request_broker::RequestBroker;
use okena_workspace::state::{WindowId, Workspace};
use gpui::*;
use std::sync::Arc;
use std::time::Duration;

/// A terminal pane view composed of child entity views.
pub struct TerminalPane<D: ActionDispatch> {
    // Identity
    workspace: Entity<Workspace>,
    pub(super) focus_manager: Entity<FocusManager>,
    request_broker: Entity<RequestBroker>,
    pub(super) window_id: WindowId,
    project_id: String,
    project_path: String,
    layout_path: Vec<usize>,

    // Terminal state
    terminal: Option<Arc<Terminal>>,
    terminal_id: Option<String>,
    backend: Arc<dyn TerminalBackend>,
    terminals: TerminalsRegistry,

    // Child views
    content: Entity<TerminalContent>,
    search_bar: Entity<SearchBar>,

    // Focus
    focus_handle: FocusHandle,
    pending_focus: bool,

    // State
    minimized: bool,
    detached: bool,
    cursor_visible: bool,
    shell_type: ShellType,
    was_focused: bool,

    // Action dispatcher (local or remote)
    pub(super) action_dispatcher: Option<D>,
}

impl<D: ActionDispatch + Send + Sync> TerminalPane<D> {
    // GPUI view constructor: each param is a distinct injected dependency.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        workspace: Entity<Workspace>,
        focus_manager: Entity<FocusManager>,
        request_broker: Entity<RequestBroker>,
        window_id: WindowId,
        project_id: String,
        project_path: String,
        layout_path: Vec<usize>,
        terminal_id: Option<String>,
        minimized: bool,
        detached: bool,
        backend: Arc<dyn TerminalBackend>,
        terminals: TerminalsRegistry,
        action_dispatcher: Option<D>,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        let shell_type = workspace
            .read(cx)
            .get_terminal_shell(&project_id, &layout_path)
            .unwrap_or(ShellType::Default);

        let content = cx.new(|cx| {
            TerminalContent::new(
                focus_handle.clone(),
                Some(window_id),
                project_id.clone(),
                layout_path.clone(),
                workspace.clone(),
                request_broker.clone(),
                cx,
            )
        });

        let search_bar = cx.new(|cx| SearchBar::new(workspace.clone(), focus_manager.clone(), cx));

        cx.subscribe(&search_bar, Self::handle_search_bar_event).detach();
        cx.subscribe(&content, Self::handle_content_event).detach();

        let mut pane = Self {
            workspace,
            focus_manager,
            request_broker,
            window_id,
            project_id,
            project_path,
            layout_path,
            terminal: None,
            terminal_id,
            backend,
            terminals,
            content,
            search_bar,
            focus_handle,
            pending_focus: false,
            minimized,
            detached,
            cursor_visible: true,
            shell_type,
            was_focused: false,
            action_dispatcher,
        };

        if let Some(ref id) = pane.terminal_id {
            pane.create_terminal_for_existing_pty(id.clone(), cx);
        } else {
            pane.create_new_terminal(cx);
        }

        if pane.terminal_id.as_deref().is_some_and(|id| id.starts_with("remote:")) {
            pane.start_remote_dirty_check_loop(cx);
        }
        pane.start_cursor_blink_loop(cx);
        pane.start_idle_check_loop(cx);

        pane
    }

    fn handle_search_bar_event(
        &mut self,
        _: Entity<SearchBar>,
        event: &SearchBarEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            SearchBarEvent::Closed => {
                self.content.update(cx, |content, _| {
                    content.set_search_highlights(Arc::new(Vec::new()), None);
                });
                self.pending_focus = true;
                cx.notify();
            }
            SearchBarEvent::MatchesChanged(matches, idx) => {
                self.content.update(cx, |content, _| {
                    content.set_search_highlights(matches.clone(), *idx);
                });
                cx.notify();
            }
        }
    }

    fn handle_content_event(
        &mut self,
        _: Entity<TerminalContent>,
        event: &TerminalContentEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            TerminalContentEvent::RequestContextMenu { position, has_selection, link_url } => {
                if let Some(ref terminal_id) = self.terminal_id {
                    self.request_broker.update(cx, |broker, cx| {
                        broker.push_overlay_request(
                            okena_workspace::requests::OverlayRequest::Project(okena_workspace::requests::ProjectOverlay {
                                project_id: self.project_id.clone(),
                                kind: okena_workspace::requests::ProjectOverlayKind::TerminalContextMenu {
                                    terminal_id: terminal_id.clone(),
                                    layout_path: self.layout_path.clone(),
                                    position: *position,
                                    has_selection: *has_selection,
                                    link_url: link_url.clone(),
                                },
                            }),
                            cx,
                        );
                    });
                }
            }
        }
    }

    fn start_remote_dirty_check_loop(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<TerminalPane<D>>, cx| {
            let interval = Duration::from_millis(8);
            loop {
                smol::Timer::after(interval).await;
                let result = this.update(cx, |pane, cx| {
                    if let Some(terminal) = pane.terminal.as_ref()
                        && terminal.take_dirty() {
                        // Parse the freshly-arrived bytes up front so derived
                        // state (bell, waiting) is current before the frame is
                        // built. Otherwise the lazy parse inside the content
                        // child's `with_content` runs *after* the pane border
                        // and sidebar read `has_bell()`, leaving those server-
                        // driven indicators stale until local input forces a
                        // second repaint (issue #128).
                        terminal.process_pending_output();
                        pane.content.update(cx, |_, cx| cx.notify());
                    }
                });
                if result.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn start_cursor_blink_loop(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<TerminalPane<D>>, cx| {
            let interval = Duration::from_millis(500);
            loop {
                smol::Timer::after(interval).await;

                let result = this.update(cx, |pane, cx| {
                    // App-set DECSCUSR blinking wins over the user setting.
                    let blink_enabled = pane
                        .terminal
                        .as_ref()
                        .and_then(|t| t.app_cursor_blinking())
                        .unwrap_or_else(|| crate::terminal_view_settings(cx).cursor_blink);

                    if blink_enabled {
                        if !pane.was_focused {
                            if !pane.cursor_visible {
                                pane.cursor_visible = true;
                                pane.content.update(cx, |content, _| {
                                    content.set_cursor_visible(true);
                                });
                            }
                            return;
                        }
                        pane.cursor_visible = !pane.cursor_visible;
                        pane.content.update(cx, |content, cx| {
                            content.set_cursor_visible(pane.cursor_visible);
                            cx.notify();
                        });
                    } else if !pane.cursor_visible {
                        pane.cursor_visible = true;
                        pane.content.update(cx, |content, cx| {
                            content.set_cursor_visible(true);
                            cx.notify();
                        });
                    }
                });

                if result.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn start_idle_check_loop(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<TerminalPane<D>>, cx| {
            let interval = Duration::from_secs(2);
            let mut was_waiting = false;
            loop {
                smol::Timer::after(interval).await;

                let check_info = this.update(cx, |pane, cx| {
                    let idle_timeout = crate::terminal_view_settings(cx).idle_timeout_secs;
                    if idle_timeout == 0 {
                        return None;
                    }
                    pane.terminal.as_ref().map(|t| {
                        let idle_threshold = Duration::from_secs(idle_timeout as u64);
                        let is_idle = t.last_output_time().elapsed() >= idle_threshold;
                        let pid = t.shell_pid();
                        let had_input = t.had_user_input();
                        let has_unseen = t.has_unseen_output();
                        (t.clone(), is_idle, pid, had_input, has_unseen)
                    })
                });

                let check_info = match check_info {
                    Ok(Some(info)) => info,
                    Ok(None) => {
                        if was_waiting {
                            was_waiting = false;
                            let _ = this.update(cx, |pane, cx| {
                                if let Some(ref t) = pane.terminal {
                                    t.set_waiting_for_input(false);
                                }
                                cx.notify();
                            });
                        }
                        continue;
                    }
                    Err(_) => break,
                };

                let (terminal, is_idle, pid, had_input, has_unseen) = check_info;

                if !had_input || !has_unseen {
                    if was_waiting {
                        was_waiting = false;
                        terminal.set_waiting_for_input(false);
                        let _ = this.update(cx, |_pane, cx| { cx.notify(); });
                    }
                    continue;
                }

                let has_children = if let Some(pid) = pid {
                    smol::unblock(move || okena_terminal::terminal::has_child_processes(pid)).await
                } else {
                    false
                };

                let is_waiting = is_idle && !has_children;

                terminal.set_waiting_for_input(is_waiting);
                if is_waiting != was_waiting {
                    was_waiting = is_waiting;
                    let _ = this.update(cx, |_pane, cx| {
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn create_terminal_for_existing_pty(&mut self, terminal_id: String, cx: &mut Context<Self>) {
        let existing = self.terminals.lock().get(&terminal_id).cloned();
        if let Some(terminal) = existing {
            if let Some(pid) = self.backend.get_shell_pid(&terminal_id) {
                terminal.set_shell_pid(pid);
            }
            self.terminal = Some(terminal.clone());
            self.update_child_terminals(terminal, cx);
            return;
        }

        let settings = terminal_view_settings(cx);
        let ws = self.workspace.read(cx);
        let shell = self.shell_type.clone().resolve_default(
            ws.project(&self.project_id).and_then(|p| p.default_shell.as_ref()),
            &settings.default_shell,
        );

        match self
            .backend
            .reconnect_terminal(&terminal_id, &self.project_path, Some(&shell))
        {
            Ok(_) => {}
            Err(e) => {
                log::error!("Failed to reconnect terminal {}: {}", terminal_id, e);
            }
        }

        let size = TerminalSize::default();
        let terminal = Arc::new(Terminal::new(terminal_id.clone(), size, self.backend.transport(), self.project_path.clone()));
        if let Some(pid) = self.backend.get_foreground_shell_pid(&terminal_id) {
            terminal.set_shell_pid(pid);
        }
        self.terminals.lock().insert(terminal_id, terminal.clone());
        self.terminal = Some(terminal.clone());
        self.update_child_terminals(terminal, cx);
    }

    fn create_new_terminal(&mut self, _cx: &mut Context<Self>) {
        log::warn!(
            "TerminalPane rendered without daemon-assigned terminal id for project {} at {:?}",
            self.project_id,
            self.layout_path
        );
    }

    fn update_child_terminals(&mut self, terminal: Arc<Terminal>, cx: &mut Context<Self>) {
        crate::register_content_pane(
            terminal.terminal_id.clone(),
            self.content.downgrade(),
        );

        self.content.update(cx, |content, cx| {
            content.set_terminal(Some(terminal.clone()), cx);
        });
        self.search_bar.update(cx, |search_bar, _| {
            search_bar.set_terminal(Some(terminal));
        });
    }

    pub fn terminal_id(&self) -> Option<String> {
        self.terminal_id.clone()
    }

    pub fn set_detached(&mut self, detached: bool, cx: &mut Context<Self>) {
        if self.detached != detached {
            self.detached = detached;
            if detached {
                self.deregister_resize_viewer(cx);
            }
            cx.notify();
        }
    }

    pub fn set_minimized(&mut self, minimized: bool, cx: &mut Context<Self>) {
        if self.minimized != minimized {
            self.minimized = minimized;
            if minimized {
                self.deregister_resize_viewer(cx);
            }
            cx.notify();
        }
    }

    pub(super) fn deregister_resize_viewer(&mut self, cx: &mut Context<Self>) {
        self.content.update(cx, |content, _| {
            content.deregister_resize_viewer();
        });
    }

    fn id_suffix(&self) -> String {
        self.terminal_id.clone().unwrap_or_else(|| {
            format!(
                "{}-{}",
                self.project_id,
                self.layout_path
                    .iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join("-")
            )
        })
    }

}

impl<D: ActionDispatch> Drop for TerminalPane<D> {
    fn drop(&mut self) {
        // Remove this pane from the spatial navigation map so stale entries
        // don't linger after a terminal is closed.
        crate::layout::navigation::deregister_pane_bounds(self.window_id, &self.project_id, &self.layout_path);
    }
}

impl<D: ActionDispatch + Send + Sync> gpui::Focusable for TerminalPane<D> {
    fn focus_handle(&self, _cx: &gpui::App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}
