use crate::theme::theme;
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
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
                        // Only insert first line (no newlines in single-line input)
                        let line = text.lines().next().unwrap_or("");
                        if !line.is_empty() {
                            self.insert_text(line, cx);
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
            // These keys are handled by parent (enter for confirm, tab for focus)
            "enter" | "tab" => {
                return KeyHandled::NotHandled;
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

        // Split value into parts for rendering with cursor
        let (before_cursor, after_cursor) = {
            let byte_pos = self.byte_position_for_char(cursor_position);
            let (before, after) = value.split_at(byte_pos);
            (before.to_string(), after.to_string())
        };

        let show_placeholder = value.is_empty() && !is_focused;

        let cursor_element = div()
            .w(px(1.0))
            .h(px(14.0))
            .when(cursor_visible, |d| d.bg(rgb(t.text_primary)));

        div()
            .id("simple-input")
            .track_focus(&focus_handle)
            .flex()
            .items_center()
            .gap(px(6.0))
            .w_full()
            .h(px(24.0))
            .px(px(8.0))
            .cursor_text()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.focus(window, cx);
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
            .child(if show_placeholder {
                div()
                    .text_color(rgb(t.text_muted))
                    .child(placeholder)
                    .into_any_element()
            } else if let Some(sel) = selection {
                // Render with selection highlight
                let sel_start_byte = self.byte_position_for_char(sel.start);
                let sel_end_byte = self.byte_position_for_char(sel.end);
                let before_sel = value[..sel_start_byte].to_string();
                let selected = value[sel_start_byte..sel_end_byte].to_string();
                let after_sel = value[sel_end_byte..].to_string();

                h_flex()
                    .text_color(rgb(t.text_primary))
                    .child(before_sel)
                    .child(
                        div()
                            .bg(rgb(t.selection_bg))
                            .text_color(rgb(t.selection_fg))
                            .child(selected),
                    )
                    .child(after_sel)
                    .child(cursor_element)
                    .into_any_element()
            } else {
                // Normal rendering with cursor
                h_flex()
                    .text_color(rgb(t.text_primary))
                    .child(before_cursor)
                    .child(cursor_element)
                    .child(after_cursor)
                    .into_any_element()
            })
    }
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
