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

// Internal imports
use content::{ContextMenuEvent, TerminalContent};
use search_bar::{SearchBar, SearchBarEvent};
use header::{TerminalHeader, HeaderEvent};

use crate::keybindings::{
    AddTab, CloseSearch, CloseTerminal, Copy, FocusDown, FocusLeft, FocusNextTerminal,
    FocusPrevTerminal, FocusRight, FocusUp, MinimizeTerminal, Paste, Search, SearchNext,
    SearchPrev, SendBacktab, SendTab, SplitHorizontal, SplitVertical, ToggleFullscreen,
};
use crate::settings::settings;
use crate::terminal::input::key_to_bytes;
use crate::terminal::pty_manager::PtyManager;
use crate::terminal::shell_config::ShellType;
use crate::terminal::terminal::{Terminal, TerminalSize};
use crate::theme::theme;
use crate::views::layout::navigation::{get_pane_map, NavigationDirection};
use crate::views::root::TerminalsRegistry;
use crate::workspace::state::{SplitDirection, Workspace};
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::sync::Arc;

/// A terminal pane view composed of child entity views.
pub struct TerminalPane {
    // Identity
    workspace: Entity<Workspace>,
    project_id: String,
    project_path: String,
    layout_path: Vec<usize>,

    // Terminal state
    terminal: Option<Arc<Terminal>>,
    terminal_id: Option<String>,
    pty_manager: Arc<PtyManager>,
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
        project_id: String,
        project_path: String,
        layout_path: Vec<usize>,
        terminal_id: Option<String>,
        minimized: bool,
        detached: bool,
        pty_manager: Arc<PtyManager>,
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
                pty_manager.supports_buffer_capture(),
                id_suffix.clone(),
                cx,
            )
        });

        let content = cx.new(|cx| {
            TerminalContent::new(
                focus_handle.clone(),
                project_id.clone(),
                layout_path.clone(),
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
            project_id,
            project_path,
            layout_path,
            terminal: None,
            terminal_id,
            pty_manager,
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

        // Create terminal if we have an ID
        if let Some(ref id) = pane.terminal_id {
            pane.create_terminal_for_existing_pty(id.clone(), cx);
        }

        // Start background loops
        pane.start_dirty_check_loop(cx);
        pane.start_cursor_blink_loop(cx);

        pane
    }

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
                    self.workspace.update(cx, |ws, cx| {
                        ws.request_shell_selector(
                            &self.project_id,
                            terminal_id,
                            current_shell.clone(),
                            cx,
                        );
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
            .pty_manager
            .create_or_reconnect_terminal_with_shell(Some(&terminal_id), &self.project_path, Some(&shell))
        {
            Ok(_) => {}
            Err(e) => {
                log::error!("Failed to reconnect terminal {}: {}", terminal_id, e);
            }
        }

        let size = TerminalSize::default();
        let terminal = Arc::new(Terminal::new(terminal_id.clone(), size, self.pty_manager.clone()));
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
            .pty_manager
            .create_terminal_with_shell(&self.project_path, Some(&shell))
        {
            Ok(terminal_id) => {
                self.terminal_id = Some(terminal_id.clone());
                self.workspace.update(cx, |ws, cx| {
                    ws.set_terminal_id(&self.project_id, &self.layout_path, terminal_id.clone(), cx);
                });

                let size = TerminalSize::default();
                let terminal =
                    Arc::new(Terminal::new(terminal_id.clone(), size, self.pty_manager.clone()));
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

    // === Actions ===

    fn handle_split(&mut self, direction: SplitDirection, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| {
            ws.split_terminal(&self.project_id, &self.layout_path, direction, cx);
        });
    }

    fn handle_add_tab(&mut self, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| {
            ws.add_tab(&self.project_id, &self.layout_path, cx);
        });
    }

    fn handle_close(&mut self, cx: &mut Context<Self>) {
        if let Some(ref id) = self.terminal_id {
            self.pty_manager.kill(id);
        }

        let layout_path = self.layout_path.clone();
        let project_id = self.project_id.clone();
        self.workspace.update(cx, |ws, cx| {
            ws.close_terminal_and_focus_sibling(&project_id, &layout_path, cx);
        });
    }

    fn handle_minimize(&mut self, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| {
            ws.toggle_terminal_minimized(&self.project_id, &self.layout_path, cx);
        });
    }

    fn handle_fullscreen(&mut self, cx: &mut Context<Self>) {
        if let Some(ref id) = self.terminal_id {
            self.workspace.update(cx, |ws, cx| {
                ws.set_fullscreen_terminal(self.project_id.clone(), id.clone(), cx);
            });
        }
    }

    fn handle_detach(&mut self, cx: &mut Context<Self>) {
        if self.terminal_id.is_some() {
            self.workspace.update(cx, |ws, cx| {
                ws.detach_terminal(&self.project_id, &self.layout_path, cx);
            });
        }
    }

    fn handle_export_buffer(&mut self, cx: &mut Context<Self>) {
        if let Some(ref terminal_id) = self.terminal_id {
            if let Some(path) = self.pty_manager.capture_buffer(terminal_id) {
                cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
            }
        }
    }

    fn handle_rename(&mut self, new_name: String, cx: &mut Context<Self>) {
        if let Some(ref terminal_id) = self.terminal_id {
            let project_id = self.project_id.clone();
            let terminal_id = terminal_id.clone();
            self.workspace.update(cx, |ws, cx| {
                ws.rename_terminal(&project_id, &terminal_id, new_name, cx);
            });
        }
    }

    fn handle_copy(&mut self, cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal {
            if let Some(text) = terminal.get_selected_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }
    }

    fn handle_paste(&mut self, cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal {
            if let Some(clipboard_item) = cx.read_from_clipboard() {
                if let Some(text) = clipboard_item.text() {
                    terminal.send_input(&text);
                }
            }
        }
    }

    fn handle_clear(&mut self, _cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal {
            terminal.clear();
        }
    }

    fn handle_select_all(&mut self, cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal {
            terminal.select_all();
            cx.notify();
        }
    }

    fn handle_file_drop(&mut self, paths: &ExternalPaths, _cx: &mut Context<Self>) {
        let Some(ref terminal) = self.terminal else {
            return;
        };

        for path in paths.paths() {
            let escaped_path = Self::shell_escape_path(path);
            terminal.send_input(&format!("{} ", escaped_path));
        }
    }

    fn shell_escape_path(path: &std::path::Path) -> String {
        let path_str = path.to_string_lossy();
        let mut escaped = String::with_capacity(path_str.len() * 2);

        for c in path_str.chars() {
            match c {
                ' ' | '(' | ')' | '[' | ']' | '{' | '}' | '\'' | '"' | '`' | '$' | '&' | '|'
                | ';' | '<' | '>' | '*' | '?' | '!' | '#' | '~' | '\\' => {
                    escaped.push('\\');
                    escaped.push(c);
                }
                _ => escaped.push(c),
            }
        }

        escaped
    }

    // === Navigation ===

    fn handle_navigation(
        &mut self,
        direction: NavigationDirection,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane_map = get_pane_map();

        let source = match pane_map.find_pane(&self.project_id, &self.layout_path) {
            Some(pane) => pane.clone(),
            None => return,
        };

        if let Some(target) = pane_map.find_nearest_in_direction(&source, direction) {
            self.workspace.update(cx, |ws, cx| {
                ws.set_focused_terminal(target.project_id.clone(), target.layout_path.clone(), cx);
            });
        }
    }

    fn handle_sequential_navigation(
        &mut self,
        next: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane_map = get_pane_map();

        let source = match pane_map.find_pane(&self.project_id, &self.layout_path) {
            Some(pane) => pane.clone(),
            None => return,
        };

        let target = if next {
            pane_map.find_next_pane(&source)
        } else {
            pane_map.find_prev_pane(&source)
        };

        if let Some(target) = target {
            self.workspace.update(cx, |ws, cx| {
                ws.set_focused_terminal(target.project_id.clone(), target.layout_path.clone(), cx);
            });
        }
    }

    // === Search ===

    fn start_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_bar.update(cx, |search_bar, cx| {
            search_bar.open(window, cx);
        });
        cx.notify();
    }

    fn close_search(&mut self, cx: &mut Context<Self>) {
        self.search_bar.update(cx, |search_bar, cx| {
            search_bar.close(cx);
        });
        cx.notify();
    }

    fn next_match(&mut self, cx: &mut Context<Self>) {
        self.search_bar.update(cx, |search_bar, cx| {
            search_bar.next_match(cx);
        });
    }

    fn prev_match(&mut self, cx: &mut Context<Self>) {
        self.search_bar.update(cx, |search_bar, cx| {
            search_bar.prev_match(cx);
        });
    }

    // === Key handling ===

    fn handle_key(&mut self, event: &KeyDownEvent, _cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal {
            if let Some(input) = key_to_bytes(event) {
                terminal.send_bytes(&input);
            }
        }
    }
}

impl Render for TerminalPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Create terminal if needed
        if self.terminal.is_none() && self.terminal_id.is_none() {
            self.create_new_terminal(cx);
        }

        let focus_handle = self.focus_handle.clone();
        let id_suffix = self.id_suffix();

        // Check focus state
        let is_modal = {
            let ws = self.workspace.read(cx);
            let is_modal = ws.focus_manager.is_modal();
            let header_renaming = self.header.read(cx).is_renaming();
            let search_active = self.search_bar.read(cx).is_active();

            if !header_renaming && !search_active && !is_modal {
                if let Some(focused) = ws.focus_manager.focused_terminal_state() {
                    if focused.project_id == self.project_id
                        && focused.layout_path == self.layout_path
                        && !focus_handle.is_focused(window)
                    {
                        self.pending_focus = true;
                    }
                }
            }
            is_modal
        };

        // Handle pending focus
        let header_renaming = self.header.read(cx).is_renaming();
        let search_active = self.search_bar.read(cx).is_active();
        if self.pending_focus
            && self.terminal.is_some()
            && !header_renaming
            && !search_active
            && !is_modal
        {
            self.pending_focus = false;
            window.focus(&self.focus_handle, cx);
        }

        let is_focused = focus_handle.is_focused(window);

        // Update content focus state
        self.content.update(cx, |content, _| {
            content.set_focused(is_focused);
        });

        // Bell handling
        let has_bell = self.terminal.as_ref().map_or(false, |t| t.has_bell());
        if is_focused && has_bell {
            if let Some(ref terminal) = self.terminal {
                terminal.clear_bell();
            }
        }

        let show_focused_border = settings(cx).show_focused_border;
        let show_border = (is_focused && show_focused_border) || has_bell;
        let border_color = if is_focused && show_focused_border {
            rgb(t.border_focused)
        } else {
            rgb(t.border_bell)
        };

        let in_tab_group = self.is_in_tab_group(cx);

        div()
            .id(format!("terminal-pane-main-{}", id_suffix))
            .track_focus(&focus_handle)
            .key_context("TerminalPane")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseDownEvent, window, cx| {
                    this.header.update(cx, |header, cx| {
                        header.close_shell_dropdown(cx);
                    });
                    window.focus(&this.focus_handle, cx);
                    this.workspace.update(cx, |ws, cx| {
                        ws.set_focused_terminal(
                            this.project_id.clone(),
                            this.layout_path.clone(),
                            cx,
                        );
                    });
                }),
            )
            // Actions
            .on_action(cx.listener(|this, _: &SplitVertical, _window, cx| {
                this.handle_split(SplitDirection::Vertical, cx);
            }))
            .on_action(cx.listener(|this, _: &SplitHorizontal, _window, cx| {
                this.handle_split(SplitDirection::Horizontal, cx);
            }))
            .on_action(cx.listener(|this, _: &AddTab, _window, cx| {
                this.handle_add_tab(cx);
            }))
            .on_action(cx.listener(|this, _: &CloseTerminal, _window, cx| {
                this.handle_close(cx);
            }))
            .on_action(cx.listener(|this, _: &MinimizeTerminal, _window, cx| {
                this.handle_minimize(cx);
            }))
            .on_action(cx.listener(|this, _: &Copy, _window, cx| {
                this.handle_copy(cx);
            }))
            .on_action(cx.listener(|this, _: &Paste, _window, cx| {
                this.handle_paste(cx);
            }))
            .on_action(cx.listener(|this, _: &Search, window, cx| {
                if !this.search_bar.read(cx).is_active() {
                    this.start_search(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CloseSearch, _window, cx| {
                if this.search_bar.read(cx).is_active() {
                    this.close_search(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SearchNext, _window, cx| {
                this.next_match(cx);
            }))
            .on_action(cx.listener(|this, _: &SearchPrev, _window, cx| {
                this.prev_match(cx);
            }))
            .on_action(cx.listener(|this, _: &FocusLeft, window, cx| {
                this.handle_navigation(NavigationDirection::Left, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusRight, window, cx| {
                this.handle_navigation(NavigationDirection::Right, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusUp, window, cx| {
                this.handle_navigation(NavigationDirection::Up, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusDown, window, cx| {
                this.handle_navigation(NavigationDirection::Down, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusNextTerminal, window, cx| {
                this.handle_sequential_navigation(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusPrevTerminal, window, cx| {
                this.handle_sequential_navigation(false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SendTab, _window, _cx| {
                if let Some(ref terminal) = this.terminal {
                    terminal.send_bytes(b"\t");
                }
            }))
            .on_action(cx.listener(|this, _: &SendBacktab, _window, _cx| {
                if let Some(ref terminal) = this.terminal {
                    terminal.send_bytes(b"\x1b[Z");
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleFullscreen, _window, cx| {
                let is_fullscreen = this.workspace.read(cx).fullscreen_terminal.is_some();
                if is_fullscreen {
                    this.workspace.update(cx, |ws, cx| {
                        ws.exit_fullscreen(cx);
                    });
                } else {
                    this.handle_fullscreen(cx);
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                this.handle_key(event, cx);
            }))
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(&this.focus_handle, cx);
                this.workspace.update(cx, |ws, cx| {
                    ws.set_focused_terminal(this.project_id.clone(), this.layout_path.clone(), cx);
                });
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, cx| {
                this.handle_file_drop(paths, cx);
            }))
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .min_w_0()
            .bg(rgb(t.bg_primary))
            .when(show_border, |d| d.border_1().border_color(border_color))
            .group("terminal-pane")
            .relative()
            // Header (hidden in tab groups)
            .when(!in_tab_group && !self.minimized, |el| {
                el.child(self.header.clone())
            })
            // Content (hidden when minimized or detached)
            .when(!self.minimized && !self.detached, |el| {
                el.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .overflow_hidden()
                        .child(self.content.clone()),
                )
            })
            // Search bar (when active)
            .when(search_active, |el: Stateful<Div>| {
                el.child(self.search_bar.clone())
            })
    }
}

impl_focusable!(TerminalPane);
