use crate::elements::terminal_element::{TerminalElement, SearchMatch, URLMatch};
use crate::keybindings::{CloseTerminal, AddTab, MinimizeTerminal, SplitHorizontal, SplitVertical, Copy, Paste, Search, SearchNext, SearchPrev, CloseSearch, FocusLeft, FocusRight, FocusUp, FocusDown, FocusNextTerminal, FocusPrevTerminal, SendTab, SendBacktab, ToggleFullscreen};
use crate::settings::settings;
use crate::terminal::input::key_to_bytes;
use crate::terminal::pty_manager::PtyManager;
use crate::terminal::terminal::{Terminal, TerminalSize};
use crate::theme::theme;
use crate::views::navigation::{NavigationDirection, get_pane_map, register_pane_bounds};
use crate::views::root::TerminalsRegistry;
use crate::workspace::state::{SplitDirection, Workspace};
use gpui::*;
use gpui::prelude::FluentBuilder;
use crate::views::simple_input::{SimpleInput, SimpleInputState};
use gpui_component::tooltip::Tooltip;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// A terminal pane view
pub struct TerminalPane {
    workspace: Entity<Workspace>,
    project_id: String,
    project_path: String,
    layout_path: Vec<usize>,
    terminal: Option<Arc<Terminal>>,
    terminal_id: Option<String>,
    minimized: bool,
    detached: bool,
    pty_manager: Arc<PtyManager>,
    terminals: TerminalsRegistry,
    focus_handle: FocusHandle,
    pending_focus: bool,
    is_selecting: bool,
    /// Element bounds captured during prepaint (using Rc<RefCell> for safe mutation during prepaint)
    element_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    context_menu_position: Option<Point<Pixels>>,
    /// Rename state
    is_renaming: bool,
    rename_input: Option<Entity<SimpleInputState>>,
    /// Last click time for double-click detection
    last_header_click: Option<std::time::Instant>,
    /// Last click time and position for terminal double/triple click detection
    last_terminal_click: Option<(std::time::Instant, usize, i32)>,
    terminal_click_count: u8,
    /// Search state
    is_searching: bool,
    search_input: Option<Entity<SimpleInputState>>,
    search_matches: Arc<Vec<SearchMatch>>,
    current_match_index: Option<usize>,
    search_case_sensitive: bool,
    search_regex: bool,
    /// Scrollbar state
    scrollbar_dragging: bool,
    scrollbar_drag_start_y: Option<f32>,
    scrollbar_drag_start_offset: Option<usize>,
    /// Last scroll activity time (for auto-hide)
    last_scroll_activity: std::time::Instant,
    /// Scrollbar visibility (auto-hidden after inactivity)
    scrollbar_visible: bool,
    /// URL detection state
    url_matches: Vec<URLMatch>,
    hovered_url_index: Option<usize>,
    /// Cursor blink state
    cursor_visible: bool,
    /// Scroll accumulator for sub-line deltas (ensures smooth scrolling)
    scroll_accumulator: f32,
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

        let mut pane = Self {
            workspace,
            project_id,
            project_path,
            layout_path,
            terminal: None,
            terminal_id,
            minimized,
            detached,
            pty_manager,
            terminals,
            focus_handle,
            pending_focus: false,
            is_selecting: false,
            element_bounds: Rc::new(RefCell::new(None)),
            context_menu_position: None,
            is_renaming: false,
            rename_input: None,
            last_header_click: None,
            last_terminal_click: None,
            terminal_click_count: 0,
            is_searching: false,
            search_input: None,
            search_matches: Arc::new(Vec::new()),
            current_match_index: None,
            search_case_sensitive: false,
            search_regex: false,
            scrollbar_dragging: false,
            scrollbar_drag_start_y: None,
            scrollbar_drag_start_offset: None,
            last_scroll_activity: std::time::Instant::now(),
            scrollbar_visible: false,
            url_matches: Vec::new(),
            hovered_url_index: None,
            cursor_visible: true,
            scroll_accumulator: 0.0,
        };

        // If we have an existing terminal ID, create terminal immediately
        // Otherwise, create PTY and terminal on first render
        if let Some(ref id) = pane.terminal_id {
            pane.create_terminal_for_existing_pty(id.clone(), cx);
        }

        // Start dirty check loop for this terminal pane
        pane.start_dirty_check_loop(cx);

        // Start cursor blink loop
        pane.start_cursor_blink_loop(cx);

        pane
    }

    /// Start a loop that checks if the terminal is dirty and needs re-render
    fn start_dirty_check_loop(&self, cx: &mut Context<Self>) {
        use std::time::Duration;

        cx.spawn(async move |this: WeakEntity<TerminalPane>, cx| {
            // Check every ~8ms (120fps) for dirty terminals
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
                    Err(_) => break, // Entity was dropped
                }
            }
        }).detach();
    }

    /// Start a loop that blinks the cursor
    fn start_cursor_blink_loop(&self, cx: &mut Context<Self>) {
        use std::time::Duration;

        cx.spawn(async move |this: WeakEntity<TerminalPane>, cx| {
            // Blink every 500ms
            let interval = Duration::from_millis(500);
            loop {
                smol::Timer::after(interval).await;

                let result = this.update(cx, |pane, cx| {
                    // Only blink if cursor_blink setting is enabled
                    if settings(cx).cursor_blink {
                        pane.cursor_visible = !pane.cursor_visible;
                        cx.notify();
                    } else if !pane.cursor_visible {
                        // If blink is disabled but cursor is hidden, show it
                        pane.cursor_visible = true;
                        cx.notify();
                    }
                });

                if result.is_err() {
                    break; // Entity was dropped
                }
            }
        }).detach();
    }

    /// Update detached state
    pub fn set_detached(&mut self, detached: bool, cx: &mut Context<Self>) {
        if self.detached != detached {
            self.detached = detached;
            cx.notify();
        }
    }

    fn create_terminal_for_existing_pty(&mut self, terminal_id: String, _cx: &mut Context<Self>) {
        // Check if terminal already exists in registry
        let existing = self.terminals.lock().get(&terminal_id).cloned();
        if let Some(terminal) = existing {
            self.terminal = Some(terminal);
            return;
        }

        // PTY doesn't exist in current session - try to reconnect (for tmux/screen persistence)
        // or create new PTY with this ID
        match self.pty_manager.create_or_reconnect_terminal(Some(&terminal_id), &self.project_path) {
            Ok(_) => {
                log::info!("Reconnected to terminal: {}", terminal_id);
            }
            Err(e) => {
                log::error!("Failed to reconnect terminal {}: {}", terminal_id, e);
                // Continue anyway - Terminal wrapper will be created but may not work
            }
        }

        // Create new terminal wrapper and register it
        let size = TerminalSize::default();
        let terminal = Arc::new(Terminal::new(terminal_id.clone(), size, self.pty_manager.clone()));
        self.terminals.lock().insert(terminal_id, terminal.clone());
        self.terminal = Some(terminal);
    }

    /// Get the terminal ID (for checking if pane needs recreation)
    pub fn terminal_id(&self) -> Option<String> {
        self.terminal_id.clone()
    }

    /// Update minimized state
    pub fn set_minimized(&mut self, minimized: bool, cx: &mut Context<Self>) {
        if self.minimized != minimized {
            self.minimized = minimized;
            cx.notify();
        }
    }

    /// Check for double-click on header
    fn check_header_double_click(&mut self) -> bool {
        let now = std::time::Instant::now();
        let is_double = if let Some(last_time) = self.last_header_click {
            now.duration_since(last_time).as_millis() < 400
        } else {
            false
        };

        if is_double {
            self.last_header_click = None;
            true
        } else {
            self.last_header_click = Some(now);
            false
        }
    }

    fn start_rename(&mut self, current_name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.is_renaming = true;
        let input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Terminal name...")
                .default_value(&current_name)
        });
        // Focus the input
        input.update(cx, |input, cx| {
            input.focus(window, cx);
        });
        self.rename_input = Some(input);
        // Clear focused terminal to prevent stealing focus back
        self.workspace.update(cx, |ws, cx| {
            ws.clear_focused_terminal(cx);
        });
        cx.notify();
    }

    fn finish_rename(&mut self, cx: &mut Context<Self>) {
        if let (Some(ref terminal_id), Some(ref input)) = (&self.terminal_id, &self.rename_input) {
            let new_name = input.read(cx).value().to_string();
            if !new_name.is_empty() {
                let project_id = self.project_id.clone();
                let terminal_id = terminal_id.clone();
                self.workspace.update(cx, |ws, cx| {
                    ws.rename_terminal(&project_id, &terminal_id, new_name, cx);
                });
            }
        }
        self.is_renaming = false;
        self.rename_input = None;
        // Restore focus after modal dismissal
        self.workspace.update(cx, |ws, cx| {
            ws.restore_focused_terminal(cx);
        });
        cx.notify();
    }

    fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        self.is_renaming = false;
        self.rename_input = None;
        // Restore focus after modal dismissal
        self.workspace.update(cx, |ws, cx| {
            ws.restore_focused_terminal(cx);
        });
        cx.notify();
    }

    fn start_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.is_searching = true;
        let input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Search...")
                .icon("icons/search.svg")
        });
        // Focus the input
        input.update(cx, |input, cx| {
            input.focus(window, cx);
        });
        self.search_input = Some(input);
        self.search_matches = Arc::new(Vec::new());
        self.current_match_index = None;
        // Clear focused terminal to prevent stealing focus back
        self.workspace.update(cx, |ws, cx| {
            ws.clear_focused_terminal(cx);
        });
        cx.notify();
    }

    fn close_search(&mut self, cx: &mut Context<Self>) {
        self.is_searching = false;
        self.search_input = None;
        self.search_matches = Arc::new(Vec::new());
        self.current_match_index = None;
        // Restore focus after modal dismissal
        self.workspace.update(cx, |ws, cx| {
            ws.restore_focused_terminal(cx);
        });
        cx.notify();
    }

    fn perform_search(&mut self, query: &str, _cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal {
            let matches = terminal.search_grid(query, self.search_case_sensitive, self.search_regex);
            let search_matches: Vec<SearchMatch> = matches.into_iter()
                .map(|(line, col, len)| SearchMatch { line, col, len })
                .collect();

            // Set current match to first if we have matches
            if !search_matches.is_empty() {
                self.current_match_index = Some(0);
            } else {
                self.current_match_index = None;
            }

            self.search_matches = Arc::new(search_matches);
        }
    }

    fn toggle_case_sensitive(&mut self, cx: &mut Context<Self>) {
        self.search_case_sensitive = !self.search_case_sensitive;
        // Re-run search with new setting
        if let Some(ref input) = self.search_input {
            let query = input.read(cx).value().to_string();
            self.perform_search(&query, cx);
        }
        cx.notify();
    }

    fn toggle_regex(&mut self, cx: &mut Context<Self>) {
        self.search_regex = !self.search_regex;
        // Re-run search with new setting
        if let Some(ref input) = self.search_input {
            let query = input.read(cx).value().to_string();
            self.perform_search(&query, cx);
        }
        cx.notify();
    }

    fn next_match(&mut self, cx: &mut Context<Self>) {
        if self.search_matches.is_empty() {
            return;
        }

        let next_idx = match self.current_match_index {
            Some(idx) => (idx + 1) % self.search_matches.len(),
            None => 0,
        };
        self.current_match_index = Some(next_idx);
        self.scroll_to_current_match();
        cx.notify();
    }

    fn prev_match(&mut self, cx: &mut Context<Self>) {
        if self.search_matches.is_empty() {
            return;
        }

        let prev_idx = match self.current_match_index {
            Some(idx) => {
                if idx == 0 {
                    self.search_matches.len() - 1
                } else {
                    idx - 1
                }
            }
            None => self.search_matches.len() - 1,
        };
        self.current_match_index = Some(prev_idx);
        self.scroll_to_current_match();
        cx.notify();
    }

    fn scroll_to_current_match(&self) {
        if let (Some(idx), Some(ref terminal)) = (self.current_match_index, &self.terminal) {
            if let Some(search_match) = self.search_matches.get(idx) {
                let screen_lines = terminal.screen_lines() as i32;
                let match_line = search_match.line;

                // If the match is outside the visible area, scroll to center it
                // match_line is in display-relative coordinates where 0 is top of visible area
                // Negative values are in scrollback history
                if match_line < 0 || match_line >= screen_lines {
                    // Calculate scroll needed to bring match to center of screen
                    let target_visible_line = screen_lines / 2;
                    let scroll_delta = target_visible_line - match_line;

                    if scroll_delta > 0 {
                        // Need to scroll up (into history)
                        terminal.scroll_up(scroll_delta);
                    } else if scroll_delta < 0 {
                        // Need to scroll down (towards current)
                        terminal.scroll_down(-scroll_delta);
                    }
                }
            }
        }
    }

    fn create_new_terminal(&mut self, cx: &mut Context<Self>) {
        log::info!("Creating new terminal for project path: {}", self.project_path);
        // Create new PTY
        match self.pty_manager.create_terminal(&self.project_path) {
            Ok(terminal_id) => {
                log::info!("PTY created with ID: {}", terminal_id);
                // Store terminal ID in workspace
                self.terminal_id = Some(terminal_id.clone());
                self.workspace.update(cx, |ws, cx| {
                    ws.set_terminal_id(&self.project_id, &self.layout_path, terminal_id.clone(), cx);
                });

                // Create terminal and register it
                let size = TerminalSize::default();
                let terminal = Arc::new(Terminal::new(terminal_id.clone(), size, self.pty_manager.clone()));
                self.terminals.lock().insert(terminal_id, terminal.clone());
                self.terminal = Some(terminal);
                self.pending_focus = true;

                cx.notify();
            }
            Err(e) => {
                log::error!("Failed to create terminal: {}", e);
            }
        }
    }

    fn handle_split(&mut self, direction: SplitDirection, cx: &mut Context<Self>) {
        log::info!("TerminalPane::handle_split called with direction {:?}, path {:?}", direction, self.layout_path);
        self.workspace.update(cx, |ws, cx| {
            ws.split_terminal(&self.project_id, &self.layout_path, direction, cx);
        });
    }

    fn handle_add_tab(&mut self, cx: &mut Context<Self>) {
        log::info!("TerminalPane::handle_add_tab called, path {:?}", self.layout_path);
        self.workspace.update(cx, |ws, cx| {
            ws.add_tab(&self.project_id, &self.layout_path, cx);
        });
    }

    /// Check if this terminal is a direct child of a Tabs container
    fn is_in_tab_group(&self, cx: &Context<Self>) -> bool {
        if self.layout_path.is_empty() {
            return false;
        }
        let parent_path = &self.layout_path[..self.layout_path.len() - 1];
        let ws = self.workspace.read(cx);
        if let Some(project) = ws.project(&self.project_id) {
            if let Some(crate::workspace::state::LayoutNode::Tabs { .. }) = project.layout.get_at_path(parent_path) {
                return true;
            }
        }
        false
    }

    fn handle_close(&mut self, cx: &mut Context<Self>) {
        // Kill PTY
        if let Some(ref id) = self.terminal_id {
            self.pty_manager.kill(id);
        }

        // Remove from layout and focus the sibling (reverse of splitting)
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
        log::info!("handle_fullscreen called, terminal_id={:?}", self.terminal_id);
        if let Some(ref id) = self.terminal_id {
            self.workspace.update(cx, |ws, cx| {
                ws.set_fullscreen_terminal(self.project_id.clone(), id.clone(), cx);
            });
        } else {
            log::warn!("handle_fullscreen: terminal_id is None!");
        }
    }

    fn handle_detach(&mut self, cx: &mut Context<Self>) {
        if self.terminal_id.is_some() {
            // Mark terminal as detached in workspace - the app will observe this and open a new window
            self.workspace.update(cx, |ws, cx| {
                ws.detach_terminal(&self.project_id, &self.layout_path, cx);
            });
        }
    }

    fn handle_export_buffer(&mut self, cx: &mut Context<Self>) {
        if let Some(ref terminal_id) = self.terminal_id {
            if let Some(path) = self.pty_manager.capture_buffer(terminal_id) {
                // Copy the path to clipboard so user can easily access it
                cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
                log::info!("Buffer exported to {} (path copied to clipboard)", path.display());
            }
        }
    }

    fn handle_copy(&mut self, cx: &mut Context<Self>) {
        log::info!("handle_copy called");
        if let Some(ref terminal) = self.terminal {
            let has_sel = terminal.has_selection();
            log::info!("Terminal has selection: {}", has_sel);
            
            if let Some(text) = terminal.get_selected_text() {
                log::info!("Got selected text: {} chars", text.len());
                // Use GPUI's native clipboard API
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                log::info!("Copied to clipboard: {} chars", text.len());
            } else {
                log::info!("No text selected to copy");
            }
        } else {
            log::warn!("No terminal for copy");
        }
    }

    fn handle_paste(&mut self, cx: &mut Context<Self>) {
        log::info!("handle_paste called");
        if let Some(ref terminal) = self.terminal {
            // Use GPUI's native clipboard API
            if let Some(clipboard_item) = cx.read_from_clipboard() {
                if let Some(text) = clipboard_item.text() {
                    terminal.send_input(&text);
                    log::info!("Pasted from clipboard: {} chars", text.len());
                } else {
                    log::info!("Clipboard has no text content");
                }
            } else {
                log::info!("Clipboard is empty");
            }
        } else {
            log::warn!("No terminal for paste");
        }
    }

    /// Handle file drop (drag & drop from external sources)
    /// Pastes shell-escaped file paths, similar to how Ghostty handles drops
    fn handle_file_drop(&mut self, paths: &ExternalPaths, _cx: &mut Context<Self>) {
        let Some(ref terminal) = self.terminal else {
            log::warn!("No terminal for file drop");
            return;
        };

        for path in paths.paths() {
            log::info!("File dropped: {}", path.display());

            // Shell-escape the path (escape spaces and special characters with backslashes)
            // This matches Ghostty's behavior for drag & drop
            let escaped_path = Self::shell_escape_path(path);

            // Add space after to make it easier to continue typing
            terminal.send_input(&format!("{} ", escaped_path));
        }
    }

    /// Shell-escape a path for safe pasting into terminal
    /// Escapes spaces and special shell characters with backslashes
    fn shell_escape_path(path: &std::path::Path) -> String {
        let path_str = path.to_string_lossy();
        let mut escaped = String::with_capacity(path_str.len() * 2);

        for c in path_str.chars() {
            match c {
                // Characters that need escaping in shell
                ' ' | '(' | ')' | '[' | ']' | '{' | '}' |
                '\'' | '"' | '`' | '$' | '&' | '|' | ';' |
                '<' | '>' | '*' | '?' | '!' | '#' | '~' | '\\' => {
                    escaped.push('\\');
                    escaped.push(c);
                }
                _ => escaped.push(c),
            }
        }

        escaped
    }

    /// Handle directional navigation to an adjacent pane
    fn handle_navigation(&mut self, direction: NavigationDirection, _window: &mut Window, cx: &mut Context<Self>) {
        // Get the pane map
        let pane_map = get_pane_map();

        // Find our current pane in the map
        let source = match pane_map.find_pane(&self.project_id, &self.layout_path) {
            Some(pane) => pane.clone(),
            None => {
                log::debug!("Navigation: current pane not found in pane map");
                return;
            }
        };

        // Find the nearest pane in the requested direction
        if let Some(target) = pane_map.find_nearest_in_direction(&source, direction) {
            log::debug!(
                "Navigation {:?}: from {:?} to {:?}",
                direction,
                self.layout_path,
                target.layout_path
            );

            // Update workspace focused terminal state
            self.workspace.update(cx, |ws, cx| {
                ws.set_focused_terminal(target.project_id.clone(), target.layout_path.clone(), cx);
            });
        } else {
            log::debug!("Navigation {:?}: no target found (at boundary)", direction);
        }
    }

    /// Handle sequential navigation to the next or previous pane
    fn handle_sequential_navigation(&mut self, next: bool, _window: &mut Window, cx: &mut Context<Self>) {
        let pane_map = get_pane_map();

        let source = match pane_map.find_pane(&self.project_id, &self.layout_path) {
            Some(pane) => pane.clone(),
            None => {
                log::debug!("Sequential navigation: current pane not found in pane map");
                return;
            }
        };

        let target = if next {
            pane_map.find_next_pane(&source)
        } else {
            pane_map.find_prev_pane(&source)
        };

        if let Some(target) = target {
            log::debug!(
                "Sequential navigation {}: from {:?} to {:?}",
                if next { "next" } else { "prev" },
                self.layout_path,
                target.layout_path
            );

            self.workspace.update(cx, |ws, cx| {
                ws.set_focused_terminal(target.project_id.clone(), target.layout_path.clone(), cx);
            });
        } else {
            log::debug!("Sequential navigation: no target found (only one pane)");
        }
    }

    fn show_context_menu(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.context_menu_position = Some(position);
        cx.notify();
    }

    fn hide_context_menu(&mut self, cx: &mut Context<Self>) {
        self.context_menu_position = None;
        cx.notify();
    }

    fn render_context_menu(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let has_selection = self.terminal.as_ref().map(|t| t.has_selection()).unwrap_or(false);

        // Approximate menu height (9 items + 3 separators + padding)
        // Items: ~26px each (6px py * 2 + 14px content), separators: ~9px each, container: 8px py
        let menu_height = 9.0 * 26.0 + 3.0 * 9.0 + 8.0; // ~269px

        // Calculate relative position within the terminal content area
        // and determine if menu should open upward
        let (relative_pos, open_upward) = if let Some(bounds) = *self.element_bounds.borrow() {
            let rel_x = position.x - bounds.origin.x;
            let rel_y = position.y - bounds.origin.y;
            let space_below = f32::from(bounds.size.height) - f32::from(rel_y);
            let should_open_up = space_below < menu_height;
            (Point { x: rel_x, y: rel_y }, should_open_up)
        } else {
            (position, false)
        };

        let menu = div()
            .id("terminal-context-menu")
            .absolute()
            .left(relative_pos.x)
            .bg(rgb(t.bg_secondary))
            .border_1()
            .border_color(rgb(t.border))
            .rounded(px(4.0))
            .shadow_lg()
            .py(px(4.0))
            .min_w(px(120.0))
            .child({
                let base = div()
                    .id("context-menu-copy")
                    .px(px(12.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(13.0))
                    .text_color(if has_selection { rgb(t.text_primary) } else { rgb(t.text_muted) })
                    .cursor(if has_selection { CursorStyle::PointingHand } else { CursorStyle::Arrow })
                    .child(
                        svg()
                            .path("icons/copy.svg")
                            .size(px(14.0))
                            .text_color(if has_selection { rgb(t.text_secondary) } else { rgb(t.text_muted) })
                    )
                    .child("Copy");
                if has_selection {
                    base.hover(|s| s.bg(rgb(t.bg_hover)))
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                            this.handle_copy(cx);
                            this.hide_context_menu(cx);
                        }))
                } else {
                    base
                }
            })
            .child(
                div()
                    .id("context-menu-paste")
                    .px(px(12.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(13.0))
                    .text_color(rgb(t.text_primary))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .cursor_pointer()
                    .child(
                        svg()
                            .path("icons/clipboard-paste.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary))
                    )
                    .child("Paste")
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                        this.handle_paste(cx);
                        this.hide_context_menu(cx);
                    })),
            )
            .child(
                div()
                    .h(px(1.0))
                    .mx(px(8.0))
                    .my(px(4.0))
                    .bg(rgb(t.border)),
            )
            .child(
                div()
                    .id("context-menu-clear")
                    .px(px(12.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(13.0))
                    .text_color(rgb(t.text_primary))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .cursor_pointer()
                    .child(
                        svg()
                            .path("icons/eraser.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary))
                    )
                    .child("Clear")
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                        if let Some(ref terminal) = this.terminal {
                            // Send Ctrl+L to clear screen
                            terminal.send_bytes(&[0x0c]);
                        }
                        this.hide_context_menu(cx);
                    })),
            )
            .child(
                div()
                    .id("context-menu-select-all")
                    .px(px(12.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(13.0))
                    .text_color(rgb(t.text_primary))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .cursor_pointer()
                    .child(
                        svg()
                            .path("icons/select-all.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary))
                    )
                    .child("Select All")
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                        // Select all is complex in terminal, just close for now
                        this.hide_context_menu(cx);
                    })),
            )
            // Separator before split/close actions
            .child(
                div()
                    .h(px(1.0))
                    .mx(px(8.0))
                    .my(px(4.0))
                    .bg(rgb(t.border)),
            )
            .child(
                div()
                    .id("context-menu-split-h")
                    .px(px(12.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(13.0))
                    .text_color(rgb(t.text_primary))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .cursor_pointer()
                    .child(
                        svg()
                            .path("icons/split-horizontal.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary))
                    )
                    .child("Split Horizontal")
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                        this.handle_split(SplitDirection::Horizontal, cx);
                        this.hide_context_menu(cx);
                    })),
            )
            .child(
                div()
                    .id("context-menu-split-v")
                    .px(px(12.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(13.0))
                    .text_color(rgb(t.text_primary))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .cursor_pointer()
                    .child(
                        svg()
                            .path("icons/split-vertical.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary))
                    )
                    .child("Split Vertical")
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                        this.handle_split(SplitDirection::Vertical, cx);
                        this.hide_context_menu(cx);
                    })),
            )
            // Separator before close
            .child(
                div()
                    .h(px(1.0))
                    .mx(px(8.0))
                    .my(px(4.0))
                    .bg(rgb(t.border)),
            )
            .child(
                div()
                    .id("context-menu-close")
                    .px(px(12.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(13.0))
                    .text_color(rgb(t.error))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .cursor_pointer()
                    .child(
                        svg()
                            .path("icons/close.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.error))
                    )
                    .child("Close")
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                        this.handle_close(cx);
                        this.hide_context_menu(cx);
                    })),
            );

        // Position menu: open upward if not enough space below
        if open_upward {
            // Position from bottom of click point
            let bottom_offset = if let Some(bounds) = *self.element_bounds.borrow() {
                f32::from(bounds.size.height) - f32::from(relative_pos.y)
            } else {
                0.0
            };
            menu.bottom(px(bottom_offset))
        } else {
            menu.top(relative_pos.y)
        }
    }

    fn handle_scroll(&mut self, delta: f32, position: gpui::Point<Pixels>, touch_phase: TouchPhase, cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal {
            let (cell_width, cell_height) = terminal.cell_dimensions();

            // Reset accumulator on new scroll gesture
            if matches!(touch_phase, TouchPhase::Started) {
                self.scroll_accumulator = 0.0;
            }

            // Check if terminal is in mouse mode (tmux, vim, etc.)
            if terminal.is_mouse_mode() {
                // Forward scroll to PTY as mouse wheel events
                // Calculate cell position relative to terminal bounds
                let (col, row) = if let Some(bounds) = *self.element_bounds.borrow() {
                    let x = (f32::from(position.x) - f32::from(bounds.origin.x)).max(0.0);
                    let y = (f32::from(position.y) - f32::from(bounds.origin.y)).max(0.0);
                    ((x / cell_width) as usize, (y / cell_height) as usize)
                } else {
                    (0, 0)
                };

                // Accumulate scroll delta for smooth sub-line scrolling
                self.scroll_accumulator += delta / cell_height;

                // Only send events for whole lines
                let lines = self.scroll_accumulator.trunc() as i32;
                if lines != 0 {
                    self.scroll_accumulator -= lines as f32;
                    // Mouse button 64 = scroll up, 65 = scroll down
                    let button = if lines > 0 { 64u8 } else { 65u8 };
                    for _ in 0..lines.abs() {
                        terminal.send_mouse_scroll(button, col, row);
                    }
                }
            } else {
                // Normal scrollback scrolling with accumulation
                self.scroll_accumulator += delta / cell_height;

                // Only scroll for whole lines, keep remainder
                let lines = self.scroll_accumulator.trunc() as i32;
                if lines != 0 {
                    self.scroll_accumulator -= lines as f32;
                    if lines > 0 {
                        terminal.scroll_up(lines);
                    } else {
                        terminal.scroll_down(-lines);
                    }
                }
            }

            // Update scroll activity for auto-hide
            self.last_scroll_activity = std::time::Instant::now();
            self.scrollbar_visible = true;
            cx.notify();
        }
    }

    /// Calculate scrollbar thumb geometry
    /// Returns (thumb_y, thumb_height, track_height) in pixels, or None if no scrollbar needed
    fn calculate_scrollbar_geometry(&self, content_height: f32) -> Option<(f32, f32, f32)> {
        let terminal = self.terminal.as_ref()?;
        let (total_lines, visible_lines, display_offset) = terminal.scroll_info();

        // No scrollbar if all content fits
        if total_lines <= visible_lines {
            return None;
        }

        let track_height = content_height;
        let scrollable_lines = total_lines - visible_lines;

        // Thumb height is proportional to visible content
        let thumb_height = (visible_lines as f32 / total_lines as f32 * track_height)
            .max(20.0); // Minimum 20px thumb

        // Thumb position: display_offset 0 = at bottom, scrollable_lines = at top
        let available_scroll_space = track_height - thumb_height;
        let scroll_ratio = display_offset as f32 / scrollable_lines as f32;
        // Invert: when display_offset is 0 (bottom), thumb should be at bottom
        let thumb_y = (1.0 - scroll_ratio) * available_scroll_space;

        Some((thumb_y, thumb_height, track_height))
    }

    /// Handle scrollbar click (jump to position)
    fn handle_scrollbar_click(&mut self, y: f32, content_height: f32, cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal {
            let (total_lines, visible_lines, _) = terminal.scroll_info();
            if total_lines <= visible_lines {
                return;
            }

            let scrollable_lines = total_lines - visible_lines;
            // Convert click position to scroll offset
            // y=0 is top (max scroll), y=content_height is bottom (scroll=0)
            let ratio = 1.0 - (y / content_height).clamp(0.0, 1.0);
            let new_offset = (ratio * scrollable_lines as f32).round() as usize;
            terminal.scroll_to(new_offset);

            self.last_scroll_activity = std::time::Instant::now();
            self.scrollbar_visible = true;
            cx.notify();
        }
    }

    /// Start scrollbar drag
    fn start_scrollbar_drag(&mut self, y: f32, cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal {
            self.scrollbar_dragging = true;
            self.scrollbar_drag_start_y = Some(y);
            self.scrollbar_drag_start_offset = Some(terminal.display_offset());
            self.last_scroll_activity = std::time::Instant::now();
            self.scrollbar_visible = true;
            cx.notify();
        }
    }

    /// Update scrollbar during drag
    fn update_scrollbar_drag(&mut self, y: f32, content_height: f32, cx: &mut Context<Self>) {
        if !self.scrollbar_dragging {
            return;
        }

        if let (Some(start_y), Some(start_offset), Some(ref terminal)) = (
            self.scrollbar_drag_start_y,
            self.scrollbar_drag_start_offset,
            &self.terminal,
        ) {
            let (total_lines, visible_lines, _) = terminal.scroll_info();
            if total_lines <= visible_lines {
                return;
            }

            let scrollable_lines = total_lines - visible_lines;
            let delta_y = y - start_y;

            // Convert pixel delta to scroll lines
            // Negative delta_y (drag up) = scroll up (increase offset)
            let lines_per_pixel = scrollable_lines as f32 / content_height;
            let delta_lines = (-delta_y * lines_per_pixel).round() as i32;

            let new_offset = (start_offset as i32 + delta_lines)
                .clamp(0, scrollable_lines as i32) as usize;
            terminal.scroll_to(new_offset);

            self.last_scroll_activity = std::time::Instant::now();
            cx.notify();
        }
    }

    /// End scrollbar drag
    fn end_scrollbar_drag(&mut self, cx: &mut Context<Self>) {
        self.scrollbar_dragging = false;
        self.scrollbar_drag_start_y = None;
        self.scrollbar_drag_start_offset = None;
        cx.notify();
    }

    /// Check if scrollbar should be visible (auto-hide after 1.5 seconds of inactivity)
    fn should_show_scrollbar(&self) -> bool {
        // Always show if dragging
        if self.scrollbar_dragging {
            return true;
        }

        // Show if recently active
        let elapsed = self.last_scroll_activity.elapsed();
        elapsed.as_millis() < 1500
    }

    fn pixel_to_cell(&self, pos: Point<Pixels>) -> Option<(usize, i32)> {
        let bounds = (*self.element_bounds.borrow())?;
        let terminal = self.terminal.as_ref()?;
        let (cell_width, cell_height) = terminal.cell_dimensions();

        // Calculate relative position within the terminal bounds
        let x = (f32::from(pos.x) - f32::from(bounds.origin.x)).max(0.0);
        let y = (f32::from(pos.y) - f32::from(bounds.origin.y)).max(0.0);

        // Use floor for consistent cell selection - clicking anywhere within a cell
        // should select that cell
        let col = (x / cell_width).floor() as usize;
        let row = (y / cell_height).floor() as i32;

        // Clamp to terminal bounds
        let size = terminal.size.lock();
        let col = col.min(size.cols.saturating_sub(1) as usize);
        let row = row.min(size.rows.saturating_sub(1) as i32);

        Some((col, row))
    }

    fn handle_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);

        if let Some(ref terminal) = self.terminal {
            if let Some((col, row)) = self.pixel_to_cell(event.position) {
                // Check for Ctrl+Click on URL
                if event.modifiers.control {
                    if let Some(url_match) = self.find_url_at(col, row) {
                        self.open_url(&url_match.url);
                        return;
                    }
                }

                let now = std::time::Instant::now();

                // Detect click count based on timing and position
                let click_count = if let Some((last_time, last_col, last_row)) = self.last_terminal_click {
                    let elapsed = now.duration_since(last_time).as_millis();
                    // Same position (or close enough) and within double-click time window
                    let same_position = (col as i32 - last_col as i32).abs() <= 1
                        && (row - last_row).abs() <= 0;
                    if elapsed < 400 && same_position {
                        // Increment click count, cycling 1 -> 2 -> 3 -> 1
                        if self.terminal_click_count >= 3 {
                            1
                        } else {
                            self.terminal_click_count + 1
                        }
                    } else {
                        1
                    }
                } else {
                    1
                };

                self.last_terminal_click = Some((now, col, row));
                self.terminal_click_count = click_count;

                // Clear any existing selection
                terminal.clear_selection();

                // Start appropriate selection based on click count
                match click_count {
                    2 => {
                        // Double-click: word selection
                        terminal.start_word_selection(col, row);
                        self.is_selecting = false; // Word selection doesn't need drag
                    }
                    3 => {
                        // Triple-click: line selection
                        terminal.start_line_selection(col, row);
                        self.is_selecting = false; // Line selection doesn't need drag
                    }
                    _ => {
                        // Single click: simple selection (drag to select)
                        terminal.start_selection(col, row);
                        self.is_selecting = true;
                    }
                }
                cx.notify();
            }
        }
    }

    fn handle_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        // Update URL hover state
        if let Some((col, row)) = self.pixel_to_cell(event.position) {
            let new_hovered = self.url_matches.iter().position(|url| {
                url.line == row && col >= url.col && col < url.col + url.len
            });
            if new_hovered != self.hovered_url_index {
                self.hovered_url_index = new_hovered;
                cx.notify();
            }
        } else if self.hovered_url_index.is_some() {
            self.hovered_url_index = None;
            cx.notify();
        }

        if self.is_selecting {
            // Check if mouse button was released outside the element
            if event.pressed_button != Some(MouseButton::Left) {
                if let Some(ref terminal) = self.terminal {
                    terminal.end_selection();
                    if !terminal.has_selection() || terminal.get_selected_text().map(|s| s.is_empty()).unwrap_or(true) {
                        terminal.clear_selection();
                    }
                }
                self.is_selecting = false;
                cx.notify();
                return;
            }

            if let Some(ref terminal) = self.terminal {
                if let Some((col, row)) = self.pixel_to_cell(event.position) {
                    terminal.update_selection(col, row);
                    cx.notify();
                }
            }
        }
    }

    fn handle_mouse_up(&mut self, _event: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.is_selecting {
            if let Some(ref terminal) = self.terminal {
                terminal.end_selection();
                self.is_selecting = false;

                // If the selection is empty (just a click), clear it
                if !terminal.has_selection() || terminal.get_selected_text().map(|s| s.is_empty()).unwrap_or(true) {
                    terminal.clear_selection();
                }
                cx.notify();
            }
        }
    }

    fn handle_key(&mut self, event: &KeyDownEvent, _cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal {
            if let Some(input) = key_to_bytes(event) {
                terminal.send_bytes(&input);
            }
        }
    }

    /// Find URL at the given cell position
    fn find_url_at(&self, col: usize, row: i32) -> Option<URLMatch> {
        self.url_matches.iter().find(|url| {
            url.line == row && col >= url.col && col < url.col + url.len
        }).cloned()
    }

    /// Open URL in default browser
    fn open_url(&self, url: &str) {
        log::info!("Opening URL: {}", url);
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("xdg-open")
                .arg(url)
                .spawn();
        }
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open")
                .arg(url)
                .spawn();
        }
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("cmd")
                .args(&["/C", "start", "", url])
                .spawn();
        }
    }

    /// Detect URLs in terminal content and update the matches list
    fn update_url_matches(&mut self) {
        if let Some(ref terminal) = self.terminal {
            let detected = terminal.detect_urls();
            self.url_matches = detected
                .into_iter()
                .map(|(line, col, len, url)| URLMatch { line, col, len, url })
                .collect();
        }
    }

    fn render_search_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let match_count = self.search_matches.len();
        let current_idx = self.current_match_index.map(|i| i + 1).unwrap_or(0);
        let match_text = if match_count > 0 {
            format!("{}/{}", current_idx, match_count)
        } else {
            "0/0".to_string()
        };
        let case_sensitive = self.search_case_sensitive;
        let is_regex = self.search_regex;

        div()
            .id("search-bar")
            .h(px(36.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .bg(rgb(t.bg_header))
            .child(
                if let Some(ref input) = self.search_input {
                    div()
                        .id("search-input-wrapper")
                        .flex_1()
                        .min_w(px(100.0))
                        .max_w(px(300.0))
                        .bg(rgb(t.bg_secondary))
                        .border_1()
                        .border_color(rgb(t.border_active))
                        .rounded(px(4.0))
                        .child(SimpleInput::new(input).text_size(px(12.0)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                            match event.keystroke.key.as_str() {
                                "enter" => {
                                    cx.stop_propagation();
                                    if event.keystroke.modifiers.shift {
                                        this.prev_match(cx);
                                    } else {
                                        this.next_match(cx);
                                    }
                                }
                                "escape" => {
                                    cx.stop_propagation();
                                    this.close_search(cx);
                                }
                                _ => {
                                    // Update search on text change
                                    if let Some(ref input) = this.search_input {
                                        let query = input.read(cx).value().to_string();
                                        this.perform_search(&query, cx);
                                        cx.notify();
                                    }
                                }
                            }
                        }))
                        .into_any_element()
                } else {
                    div().flex_1().into_any_element()
                },
            )
            // Case-sensitive toggle button
            .child(
                div()
                    .id("search-case-sensitive-btn")
                    .cursor_pointer()
                    .w(px(24.0))
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .when(case_sensitive, |s| s.bg(rgb(t.bg_selection)))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.toggle_case_sensitive(cx);
                    }))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(if case_sensitive { rgb(t.text_primary) } else { rgb(t.text_secondary) })
                            .child("Aa")
                    ),
            )
            // Regex toggle button
            .child(
                div()
                    .id("search-regex-btn")
                    .cursor_pointer()
                    .w(px(24.0))
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .when(is_regex, |s| s.bg(rgb(t.bg_selection)))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.toggle_regex(cx);
                    }))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(if is_regex { rgb(t.text_primary) } else { rgb(t.text_secondary) })
                            .child(".*")
                    ),
            )
            .child(
                // Match counter
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(t.text_secondary))
                    .min_w(px(40.0))
                    .child(match_text),
            )
            .child(
                // Previous match button
                div()
                    .id("search-prev-btn")
                    .cursor_pointer()
                    .w(px(24.0))
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.prev_match(cx);
                    }))
                    .child(
                        svg()
                            .path("icons/chevron-up.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary))
                    ),
            )
            .child(
                // Next match button
                div()
                    .id("search-next-btn")
                    .cursor_pointer()
                    .w(px(24.0))
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.next_match(cx);
                    }))
                    .child(
                        svg()
                            .path("icons/chevron-down.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary))
                    ),
            )
            .child(
                // Close search button
                div()
                    .id("search-close-btn")
                    .cursor_pointer()
                    .w(px(24.0))
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .hover(|s| s.bg(rgba(0xf14c4c99)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.close_search(cx);
                    }))
                    .child(
                        svg()
                            .path("icons/close.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary))
                    ),
            )
    }

    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        // Priority: custom name > OSC title > terminal ID prefix
        let terminal_name = if let Some(ref terminal_id) = self.terminal_id {
            // Check for custom name first
            let custom_name = {
                let workspace = self.workspace.read(cx);
                workspace
                    .project(&self.project_id)
                    .and_then(|p| p.terminal_names.get(terminal_id).cloned())
            };

            if let Some(name) = custom_name {
                name
            } else if let Some(ref terminal) = self.terminal {
                // Check for OSC title
                terminal.title().unwrap_or_else(|| terminal_id.chars().take(8).collect())
            } else {
                terminal_id.chars().take(8).collect()
            }
        } else {
            "Terminal".to_string()
        };

        let terminal_name_for_rename = terminal_name.clone();

        div()
            .id("terminal-header")
            .group("terminal-header")
            .h(px(28.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(4.0))
            .min_w_0()
            .overflow_hidden()
            .bg(rgb(t.bg_header))
            .child(
                // Terminal name (or input if renaming)
                if self.is_renaming {
                    if let Some(ref input) = self.rename_input {
                        div()
                            .id("terminal-rename-input")
                            .flex_1()
                            .min_w_0()
                            .bg(rgb(t.bg_secondary))
                            .border_1()
                            .border_color(rgb(t.border_active))
                            .rounded(px(4.0))
                            .child(SimpleInput::new(input).text_size(px(12.0)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(|_, _window, cx| {
                                cx.stop_propagation();
                            })
                            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                                match event.keystroke.key.as_str() {
                                    "enter" => this.finish_rename(cx),
                                    "escape" => this.cancel_rename(cx),
                                    _ => {}
                                }
                            }))
                            .into_any_element()
                    } else {
                        div().flex_1().min_w_0().into_any_element()
                    }
                } else {
                    div()
                        .id("terminal-header-name")
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.0))
                        .text_color(rgb(t.text_primary))
                        .text_ellipsis()
                        .child(terminal_name)
                        .on_click(cx.listener({
                            let name = terminal_name_for_rename;
                            move |this, _, window, cx| {
                                if this.check_header_double_click() {
                                    this.start_rename(name.clone(), window, cx);
                                }
                            }
                        }))
                        .into_any_element()
                },
            )
            .child(self.render_controls(cx))
    }

    /// Render the scrollbar overlay
    fn render_scrollbar(&mut self, id_suffix: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let scrollbar_dragging = self.scrollbar_dragging;

        // Determine visibility and opacity
        let should_show = self.should_show_scrollbar();
        let has_scroll_content = self.terminal.as_ref()
            .map(|t| {
                let (total, visible, _) = t.scroll_info();
                total > visible
            })
            .unwrap_or(false);

        // Don't render scrollbar if no scroll content
        if !has_scroll_content {
            return div().into_any_element();
        }

        // Scrollbar colors
        let scrollbar_color = if scrollbar_dragging {
            rgb(t.scrollbar_hover)
        } else {
            rgb(t.scrollbar)
        };
        let scrollbar_hover_color = rgb(t.scrollbar_hover);

        // Opacity for auto-hide effect
        let opacity = if should_show { 1.0 } else { 0.0 };

        // Clone terminal for canvas closure
        let terminal_for_canvas = self.terminal.clone();

        // Scrollbar with absolute positioning (overlay on terminal content)
        div()
            .id(format!("scrollbar-track-{}", id_suffix))
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(px(10.0))
            .opacity(opacity)
            .cursor(CursorStyle::Arrow)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                cx.stop_propagation();
                this.last_scroll_activity = std::time::Instant::now();
                this.scrollbar_visible = true;
                let bounds_opt = *this.element_bounds.borrow();
                if let Some(bounds) = bounds_opt {
                    let relative_y = f32::from(event.position.y) - f32::from(bounds.origin.y);
                    let content_height = f32::from(bounds.size.height);
                    if let Some((thumb_y, thumb_height, _)) = this.calculate_scrollbar_geometry(content_height) {
                        if relative_y >= thumb_y && relative_y <= thumb_height + thumb_y {
                            this.start_scrollbar_drag(f32::from(event.position.y), cx);
                        } else {
                            this.handle_scrollbar_click(relative_y, content_height, cx);
                        }
                    }
                }
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.scrollbar_dragging {
                    let bounds_opt = *this.element_bounds.borrow();
                    if let Some(bounds) = bounds_opt {
                        let content_height = f32::from(bounds.size.height);
                        this.update_scrollbar_drag(f32::from(event.position.y), content_height, cx);
                    }
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                this.end_scrollbar_drag(cx);
            }))
            // Canvas-based thumb rendering
            .child(
                canvas(
                    |_bounds, _window, _cx| {},
                    move |bounds: Bounds<Pixels>, _state: (), window: &mut Window, _cx: &mut App| {
                        if let Some(ref terminal) = terminal_for_canvas {
                            let (total_lines, visible_lines, display_offset) = terminal.scroll_info();
                            if total_lines > visible_lines {
                                let track_height = f32::from(bounds.size.height);
                                let scrollable_lines = total_lines - visible_lines;
                                let thumb_height = (visible_lines as f32 / total_lines as f32 * track_height).max(20.0);
                                let available_scroll_space = track_height - thumb_height;
                                let scroll_ratio = display_offset as f32 / scrollable_lines as f32;
                                let thumb_y = (1.0 - scroll_ratio) * available_scroll_space;

                                let thumb_color = if scrollbar_dragging {
                                    scrollbar_hover_color
                                } else {
                                    scrollbar_color
                                };

                                // Paint thumb using bounds origin (absolute coordinates)
                                let thumb_bounds = Bounds {
                                    origin: point(bounds.origin.x + px(2.0), bounds.origin.y + px(thumb_y)),
                                    size: size(px(6.0), px(thumb_height)),
                                };
                                window.paint_quad(fill(thumb_bounds, thumb_color).corner_radii(px(3.0)));
                            }
                        }
                    },
                )
                .size_full()
            )
            .into_any_element()
    }

    fn render_terminal_content(&mut self, is_focused: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let term_bg = if is_focused { t.term_background } else { t.term_background_unfocused };
        // When minimized or detached, don't render terminal content
        // (detached terminals are hidden entirely in layout, this is just a safety check)
        if self.minimized || self.detached {
            return div().into_any_element();
        }

        // Update URL matches for hover detection
        self.update_url_matches();

        if let Some(ref terminal) = self.terminal {
            let id_suffix = self
                .terminal_id
                .clone()
                .unwrap_or_else(|| format!("{}-{}", self.project_id, self.layout_path.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("-")));
            let focus_handle = self.focus_handle.clone();
            let terminal_clone = terminal.clone();

            let element_bounds_setter = {
                let element_bounds = self.element_bounds.clone();
                let project_id = self.project_id.clone();
                let layout_path = self.layout_path.clone();
                move |bounds: Bounds<Pixels>, _window: &mut Window, _cx: &mut App| {
                    // Register bounds with the global navigation pane map
                    register_pane_bounds(project_id.clone(), layout_path.clone(), bounds);

                    *element_bounds.borrow_mut() = Some(bounds);
                }
            };

            // Build context menu element if position is set
            let context_menu = self.context_menu_position.map(|pos| {
                self.render_context_menu(pos, cx)
            });

            // Render scrollbar
            let scrollbar = self.render_scrollbar(&id_suffix, cx);

            div()
                .id(format!("terminal-content-wrapper-{}", id_suffix))
                .size_full()
                .min_h_0()
                .relative()
                .bg(rgb(t.bg_primary))
                .cursor(CursorStyle::Arrow)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    // Close context menu on left click
                    if this.context_menu_position.is_some() {
                        this.hide_context_menu(cx);
                        return;
                    }
                    // End scrollbar drag if active
                    if this.scrollbar_dragging {
                        this.end_scrollbar_drag(cx);
                        return;
                    }
                    this.handle_mouse_down(event, window, cx);
                }))
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                    // Handle scrollbar drag first
                    if this.scrollbar_dragging {
                        let bounds_opt = *this.element_bounds.borrow();
                        if let Some(bounds) = bounds_opt {
                            let content_height = f32::from(bounds.size.height);
                            this.update_scrollbar_drag(f32::from(event.position.y), content_height, cx);
                        }
                        return;
                    }
                    this.handle_mouse_move(event, cx);
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                    // End scrollbar drag if active
                    if this.scrollbar_dragging {
                        this.end_scrollbar_drag(cx);
                        return;
                    }
                    this.handle_mouse_up(event, cx);
                }))
                .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                    // Use actual cell height from terminal, fallback to 17.0
                    let cell_height = this.terminal.as_ref()
                        .map(|t| t.cell_dimensions().1)
                        .unwrap_or(17.0);
                    let delta = event.delta.pixel_delta(px(cell_height));
                    this.handle_scroll(f32::from(delta.y), event.position, event.touch_phase, cx);
                }))
                .on_mouse_down(MouseButton::Right, cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    // Right-click: show context menu
                    this.show_context_menu(event.position, cx);
                }))
                .child(
                    canvas(
                        element_bounds_setter,
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                )
                .child(
                    div()
                        .size_full()
                        .p(px(4.0))
                        .bg(rgb(term_bg))
                        .child(
                            TerminalElement::new(terminal_clone, focus_handle.clone())
                                .with_search(self.search_matches.clone(), self.current_match_index)
                                .with_urls(Arc::new(self.url_matches.clone()), self.hovered_url_index)
                                .with_cursor_visible(self.cursor_visible)
                        )
                )
                // Scrollbar overlay
                .child(scrollbar)
                .children(context_menu)
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h(px(200.0))
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(t.text_muted))
                .child("Creating terminal...")
                .into_any_element()
        }
    }

    fn render_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let id_suffix = self
            .terminal_id
            .clone()
            .unwrap_or_else(|| format!("{}-{}", self.project_id, self.layout_path.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("-")));

        div()
            .flex()
            .flex_none()
            .gap(px(2.0))
            .opacity(0.0)
            .group_hover("terminal-header", |s| s.opacity(1.0))
            .child(
                // Split vertical button
                div()
                    .id(format!("split-vertical-btn-{}", id_suffix))
                    .cursor_pointer()
                    .w(px(22.0))
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.handle_split(SplitDirection::Vertical, cx);
                    }))
                    .child(
                        svg()
                            .path("icons/split-vertical.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary))
                    )
                    .tooltip(|_window, cx| Tooltip::new("Split Vertical").build(_window, cx)),
            )
            .child(
                // Split horizontal button
                div()
                    .id(format!("split-horizontal-btn-{}", id_suffix))
                    .cursor_pointer()
                    .w(px(22.0))
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.handle_split(SplitDirection::Horizontal, cx);
                    }))
                    .child(
                        svg()
                            .path("icons/split-horizontal.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary))
                    )
                    .tooltip(|_window, cx| Tooltip::new("Split Horizontal").build(_window, cx)),
            )
            .child(
                // Add tab button
                div()
                    .id(format!("add-tab-btn-{}", id_suffix))
                    .cursor_pointer()
                    .w(px(22.0))
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.handle_add_tab(cx);
                    }))
                    .child(
                        svg()
                            .path("icons/tabs.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary))
                    )
                    .tooltip(|_window, cx| Tooltip::new("Add Tab").build(_window, cx)),
            )
            .child(
                // Minimize button
                div()
                    .id(format!("minimize-btn-{}", id_suffix))
                    .cursor_pointer()
                    .w(px(22.0))
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.handle_minimize(cx);
                    }))
                    .child(
                        svg()
                            .path("icons/minimize.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary))
                    )
                    .tooltip(|_window, cx| Tooltip::new("Minimize").build(_window, cx)),
            )
            .when(self.pty_manager.supports_buffer_capture(), |el| {
                el.child(
                    // Export buffer button
                    div()
                        .id(format!("export-buffer-btn-{}", id_suffix))
                        .cursor_pointer()
                        .w(px(22.0))
                        .h(px(22.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.0))
                        .hover(|s| s.bg(rgb(t.bg_hover)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_click(cx.listener(|this, _, _window, cx| {
                            cx.stop_propagation();
                            this.handle_export_buffer(cx);
                        }))
                        .child(
                            svg()
                                .path("icons/copy.svg")
                                .size(px(14.0))
                                .text_color(rgb(t.text_secondary))
                        )
                        .tooltip(|_window, cx| Tooltip::new("Export Buffer to File").build(_window, cx)),
                )
            })
            .child(
                // Fullscreen button
                div()
                    .id(format!("fullscreen-btn-{}", id_suffix))
                    .cursor_pointer()
                    .w(px(22.0))
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.handle_fullscreen(cx);
                    }))
                    .child(
                        svg()
                            .path("icons/fullscreen.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary))
                    )
                    .tooltip(|_window, cx| Tooltip::new("Fullscreen").build(_window, cx)),
            )
            .child(
                // Detach button
                div()
                    .id(format!("detach-btn-{}", id_suffix))
                    .cursor_pointer()
                    .w(px(22.0))
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.handle_detach(cx);
                    }))
                    .child(
                        svg()
                            .path("icons/detach.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary))
                    )
                    .tooltip(|_window, cx| Tooltip::new("Detach to Window").build(_window, cx)),
            )
            .child(
                // Close button
                div()
                    .id(format!("close-btn-{}", id_suffix))
                    .cursor_pointer()
                    .w(px(22.0))
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .hover(|s| s.bg(rgba(0xf14c4c99)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        cx.stop_propagation();
                        this.handle_close(cx);
                    }))
                    .child(
                        svg()
                            .path("icons/close.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary))
                    )
                    .tooltip(|_window, cx| Tooltip::new("Close").build(_window, cx)),
            )
    }
}

impl Render for TerminalPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        // Create terminal if needed
        if self.terminal.is_none() && self.terminal_id.is_none() {
            self.create_new_terminal(cx);
        }

        let focus_handle = self.focus_handle.clone();
        let id_suffix = self
            .terminal_id
            .clone()
            .unwrap_or_else(|| format!("{}-{}", self.project_id, self.layout_path.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("-")));

        // Check if this terminal should be focused based on workspace state
        // This enables focusing from the sidebar
        // Skip if we're currently renaming, searching, or a modal is open - don't steal focus from inputs
        let is_modal = {
            let ws = self.workspace.read(cx);
            let is_modal = ws.focus_manager.is_modal();
            if !self.is_renaming && !self.is_searching && !is_modal {
                if let Some(ref focused) = ws.focused_terminal {
                    if focused.project_id == self.project_id && focused.layout_path == self.layout_path {
                        // This terminal should be focused
                        if !focus_handle.is_focused(_window) {
                            self.pending_focus = true;
                        }
                    }
                }
            }
            is_modal
        };

        // If we just created/attached a terminal, focus it once on the next render.
        // (Do it here because we have access to the Window.)
        // Skip if we're currently renaming, searching, or a modal is open
        if self.pending_focus && self.terminal.is_some() && !self.is_renaming && !self.is_searching && !is_modal {
            self.pending_focus = false;
            _window.focus(&self.focus_handle, cx);
        }

        // Check if this terminal is focused using the focus handle
        let is_focused = focus_handle.is_focused(_window);

        // Check if terminal has unread bell notification
        let has_bell = self.terminal.as_ref().map_or(false, |t| t.has_bell());

        // Clear bell when terminal gains focus
        if is_focused && has_bell {
            if let Some(ref terminal) = self.terminal {
                terminal.clear_bell();
            }
        }

        // Get show_focused_border setting from global settings
        let show_focused_border = settings(cx).show_focused_border;

        // Only show border when focused (if setting enabled) or has bell
        let show_border = (is_focused && show_focused_border) || has_bell;
        let border_color = if is_focused && show_focused_border {
            rgb(t.border_focused)
        } else {
            rgb(t.border_bell)
        };

        // Check if terminal is in a tab group (header will be hidden)
        let in_tab_group = self.is_in_tab_group(cx);

        div()
            .id(format!("terminal-pane-main-{}", id_suffix))
            .track_focus(&focus_handle)
            .key_context("TerminalPane")
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _event: &MouseDownEvent, window, cx| {
                log::debug!("TerminalPane mouse_down, focusing...");
                window.focus(&this.focus_handle, cx);
                // Update workspace focused terminal state
                this.workspace.update(cx, |ws, cx| {
                    ws.set_focused_terminal(this.project_id.clone(), this.layout_path.clone(), cx);
                });
            }))
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
                if !this.is_searching {
                    this.start_search(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CloseSearch, _window, cx| {
                if this.is_searching {
                    this.close_search(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SearchNext, _window, cx| {
                this.next_match(cx);
            }))
            .on_action(cx.listener(|this, _: &SearchPrev, _window, cx| {
                this.prev_match(cx);
            }))
            // Navigation actions
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
                // Toggle fullscreen: if already fullscreen, exit; otherwise enter fullscreen
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
                log::debug!("on_key_down TRIGGERED! keystroke: {:?}", event.keystroke);
                this.handle_key(event, cx);
            }))
            .on_click(cx.listener(|this, _, window, cx| {
                log::debug!("TerminalPane clicked, focusing...");
                window.focus(&this.focus_handle, cx);
                // Update workspace focused terminal state
                this.workspace.update(cx, |ws, cx| {
                    ws.set_focused_terminal(this.project_id.clone(), this.layout_path.clone(), cx);
                });
            }))
            // Handle file drops (drag & drop from external sources)
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, cx| {
                log::info!("Files dropped onto terminal pane");
                this.handle_file_drop(paths, cx);
            }))
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .bg(rgb(t.bg_primary))
            .when(show_border, |d| d.border_1().border_color(border_color))
            .group("terminal-pane")
            .when(!in_tab_group, |el| el.child(self.render_header(cx)))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .child(self.render_terminal_content(is_focused, cx))
            )
            .when(self.is_searching, |el: Stateful<Div>| {
                el.child(self.render_search_bar(cx))
            })
    }
}

impl Focusable for TerminalPane {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
