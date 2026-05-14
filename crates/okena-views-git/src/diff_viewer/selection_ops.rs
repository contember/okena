//! Context expansion and text selection ops for the diff viewer.

use super::types::{self, DisplayItem, SideBySideSide};
use super::DiffViewer;

use okena_core::types::DiffViewMode;
use okena_files::code_view::extract_selected_text;
use okena_files::selection::copy_to_clipboard;

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
