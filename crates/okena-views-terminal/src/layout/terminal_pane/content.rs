//! Terminal content component.

use crate::elements::terminal_element::{
    deregister_resize_viewer as deregister_shared_resize_viewer, next_resize_viewer_id, LinkKind,
    SearchMatch, TerminalElement,
};
use crate::terminal_view_settings;
use okena_terminal::terminal::Terminal;
use okena_files::theme::theme;
use okena_ui::color_utils::tint_color;
use crate::layout::navigation::register_pane_bounds;
use okena_workspace::state::{WindowId, Workspace};
use gpui::*;
use std::sync::Arc;
use std::time::Instant;

use super::scrollbar::Scrollbar;
use super::url_detector::UrlDetector;

/// Events emitted by terminal content.
pub enum TerminalContentEvent {
    RequestContextMenu {
        position: Point<Pixels>,
        has_selection: bool,
        link_url: Option<String>,
    },
}

/// Terminal content view handling display and mouse interactions.
pub struct TerminalContent {
    terminal: Option<Arc<Terminal>>,
    resize_viewer_id: u64,
    focus_handle: FocusHandle,
    url_detector: UrlDetector,
    scrollbar: Entity<Scrollbar>,
    is_selecting: bool,
    element_bounds: Option<Bounds<Pixels>>,
    last_click: Option<(Instant, usize, i32)>,
    click_count: u8,
    cursor_visible: bool,
    search_matches: Arc<Vec<SearchMatch>>,
    search_current_index: Option<usize>,
    project_id: String,
    layout_path: Vec<usize>,
    window_id: Option<WindowId>,
    workspace: Entity<Workspace>,
    scroll_accumulator: f32,
    mouse_down_cell: Option<(usize, i32)>,
    forwarded_button: Option<(u8, u8)>,
    last_focus_reported: Option<bool>,
}

impl TerminalContent {
    pub fn new(
        focus_handle: FocusHandle,
        window_id: Option<WindowId>,
        project_id: String,
        layout_path: Vec<usize>,
        workspace: Entity<Workspace>,
        cx: &mut Context<Self>,
    ) -> Self {
        let scrollbar = cx.new(|cx| Scrollbar::new(cx));

        Self {
            terminal: None,
            resize_viewer_id: next_resize_viewer_id(),
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
            window_id,
            workspace,
            scroll_accumulator: 0.0,
            mouse_down_cell: None,
            forwarded_button: None,
            last_focus_reported: None,
        }
    }

    fn mouse_modifier_bits(m: &Modifiers) -> u8 {
        let mut bits = 0u8;
        if m.shift {
            bits |= 4;
        }
        if m.alt {
            bits |= 8;
        }
        if m.control {
            bits |= 16;
        }
        bits
    }

    /// Forward a button press to the PTY if the app has mouse mode enabled.
    /// `button_code` is 0=left, 1=middle, 2=right. Returns true if forwarded.
    fn try_forward_mouse_press(
        &mut self,
        button_code: u8,
        event_position: Point<Pixels>,
        modifiers: &Modifiers,
    ) -> bool {
        let Some(terminal) = self.terminal.as_ref() else {
            return false;
        };
        if !terminal.is_mouse_mode() {
            return false;
        }
        // Shift bypasses mouse reporting so users can still select text
        // in apps like nano/tmux that capture mouse. Matches xterm/iTerm2/WezTerm.
        if modifiers.shift {
            return false;
        }
        let Some((col, row, _)) = self.pixel_to_cell(event_position) else {
            return false;
        };
        let mods = Self::mouse_modifier_bits(modifiers);
        terminal.send_mouse_button(button_code, true, col, row as usize, mods);
        self.forwarded_button = Some((button_code, mods));
        self.mouse_down_cell = None;
        self.is_selecting = false;
        true
    }

    /// Forward a button release to the PTY if that button's press was forwarded.
    /// Returns true if a release was sent (or the forwarded state was cleared).
    fn try_forward_mouse_release(
        &mut self,
        button_code: u8,
        event_position: Point<Pixels>,
        modifiers: &Modifiers,
    ) -> bool {
        let Some((forwarded, _)) = self.forwarded_button else {
            return false;
        };
        if forwarded != button_code {
            return false;
        }
        if let Some(terminal) = self.terminal.as_ref() {
            if let Some((col, row, _)) = self.pixel_to_cell(event_position) {
                let mods = Self::mouse_modifier_bits(modifiers);
                terminal.send_mouse_button(button_code, false, col, row as usize, mods);
            }
        }
        self.forwarded_button = None;
        self.mouse_down_cell = None;
        true
    }

    pub fn set_terminal(&mut self, terminal: Option<Arc<Terminal>>, cx: &mut Context<Self>) {
        if let Some(old_terminal) = self.terminal.as_ref() {
            let next_id = terminal.as_ref().map(|terminal| terminal.terminal_id.as_str());
            if next_id != Some(old_terminal.terminal_id.as_str()) {
                deregister_shared_resize_viewer(&old_terminal.terminal_id, self.resize_viewer_id);
            }
        }
        self.terminal = terminal.clone();
        self.scrollbar.update(cx, |scrollbar, _| {
            scrollbar.set_terminal(terminal);
        });
    }

    pub(crate) fn deregister_resize_viewer(&mut self) {
        if let Some(terminal) = self.terminal.as_ref() {
            deregister_shared_resize_viewer(&terminal.terminal_id, self.resize_viewer_id);
        }
    }

    pub fn set_cursor_visible(&mut self, visible: bool) {
        self.cursor_visible = visible;
    }

    pub fn set_search_highlights(
        &mut self,
        matches: Arc<Vec<SearchMatch>>,
        current_index: Option<usize>,
    ) {
        self.search_matches = matches;
        self.search_current_index = current_index;
    }

    pub fn mark_scroll_activity(&mut self, cx: &mut Context<Self>) {
        self.scrollbar.update(cx, |scrollbar, _| {
            scrollbar.mark_activity();
        });
    }

    pub fn handle_scroll(
        &mut self,
        delta: f32,
        position: Point<Pixels>,
        shift: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(ref terminal) = self.terminal {
            let (cell_width, cell_height) = terminal.cell_dimensions();

            if terminal.is_mouse_mode() && !shift {
                self.scroll_accumulator += delta;
                let lines = (self.scroll_accumulator / cell_height) as i32;
                if lines != 0 {
                    self.scroll_accumulator -= lines as f32 * cell_height;
                    let (col, row) = self.pixel_to_cell_raw(position, cell_width, cell_height);
                    let button = if lines > 0 { 64u8 } else { 65u8 };
                    terminal.send_mouse_scroll(button, col, row, lines.unsigned_abs() as usize);
                }
            } else {
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

    pub fn update_scrollbar_drag(&mut self, y: f32, cx: &mut Context<Self>) {
        if let Some(bounds) = self.element_bounds {
            let content_height = f32::from(bounds.size.height);
            self.scrollbar.update(cx, |scrollbar, cx| {
                scrollbar.update_drag(y, content_height, cx);
            });
        }
    }

    pub fn end_scrollbar_drag(&mut self, cx: &mut Context<Self>) {
        self.scrollbar.update(cx, |scrollbar, cx| {
            scrollbar.end_drag(cx);
        });
    }

    const TERMINAL_PADDING: f32 = 4.0;

    fn pixel_to_cell(&self, pos: Point<Pixels>) -> Option<(usize, i32, alacritty_terminal::index::Side)> {
        let bounds = self.element_bounds?;
        let terminal = self.terminal.as_ref()?;
        let (cell_width, cell_height) = terminal.cell_dimensions();

        let x = (f32::from(pos.x) - f32::from(bounds.origin.x) - Self::TERMINAL_PADDING).max(0.0);
        let y = (f32::from(pos.y) - f32::from(bounds.origin.y) - Self::TERMINAL_PADDING).max(0.0);

        let col_exact = x / cell_width;
        let col = col_exact.floor() as usize;
        let row = (y / cell_height).floor() as i32;

        let size = terminal.resize_state.lock();
        let col = col.min(size.size.cols.saturating_sub(1) as usize);
        let row = row.min(size.size.rows.saturating_sub(1) as i32);

        let side = if col_exact.fract() < 0.5 {
            alacritty_terminal::index::Side::Left
        } else {
            alacritty_terminal::index::Side::Right
        };

        Some((col, row, side))
    }

    fn pixel_to_cell_raw(&self, pos: Point<Pixels>, cell_width: f32, cell_height: f32) -> (usize, usize) {
        if let Some(bounds) = self.element_bounds {
            let x = (f32::from(pos.x) - f32::from(bounds.origin.x)).max(0.0);
            let y = (f32::from(pos.y) - f32::from(bounds.origin.y)).max(0.0);
            ((x / cell_width) as usize, (y / cell_height) as usize)
        } else {
            (0, 0)
        }
    }

    fn handle_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);

        let Some((col, row, side)) = self.pixel_to_cell(event.position) else {
            return;
        };
        self.mouse_down_cell = Some((col, row));

        if event.modifiers.platform || event.modifiers.control {
            if let Some(uri) = self.terminal.as_ref().and_then(|t| t.hyperlink_at(col, row)) {
                UrlDetector::open_url(&uri);
                self.mouse_down_cell = None;
                return;
            }
            if let Some(url_match) = self.url_detector.find_at(col, row) {
                match &url_match.kind {
                    LinkKind::Url => {
                        UrlDetector::open_url(&url_match.url);
                    }
                    LinkKind::FilePath { line, col } => {
                        let file_opener = terminal_view_settings(cx).file_opener.clone();
                        UrlDetector::open_file(&url_match.url, *line, *col, &file_opener);
                    }
                }
                self.mouse_down_cell = None;
                return;
            }
        }

        if self.try_forward_mouse_press(0, event.position, &event.modifiers) {
            cx.notify();
            return;
        }

        let now = Instant::now();

        let click_count = if let Some((last_time, last_col, last_row)) = self.last_click {
            let elapsed = now.duration_since(last_time).as_millis();
            let same_position =
                (col as i32 - last_col as i32).abs() <= 1 && (row - last_row).abs() <= 0;
            if elapsed < 400 && same_position {
                if self.click_count >= 3 { 1 } else { self.click_count + 1 }
            } else {
                1
            }
        } else {
            1
        };

        self.last_click = Some((now, col, row));
        self.click_count = click_count;

        let Some(terminal) = self.terminal.as_ref() else {
            return;
        };
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
                terminal.start_selection(col, row, side);
                self.is_selecting = true;
            }
        }
        cx.notify();
    }

    fn handle_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if let Some((col, row, _side)) = self.pixel_to_cell(event.position) {
            if self.url_detector.update_hover(col, row) {
                cx.notify();
            }
        } else if self.url_detector.clear_hover() {
            cx.notify();
        }

        if let Some((button, mods)) = self.forwarded_button {
            if let Some(ref terminal) = self.terminal {
                if terminal.supports_mouse_drag() {
                    if let Some((col, row, _side)) = self.pixel_to_cell(event.position) {
                        terminal.send_mouse_drag(button, col, row as usize, mods);
                    }
                }
            }
            return;
        }

        if self.is_selecting {
            if event.pressed_button != Some(MouseButton::Left) {
                if let Some(ref terminal) = self.terminal {
                    terminal.end_selection();
                    if !terminal.has_selection()
                        || terminal.get_selected_text().map(|s| s.is_empty()).unwrap_or(true)
                    {
                        terminal.clear_selection();
                    }
                }
                self.is_selecting = false;
                cx.notify();
                return;
            }

            if let Some(ref terminal) = self.terminal {
                if let Some((col, row, side)) = self.pixel_to_cell(event.position) {
                    terminal.update_selection(col, row, side);
                    cx.notify();
                }
            }
        }
    }

    fn handle_mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.try_forward_mouse_release(0, event.position, &event.modifiers) {
            cx.notify();
            return;
        }

        if self.is_selecting {
            if let Some(ref terminal) = self.terminal {
                terminal.end_selection();
                self.is_selecting = false;

                let empty_selection = !terminal.has_selection()
                    || terminal.get_selected_text().map(|s| s.is_empty()).unwrap_or(true);

                if empty_selection {
                    terminal.clear_selection();

                    // Click-to-cursor: on a clean single click (no drag), move cursor
                    if self.click_count == 1 {
                        if let Some((col, row)) = self.mouse_down_cell.take() {
                            if !terminal.is_mouse_mode() && !terminal.is_alt_screen() && !terminal.has_running_child() {
                                terminal.move_cursor_to_click(col, row);
                            }
                        }
                    }
                }
                cx.notify();
            }
        }

        // Sync any non-empty selection to PRIMARY so middle-click paste works
        // for drag, double-click (word), and triple-click (line) selections.
        #[cfg(target_os = "linux")]
        if let Some(ref terminal) = self.terminal {
            if let Some(text) = terminal.get_selected_text() {
                if !text.is_empty() {
                    cx.write_to_primary(ClipboardItem::new_string(text));
                }
            }
        }

        self.mouse_down_cell = None;
    }
}

impl Render for TerminalContent {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let is_focused = self.focus_handle.is_focused(window);

        if self.last_focus_reported != Some(is_focused) {
            if let Some(ref terminal) = self.terminal {
                if terminal.wants_focus_events() {
                    terminal.send_focus(is_focused);
                }
            }
            self.last_focus_reported = Some(is_focused);
        }

        if let Some(ref terminal) = self.terminal {
            terminal.set_palette(t);
            for text in terminal.take_pending_clipboard_writes() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }

        let base_bg = if is_focused {
            t.term_background
        } else {
            t.term_background_unfocused
        };

        let settings = crate::terminal_view_settings(cx);
        let bg_tint = if settings.color_tinted_background {
            let ws = self.workspace.read(cx);
            ws.project(&self.project_id).and_then(|p| {
                let color = ws.effective_folder_color(p);
                if color != okena_core::theme::FolderColor::Default {
                    Some(t.get_folder_color(color))
                } else {
                    None
                }
            })
        } else {
            None
        };
        let term_bg = match bg_tint {
            Some(tint) => tint_color(base_bg, tint, 0.025),
            None => base_bg,
        };

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
            let window_id = self.window_id;
            let project_id = self.project_id.clone();
            let layout_path = self.layout_path.clone();
            let fh = self.focus_handle.clone();
            move |bounds: Bounds<Pixels>, _window: &mut Window, cx: &mut App| {
                if let Some(window_id) = window_id {
                    register_pane_bounds(window_id, project_id.clone(), layout_path.clone(), bounds, Some(fh.clone()));
                }

                if let Some(entity) = entity.upgrade() {
                    entity.update(cx, |this, _| {
                        this.element_bounds = Some(bounds);
                    });
                }
            }
        };

        let render_settings = crate::terminal_view_settings(cx);

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
                if event.modifiers.shift {
                    return;
                }
                let delta = event.delta.pixel_delta(px(17.0));
                if event.modifiers.control {
                    let current_zoom = this.workspace.read(cx).get_terminal_zoom(&this.project_id, &this.layout_path);
                    let zoom_delta = if f32::from(delta.y) > 0.0 { 0.1 } else { -0.1 };
                    let new_zoom = (current_zoom + zoom_delta).clamp(0.5, 3.0);
                    let project_id = this.project_id.clone();
                    let layout_path = this.layout_path.clone();
                    this.workspace.update(cx, |workspace, cx| {
                        workspace.set_terminal_zoom(&project_id, &layout_path, new_zoom, cx);
                    });
                } else {
                    this.handle_scroll(f32::from(delta.y), event.position, event.modifiers.shift, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    if this.try_forward_mouse_press(2, event.position, &event.modifiers) {
                        cx.notify();
                        return;
                    }
                    let has_selection = this.terminal.as_ref().map(|t| t.has_selection()).unwrap_or(false);
                    let link_url = this.pixel_to_cell(event.position).and_then(|(col, row, _side)| {
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
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                    if this.try_forward_mouse_release(2, event.position, &event.modifiers) {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    if this.try_forward_mouse_press(1, event.position, &event.modifiers) {
                        cx.notify();
                        return;
                    }
                    #[cfg(target_os = "linux")]
                    if let Some(ref terminal) = this.terminal {
                        if let Some(item) = cx.read_from_primary() {
                            if let Some(text) = item.text() {
                                if !text.is_empty() {
                                    terminal.send_paste(&text);
                                }
                            }
                        }
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                    if this.try_forward_mouse_release(1, event.position, &event.modifiers) {
                        cx.notify();
                    }
                }),
            )
            .child(canvas(element_bounds_setter, |_, _, _, _| {}).absolute().size_full())
            .child(
                div()
                    .size_full()
                    .p(px(4.0))
                    .bg(rgb(term_bg))
                    .child(
                        TerminalElement::new(terminal_clone, focus_handle, self.resize_viewer_id)
                            .with_zoom(zoom_level)
                            .with_bg_tint(bg_tint)
                            .with_search(self.search_matches.clone(), self.search_current_index)
                            .with_urls(
                                self.url_detector.matches_arc(),
                                self.url_detector.hovered_group(),
                            )
                            .with_cursor_visible(self.cursor_visible)
                            .with_cursor_style(render_settings.cursor_style),
                    ),
            )
            .child(self.scrollbar.clone())
            .into_any_element()
    }
}

impl Drop for TerminalContent {
    fn drop(&mut self) {
        self.deregister_resize_viewer();
    }
}

impl EventEmitter<TerminalContentEvent> for TerminalContent {}
