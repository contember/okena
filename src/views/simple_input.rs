use crate::theme::theme;
use gpui::prelude::*;
use gpui::*;
use std::ops::Range;

/// A simple text input state that doesn't rely on gpui-component's Root entity
pub struct SimpleInputState {
    focus_handle: FocusHandle,
    value: String,
    placeholder: String,
    cursor_position: usize,
    selection: Option<Range<usize>>,
}

impl SimpleInputState {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            value: String::new(),
            placeholder: String::new(),
            cursor_position: 0,
            selection: None,
        }
    }

    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn default_value(mut self, value: impl Into<String>) -> Self {
        let v = value.into();
        self.cursor_position = v.len();
        self.value = v;
        self
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn set_value(&mut self, value: impl Into<String>, cx: &mut Context<Self>) {
        let v = value.into();
        self.cursor_position = v.len();
        self.value = v;
        self.selection = None;
        cx.notify();
    }

    pub fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
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
        cx.notify();
    }

    fn delete_backward(&mut self, cx: &mut Context<Self>) {
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
        cx.notify();
    }

    fn delete_forward(&mut self, cx: &mut Context<Self>) {
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
        cx.notify();
    }

    fn move_cursor_left(&mut self, cx: &mut Context<Self>) {
        self.selection = None;
        if self.cursor_position > 0 {
            self.cursor_position -= 1;
            cx.notify();
        }
    }

    fn move_cursor_right(&mut self, cx: &mut Context<Self>) {
        self.selection = None;
        let char_count = self.value.chars().count();
        if self.cursor_position < char_count {
            self.cursor_position += 1;
            cx.notify();
        }
    }

    fn move_to_start(&mut self, cx: &mut Context<Self>) {
        self.selection = None;
        self.cursor_position = 0;
        cx.notify();
    }

    fn move_to_end(&mut self, cx: &mut Context<Self>) {
        self.selection = None;
        self.cursor_position = self.value.chars().count();
        cx.notify();
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        let char_count = self.value.chars().count();
        if char_count > 0 {
            self.selection = Some(0..char_count);
            self.cursor_position = char_count;
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

    fn handle_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let key = event.keystroke.key.as_str();
        let modifiers = &event.keystroke.modifiers;

        // Handle special keys
        match key {
            "backspace" => {
                self.delete_backward(cx);
                return true;
            }
            "delete" => {
                self.delete_forward(cx);
                return true;
            }
            "left" => {
                if modifiers.platform || modifiers.control {
                    self.move_to_start(cx);
                } else {
                    self.move_cursor_left(cx);
                }
                return true;
            }
            "right" => {
                if modifiers.platform || modifiers.control {
                    self.move_to_end(cx);
                } else {
                    self.move_cursor_right(cx);
                }
                return true;
            }
            "home" => {
                self.move_to_start(cx);
                return true;
            }
            "end" => {
                self.move_to_end(cx);
                return true;
            }
            "a" if modifiers.platform || modifiers.control => {
                self.select_all(cx);
                return true;
            }
            // Skip special keys that shouldn't insert text
            "enter" | "escape" | "tab" | "shift" | "control" | "alt" | "meta" | "capslock"
            | "f1" | "f2" | "f3" | "f4" | "f5" | "f6" | "f7" | "f8" | "f9" | "f10" | "f11"
            | "f12" | "up" | "down" | "pageup" | "pagedown" => {
                return false;
            }
            _ => {}
        }

        // Handle character input via key_char (it's a String, not a char)
        if let Some(ref s) = event.keystroke.key_char {
            // Skip control characters (except for normal space/printable)
            if !s.is_empty() && !s.chars().next().map_or(true, |c| c.is_control() && c != ' ') {
                self.insert_text(s, cx);
                return true;
            }
        }

        false
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

        // Split value into parts for rendering with cursor
        let (before_cursor, after_cursor) = {
            let byte_pos = self.byte_position_for_char(cursor_position);
            let (before, after) = value.split_at(byte_pos);
            (before.to_string(), after.to_string())
        };

        let show_placeholder = value.is_empty() && !is_focused;

        div()
            .id("simple-input")
            .track_focus(&focus_handle)
            .flex()
            .items_center()
            .w_full()
            .h(px(24.0))
            .px(px(8.0))
            .cursor_text()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.focus(window, cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                this.handle_key_down(event, cx);
            }))
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

                div()
                    .flex()
                    .items_center()
                    .text_color(rgb(t.text_primary))
                    .child(before_sel)
                    .child(
                        div()
                            .bg(rgb(t.selection_bg))
                            .text_color(rgb(t.selection_fg))
                            .child(selected),
                    )
                    .child(after_sel)
                    .when(is_focused, |this| {
                        this.child(div().w(px(1.0)).h(px(14.0)).bg(rgb(t.text_primary)))
                    })
                    .into_any_element()
            } else {
                // Normal rendering with cursor
                div()
                    .flex()
                    .items_center()
                    .text_color(rgb(t.text_primary))
                    .child(before_cursor)
                    .when(is_focused, |this| {
                        this.child(div().w(px(1.0)).h(px(14.0)).bg(rgb(t.text_primary)))
                    })
                    .child(after_cursor)
                    .into_any_element()
            })
    }
}

impl Focusable for SimpleInputState {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

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
