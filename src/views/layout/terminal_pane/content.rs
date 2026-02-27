//! Terminal content component.
//!
//! An Entity with Render that handles terminal display, mouse interactions, and selection.

use crate::elements::terminal_element::{LinkKind, SearchMatch, TerminalElement};
use crate::settings::settings_entity;
use crate::terminal::terminal::Terminal;
use crate::theme::theme;
use crate::views::layout::navigation::register_pane_bounds;
use crate::workspace::state::Workspace;
use gpui::*;
use std::sync::Arc;
use std::time::Instant;

use super::scrollbar::Scrollbar;
use super::url_detector::UrlDetector;

/// Events emitted by terminal content.
pub enum TerminalContentEvent {
    /// Request to show context menu at a position.
    RequestContextMenu {
        position: Point<Pixels>,
        has_selection: bool,
        /// URL at the click position (if any), for "Open in Browser" / "Copy Link".
        link_url: Option<String>,
    },
}

/// Terminal content view handling display and mouse interactions.
pub struct TerminalContent {
    /// Terminal reference
    terminal: Option<Arc<Terminal>>,
    /// Focus handle from parent
    focus_handle: FocusHandle,
    /// URL detector
    url_detector: UrlDetector,
    /// Scrollbar child entity
    scrollbar: Entity<Scrollbar>,
    /// Whether currently selecting
    is_selecting: bool,
    /// Element bounds
    element_bounds: Option<Bounds<Pixels>>,
    /// Last click info for multi-click detection
    last_click: Option<(Instant, usize, i32)>,
    /// Click count
    click_count: u8,
    /// Cursor visibility for blink
    cursor_visible: bool,
    /// Search matches for highlighting
    search_matches: Arc<Vec<SearchMatch>>,
    /// Current search match index
    search_current_index: Option<usize>,
    /// Project ID for pane registration
    project_id: String,
    /// Layout path for pane registration
    layout_path: Vec<usize>,
    /// Workspace entity for accessing per-terminal zoom
    workspace: Entity<Workspace>,
    /// Accumulated scroll delta for smooth trackpad scrolling
    scroll_accumulator: f32,
}

impl TerminalContent {
    pub fn new(
        focus_handle: FocusHandle,
        project_id: String,
        layout_path: Vec<usize>,
        workspace: Entity<Workspace>,
        cx: &mut Context<Self>,
    ) -> Self {
        let scrollbar = cx.new(|cx| Scrollbar::new(cx));

        Self {
            terminal: None,
            focus_handle,
            url_detector: UrlDetector::new(),
            scrollbar,
            is_selecting: false,
            element_bounds: None,
            last_click: None,
            click_count: 0,
            cursor_visible: true,
            search_matches: Arc::new(Vec::new()),
            search_current_index: None,
            project_id,
            layout_path,
            workspace,
            scroll_accumulator: 0.0,
        }
    }

    /// Set terminal reference.
    pub fn set_terminal(&mut self, terminal: Option<Arc<Terminal>>, cx: &mut Context<Self>) {
        self.terminal = terminal.clone();
        self.scrollbar.update(cx, |scrollbar, _| {
            scrollbar.set_terminal(terminal);
        });
    }

    /// Set cursor visibility.
    pub fn set_cursor_visible(&mut self, visible: bool) {
        self.cursor_visible = visible;
    }

    /// Set search highlights.
    pub fn set_search_highlights(
        &mut self,
        matches: Arc<Vec<SearchMatch>>,
        current_index: Option<usize>,
    ) {
        self.search_matches = matches;
        self.search_current_index = current_index;
    }

    /// Mark scroll activity.
    pub fn mark_scroll_activity(&mut self, cx: &mut Context<Self>) {
        self.scrollbar.update(cx, |scrollbar, _| {
            scrollbar.mark_activity();
        });
    }

    /// Handle scroll.
    pub fn handle_scroll(
        &mut self,
        delta: f32,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if let Some(ref terminal) = self.terminal {
            let (cell_width, cell_height) = terminal.cell_dimensions();

            if terminal.is_mouse_mode() {
                // Forward scroll to PTY as mouse wheel events
                self.scroll_accumulator += delta;
                let lines = (self.scroll_accumulator / cell_height) as i32;
                if lines != 0 {
                    self.scroll_accumulator -= lines as f32 * cell_height;
                    let (col, row) = self.pixel_to_cell_raw(position, cell_width, cell_height);
                    let button = if lines > 0 { 64u8 } else { 65u8 };
                    for _ in 0..lines.abs() {
                        terminal.send_mouse_scroll(button, col, row);
                    }
                }
            } else {
                // Normal scrollback scrolling
                self.scroll_accumulator += delta;
                let lines = (self.scroll_accumulator / cell_height) as i32;
                if lines != 0 {
                    self.scroll_accumulator -= lines as f32 * cell_height;
                    if lines > 0 {
                        terminal.scroll_up(lines);
                    } else {
                        terminal.scroll_down(-lines);
                    }
                }
            }
            self.mark_scroll_activity(cx);
            cx.notify();
        }
    }

    /// Update scrollbar drag.
    pub fn update_scrollbar_drag(&mut self, y: f32, cx: &mut Context<Self>) {
        if let Some(bounds) = self.element_bounds {
            let content_height = f32::from(bounds.size.height);
            self.scrollbar.update(cx, |scrollbar, cx| {
                scrollbar.update_drag(y, content_height, cx);
            });
        }
    }

    /// End scrollbar drag.
    pub fn end_scrollbar_drag(&mut self, cx: &mut Context<Self>) {
        self.scrollbar.update(cx, |scrollbar, cx| {
            scrollbar.end_drag(cx);
        });
    }

    /// Convert pixel position to cell coordinates.
    fn pixel_to_cell(&self, pos: Point<Pixels>) -> Option<(usize, i32)> {
        let bounds = self.element_bounds?;
        let terminal = self.terminal.as_ref()?;
        let (cell_width, cell_height) = terminal.cell_dimensions();

        let x = (f32::from(pos.x) - f32::from(bounds.origin.x)).max(0.0);
        let y = (f32::from(pos.y) - f32::from(bounds.origin.y)).max(0.0);

        let col = (x / cell_width).floor() as usize;
        let row = (y / cell_height).floor() as i32;

        let size = terminal.resize_state.lock();
        let col = col.min(size.size.cols.saturating_sub(1) as usize);
        let row = row.min(size.size.rows.saturating_sub(1) as i32);

        Some((col, row))
    }

    /// Convert pixel to cell without bounds check.
    fn pixel_to_cell_raw(&self, pos: Point<Pixels>, cell_width: f32, cell_height: f32) -> (usize, usize) {
        if let Some(bounds) = self.element_bounds {
            let x = (f32::from(pos.x) - f32::from(bounds.origin.x)).max(0.0);
            let y = (f32::from(pos.y) - f32::from(bounds.origin.y)).max(0.0);
            ((x / cell_width) as usize, (y / cell_height) as usize)
        } else {
            (0, 0)
        }
    }

    /// Handle mouse down.
    fn handle_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);

        if let Some(ref terminal) = self.terminal {
            if let Some((col, row)) = self.pixel_to_cell(event.position) {
                // Check for Cmd+Click (macOS) / Ctrl+Click (Linux/Windows) on URL or file path
                if event.modifiers.platform || event.modifiers.control {
                    if let Some(url_match) = self.url_detector.find_at(col, row) {
                        match &url_match.kind {
                            LinkKind::Url => {
                                UrlDetector::open_url(&url_match.url);
                            }
                            LinkKind::FilePath { line, col } => {
                                let file_opener = settings_entity(cx).read(cx).settings.file_opener.clone();
                                UrlDetector::open_file(&url_match.url, *line, *col, &file_opener);
                            }
                        }
                        return;
                    }
                }

                let now = Instant::now();

                // Detect click count
                let click_count = if let Some((last_time, last_col, last_row)) = self.last_click {
                    let elapsed = now.duration_since(last_time).as_millis();
                    let same_position =
                        (col as i32 - last_col as i32).abs() <= 1 && (row - last_row).abs() <= 0;
                    if elapsed < 400 && same_position {
                        if self.click_count >= 3 {
                            1
                        } else {
                            self.click_count + 1
                        }
                    } else {
                        1
                    }
                } else {
                    1
                };

                self.last_click = Some((now, col, row));
                self.click_count = click_count;

                terminal.clear_selection();

                match click_count {
                    2 => {
                        terminal.start_word_selection(col, row);
                        self.is_selecting = false;
                    }
                    3 => {
                        terminal.start_line_selection(col, row);
                        self.is_selecting = false;
                    }
                    _ => {
                        terminal.start_selection(col, row);
                        self.is_selecting = true;
                    }
                }
                cx.notify();
            }
        }
    }

    /// Handle mouse move.
    fn handle_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        // Update URL hover state
        if let Some((col, row)) = self.pixel_to_cell(event.position) {
            if self.url_detector.update_hover(col, row) {
                cx.notify();
            }
        } else if self.url_detector.clear_hover() {
            cx.notify();
        }

        if self.is_selecting {
            if event.pressed_button != Some(MouseButton::Left) {
                if let Some(ref terminal) = self.terminal {
                    terminal.end_selection();
                    if !terminal.has_selection()
                        || terminal
                            .get_selected_text()
                            .map(|s| s.is_empty())
                            .unwrap_or(true)
                    {
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

    /// Handle mouse up.
    fn handle_mouse_up(&mut self, _event: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.is_selecting {
            if let Some(ref terminal) = self.terminal {
                terminal.end_selection();
                self.is_selecting = false;

                if !terminal.has_selection()
                    || terminal
                        .get_selected_text()
                        .map(|s| s.is_empty())
                        .unwrap_or(true)
                {
                    terminal.clear_selection();
                }
                cx.notify();
            }
        }
    }

}

impl Render for TerminalContent {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let term_bg = if self.focus_handle.is_focused(window) {
            t.term_background
        } else {
            t.term_background_unfocused
        };

        // Update URL matches
        self.url_detector.update_matches(&self.terminal);

        let Some(ref terminal) = self.terminal else {
            return div()
                .flex_1()
                .min_h(px(200.0))
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(t.text_muted))
                .child("Creating terminal...")
                .into_any_element();
        };

        let terminal_clone = terminal.clone();
        let focus_handle = self.focus_handle.clone();
        let zoom_level = self.workspace.read(cx).get_terminal_zoom(&self.project_id, &self.layout_path);

        let element_bounds_setter = {
            let entity = cx.entity().downgrade();
            let project_id = self.project_id.clone();
            let layout_path = self.layout_path.clone();
            move |bounds: Bounds<Pixels>, _window: &mut Window, cx: &mut App| {
                register_pane_bounds(project_id.clone(), layout_path.clone(), bounds);

                if let Some(entity) = entity.upgrade() {
                    entity.update(cx, |this, _| {
                        this.element_bounds = Some(bounds);
                    });
                }
            }
        };

        div()
            .id("terminal-content")
            .size_full()
            .min_h_0()
            .relative()
            .bg(rgb(t.bg_primary))
            .cursor(CursorStyle::Arrow)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    if this.scrollbar.read(cx).is_dragging() {
                        this.end_scrollbar_drag(cx);
                        return;
                    }
                    this.handle_mouse_down(event, window, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.scrollbar.read(cx).is_dragging() {
                    this.update_scrollbar_drag(f32::from(event.position.y), cx);
                    return;
                }
                this.handle_mouse_move(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                    if this.scrollbar.read(cx).is_dragging() {
                        this.end_scrollbar_drag(cx);
                        return;
                    }
                    this.handle_mouse_up(event, cx);
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                // Shift+scroll is reserved for horizontal project column scrolling
                if event.modifiers.shift {
                    return;
                }
                let delta = event.delta.pixel_delta(px(17.0));
                if event.modifiers.control {
                    // Ctrl+scroll = per-terminal zoom (Linux/Windows)
                    let current_zoom = this.workspace.read(cx).get_terminal_zoom(&this.project_id, &this.layout_path);
                    let zoom_delta = if f32::from(delta.y) > 0.0 { 0.1 } else { -0.1 };
                    let new_zoom = (current_zoom + zoom_delta).clamp(0.5, 3.0);
                    let project_id = this.project_id.clone();
                    let layout_path = this.layout_path.clone();
                    this.workspace.update(cx, |workspace, cx| {
                        workspace.set_terminal_zoom(&project_id, &layout_path, new_zoom, cx);
                    });
                } else {
                    this.handle_scroll(f32::from(delta.y), event.position, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    let has_selection = this.terminal.as_ref().map(|t| t.has_selection()).unwrap_or(false);
                    // Detect URL at click position for context menu link actions
                    let link_url = this.pixel_to_cell(event.position).and_then(|(col, row)| {
                        this.url_detector.find_at(col, row)
                            .filter(|m| m.kind == LinkKind::Url)
                            .map(|m| m.url)
                    });
                    cx.emit(TerminalContentEvent::RequestContextMenu {
                        position: event.position,
                        has_selection,
                        link_url,
                    });
                }),
            )
            .child(canvas(element_bounds_setter, |_, _, _, _| {}).absolute().size_full())
            .child(
                div()
                    .size_full()
                    .p(px(4.0))
                    .bg(rgb(term_bg))
                    .child(
                        TerminalElement::new(terminal_clone, focus_handle)
                            .with_zoom(zoom_level)
                            .with_search(self.search_matches.clone(), self.search_current_index)
                            .with_urls(
                                self.url_detector.matches_arc(),
                                self.url_detector.hovered_group(),
                            )
                            .with_cursor_visible(self.cursor_visible)
                            .with_cursor_style(settings_entity(cx).read(cx).settings.cursor_style),
                    ),
            )
            .child(self.scrollbar.clone())
            .into_any_element()
    }
}

impl EventEmitter<TerminalContentEvent> for TerminalContent {}
