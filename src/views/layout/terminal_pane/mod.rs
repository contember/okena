//! Terminal pane view - composition of child entity views.
//!
//! This is the main TerminalPane that composes:
//! - TerminalContent: terminal display with scrollbar
//! - SearchBar: search functionality
//!
//! The tab bar (name, shell selector, action buttons) is rendered by
//! LayoutContainer, not by TerminalPane.
//!
//! Each component is a proper GPUI Entity implementing Render.

pub(crate) mod url_detector;
mod scrollbar;
mod search_bar;
mod content;
mod actions;
mod zoom;
mod navigation;
mod render;

// Internal imports
use content::TerminalContentEvent;
use search_bar::{SearchBar, SearchBarEvent};

// Re-export TerminalContent (used by tests/internal consumers)
pub use content::TerminalContent;

use crate::action_dispatch::ActionDispatcher;
use crate::settings::settings;
use crate::terminal::backend::TerminalBackend;
use crate::terminal::shell_config::ShellType;
use crate::terminal::terminal::{Terminal, TerminalSize};
use crate::views::panels::toast::ToastManager;
use crate::views::root::TerminalsRegistry;
use crate::workspace::request_broker::RequestBroker;
use crate::workspace::state::Workspace;
use gpui::*;
use std::sync::Arc;
use std::time::Duration;

/// A terminal pane view composed of child entity views.
pub struct TerminalPane {
    // Identity
    workspace: Entity<Workspace>,
    request_broker: Entity<RequestBroker>,
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
    pub(super) action_dispatcher: Option<ActionDispatcher>,
}

impl TerminalPane {
    pub fn new(
        workspace: Entity<Workspace>,
        request_broker: Entity<RequestBroker>,
        project_id: String,
        project_path: String,
        layout_path: Vec<usize>,
        terminal_id: Option<String>,
        minimized: bool,
        detached: bool,
        backend: Arc<dyn TerminalBackend>,
        terminals: TerminalsRegistry,
        action_dispatcher: Option<ActionDispatcher>,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        // Read shell_type from workspace state
        let shell_type = workspace
            .read(cx)
            .get_terminal_shell(&project_id, &layout_path)
            .unwrap_or(ShellType::Default);

        // Create child entities
        let content = cx.new(|cx| {
            TerminalContent::new(
                focus_handle.clone(),
                project_id.clone(),
                layout_path.clone(),
                workspace.clone(),
                cx,
            )
        });

        let search_bar = cx.new(|cx| SearchBar::new(workspace.clone(), cx));

        // Subscribe to search bar events
        cx.subscribe(&search_bar, Self::handle_search_bar_event).detach();

        // Subscribe to content events (context menu request)
        cx.subscribe(&content, Self::handle_content_event).detach();

        let mut pane = Self {
            workspace,
            request_broker,
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

        // Create terminal: either reconnect to existing PTY or create new one
        if let Some(ref id) = pane.terminal_id {
            pane.create_terminal_for_existing_pty(id.clone(), cx);
        } else {
            // No terminal ID - create a new terminal immediately
            pane.create_new_terminal(cx);
        }

        // Start background loops
        pane.start_dirty_check_loop(cx);
        pane.start_cursor_blink_loop(cx);
        pane.start_idle_check_loop(cx);

        pane
    }

    // === Event handlers ===

    /// Handle events from search bar.
    fn handle_search_bar_event(
        &mut self,
        _: Entity<SearchBar>,
        event: &SearchBarEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            SearchBarEvent::Closed => {
                // Focus will be handled in next render cycle
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

    /// Handle events from content (context menu request).
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
                            crate::workspace::requests::OverlayRequest::TerminalContextMenu {
                                terminal_id: terminal_id.clone(),
                                project_id: self.project_id.clone(),
                                layout_path: self.layout_path.clone(),
                                position: *position,
                                has_selection: *has_selection,
                                link_url: link_url.clone(),
                            },
                            cx,
                        );
                    });
                }
            }
        }
    }

    // === Background loops ===

    /// Start dirty check loop.
    fn start_dirty_check_loop(&self, cx: &mut Context<Self>) {
        use std::time::Duration;

        cx.spawn(async move |this: WeakEntity<TerminalPane>, cx| {
            let interval = Duration::from_millis(8);
            loop {
                smol::Timer::after(interval).await;

                let should_notify = this.update(cx, |pane, _cx| {
                    if let Some(ref terminal) = pane.terminal {
                        terminal.take_dirty()
                    } else {
                        false
                    }
                });

                match should_notify {
                    Ok(true) => {
                        let _ = this.update(cx, |_pane, cx| {
                            cx.notify();
                        });
                    }
                    Ok(false) => {}
                    Err(_) => break,
                }
            }
        })
        .detach();
    }

    /// Start cursor blink loop.
    fn start_cursor_blink_loop(&self, cx: &mut Context<Self>) {
        use std::time::Duration;

        cx.spawn(async move |this: WeakEntity<TerminalPane>, cx| {
            let interval = Duration::from_millis(500);
            loop {
                smol::Timer::after(interval).await;

                let result = this.update(cx, |pane, cx| {
                    if settings(cx).cursor_blink {
                        pane.cursor_visible = !pane.cursor_visible;
                        pane.content.update(cx, |content, _| {
                            content.set_cursor_visible(pane.cursor_visible);
                        });
                        cx.notify();
                    } else if !pane.cursor_visible {
                        pane.cursor_visible = true;
                        pane.content.update(cx, |content, _| {
                            content.set_cursor_visible(true);
                        });
                        cx.notify();
                    }
                });

                if result.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Start idle check loop — polls terminal idle state every 2 seconds.
    /// Runs the pgrep check on a background thread via smol::unblock to avoid
    /// blocking the GPUI thread. Only triggers re-render on state transitions.
    fn start_idle_check_loop(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<TerminalPane>, cx| {
            let interval = Duration::from_secs(2);
            let mut was_waiting = false;
            loop {
                smol::Timer::after(interval).await;

                // Step 1: gather data from the main thread (cheap, no subprocess)
                let check_info = this.update(cx, |pane, cx| {
                    let idle_timeout = settings(cx).idle_timeout_secs;
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
                        // Feature disabled or no terminal — clear waiting state
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
                    Err(_) => break, // Entity dropped
                };

                let (terminal, is_idle, pid, had_input, has_unseen) = check_info;

                // Skip terminals the user has never interacted with (fresh/untouched)
                // or terminals where the user already saw the last output
                if !had_input || !has_unseen {
                    if was_waiting {
                        was_waiting = false;
                        terminal.set_waiting_for_input(false);
                        let _ = this.update(cx, |_pane, cx| { cx.notify(); });
                    }
                    continue;
                }

                // Step 2: run pgrep on a background thread (expensive, off main thread)
                let has_children = if let Some(pid) = pid {
                    smol::unblock(move || crate::terminal::terminal::has_child_processes(pid)).await
                } else {
                    false
                };

                // Step 3: compute waiting state
                // Flag as waiting if: idle + no child processes running
                let is_waiting = is_idle && !has_children;

                // Step 4: update cache and notify on transitions or while waiting
                // (continuous notify while waiting keeps the duration display updated)
                terminal.set_waiting_for_input(is_waiting);
                if is_waiting || is_waiting != was_waiting {
                    was_waiting = is_waiting;
                    let _ = this.update(cx, |_pane, cx| {
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    // === Terminal creation ===

    /// Create terminal for existing PTY.
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

        let shell = if self.shell_type == ShellType::Default {
            settings(cx).default_shell.clone()
        } else {
            self.shell_type.clone()
        };

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
        if let Some(pid) = self.backend.get_shell_pid(&terminal_id) {
            terminal.set_shell_pid(pid);
        }
        self.terminals.lock().insert(terminal_id, terminal.clone());
        self.terminal = Some(terminal.clone());
        self.update_child_terminals(terminal, cx);
    }

    /// Create new terminal.
    fn create_new_terminal(&mut self, cx: &mut Context<Self>) {
        // Remote terminals arrive via state sync with terminal_id already set.
        // Local PTY creation would fail for remote backends, so bail out early.
        if self.backend.is_remote() {
            return;
        }

        let shell = if self.shell_type == ShellType::Default {
            settings(cx).default_shell.clone()
        } else {
            self.shell_type.clone()
        };

        // Read fresh path from workspace state (handles tilde-expanded paths)
        let project_path = self.workspace.read(cx)
            .project(&self.project_id)
            .map(|p| p.path.clone())
            .unwrap_or_else(|| self.project_path.clone());

        match self
            .backend
            .create_terminal(&project_path, Some(&shell))
        {
            Ok(terminal_id) => {
                self.terminal_id = Some(terminal_id.clone());
                self.workspace.update(cx, |ws, cx| {
                    ws.set_terminal_id(&self.project_id, &self.layout_path, terminal_id.clone(), cx);
                });

                let size = TerminalSize::default();
                let terminal =
                    Arc::new(Terminal::new(terminal_id.clone(), size, self.backend.transport(), project_path));
                if let Some(pid) = self.backend.get_shell_pid(&terminal_id) {
                    terminal.set_shell_pid(pid);
                }
                self.terminals.lock().insert(terminal_id.clone(), terminal.clone());
                self.terminal = Some(terminal.clone());

                // Update child entities
                self.update_child_terminals(terminal, cx);

                self.pending_focus = true;
                cx.notify();
            }
            Err(e) => {
                log::error!("Failed to create terminal: {}", e);
                ToastManager::error(format!("Failed to create terminal: {}", e), cx);
            }
        }
    }

    /// Update terminal reference in child entities.
    fn update_child_terminals(&mut self, terminal: Arc<Terminal>, cx: &mut Context<Self>) {
        self.content.update(cx, |content, cx| {
            content.set_terminal(Some(terminal.clone()), cx);
        });
        self.search_bar.update(cx, |search_bar, _| {
            search_bar.set_terminal(Some(terminal));
        });
    }

    // === Public accessors ===

    /// Get terminal ID.
    pub fn terminal_id(&self) -> Option<String> {
        self.terminal_id.clone()
    }

    /// Set detached state.
    pub fn set_detached(&mut self, detached: bool, cx: &mut Context<Self>) {
        if self.detached != detached {
            self.detached = detached;
            cx.notify();
        }
    }

    /// Set minimized state.
    pub fn set_minimized(&mut self, minimized: bool, cx: &mut Context<Self>) {
        if self.minimized != minimized {
            self.minimized = minimized;
            cx.notify();
        }
    }

    // === Helpers ===

    /// Get ID suffix for element IDs.
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

impl_focusable!(TerminalPane);
