use crate::theme::theme;
use crate::views::components::code_view::find_word_boundaries;
use gpui::prelude::*;
use gpui::*;

use std::ops::Range;
use std::time::Duration;

/// Event emitted when input value changes
pub struct InputChangedEvent;

/// Result of key handling
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum KeyHandled {
    /// Key was handled, stop propagation
    Handled,
    /// Key was not handled, let parent handle (e.g., Enter, Escape with no selection)
    NotHandled,
    /// Key was ignored (modifier-only, function keys), don't stop propagation
    Ignored,
}

/// A simple text input state that doesn't rely on gpui-component's Root entity
pub struct SimpleInputState {
    focus_handle: FocusHandle,
    value: String,
    placeholder: String,
    cursor_position: usize,
    selection: Option<Range<usize>>,
    cursor_visible: bool,
    _blink_task: Option<Task<()>>,
    icon: Option<SharedString>,
    highlight_vars: bool,
    multiline: bool,
    input_bounds: Option<Bounds<Pixels>>,
    /// Per-line TextLayouts for accurate click-to-cursor mapping via index_for_position().
    text_layouts: Vec<TextLayout>,
    /// Whether the user is currently dragging to select text.
    is_selecting: bool,
    /// Anchor position (char offset) for drag selection — where the mouse-down started.
    select_anchor: usize,
}

impl SimpleInputState {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();

        // Start cursor blink task
        let blink_task = cx.spawn(async move |this: WeakEntity<SimpleInputState>, cx| {
            loop {
                smol::Timer::after(Duration::from_millis(530)).await;
                let result = cx.update(|cx| {
                    this.update(cx, |state, cx| {
                        state.cursor_visible = !state.cursor_visible;
                        cx.notify();
                    })
                });
                if result.is_err() {
                    break;
                }
            }
        });

        Self {
            focus_handle,
            value: String::new(),
            placeholder: String::new(),
            cursor_position: 0,
            selection: None,
            cursor_visible: true,
            _blink_task: Some(blink_task),
            icon: None,
            highlight_vars: false,
            multiline: false,
            input_bounds: None,
            text_layouts: Vec::new(),
            is_selecting: false,
            select_anchor: 0,
        }
    }

    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Set placeholder text on an existing instance (for use with &mut self).
    pub fn set_placeholder(&mut self, placeholder: impl Into<String>) {
        self.placeholder = placeholder.into();
    }

    pub fn default_value(mut self, value: impl Into<String>) -> Self {
        let v = value.into();
        self.cursor_position = v.len();
        self.value = v;
        self
    }

    pub fn icon(mut self, icon: impl Into<SharedString>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Enable highlighting of `{var}` template variables in a distinct color.
    pub fn highlight_vars(mut self) -> Self {
        self.highlight_vars = true;
        self
    }

    /// Enable multiline mode: Enter inserts newline, paste preserves lines, Up/Down navigate.
    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn set_value(&mut self, value: impl Into<String>, cx: &mut Context<Self>) {
        let v = value.into();
        let changed = v != self.value;
        self.cursor_position = v.chars().count();
        self.value = v;
        self.selection = None;
        if changed {
            cx.emit(InputChangedEvent);
        }
        cx.notify();
    }

    pub fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
    }

    fn reset_cursor_blink(&mut self) {
        self.cursor_visible = true;
    }

    fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        // Delete selection first if any
        if let Some(range) = self.selection.take() {
            self.value.replace_range(range.clone(), "");
            self.cursor_position = range.start;
        }

        // Insert at cursor position
        let byte_pos = self.byte_position_for_char(self.cursor_position);
        self.value.insert_str(byte_pos, text);
        self.cursor_position += text.chars().count();
        self.reset_cursor_blink();
        cx.emit(InputChangedEvent);
        cx.notify();
    }

    fn delete_backward(&mut self, cx: &mut Context<Self>) {
        let had_content = !self.value.is_empty() || self.selection.is_some();
        if let Some(range) = self.selection.take() {
            // Delete selection
            self.value
                .replace_range(self.byte_range_for_chars(&range), "");
            self.cursor_position = range.start;
        } else if self.cursor_position > 0 {
            // Delete character before cursor
            let prev_pos = self.cursor_position - 1;
            let byte_range = self.byte_range_for_chars(&(prev_pos..self.cursor_position));
            self.value.replace_range(byte_range, "");
            self.cursor_position = prev_pos;
        }
        self.reset_cursor_blink();
        if had_content {
            cx.emit(InputChangedEvent);
        }
        cx.notify();
    }

    fn delete_forward(&mut self, cx: &mut Context<Self>) {
        let had_content = !self.value.is_empty() || self.selection.is_some();
        if let Some(range) = self.selection.take() {
            // Delete selection
            self.value
                .replace_range(self.byte_range_for_chars(&range), "");
            self.cursor_position = range.start;
        } else {
            let char_count = self.value.chars().count();
            if self.cursor_position < char_count {
                let next_pos = self.cursor_position + 1;
                let byte_range = self.byte_range_for_chars(&(self.cursor_position..next_pos));
                self.value.replace_range(byte_range, "");
            }
        }
        self.reset_cursor_blink();
        if had_content {
            cx.emit(InputChangedEvent);
        }
        cx.notify();
    }

    fn move_cursor_left(&mut self, extend_selection: bool, cx: &mut Context<Self>) {
        if self.cursor_position > 0 {
            let old_pos = self.cursor_position;
            self.cursor_position -= 1;

            if extend_selection {
                self.extend_selection(old_pos, self.cursor_position);
            } else if self.selection.is_some() {
                // Move cursor to start of selection
                self.cursor_position = self.selection.as_ref().unwrap().start;
                self.selection = None;
            }
            if !extend_selection {
                self.selection = None;
            }
            self.reset_cursor_blink();
            cx.notify();
        } else if !extend_selection && self.selection.is_some() {
            self.selection = None;
            cx.notify();
        }
    }

    fn move_cursor_right(&mut self, extend_selection: bool, cx: &mut Context<Self>) {
        let char_count = self.value.chars().count();
        if self.cursor_position < char_count {
            let old_pos = self.cursor_position;
            self.cursor_position += 1;

            if extend_selection {
                self.extend_selection(old_pos, self.cursor_position);
            } else if self.selection.is_some() {
                // Move cursor to end of selection
                self.cursor_position = self.selection.as_ref().unwrap().end;
                self.selection = None;
            }
            if !extend_selection {
                self.selection = None;
            }
            self.reset_cursor_blink();
            cx.notify();
        } else if !extend_selection && self.selection.is_some() {
            self.selection = None;
            cx.notify();
        }
    }

    fn move_to_start(&mut self, extend_selection: bool, cx: &mut Context<Self>) {
        let old_pos = self.cursor_position;
        self.cursor_position = 0;

        if extend_selection && old_pos > 0 {
            self.extend_selection(old_pos, 0);
        } else {
            self.selection = None;
        }
        self.reset_cursor_blink();
        cx.notify();
    }

    fn move_to_end(&mut self, extend_selection: bool, cx: &mut Context<Self>) {
        let old_pos = self.cursor_position;
        let char_count = self.value.chars().count();
        self.cursor_position = char_count;

        if extend_selection && old_pos < char_count {
            self.extend_selection(old_pos, char_count);
        } else {
            self.selection = None;
        }
        self.reset_cursor_blink();
        cx.notify();
    }

    /// Extend selection from anchor to new position
    fn extend_selection(&mut self, anchor: usize, new_pos: usize) {
        let (start, end) = if let Some(ref sel) = self.selection {
            // Extend existing selection
            if anchor == sel.end {
                // Extending from end
                if new_pos < sel.start {
                    (new_pos, sel.start)
                } else {
                    (sel.start, new_pos)
                }
            } else {
                // Extending from start
                if new_pos > sel.end {
                    (sel.end, new_pos)
                } else {
                    (new_pos, sel.end)
                }
            }
        } else {
            // Start new selection
            (anchor.min(new_pos), anchor.max(new_pos))
        };
        if start != end {
            self.selection = Some(start..end);
        } else {
            self.selection = None;
        }
    }

    /// Clear selection without other side effects
    fn clear_selection(&mut self, cx: &mut Context<Self>) -> bool {
        if self.selection.is_some() {
            self.selection = None;
            cx.notify();
            true
        } else {
            false
        }
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        let char_count = self.value.chars().count();
        if char_count > 0 {
            self.selection = Some(0..char_count);
            self.cursor_position = char_count;
            self.reset_cursor_blink();
            cx.notify();
        }
    }

    fn byte_position_for_char(&self, char_pos: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_pos)
            .map(|(i, _)| i)
            .unwrap_or(self.value.len())
    }

    fn byte_range_for_chars(&self, char_range: &Range<usize>) -> Range<usize> {
        let start = self.byte_position_for_char(char_range.start);
        let end = self.byte_position_for_char(char_range.end);
        start..end
    }

    /// Get (line_index, col_chars) for the current cursor position.
    fn cursor_line_col(&self) -> (usize, usize) {
        let byte_pos = self.byte_position_for_char(self.cursor_position);
        let before = &self.value[..byte_pos];
        let line_idx = before.matches('\n').count();
        let col_bytes = before.rfind('\n').map_or(before.len(), |i| before.len() - i - 1);
        let col_chars = before[before.len() - col_bytes..].chars().count();
        (line_idx, col_chars)
    }

    /// Get the char offset of the start of a given line.
    fn line_start_char(&self, target_line: usize) -> usize {
        let mut line = 0;
        for (i, c) in self.value.chars().enumerate() {
            if line == target_line {
                return i;
            }
            if c == '\n' {
                line += 1;
            }
        }
        if line == target_line {
            self.value.chars().count()
        } else {
            0
        }
    }

    /// Get the char count of a given line (not counting '\n').
    fn line_char_count(&self, target_line: usize) -> usize {
        self.value
            .split('\n')
            .nth(target_line)
            .map_or(0, |l| l.chars().count())
    }

    fn move_cursor_up(&mut self, extend_selection: bool, cx: &mut Context<Self>) {
        let (line, col) = self.cursor_line_col();
        if line > 0 {
            let old_pos = self.cursor_position;
            let prev_line_start = self.line_start_char(line - 1);
            let prev_line_len = self.line_char_count(line - 1);
            self.cursor_position = prev_line_start + col.min(prev_line_len);
            if extend_selection {
                self.extend_selection(old_pos, self.cursor_position);
            } else {
                self.selection = None;
            }
            self.reset_cursor_blink();
            cx.notify();
        }
    }

    fn move_cursor_down(&mut self, extend_selection: bool, cx: &mut Context<Self>) {
        let (line, col) = self.cursor_line_col();
        let total_lines = self.value.split('\n').count();
        if line + 1 < total_lines {
            let old_pos = self.cursor_position;
            let next_line_start = self.line_start_char(line + 1);
            let next_line_len = self.line_char_count(line + 1);
            self.cursor_position = next_line_start + col.min(next_line_len);
            if extend_selection {
                self.extend_selection(old_pos, self.cursor_position);
            } else {
                self.selection = None;
            }
            self.reset_cursor_blink();
            cx.notify();
        }
    }

    /// Resolve a mouse position to a char offset using stored text_layouts and input_bounds.
    fn char_position_for_mouse(&self, position: Point<Pixels>) -> usize {
        if self.multiline && self.value.contains('\n') {
            if let Some(bounds) = self.input_bounds {
                let line_height: f32 = 18.0;
                let top_padding: f32 = 4.0;
                let click_y = f32::from(position.y) - f32::from(bounds.origin.y) - top_padding;
                let clicked_line = (click_y / line_height).floor().max(0.0) as usize;
                let total_lines = self.value.split('\n').count();
                let line_idx = clicked_line.min(total_lines - 1);

                let col = if line_idx < self.text_layouts.len() {
                    self.text_layouts[line_idx]
                        .index_for_position(position)
                        .unwrap_or_else(|ix| ix)
                        .min(self.line_char_count(line_idx))
                } else {
                    0
                };
                self.line_start_char(line_idx) + col
            } else {
                self.value.chars().count()
            }
        } else {
            let char_count = self.value.chars().count();
            if let Some(layout) = self.text_layouts.first() {
                layout
                    .index_for_position(position)
                    .unwrap_or_else(|ix| ix)
                    .min(char_count)
            } else {
                char_count
            }
        }
    }

    /// Select the word around the given char position.
    fn select_word_at(&mut self, pos: usize, cx: &mut Context<Self>) {
        let (start, end) = find_word_boundaries(&self.value, pos);
        if start != end {
            self.selection = Some(start..end);
            self.cursor_position = end;
        }
        self.reset_cursor_blink();
        cx.notify();
    }

    /// Select the entire line containing the given char position (for multiline), or select all.
    fn select_line_at(&mut self, pos: usize, cx: &mut Context<Self>) {
        if self.multiline && self.value.contains('\n') {
            // Find which line contains pos
            let byte_pos = self.byte_position_for_char(pos);
            let before = &self.value[..byte_pos];
            let line_idx = before.matches('\n').count();
            let start = self.line_start_char(line_idx);
            let end = start + self.line_char_count(line_idx);
            self.selection = Some(start..end);
            self.cursor_position = end;
        } else {
            self.select_all(cx);
            return;
        }
        self.reset_cursor_blink();
        cx.notify();
    }

    /// Handle key down event. Returns KeyHandled enum indicating how the key was processed.
    fn handle_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> KeyHandled {
        let key = event.keystroke.key.as_str();
        let modifiers = &event.keystroke.modifiers;
        let extend_selection = modifiers.shift;

        // Handle special keys
        match key {
            "backspace" => {
                self.delete_backward(cx);
                return KeyHandled::Handled;
            }
            "delete" => {
                self.delete_forward(cx);
                return KeyHandled::Handled;
            }
            "left" => {
                if modifiers.platform || modifiers.control {
                    self.move_to_start(extend_selection, cx);
                } else {
                    self.move_cursor_left(extend_selection, cx);
                }
                return KeyHandled::Handled;
            }
            "right" => {
                if modifiers.platform || modifiers.control {
                    self.move_to_end(extend_selection, cx);
                } else {
                    self.move_cursor_right(extend_selection, cx);
                }
                return KeyHandled::Handled;
            }
            "home" => {
                self.move_to_start(extend_selection, cx);
                return KeyHandled::Handled;
            }
            "end" => {
                self.move_to_end(extend_selection, cx);
                return KeyHandled::Handled;
            }
            "a" if modifiers.platform || modifiers.control => {
                self.select_all(cx);
                return KeyHandled::Handled;
            }
            "v" if modifiers.platform || modifiers.control => {
                if let Some(clipboard_item) = cx.read_from_clipboard() {
                    if let Some(text) = clipboard_item.text() {
                        if self.multiline {
                            if !text.is_empty() {
                                self.insert_text(&text, cx);
                            }
                        } else {
                            // Only insert first line (no newlines in single-line input)
                            let line = text.lines().next().unwrap_or("");
                            if !line.is_empty() {
                                self.insert_text(line, cx);
                            }
                        }
                    }
                }
                return KeyHandled::Handled;
            }
            "c" if modifiers.platform || modifiers.control => {
                if let Some(ref sel) = self.selection {
                    let byte_range = self.byte_range_for_chars(sel);
                    let selected_text = &self.value[byte_range];
                    cx.write_to_clipboard(ClipboardItem::new_string(selected_text.to_string()));
                }
                return KeyHandled::Handled;
            }
            "x" if modifiers.platform || modifiers.control => {
                if let Some(ref sel) = self.selection {
                    let byte_range = self.byte_range_for_chars(sel);
                    let selected_text = &self.value[byte_range];
                    cx.write_to_clipboard(ClipboardItem::new_string(selected_text.to_string()));
                }
                // Delete selection (reuse delete_backward which handles selection)
                if self.selection.is_some() {
                    self.delete_backward(cx);
                }
                return KeyHandled::Handled;
            }
            "escape" => {
                // If there's a selection, clear it. Otherwise let parent handle.
                if self.clear_selection(cx) {
                    return KeyHandled::Handled;
                }
                return KeyHandled::NotHandled;
            }
            "enter" => {
                if self.multiline {
                    self.insert_text("\n", cx);
                    return KeyHandled::Handled;
                }
                return KeyHandled::NotHandled;
            }
            "tab" => {
                return KeyHandled::NotHandled;
            }
            "up" if self.multiline => {
                self.move_cursor_up(extend_selection, cx);
                return KeyHandled::Handled;
            }
            "down" if self.multiline => {
                self.move_cursor_down(extend_selection, cx);
                return KeyHandled::Handled;
            }
            // Skip modifier-only and function keys
            "shift" | "control" | "alt" | "meta" | "capslock"
            | "f1" | "f2" | "f3" | "f4" | "f5" | "f6" | "f7" | "f8" | "f9" | "f10" | "f11"
            | "f12" | "up" | "down" | "pageup" | "pagedown" => {
                return KeyHandled::Ignored;
            }
            _ => {}
        }

        // Handle character input via key_char (it's a String, not a char)
        if let Some(ref s) = event.keystroke.key_char {
            // Skip control characters (except for normal space/printable)
            if !s.is_empty() && !s.chars().next().map_or(true, |c| c.is_control() && c != ' ') {
                self.insert_text(s, cx);
                return KeyHandled::Handled;
            }
        }

        KeyHandled::Ignored
    }

}

/// Parse text into segments of (text, is_variable) based on `{...}` patterns.
fn var_segments(text: &str) -> Vec<(&str, bool)> {
    let mut segments = Vec::new();
    let mut last_end = 0;
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(close) = text[i..].find('}') {
                if i > last_end {
                    segments.push((&text[last_end..i], false));
                }
                let end = i + close + 1;
                segments.push((&text[i..end], true));
                last_end = end;
                i = end;
                continue;
            }
        }
        i += 1;
    }
    if last_end < text.len() {
        segments.push((&text[last_end..], false));
    }
    segments
}

impl Render for SimpleInputState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let is_focused = self.focus_handle.is_focused(window);
        let value = self.value.clone();
        let placeholder = self.placeholder.clone();
        let cursor_position = self.cursor_position;
        let selection = self.selection.clone();
        let cursor_visible = self.cursor_visible && is_focused;
        let icon = self.icon.clone();

        let cursor_byte = self.byte_position_for_char(cursor_position);

        let show_placeholder = value.is_empty() && !is_focused;
        let highlight_vars = self.highlight_vars;
        let multiline = self.multiline;
        let var_color = t.term_cyan;
        let cursor_color = rgb(t.text_primary);

        // Build StyledText per line — used for both rendering AND click/cursor mapping.
        // Text is rendered as single unbroken elements so shaping is preserved.
        self.text_layouts.clear();

        let content: AnyElement = if show_placeholder {
            if highlight_vars {
                let mut highlights = Vec::new();
                for (seg, is_var) in var_segments(&placeholder) {
                    if is_var {
                        let start = seg.as_ptr() as usize - placeholder.as_ptr() as usize;
                        highlights.push((start..start + seg.len(), HighlightStyle {
                            color: Some(rgb(var_color).into()),
                            ..Default::default()
                        }));
                    }
                }
                div()
                    .text_color(rgb(t.text_muted))
                    .child(StyledText::new(placeholder).with_highlights(highlights))
                    .into_any_element()
            } else {
                div()
                    .text_color(rgb(t.text_muted))
                    .child(placeholder)
                    .into_any_element()
            }
        } else if multiline && value.contains('\n') {
            // Multiline: each line as a StyledText in its own row
            let lines: Vec<&str> = value.split('\n').collect();
            let (cursor_line, _) = self.cursor_line_col();

            let mut container = div().flex().flex_col().text_color(cursor_color).relative();
            let mut byte_offset = 0;

            for (line_idx, line_text) in lines.iter().enumerate() {
                let display = if line_text.is_empty() { "\u{200B}".to_string() } else { line_text.to_string() };
                let line_char_count = line_text.chars().count();

                // Compute per-line selection highlights
                let styled = if let Some(ref sel) = selection {
                    let line_start = self.line_start_char(line_idx);
                    let line_end = line_start + line_char_count;
                    let sel_start_in_line = sel.start.max(line_start).saturating_sub(line_start);
                    let sel_end_in_line = sel.end.min(line_end).saturating_sub(line_start);
                    if sel_start_in_line < sel_end_in_line && sel.end > line_start && sel.start < line_end {
                        // Compute byte offsets within this line's text
                        let sel_start_byte: usize = line_text.char_indices().nth(sel_start_in_line).map(|(i, _)| i).unwrap_or(line_text.len());
                        let sel_end_byte: usize = line_text.char_indices().nth(sel_end_in_line).map(|(i, _)| i).unwrap_or(line_text.len());
                        let highlights = vec![(sel_start_byte..sel_end_byte, HighlightStyle {
                            background_color: Some(rgb(t.selection_bg).into()),
                            color: Some(rgb(t.selection_fg).into()),
                            ..Default::default()
                        })];
                        StyledText::new(display).with_highlights(highlights)
                    } else {
                        StyledText::new(display)
                    }
                } else {
                    StyledText::new(display)
                };
                let layout = styled.layout().clone();
                self.text_layouts.push(layout.clone());

                let local_cursor_byte = cursor_byte.saturating_sub(byte_offset);
                let is_cursor_line = line_idx == cursor_line;

                container = container.child(
                    div()
                        .relative()
                        .min_h(px(18.0))
                        .child(styled)
                        .when(is_cursor_line, |d| {
                            d.child(cursor_canvas(layout, local_cursor_byte, cursor_visible, cursor_color))
                        })
                );

                byte_offset += line_text.len() + 1; // +1 for '\n'
            }
            container.into_any_element()
        } else {
            // Single-line: one StyledText with optional highlights
            let styled = if let Some(ref sel) = selection {
                let sel_start_byte = self.byte_position_for_char(sel.start);
                let sel_end_byte = self.byte_position_for_char(sel.end);
                let highlights = vec![(sel_start_byte..sel_end_byte, HighlightStyle {
                    background_color: Some(rgb(t.selection_bg).into()),
                    color: Some(rgb(t.selection_fg).into()),
                    ..Default::default()
                })];
                StyledText::new(value.clone()).with_highlights(highlights)
            } else if highlight_vars {
                let mut highlights = Vec::new();
                for (seg, is_var) in var_segments(&value) {
                    if is_var {
                        let start = seg.as_ptr() as usize - value.as_ptr() as usize;
                        highlights.push((start..start + seg.len(), HighlightStyle {
                            color: Some(rgb(var_color).into()),
                            ..Default::default()
                        }));
                    }
                }
                StyledText::new(value.clone()).with_highlights(highlights)
            } else {
                StyledText::new(value.clone())
            };

            let layout = styled.layout().clone();
            self.text_layouts.push(layout.clone());

            div()
                .relative()
                .text_color(cursor_color)
                .child(styled)
                .child(cursor_canvas(layout, cursor_byte, cursor_visible, cursor_color))
                .into_any_element()
        };

        div()
            .id("simple-input")
            .track_focus(&focus_handle)
            .relative()
            .flex()
            .when(multiline, |d| d.items_start())
            .when(!multiline, |d| d.items_center())
            .gap(px(6.0))
            .w_full()
            .when(multiline, |d| d.min_h(px(24.0)).py(px(4.0)))
            .when(!multiline, |d| d.h(px(24.0)))
            .px(px(8.0))
            .cursor_text()
            .child(canvas({
                let entity = cx.entity().downgrade();
                move |bounds, _, cx: &mut App| {
                    if let Some(entity) = entity.upgrade() {
                        entity.update(cx, |this, _| {
                            this.input_bounds = Some(bounds);
                        });
                    }
                }
            }, |_, _, _, _| {}).absolute().size_full())
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                this.focus(window, cx);
                let pos = this.char_position_for_mouse(event.position);

                if event.click_count >= 3 {
                    // Triple-click: select line (multiline) or all
                    this.is_selecting = false;
                    this.select_line_at(pos, cx);
                } else if event.click_count == 2 {
                    // Double-click: select word
                    this.is_selecting = false;
                    this.select_word_at(pos, cx);
                } else {
                    // Single click: position cursor, start drag selection
                    this.cursor_position = pos;
                    this.selection = None;
                    this.is_selecting = true;
                    this.select_anchor = pos;
                    this.reset_cursor_blink();
                    cx.notify();
                }
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.is_selecting {
                    if event.pressed_button != Some(MouseButton::Left) {
                        this.is_selecting = false;
                        return;
                    }
                    let pos = this.char_position_for_mouse(event.position);
                    this.cursor_position = pos;
                    let anchor = this.select_anchor;
                    if pos != anchor {
                        this.selection = Some(anchor.min(pos)..anchor.max(pos));
                    } else {
                        this.selection = None;
                    }
                    this.reset_cursor_blink();
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _event: &MouseUpEvent, _window, _cx| {
                this.is_selecting = false;
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                if this.handle_key_down(event, cx) == KeyHandled::Handled {
                    cx.stop_propagation();
                }
            }))
            // Optional icon
            .when_some(icon, |d, icon_path| {
                d.child(
                    svg()
                        .path(icon_path)
                        .size(px(12.0))
                        .text_color(rgb(t.text_muted))
                )
            })
            .child(content)
    }
}

/// Canvas element that paints a cursor line at the position from a TextLayout.
/// The layout is read during prepaint (after the sibling StyledText has been laid out),
/// and the cursor is painted during the paint phase.
fn cursor_canvas(
    layout: TextLayout,
    cursor_byte: usize,
    visible: bool,
    color: impl Into<Hsla> + Clone + 'static,
) -> impl IntoElement {
    let color: Hsla = color.into();
    canvas(
        // Prepaint: read cursor position and line height from the text layout
        move |_bounds, _window, _cx| {
            let pos = layout.position_for_index(cursor_byte);
            let line_h = layout.line_height();
            (pos, line_h)
        },
        // Paint: draw the 1px cursor line, vertically centered within the text line
        move |_bounds, (cursor_pos, line_h), window, _cx| {
            if visible {
                if let Some(pos) = cursor_pos {
                    let cursor_h = px(14.0).min(line_h);
                    let y_offset = (line_h - cursor_h) * 0.5;
                    let adjusted = point(pos.x, pos.y + y_offset);
                    window.paint_quad(fill(
                        Bounds::new(adjusted, size(px(1.0), cursor_h)),
                        color,
                    ));
                }
            }
        },
    )
    .absolute()
    .size_full()
}

impl_focusable!(SimpleInputState);

impl EventEmitter<InputChangedEvent> for SimpleInputState {}

/// Simple input element builder for use in render functions
pub struct SimpleInput {
    state: Entity<SimpleInputState>,
    text_size: Option<Pixels>,
}

impl SimpleInput {
    pub fn new(state: &Entity<SimpleInputState>) -> Self {
        Self {
            state: state.clone(),
            text_size: None,
        }
    }

    pub fn text_size(mut self, size: Pixels) -> Self {
        self.text_size = Some(size);
        self
    }
}

impl IntoElement for SimpleInput {
    type Element = Div;

    fn into_element(self) -> Self::Element {
        let state = self.state.clone();
        let text_size = self.text_size.unwrap_or(px(12.0));

        div().w_full().text_size(text_size).child(state)
    }
}
