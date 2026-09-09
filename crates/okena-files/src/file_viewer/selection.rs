//! Selection, clipboard, scrollbar, and navigation for the file viewer.

use crate::code_view::{clamp_to_char_boundary, start_scrollbar_drag, update_scrollbar_drag};
use crate::selection::{Selection1DExtension, Selection2DNonEmpty, copy_to_clipboard};
use gpui::*;
use okena_core::send_payload::{CodeBlock, SendPayload};
use std::path::PathBuf;

use super::{DisplayMode, FileViewer, FileViewerEvent, FileViewerTab};

impl FileViewer {
    /// Toggle between source and preview display modes. Only meaningful for
    /// tabs that actually have both views — markdown (rendered ↔ source) and
    /// SVG (rasterized image ↔ XML).
    pub(super) fn toggle_display_mode(&mut self, cx: &mut Context<Self>) {
        let tab = self.active_tab_mut();
        if !tab.is_markdown && !tab.is_svg {
            return;
        }
        tab.display_mode = match tab.display_mode {
            DisplayMode::Source => DisplayMode::Preview,
            DisplayMode::Preview => DisplayMode::Source,
        };
        cx.notify();
    }

    pub(super) fn toggle_line_wrap(&mut self, cx: &mut Context<Self>) {
        let tab = self.active_tab_mut();
        tab.wrap_lines = !tab.wrap_lines;
        tab.selection.clear();
        tab.source_scroll_handle
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.0), px(0.0)));
        tab.rebuild_source_rows();
        self.perform_file_search(cx);
        cx.notify();
    }

    pub(super) fn toggle_json_pretty(&mut self, cx: &mut Context<Self>) {
        let path = self.active_tab().file_path.clone();
        let syntax_set = self.syntax_set.clone();
        let is_dark = self.is_dark;
        let tab = self.active_tab_mut();
        let Some(mut alternate) = tab.json_alternate.take() else {
            return;
        };

        std::mem::swap(&mut tab.content, &mut alternate.content);
        let next_lines = alternate.highlighted_lines.take().unwrap_or_else(|| {
            crate::syntax::highlight_content(
                &tab.content,
                &path,
                &syntax_set,
                super::MAX_LINES,
                is_dark,
            )
        });
        alternate.highlighted_lines =
            Some(std::mem::replace(&mut tab.highlighted_lines, next_lines));
        tab.json_alternate = Some(alternate);
        tab.json_pretty = !tab.json_pretty;
        tab.selection.clear();
        tab.rebuild_source_rows();
        tab.source_scroll_handle
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.0), px(0.0)));
        self.perform_file_search(cx);
        cx.notify();
    }

    /// Close the viewer.
    pub(super) fn close(&self, cx: &mut Context<Self>) {
        cx.emit(FileViewerEvent::Close);
    }

    pub(super) fn back_or_close(&self, cx: &mut Context<Self>) {
        if self.can_go_back {
            cx.emit(FileViewerEvent::Back);
        } else {
            self.close(cx);
        }
    }

    /// Get selected text using the shared utility.
    pub(super) fn get_selected_text(&self) -> Option<String> {
        let tab = self.active_tab();
        extract_selected_source_text(tab)
    }

    /// Copy selected text to clipboard.
    pub(super) fn copy_selection(&self, cx: &mut Context<Self>) {
        copy_to_clipboard(cx, self.get_selected_text());
    }

    /// Build a single-block code payload from the active tab's selection.
    /// Returns None for empty selections or unloaded tabs. The block's path is
    /// the absolute file path on disk; the dispatcher rewrites it relative to
    /// the receiving terminal's CWD at format time.
    pub(super) fn selection_to_send_payload(&self) -> Option<SendPayload> {
        let tab = self.active_tab();
        if tab.is_empty() {
            return None;
        }
        let ((start_line, _), (end_line, _)) = tab.selection.normalized_non_empty()?;

        // Convert from 0-based line index to 1-based, clamp to file length.
        let last_row_idx = tab.source_rows.len().checked_sub(1)?;
        let first_idx = start_line.min(last_row_idx);
        let last_idx = end_line.min(last_row_idx);
        let first_source_line = tab.source_rows.get(first_idx)?.logical_line;
        let last_source_line = tab.source_rows.get(last_idx)?.logical_line;
        let text = tab
            .highlighted_lines
            .get(first_source_line..=last_source_line)?
            .iter()
            .map(|line| line.plain_text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        Some(SendPayload::code(vec![CodeBlock {
            absolute_path: self
                .project_fs
                .absolute_path(&tab.relative_path)
                .map(PathBuf::from)
                .unwrap_or_else(|| tab.file_path.clone()),
            first: first_source_line + 1,
            last: last_source_line + 1,
            text,
        }]))
    }

    /// Emit SendToTerminal with the active selection's payload.
    pub(super) fn send_selection_to_terminal(&mut self, cx: &mut Context<Self>) {
        if let Some(payload) = self.selection_to_send_payload() {
            cx.emit(FileViewerEvent::SendToTerminal(payload));
        }
        cx.notify();
    }

    /// Clear the active tab's source selection.
    pub(super) fn clear_source_selection(&mut self, cx: &mut Context<Self>) {
        self.active_tab_mut().selection.clear();
        cx.notify();
    }

    /// Select all text.
    pub(super) fn select_all(&mut self, cx: &mut Context<Self>) {
        let tab = self.active_tab_mut();
        if tab.source_rows.is_empty() {
            return;
        }
        let last_line = tab.source_rows.len() - 1;
        let last_col = tab.source_rows[last_line].byte_range.len();
        tab.selection.start = Some((0, 0));
        tab.selection.end = Some((last_line, last_col));
        cx.notify();
    }

    /// Get selected text from markdown preview (using character indices).
    pub(super) fn get_selected_markdown_text(&self) -> Option<String> {
        let tab = self.active_tab();
        let doc = tab.markdown_doc.as_ref()?;
        let (start, end) = tab.markdown_selection.normalized_non_empty()?;

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
        let tab = self.active_tab_mut();
        if let Some(doc) = &tab.markdown_doc {
            let count = doc.plain_text.chars().count();
            tab.markdown_selection.start = Some(0);
            tab.markdown_selection.end = Some(count);
            cx.notify();
        }
    }

    /// Select a file from the tree — opens in a new tab (like VS Code).
    /// If the file is already open, switches to that tab.
    /// If the current tab is empty (no file), replaces it instead of creating a new one.
    pub(super) fn select_file(&mut self, relative_path: String, cx: &mut Context<Self>) {
        self.open_file_in_tab(relative_path, cx);
    }

    /// Toggle a folder's expanded/collapsed state. Lazy-loads its children on
    /// first expand.
    pub(super) fn toggle_folder(&mut self, folder_path: &str, cx: &mut Context<Self>) {
        if self.expanded_folders.remove(folder_path) {
            // Collapsing — nothing to fetch.
        } else {
            self.expanded_folders.insert(folder_path.to_string());
            self.fetch_directory(folder_path.to_string(), cx);
        }
        self.invalidate_visible_tree_rows();
        cx.notify();
    }

    /// Toggle sidebar visibility.
    pub(super) fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        cx.notify();
    }

    /// Toggle the gitignore filter and refresh the tree.
    pub(super) fn toggle_filter(&mut self, filter: &str, cx: &mut Context<Self>) {
        if filter == "ignored" {
            self.show_ignored = !self.show_ignored;
        }
        self.refresh_file_tree_async(cx);
        cx.notify();
    }

    /// Close the active tab.
    pub(super) fn close_active_tab(&mut self, cx: &mut Context<Self>) {
        let idx = self.active_tab;
        self.close_tab(idx, cx);
    }

    /// Switch to the next tab.
    pub(super) fn next_tab(&mut self, cx: &mut Context<Self>) {
        if self.tabs.len() > 1 {
            let next = (self.active_tab + 1) % self.tabs.len();
            self.set_active_tab(next, cx);
        }
    }

    /// Switch to the previous tab.
    pub(super) fn prev_tab(&mut self, cx: &mut Context<Self>) {
        if self.tabs.len() > 1 {
            let prev = if self.active_tab == 0 {
                self.tabs.len() - 1
            } else {
                self.active_tab - 1
            };
            self.set_active_tab(prev, cx);
        }
    }

    // Scrollbar methods using shared utilities

    pub(super) fn start_scrollbar_drag(&mut self, y: f32, cx: &mut Context<Self>) {
        let tab = self.active_tab_mut();
        let mut drag = start_scrollbar_drag(&tab.source_scroll_handle);
        drag.start_y = y;
        tab.scrollbar_drag = Some(drag);
        cx.notify();
    }

    pub(super) fn update_scrollbar_drag(&mut self, y: f32, cx: &mut Context<Self>) {
        let tab = self.active_tab_mut();
        if let Some(drag) = tab.scrollbar_drag {
            update_scrollbar_drag(&tab.source_scroll_handle, drag, y);
            cx.notify();
        }
    }

    pub(super) fn end_scrollbar_drag(&mut self, cx: &mut Context<Self>) {
        self.active_tab_mut().scrollbar_drag = None;
        cx.notify();
    }

    pub(super) fn start_tree_scrollbar_drag(&mut self, y: f32, cx: &mut Context<Self>) {
        let mut drag = start_scrollbar_drag(&self.tree_scroll_handle);
        drag.start_y = y;
        self.tree_scrollbar_drag = Some(drag);
        cx.notify();
    }

    pub(super) fn update_tree_scrollbar_drag(&mut self, y: f32, cx: &mut Context<Self>) {
        if let Some(drag) = self.tree_scrollbar_drag {
            update_scrollbar_drag(&self.tree_scroll_handle, drag, y);
            cx.notify();
        }
    }

    pub(super) fn end_tree_scrollbar_drag(&mut self, cx: &mut Context<Self>) {
        self.tree_scrollbar_drag = None;
        cx.notify();
    }

    pub(super) fn image_zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        if let Some(renderer) = self.active_tab().file_renderer.clone() {
            renderer.update(cx, |renderer, cx| renderer.zoom_by(factor, cx));
        }
    }

    pub(super) fn image_fit(&mut self, cx: &mut Context<Self>) {
        if let Some(renderer) = self.active_tab().file_renderer.clone() {
            renderer.update(cx, |renderer, cx| renderer.fit(cx));
        }
    }
}

fn extract_selected_source_text(tab: &FileViewerTab) -> Option<String> {
    let ((start_row, start_col), (end_row, end_col)) = tab.selection.normalized_non_empty()?;
    let mut output = String::new();
    let mut previous_logical_line = None;

    for row_index in start_row..=end_row.min(tab.source_rows.len().saturating_sub(1)) {
        let row = tab.source_rows.get(row_index)?;
        let line = tab.highlighted_lines.get(row.logical_line)?;
        let row_text = &line.plain_text[row.byte_range.clone()];
        if previous_logical_line.is_some_and(|previous| previous != row.logical_line) {
            output.push('\n');
        }

        let start = if row_index == start_row {
            clamp_to_char_boundary(row_text, start_col)
        } else {
            0
        };
        let end = if row_index == end_row {
            clamp_to_char_boundary(row_text, end_col)
        } else {
            row_text.len()
        };
        output.push_str(&row_text[start..end]);
        previous_logical_line = Some(row.logical_line);
    }

    (!output.is_empty()).then_some(output)
}

#[cfg(test)]
mod tests {
    use super::{FileViewerTab, extract_selected_source_text};
    use crate::file_viewer::loading::LoadedContent;
    use crate::syntax::HighlightedLine;
    use gpui::TestAppContext;
    use syntect::parsing::SyntaxSet;

    fn single_line(text: &str) -> LoadedContent {
        LoadedContent::Text {
            source: text.to_string(),
            highlighted_lines: vec![HighlightedLine {
                spans: Vec::new(),
                plain_text: text.to_string(),
            }],
            pretty_json: None,
        }
    }

    fn loaded(text: &str, cx: &mut TestAppContext) -> FileViewerTab {
        let mut tab = FileViewerTab::new_empty();
        cx.update(|cx| {
            tab.apply_loaded_content(Ok(single_line(text)), None, &SyntaxSet::new(), true, cx)
        });
        tab
    }

    /// Freshness polling can swap the content under a live selection whose
    /// columns are byte offsets into the text that is gone.
    #[gpui::test]
    fn a_reload_drops_a_selection_the_new_content_cannot_carry(cx: &mut TestAppContext) {
        let mut tab = loaded("a", cx);
        tab.selection.start = Some((0, 0));
        tab.selection.end = Some((0, 1));

        cx.update(|cx| {
            tab.apply_loaded_content(Ok(single_line("é")), None, &SyntaxSet::new(), true, cx)
        });

        assert_eq!(extract_selected_source_text(&tab), None);
        assert!(
            !tab.selection.has_selection(),
            "the reload kept the previous content's selection"
        );
    }
}
