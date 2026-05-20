//! Context expansion and text selection ops for the diff viewer.

use super::types::{self, DisplayItem, SideBySideSide};
use super::DiffViewer;

use okena_core::types::DiffViewMode;
use okena_files::code_view::extract_selected_text;
use okena_files::selection::{copy_to_clipboard, Selection2DNonEmpty};

use gpui::*;

impl DiffViewer {
    /// Expand all hidden context lines. Finds the expander by matching old/new range.
    pub(super) fn expand_context_by_range(
        &mut self,
        old_range: (usize, usize),
        new_range: (usize, usize),
        cx: &mut Context<Self>,
    ) {
        let file = match self.current_file.as_ref() {
            Some(f) => f,
            None => return,
        };
        let item_index = file.items.iter().position(|item| {
            matches!(item, DisplayItem::Expander(e) if e.old_range == old_range && e.new_range == new_range)
        });
        if let Some(idx) = item_index {
            self.expand_context(idx, cx);
        }
    }

    /// Expand all hidden context lines at the given item index.
    pub(super) fn expand_context(&mut self, item_index: usize, cx: &mut Context<Self>) {
        let file = match self.current_file.as_mut() {
            Some(f) => f,
            None => return,
        };

        let expander = match &file.items[item_index] {
            DisplayItem::Expander(e) => e.clone(),
            _ => return,
        };

        let (old_start, old_end) = expander.old_range;
        let (new_start, new_end) = expander.new_range;

        // Validate ranges
        if new_start == 0 || new_end < new_start || old_end < old_start {
            return;
        }

        self.selection.clear();
        self.selection_side = None;

        let old_lines: Vec<&str> = self.current_file_old_content
            .as_deref()
            .map(|c| c.lines().collect())
            .unwrap_or_default();
        let new_lines: Vec<&str> = self.current_file_new_content
            .as_deref()
            .map(|c| c.lines().collect())
            .unwrap_or_default();

        let count = new_end - new_start + 1;
        let mut new_items: Vec<DisplayItem> = Vec::with_capacity(count);

        for i in 0..count {
            let new_ln = new_start + i;
            let old_ln = old_start + i;

            let spans = file.new_highlighted.get(&new_ln)
                .or_else(|| file.old_highlighted.get(&old_ln))
                .cloned()
                .unwrap_or_default();

            let plain_text = new_lines.get(new_ln - 1)
                .or_else(|| old_lines.get(old_ln - 1))
                .unwrap_or(&"")
                .replace('\t', "    ");

            new_items.push(DisplayItem::Line(types::DisplayLine {
                line_type: okena_git::DiffLineType::Context,
                old_line_num: if old_ln >= 1 && old_ln <= file.old_line_count { Some(old_ln) } else { None },
                new_line_num: if new_ln >= 1 && new_ln <= file.new_line_count { Some(new_ln) } else { None },
                spans,
                plain_text,
            }));
        }

        let Some(file) = self.current_file.as_mut() else {
            return;
        };
        file.items.splice(item_index..=item_index, new_items);

        self.max_line_chars = Self::calc_max_line_chars(file);
        self.update_side_by_side_cache();
        cx.notify();
    }

    pub(super) fn get_selected_text(&self) -> Option<String> {
        if let Some(side) = self.selection_side {
            let lines = &self.side_by_side_lines;
            extract_selected_text(&self.selection, lines.len(), |i| {
                let sbs_line = &lines[i];
                if sbs_line.expander.is_some() {
                    return "";
                }
                let content = match side {
                    SideBySideSide::Left => &sbs_line.left,
                    SideBySideSide::Right => &sbs_line.right,
                };
                content.as_ref().map(|c| c.plain_text.as_str()).unwrap_or("")
            })
        } else {
            let file = self.current_file.as_ref()?;
            extract_selected_text(&self.selection, file.items.len(), |i| {
                match &file.items[i] {
                    DisplayItem::Line(line) => &line.plain_text,
                    DisplayItem::Expander(_) => "",
                }
            })
        }
    }

    pub(super) fn copy_selection(&self, cx: &mut Context<Self>) {
        copy_to_clipboard(cx, self.get_selected_text());
    }

    /// Build a single-block code payload from the current file's selection.
    /// Returns None for empty/header-only selections, or when the current file
    /// is binary or pure-deletion.
    pub(super) fn selection_to_send_payload(&self) -> Option<okena_core::send_payload::SendPayload> {
        use okena_core::send_payload::{CodeBlock, SendPayload};
        use std::path::PathBuf;

        let file = self.current_file.as_ref()?;
        let stats = self.file_stats.get(self.selected_file_index)?;
        if stats.is_deleted || stats.is_binary {
            return None;
        }

        let (first, last, text) = if let Some(side) = self.selection_side {
            // side-by-side
            let ((start, _), (end, _)) = self.selection.normalized_non_empty()?;
            let end = end.min(self.side_by_side_lines.len().saturating_sub(1));
            let mut first_num: Option<usize> = None;
            let mut last_num: usize = 0;
            let mut texts: Vec<String> = Vec::new();
            for i in start..=end {
                let sbs_line = self.side_by_side_lines.get(i)?;
                if sbs_line.expander.is_some() || sbs_line.is_header {
                    continue;
                }
                let content = match side {
                    SideBySideSide::Left => sbs_line.left.as_ref(),
                    SideBySideSide::Right => sbs_line.right.as_ref(),
                }?;
                if first_num.is_none() {
                    first_num = Some(content.line_num);
                }
                last_num = content.line_num;
                texts.push(content.plain_text.clone());
            }
            let first = first_num?;
            if texts.is_empty() {
                return None;
            }
            (first, last_num, texts.join("\n"))
        } else {
            // unified
            let ((start, _), (end, _)) = self.selection.normalized_non_empty()?;
            let end = end.min(file.items.len().saturating_sub(1));
            let mut first_num: Option<usize> = None;
            let mut last_num: usize = 0;
            let mut texts: Vec<String> = Vec::new();
            for i in start..=end {
                let DisplayItem::Line(line) = file.items.get(i)? else { continue };
                let line_num = line.new_line_num.or(line.old_line_num)?;
                if first_num.is_none() {
                    first_num = Some(line_num);
                }
                last_num = line_num;
                texts.push(line.plain_text.clone());
            }
            let first = first_num?;
            if texts.is_empty() {
                return None;
            }
            (first, last_num, texts.join("\n"))
        };

        let absolute_path = self
            .provider
            .absolute_file_path(&stats.path)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(&stats.path));

        Some(SendPayload::Code(vec![CodeBlock {
            absolute_path,
            first,
            last,
            text,
        }]))
    }

    /// Emit SendToTerminal with the selection payload.
    pub(super) fn send_selection_to_terminal(&mut self, cx: &mut Context<Self>) {
        if let Some(payload) = self.selection_to_send_payload() {
            cx.emit(super::DiffViewerEvent::SendToTerminal(payload));
        }
        cx.notify();
    }

    pub(super) fn select_all(&mut self, cx: &mut Context<Self>) {
        // Use effective view mode (new/deleted files forced to unified)
        let is_new_or_deleted = self
            .file_stats
            .get(self.selected_file_index)
            .map(|f| f.is_new || f.is_deleted)
            .unwrap_or(false);
        let effective_mode = if is_new_or_deleted {
            DiffViewMode::Unified
        } else {
            self.view_mode
        };

        match effective_mode {
            DiffViewMode::Unified => {
                if let Some(file) = &self.current_file {
                    if file.items.is_empty() {
                        return;
                    }
                    let last_line = file.items.len() - 1;
                    let last_col = match &file.items[last_line] {
                        DisplayItem::Line(l) => l.plain_text.len(),
                        DisplayItem::Expander(_) => 0,
                    };
                    self.selection.start = Some((0, 0));
                    self.selection.end = Some((last_line, last_col));
                    self.selection_side = None;
                    cx.notify();
                }
            }
            DiffViewMode::SideBySide => {
                if self.side_by_side_lines.is_empty() {
                    return;
                }
                let side = self.selection_side.unwrap_or(SideBySideSide::Right);
                let last_line = self.side_by_side_lines.len() - 1;
                let last_col = {
                    let sbs_line = &self.side_by_side_lines[last_line];
                    let content = match side {
                        SideBySideSide::Left => &sbs_line.left,
                        SideBySideSide::Right => &sbs_line.right,
                    };
                    content.as_ref().map(|c| c.plain_text.len()).unwrap_or(0)
                };
                self.selection.start = Some((0, 0));
                self.selection.end = Some((last_line, last_col));
                self.selection_side = Some(side);
                cx.notify();
            }
        }
    }
}
