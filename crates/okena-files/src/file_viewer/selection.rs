//! Selection, clipboard, scrollbar, and navigation for the file viewer.

use crate::code_view::{get_selected_text, start_scrollbar_drag, update_scrollbar_drag};
use crate::selection::{copy_to_clipboard, Selection1DExtension, Selection2DNonEmpty};
use gpui::*;
use okena_core::send_payload::{CodeBlock, SendPayload};

use super::{DisplayMode, FileViewer, FileViewerEvent, PreviewBackground};

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

    /// Close the viewer.
    pub(super) fn close(&self, cx: &mut Context<Self>) {
        cx.emit(FileViewerEvent::Close);
    }

    /// Get selected text using the shared utility.
    pub(super) fn get_selected_text(&self) -> Option<String> {
        let tab = self.active_tab();
        get_selected_text(&tab.highlighted_lines, &tab.selection)
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
        let last_line_idx = tab.line_count.checked_sub(1)?;
        let first_idx = start_line.min(last_line_idx);
        let last_idx = end_line.min(last_line_idx);

        let text: String = tab.highlighted_lines
            .get(first_idx..=last_idx)?
            .iter()
            .map(|l| l.plain_text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        Some(SendPayload::Code(vec![CodeBlock {
            absolute_path: tab.file_path.clone(),
            first: first_idx + 1,
            last: last_idx + 1,
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
        if tab.highlighted_lines.is_empty() {
            return;
        }
        let last_line = tab.highlighted_lines.len() - 1;
        let last_col = tab.highlighted_lines[last_line].plain_text.len();
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

    // ── Image zoom / pan / background ────────────────────────────────────

    /// Multiply the active tab's image zoom by `factor` (e.g. 1.25 in,
    /// 1/1.25 out). Leaves auto-fit mode and, for SVG tabs, schedules a
    /// fresh raster at the new scale so the preview stays crisp.
    pub(super) fn image_zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        let tab = self.active_tab_mut();
        if !tab.is_image {
            return;
        }
        tab.image_view.zoom_by(factor);
        cx.notify();
        self.maybe_rerender_svg(cx);
    }

    /// Set the active tab's image zoom to an explicit factor (1.0 = 100%).
    pub(super) fn image_set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        let tab = self.active_tab_mut();
        if !tab.is_image {
            return;
        }
        tab.image_view.set_zoom(zoom);
        cx.notify();
        self.maybe_rerender_svg(cx);
    }

    /// Reset the active tab's image view to fit-to-pane.
    pub(super) fn image_fit(&mut self, cx: &mut Context<Self>) {
        let tab = self.active_tab_mut();
        if !tab.is_image {
            return;
        }
        tab.image_view.reset_to_fit();
        cx.notify();
    }

    /// Begin a pan drag at the given screen-space position.
    pub(super) fn image_start_pan(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let tab = self.active_tab_mut();
        if !tab.is_image {
            return;
        }
        // Panning only matters once we've left auto-fit, so promote to
        // explicit zoom on first drag.
        if tab.image_view.auto_fit {
            tab.image_view.auto_fit = false;
            tab.image_view.zoom = 1.0;
        }
        tab.image_view.is_panning = true;
        tab.image_view.pan_anchor = Some(position);
        tab.image_view.pan_anchor_offset = tab.image_view.pan;
        cx.notify();
    }

    /// Continue a pan drag — translate by (current - anchor).
    pub(super) fn image_update_pan(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let tab = self.active_tab_mut();
        if !tab.is_image || !tab.image_view.is_panning {
            return;
        }
        let Some(anchor) = tab.image_view.pan_anchor else {
            return;
        };
        tab.image_view.pan = gpui::Point::new(
            tab.image_view.pan_anchor_offset.x + (position.x - anchor.x),
            tab.image_view.pan_anchor_offset.y + (position.y - anchor.y),
        );
        cx.notify();
    }

    /// End a pan drag.
    pub(super) fn image_end_pan(&mut self, cx: &mut Context<Self>) {
        let tab = self.active_tab_mut();
        if !tab.is_image {
            return;
        }
        tab.image_view.is_panning = false;
        tab.image_view.pan_anchor = None;
        cx.notify();
    }

    /// Set the preview background explicitly (header button click).
    pub(super) fn image_set_background(
        &mut self,
        background: PreviewBackground,
        cx: &mut Context<Self>,
    ) {
        let tab = self.active_tab_mut();
        if !tab.is_image {
            return;
        }
        tab.image_view.background = background;
        cx.notify();
    }
}
