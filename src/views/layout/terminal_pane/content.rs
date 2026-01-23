//! Terminal content component.
//!
//! An Entity with Render that handles terminal display, mouse interactions, and selection.

use crate::elements::terminal_element::{SearchMatch, TerminalElement};
use crate::terminal::terminal::Terminal;
use crate::theme::{theme, ThemeColors};
use crate::views::layout::navigation::register_pane_bounds;
use crate::workspace::state::{SplitDirection, Workspace};
use gpui::prelude::FluentBuilder;
use gpui::*;
use std::sync::Arc;
use std::time::Instant;

use super::scrollbar::Scrollbar;
use super::url_detector::UrlDetector;

/// Events emitted by context menu actions.
pub enum ContextMenuEvent {
    Copy,
    Paste,
    Clear,
    SelectAll,
    Split(SplitDirection),
    Close,
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
    /// Context menu position (if open)
    context_menu_position: Option<Point<Pixels>>,
    /// Whether terminal is focused
    is_focused: bool,
    /// Project ID for pane registration
    project_id: String,
    /// Layout path for pane registration
    layout_path: Vec<usize>,
    /// Workspace entity for accessing per-terminal zoom
    workspace: Entity<Workspace>,
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
            context_menu_position: None,
            is_focused: false,
            project_id,
            layout_path,
            workspace,
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

    /// Set focused state.
    pub fn set_focused(&mut self, focused: bool) {
        self.is_focused = focused;
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
            if terminal.is_mouse_mode() {
                // Forward scroll to PTY as mouse wheel events
                let (cell_width, cell_height) = terminal.cell_dimensions();
                let (col, row) = self.pixel_to_cell_raw(position, cell_width, cell_height);
                let lines = (delta.abs() / cell_height).max(1.0) as i32;
                let button = if delta > 0.0 { 64u8 } else { 65u8 };

                for _ in 0..lines {
                    terminal.send_mouse_scroll(button, col, row);
                }
            } else {
                // Normal scrollback scrolling
                let (_, cell_height) = terminal.cell_dimensions();
                let lines = (delta / cell_height) as i32;
                if lines > 0 {
                    terminal.scroll_up(lines);
                } else if lines < 0 {
                    terminal.scroll_down(-lines);
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

    /// Hide context menu.
    pub fn hide_context_menu(&mut self, cx: &mut Context<Self>) {
        self.context_menu_position = None;
        cx.notify();
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

        let size = terminal.size.lock();
        let col = col.min(size.cols.saturating_sub(1) as usize);
        let row = row.min(size.rows.saturating_sub(1) as i32);

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
                // Check for Ctrl+Click on URL
                if event.modifiers.control {
                    if let Some(url_match) = self.url_detector.find_at(col, row) {
                        UrlDetector::open_url(&url_match.url);
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

    /// Render context menu with click handlers.
    fn render_context_menu_with_handlers(
        &self,
        has_selection: bool,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let position = self.context_menu_position.unwrap_or_default();

        // Calculate menu height for positioning
        let menu_height = 9.0 * 26.0 + 3.0 * 9.0 + 8.0;

        // Calculate relative position and direction
        let (relative_pos, open_upward) = if let Some(bounds) = self.element_bounds {
            let rel_x = position.x - bounds.origin.x;
            let rel_y = position.y - bounds.origin.y;
            let space_below = f32::from(bounds.size.height) - f32::from(rel_y);
            let should_open_up = space_below < menu_height;
            (Point { x: rel_x, y: rel_y }, should_open_up)
        } else {
            (position, false)
        };

        // Capture colors for closures
        let bg_hover = t.bg_hover;

        let menu = div()
            .id("terminal-context-menu-interactive")
            .absolute()
            .left(relative_pos.x)
            .bg(rgb(t.bg_secondary))
            .border_1()
            .border_color(rgb(t.border))
            .rounded(px(4.0))
            .shadow_lg()
            .py(px(4.0))
            .min_w(px(120.0))
            // Stop propagation so parent doesn't hide menu before click is processed
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            // Copy
            .child(
                div()
                    .id("context-menu-copy-interactive")
                    .px(px(12.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(13.0))
                    .text_color(if has_selection {
                        rgb(t.text_primary)
                    } else {
                        rgb(t.text_muted)
                    })
                    .cursor(if has_selection {
                        CursorStyle::PointingHand
                    } else {
                        CursorStyle::Arrow
                    })
                    .when(has_selection, |el: Stateful<Div>| {
                        el.hover(move |s| s.bg(rgb(bg_hover)))
                    })
                    .when(has_selection, |el: Stateful<Div>| {
                        el.on_click(cx.listener(|this, _, _window, cx| {
                            cx.emit(ContextMenuEvent::Copy);
                            this.hide_context_menu(cx);
                        }))
                    })
                    .child(
                        svg()
                            .path("icons/copy.svg")
                            .size(px(14.0))
                            .text_color(if has_selection {
                                rgb(t.text_secondary)
                            } else {
                                rgb(t.text_muted)
                            }),
                    )
                    .child("Copy"),
            )
            // Paste
            .child(
                div()
                    .id("context-menu-paste-interactive")
                    .px(px(12.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(13.0))
                    .text_color(rgb(t.text_primary))
                    .hover(move |s| s.bg(rgb(bg_hover)))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _window, cx| {
                        cx.emit(ContextMenuEvent::Paste);
                        this.hide_context_menu(cx);
                    }))
                    .child(
                        svg()
                            .path("icons/clipboard-paste.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary)),
                    )
                    .child("Paste"),
            )
            // Separator
            .child(div().h(px(1.0)).mx(px(8.0)).my(px(4.0)).bg(rgb(t.border)))
            // Clear
            .child(
                div()
                    .id("context-menu-clear-interactive")
                    .px(px(12.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(13.0))
                    .text_color(rgb(t.text_primary))
                    .hover(move |s| s.bg(rgb(bg_hover)))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _window, cx| {
                        cx.emit(ContextMenuEvent::Clear);
                        this.hide_context_menu(cx);
                    }))
                    .child(
                        svg()
                            .path("icons/eraser.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary)),
                    )
                    .child("Clear"),
            )
            // Select All
            .child(
                div()
                    .id("context-menu-select-all-interactive")
                    .px(px(12.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(13.0))
                    .text_color(rgb(t.text_primary))
                    .hover(move |s| s.bg(rgb(bg_hover)))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _window, cx| {
                        cx.emit(ContextMenuEvent::SelectAll);
                        this.hide_context_menu(cx);
                    }))
                    .child(
                        svg()
                            .path("icons/select-all.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary)),
                    )
                    .child("Select All"),
            )
            // Separator
            .child(div().h(px(1.0)).mx(px(8.0)).my(px(4.0)).bg(rgb(t.border)))
            // Split Horizontal
            .child(
                div()
                    .id("context-menu-split-h-interactive")
                    .px(px(12.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(13.0))
                    .text_color(rgb(t.text_primary))
                    .hover(move |s| s.bg(rgb(bg_hover)))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _window, cx| {
                        cx.emit(ContextMenuEvent::Split(SplitDirection::Horizontal));
                        this.hide_context_menu(cx);
                    }))
                    .child(
                        svg()
                            .path("icons/split-horizontal.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary)),
                    )
                    .child("Split Horizontal"),
            )
            // Split Vertical
            .child(
                div()
                    .id("context-menu-split-v-interactive")
                    .px(px(12.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(13.0))
                    .text_color(rgb(t.text_primary))
                    .hover(move |s| s.bg(rgb(bg_hover)))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _window, cx| {
                        cx.emit(ContextMenuEvent::Split(SplitDirection::Vertical));
                        this.hide_context_menu(cx);
                    }))
                    .child(
                        svg()
                            .path("icons/split-vertical.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.text_secondary)),
                    )
                    .child("Split Vertical"),
            )
            // Separator
            .child(div().h(px(1.0)).mx(px(8.0)).my(px(4.0)).bg(rgb(t.border)))
            // Close
            .child(
                div()
                    .id("context-menu-close-interactive")
                    .px(px(12.0))
                    .py(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(13.0))
                    .text_color(rgb(t.error))
                    .hover(move |s| s.bg(rgb(bg_hover)))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _window, cx| {
                        cx.emit(ContextMenuEvent::Close);
                        this.hide_context_menu(cx);
                    }))
                    .child(
                        svg()
                            .path("icons/close.svg")
                            .size(px(14.0))
                            .text_color(rgb(t.error)),
                    )
                    .child("Close"),
            );

        // Position menu
        if open_upward {
            let bottom_offset = if let Some(bounds) = self.element_bounds {
                f32::from(bounds.size.height) - f32::from(relative_pos.y)
            } else {
                0.0
            };
            menu.bottom(px(bottom_offset))
        } else {
            menu.top(relative_pos.y)
        }
    }
}

impl Render for TerminalContent {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let term_bg = if self.is_focused {
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
        let has_selection = terminal.has_selection();
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
                    if this.context_menu_position.is_some() {
                        this.hide_context_menu(cx);
                        return;
                    }
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
                let delta = event.delta.pixel_delta(px(17.0));
                if event.modifiers.control {
                    // Ctrl+scroll = per-terminal zoom
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
                    this.context_menu_position = Some(event.position);
                    cx.notify();
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
                                self.url_detector.hovered_index(),
                            )
                            .with_cursor_visible(self.cursor_visible),
                    ),
            )
            .child(self.scrollbar.clone())
            // Context menu with click handlers
            .when(self.context_menu_position.is_some(), |el: Stateful<Div>| {
                el.child(self.render_context_menu_with_handlers(has_selection, &t, cx))
            })
            .into_any_element()
    }
}

impl EventEmitter<ContextMenuEvent> for TerminalContent {}
