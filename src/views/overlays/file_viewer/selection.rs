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

    /// Select a file from the tree and load it.
    pub(super) fn select_file(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(file) = self.files.get(index) {
            self.selected_file_index = Some(index);
            let path = file.path.clone();
            self.file_path = path.clone();
            self.is_markdown = Self::is_markdown_file(&self.file_path);
            self.display_mode = if self.is_markdown { DisplayMode::Preview } else { DisplayMode::Source };
            self.content.clear();
            self.highlighted_lines.clear();
            self.line_count = 0;
            self.error_message = None;
            self.selection.clear();
            self.markdown_doc = None;
            self.markdown_selection.clear();
            self.load_file(&path);
            // Expand ancestors of the newly selected file
            let expanded = Self::compute_expanded_for_path(&self.file_path, &self.project_path);
            self.expanded_folders.extend(expanded);
            cx.notify();
        }
    }

    /// Toggle a folder's expanded/collapsed state.
    pub(super) fn toggle_folder(&mut self, folder_path: &str, cx: &mut Context<Self>) {
        if !self.expanded_folders.remove(folder_path) {
            self.expanded_folders.insert(folder_path.to_string());
        }
        cx.notify();
    }

    /// Toggle sidebar visibility.
    pub(super) fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        cx.notify();
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
