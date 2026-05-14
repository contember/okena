//! Async loading and processing for the diff viewer: fetching the diff,
//! syntax-highlighting the selected file, and (re-)building the file tree.

use super::syntax::process_file;
use super::types::{DiffDisplayFile, DisplayItem, FileStats, FileTreeNode};
use super::DiffViewer;

use okena_files::file_tree::build_file_tree;
use okena_git::{DiffMode, DiffResult};

use gpui::*;
use std::collections::HashSet;

impl DiffViewer {
    pub(super) fn load_diff_async(
        &mut self,
        mode: DiffMode,
        select_file: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.diff_mode = mode.clone();
        self.loading = true;
        self.error_message = None;
        self.raw_files.clear();
        self.file_stats.clear();
        self.current_file = None;
        self.current_file_old_content = None;
        self.current_file_new_content = None;
        self.file_tree = FileTreeNode::default();
        self.selected_file_index = 0;
        self.selection.clear();
        self.selection_side = None;
        self.side_by_side_lines.clear();
        self.scroll_x = 0.0;
        self.max_line_chars = 0;
        cx.notify();

        let provider = self.provider.clone();
        let ignore_whitespace = self.ignore_whitespace;

        cx.spawn(async move |this, cx| {
            let mode_for_fallback = mode.clone();
            let result = smol::unblock(move || {
                provider.get_diff(mode, ignore_whitespace)
            }).await;

            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(diff_result) => {
                        if diff_result.is_empty() {
                            // Auto-fallback: if WorkingTree is empty, try Staged
                            if mode_for_fallback == DiffMode::WorkingTree {
                                this.load_diff_async(DiffMode::Staged, select_file, cx);
                                return;
                            }
                            this.error_message = Some(format!("No {} changes", mode_for_fallback.display_name().to_lowercase()));
                        } else {
                            this.store_diff_result(diff_result);
                            this.build_file_tree();

                            // Select specific file if requested
                            if let Some(ref file_path) = select_file {
                                if let Some(index) = this.file_stats.iter().position(|f| f.path == *file_path) {
                                    this.selected_file_index = index;
                                }
                            }

                            this.process_current_file_async(cx);
                        }
                    }
                    Err(e) => {
                        this.error_message = Some(e);
                    }
                }
                cx.notify();
            });
        }).detach();
    }

    /// Store raw diff data and extract lightweight stats (no syntax highlighting).
    fn store_diff_result(&mut self, result: DiffResult) {
        for file in result.files {
            self.file_stats.push(FileStats::from(&file));
            self.raw_files.push(file);
        }
    }

    /// Process the currently selected file with syntax highlighting (async).
    pub(super) fn process_current_file_async(&mut self, cx: &mut Context<Self>) {
        let Some(raw_file) = self.raw_files.get(self.selected_file_index).cloned() else {
            self.current_file = None;
            self.current_file_old_content = None;
            self.current_file_new_content = None;
            return;
        };

        let provider = self.provider.clone();
        let file_path = raw_file.display_name().to_string();
        let diff_mode = self.diff_mode.clone();
        let syntax_set = self.syntax_set.clone();
        let is_dark = self.is_dark;

        cx.spawn(async move |this, cx| {
            let (old_content, new_content, display_file, max_line_num) = smol::unblock(move || {
                let (old_content, new_content) = provider.get_file_contents(&file_path, diff_mode);
                let mut max_line_num = 0usize;
                let display_file = process_file(
                    &raw_file,
                    &mut max_line_num,
                    &syntax_set,
                    old_content.clone(),
                    new_content.clone(),
                    is_dark,
                );
                (old_content, new_content, display_file, max_line_num)
            }).await;

            let _ = this.update(cx, |this, cx| {
                this.current_file_old_content = old_content;
                this.current_file_new_content = new_content;
                this.line_num_width = max_line_num.to_string().len().max(3);
                this.max_line_chars = Self::calc_max_line_chars(&display_file);
                this.current_file = Some(display_file);
                this.update_side_by_side_cache();
                cx.notify();
            });
        }).detach();
    }

    /// Re-highlight current file using cached content (for theme changes).
    pub(super) fn rehighlight_current_file(&mut self) {
        let Some(raw_file) = self.raw_files.get(self.selected_file_index) else {
            return;
        };

        let mut max_line_num = 0usize;
        let display_file = process_file(
            raw_file,
            &mut max_line_num,
            &self.syntax_set,
            self.current_file_old_content.clone(),
            self.current_file_new_content.clone(),
            self.is_dark,
        );

        self.line_num_width = max_line_num.to_string().len().max(3);
        self.max_line_chars = Self::calc_max_line_chars(&display_file);
        self.current_file = Some(display_file);
    }

    pub(super) fn build_file_tree(&mut self) {
        self.file_tree = build_file_tree(
            self.file_stats.iter().enumerate().map(|(i, f)| (i, &f.path))
        );
        // Auto-expand all folders in diff view
        self.expanded_folders.clear();
        Self::collect_folder_paths(&self.file_tree, "", &mut self.expanded_folders);
    }

    fn collect_folder_paths(node: &FileTreeNode, parent: &str, out: &mut HashSet<String>) {
        for (name, child) in &node.children {
            let path = if parent.is_empty() { name.clone() } else { format!("{parent}/{name}") };
            out.insert(path.clone());
            Self::collect_folder_paths(child, &path, out);
        }
    }

    pub(super) fn calc_max_line_chars(file: &DiffDisplayFile) -> usize {
        file.items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Line(l) => Some(l.plain_text.chars().count()),
                DisplayItem::Expander(_) => None,
            })
            .max()
            .unwrap_or(0)
    }
}
