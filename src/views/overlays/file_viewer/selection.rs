//! Selection, clipboard, scrollbar, and navigation for the file viewer.

use crate::ui::{copy_to_clipboard, Selection1DExtension};
use crate::views::components::{get_selected_text, start_scrollbar_drag, update_scrollbar_drag};
use gpui::*;
use super::{DisplayMode, FileViewer, FileViewerEvent};

impl FileViewer {
    /// Toggle between source and preview display modes.
    pub(super) fn toggle_display_mode(&mut self, cx: &mut Context<Self>) {
        if !self.is_markdown {
            return;
        }
        self.display_mode = match self.display_mode {
            DisplayMode::Source => DisplayMode::Preview,
            DisplayMode::Preview => DisplayMode::Source,
        };
        cx.notify();
    }

    /// Close the viewer.
    pub(super) fn close(&self, cx: &mut Context<Self>) {
        cx.emit(FileViewerEvent::Close);
    }

    /// Get selected text using the shared utility.
    pub(super) fn get_selected_text(&self) -> Option<String> {
        get_selected_text(&self.highlighted_lines, &self.selection)
    }

    /// Copy selected text to clipboard.
    pub(super) fn copy_selection(&self, cx: &mut Context<Self>) {
        copy_to_clipboard(cx, self.get_selected_text());
    }

    /// Select all text.
    pub(super) fn select_all(&mut self, cx: &mut Context<Self>) {
        if self.highlighted_lines.is_empty() {
            return;
        }
        let last_line = self.highlighted_lines.len() - 1;
        let last_col = self.highlighted_lines[last_line].plain_text.len();
        self.selection.start = Some((0, 0));
        self.selection.end = Some((last_line, last_col));
        cx.notify();
    }

    /// Get selected text from markdown preview (using character indices).
    pub(super) fn get_selected_markdown_text(&self) -> Option<String> {
        let doc = self.markdown_doc.as_ref()?;
        let (start, end) = self.markdown_selection.normalized_non_empty()?;

        let chars: Vec<char> = doc.plain_text.chars().collect();
        let char_count = chars.len();
        let start = start.min(char_count);
        let end = end.min(char_count);

        Some(chars[start..end].iter().collect())
    }

    /// Copy selected markdown text to clipboard.
    pub(super) fn copy_markdown_selection(&self, cx: &mut Context<Self>) {
        copy_to_clipboard(cx, self.get_selected_markdown_text());
    }

    /// Select all markdown text (using character count).
    pub(super) fn select_all_markdown(&mut self, cx: &mut Context<Self>) {
        if let Some(doc) = &self.markdown_doc {
            self.markdown_selection.start = Some(0);
            self.markdown_selection.end = Some(doc.plain_text.chars().count());
            cx.notify();
        }
    }

    /// Calculate column position from x coordinate.
    pub(super) fn x_to_column(&self, x: f32, line_num_width: usize) -> usize {
        // Approximate char width based on font size (monospace fonts are ~0.6 of font size)
        let char_width = self.file_font_size * 0.6;
        let gutter_width = (line_num_width * 8 + 16) as f32;
        let text_x = (x - gutter_width).max(0.0);
        (text_x / char_width) as usize
    }

    // Scrollbar methods using shared utilities

    pub(super) fn start_scrollbar_drag(&mut self, y: f32, cx: &mut Context<Self>) {
        let mut drag = start_scrollbar_drag(&self.source_scroll_handle);
        drag.start_y = y;
        self.scrollbar_drag = Some(drag);
        cx.notify();
    }

    pub(super) fn update_scrollbar_drag(&mut self, y: f32, cx: &mut Context<Self>) {
        if let Some(drag) = self.scrollbar_drag {
            update_scrollbar_drag(&self.source_scroll_handle, drag, y);
            cx.notify();
        }
    }

    pub(super) fn end_scrollbar_drag(&mut self, cx: &mut Context<Self>) {
        self.scrollbar_drag = None;
        cx.notify();
    }
}
