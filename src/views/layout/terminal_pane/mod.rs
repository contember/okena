//! Terminal pane view - composition of child entity views.
//!
//! This is the main TerminalPane that composes:
//! - TerminalHeader: header with name, shell selector, and controls
//! - TerminalContent: terminal display with scrollbar
//! - SearchBar: search functionality
//!
//! Each component is a proper GPUI Entity implementing Render.

mod url_detector;
mod scrollbar;
mod shell_selector;
mod search_bar;
mod header;
mod content;
mod actions;
mod zoom;
mod navigation;
mod render;

// Internal imports
use content::ContextMenuEvent;
use search_bar::{SearchBar, SearchBarEvent};
use header::{TerminalHeader, HeaderEvent};

// Re-export TerminalContent (used by tests/internal consumers)
pub use content::TerminalContent;

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
    header: Entity<TerminalHeader>,
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
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        // Read shell_type from workspace state
        let shell_type = workspace
            .read(cx)
            .get_terminal_shell(&project_id, &layout_path)
            .unwrap_or(ShellType::Default);

        let id_suffix = terminal_id.clone().unwrap_or_else(|| {
            format!(
                "{}-{}",
                project_id,
                layout_path
                    .iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join("-")
            )
        });

        // Create child entities
        let header = cx.new(|cx| {
            TerminalHeader::new(
                workspace.clone(),
                project_id.clone(),
                terminal_id.clone(),
                shell_type.clone(),
                backend.supports_buffer_capture(),
                backend.is_remote(),
                id_suffix.clone(),
                cx,
            )
        });

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

        // Subscribe to header events
        cx.subscribe(&header, Self::handle_header_event).detach();

        // Subscribe to search bar events
        cx.subscribe(&search_bar, Self::handle_search_bar_event).detach();

        // Subscribe to content events (context menu actions)
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
            header,
            content,
            search_bar,
            focus_handle,
            pending_focus: false,
            minimized,
            detached,
            cursor_visible: true,
            shell_type,
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

        pane
    }

    // === Event handlers ===

    /// Handle events from header.
    fn handle_header_event(
        &mut self,
        _: Entity<TerminalHeader>,
        event: &HeaderEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            HeaderEvent::Split(dir) => self.handle_split(*dir, cx),
            HeaderEvent::AddTab => self.handle_add_tab(cx),
            HeaderEvent::Close => self.handle_close(cx),
            HeaderEvent::Minimize => self.handle_minimize(cx),
            HeaderEvent::Fullscreen => self.handle_fullscreen(cx),
            HeaderEvent::Detach => self.handle_detach(cx),
            HeaderEvent::ExportBuffer => self.handle_export_buffer(cx),
            HeaderEvent::Renamed(name) => self.handle_rename(name.clone(), cx),
            HeaderEvent::OpenShellSelector(current_shell) => {
                if let Some(ref terminal_id) = self.terminal_id {
                    self.request_broker.update(cx, |broker, cx| {
                        broker.push_overlay_request(crate::workspace::requests::OverlayRequest::ShellSelector {
                            project_id: self.project_id.clone(),
                            terminal_id: terminal_id.clone(),
                            current_shell: current_shell.clone(),
                        }, cx);
                    });
                }
            }
        }
    }

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

    /// Handle events from content (context menu actions).
    fn handle_content_event(
        &mut self,
        _: Entity<TerminalContent>,
        event: &ContextMenuEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            ContextMenuEvent::Copy => self.handle_copy(cx),
            ContextMenuEvent::Paste => self.handle_paste(cx),
            ContextMenuEvent::Clear => self.handle_clear(cx),
            ContextMenuEvent::SelectAll => self.handle_select_all(cx),
            ContextMenuEvent::Split(dir) => self.handle_split(*dir, cx),
            ContextMenuEvent::Close => self.handle_close(cx),
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

    // === Terminal creation ===

    /// Create terminal for existing PTY.
    fn create_terminal_for_existing_pty(&mut self, terminal_id: String, cx: &mut Context<Self>) {
        let existing = self.terminals.lock().get(&terminal_id).cloned();
        if let Some(terminal) = existing {
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
        self.terminals.lock().insert(terminal_id, terminal.clone());
        self.terminal = Some(terminal.clone());
        self.update_child_terminals(terminal, cx);
    }

    /// Create new terminal.
    fn create_new_terminal(&mut self, cx: &mut Context<Self>) {
        let shell = if self.shell_type == ShellType::Default {
            settings(cx).default_shell.clone()
        } else {
            self.shell_type.clone()
        };

        match self
            .backend
            .create_terminal(&self.project_path, Some(&shell))
        {
            Ok(terminal_id) => {
                self.terminal_id = Some(terminal_id.clone());
                self.workspace.update(cx, |ws, cx| {
                    ws.set_terminal_id(&self.project_id, &self.layout_path, terminal_id.clone(), cx);
                });

                let size = TerminalSize::default();
                let terminal =
                    Arc::new(Terminal::new(terminal_id.clone(), size, self.backend.transport(), self.project_path.clone()));
                self.terminals.lock().insert(terminal_id.clone(), terminal.clone());
                self.terminal = Some(terminal.clone());

                // Update child entities
                self.update_child_terminals(terminal, cx);
                self.header.update(cx, |header, _| {
                    header.set_terminal_id(Some(terminal_id));
                });

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
            search_bar.set_terminal(Some(terminal.clone()));
        });
        self.header.update(cx, |header, _| {
            header.set_terminal(Some(terminal));
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

    /// Check if terminal is in a tab group.
    fn is_in_tab_group(&self, cx: &Context<Self>) -> bool {
        if self.layout_path.is_empty() {
            return false;
        }
        let parent_path = &self.layout_path[..self.layout_path.len() - 1];
        let ws = self.workspace.read(cx);
        if let Some(project) = ws.project(&self.project_id) {
            if let Some(crate::workspace::state::LayoutNode::Tabs { .. }) =
                project.layout.as_ref().and_then(|l| l.get_at_path(parent_path))
            {
                return true;
            }
        }
        false
    }
}

impl_focusable!(TerminalPane);
