//! Git diff viewer overlay.
//!
//! Provides a read-only view of git diffs with working/staged toggle,
//! file tree sidebar, syntax highlighting, and selection support.

mod line_render;
mod render;
mod scrollbar;
mod syntax;
mod types;

use crate::git::{get_diff, is_git_repo, DiffMode, DiffResult};
use crate::settings::settings_entity;
use crate::theme::theme;
use crate::ui::{copy_to_clipboard, SelectionState};
use crate::views::components::{modal_backdrop, modal_content};
use gpui::prelude::*;
use gpui::*;
use std::sync::Arc;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

use syntax::process_file;
use types::{DiffDisplayFile, FileTreeNode, ScrollbarDrag, SideBySideLine};

mod side_by_side;

// Re-export for use in settings (and use locally)
pub use types::DiffViewMode;

/// Type alias for diff selection (line index, column).
type Selection = SelectionState<(usize, usize)>;

/// Width of file tree sidebar.
const SIDEBAR_WIDTH: f32 = 240.0;

/// Git diff viewer overlay.
pub struct DiffViewer {
    focus_handle: FocusHandle,
    diff_mode: DiffMode,
    view_mode: DiffViewMode,
    project_path: String,
    files: Vec<DiffDisplayFile>,
    file_tree: FileTreeNode,
    selected_file_index: usize,
    selection: Selection,
    scroll_handle: UniformListScrollHandle,
    tree_scroll_handle: ScrollHandle,
    error_message: Option<String>,
    line_num_width: usize,
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
    scrollbar_drag: Option<ScrollbarDrag>,
    file_font_size: f32,
    /// Cached side-by-side lines for current file.
    side_by_side_lines: Vec<SideBySideLine>,
}

impl DiffViewer {
    /// Create a new diff viewer for the given project path, optionally selecting a specific file.
    pub fn new(project_path: String, select_file: Option<String>, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let settings = settings_entity(cx).read(cx);
        let file_font_size = settings.settings.file_font_size;
        let view_mode = settings.settings.diff_view_mode;

        let mut viewer = Self {
            focus_handle,
            diff_mode: DiffMode::WorkingTree,
            view_mode,
            project_path: project_path.clone(),
            files: Vec::new(),
            file_tree: FileTreeNode::default(),
            selected_file_index: 0,
            selection: Selection::default(),
            scroll_handle: UniformListScrollHandle::new(),
            tree_scroll_handle: ScrollHandle::new(),
            error_message: None,
            line_num_width: 4,
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
            scrollbar_drag: None,
            file_font_size,
            side_by_side_lines: Vec::new(),
        };

        if !is_git_repo(std::path::Path::new(&project_path)) {
            viewer.error_message = Some("Not a git repository".to_string());
            return viewer;
        }

        viewer.load_diff(DiffMode::WorkingTree);

        // Select specific file if requested
        if let Some(file_path) = select_file {
            if let Some(index) = viewer.files.iter().position(|f| f.path == file_path) {
                viewer.selected_file_index = index;
                viewer.update_side_by_side_cache();
            }
        }

        viewer
    }

    fn load_diff(&mut self, mode: DiffMode) {
        self.diff_mode = mode;
        self.error_message = None;
        self.files.clear();
        self.file_tree = FileTreeNode::default();
        self.selected_file_index = 0;
        self.selection.clear();
        self.side_by_side_lines.clear();

        let path = std::path::Path::new(&self.project_path);
        match get_diff(path, mode) {
            Ok(result) => {
                if result.is_empty() {
                    self.error_message =
                        Some(format!("No {} changes", mode.display_name().to_lowercase()));
                } else {
                    self.process_diff_result(result);
                    self.build_file_tree();
                    self.update_side_by_side_cache();
                }
            }
            Err(e) => {
                self.error_message = Some(e);
            }
        }
    }

    fn process_diff_result(&mut self, result: DiffResult) {
        let mut max_line_num = 0usize;
        let repo_path = std::path::Path::new(&self.project_path);

        for file in result.files {
            let display_file = process_file(
                &file,
                &mut max_line_num,
                &self.syntax_set,
                &self.theme_set,
                repo_path,
                self.diff_mode,
            );
            self.files.push(display_file);
        }

        self.line_num_width = max_line_num.to_string().len().max(3);
    }

    fn build_file_tree(&mut self) {
        self.file_tree = FileTreeNode::default();

        for (index, file) in self.files.iter().enumerate() {
            let parts: Vec<&str> = file.path.split('/').collect();
            let mut node = &mut self.file_tree;

            for (i, part) in parts.iter().enumerate() {
                if i == parts.len() - 1 {
                    node.files.push(index);
                } else {
                    node = node
                        .children
                        .entry(part.to_string())
                        .or_insert_with(FileTreeNode::default);
                }
            }
        }
    }

    fn toggle_mode(&mut self, cx: &mut Context<Self>) {
        let new_mode = self.diff_mode.toggle();
        self.load_diff(new_mode);
        cx.notify();
    }

    fn toggle_view_mode(&mut self, cx: &mut Context<Self>) {
        self.view_mode = self.view_mode.toggle();
        self.update_side_by_side_cache();
        // Save to global settings
        settings_entity(cx).update(cx, |settings, cx| {
            settings.set_diff_view_mode(self.view_mode, cx);
        });
        cx.notify();
    }

    fn update_side_by_side_cache(&mut self) {
        if self.view_mode == DiffViewMode::SideBySide {
            if let Some(file) = self.files.get(self.selected_file_index) {
                self.side_by_side_lines = side_by_side::to_side_by_side(&file.lines);
            } else {
                self.side_by_side_lines.clear();
            }
        } else {
            self.side_by_side_lines.clear();
        }
    }

    fn select_file(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.files.len() {
            self.selected_file_index = index;
            self.selection.clear();
            self.update_side_by_side_cache();
            cx.notify();
        }
    }

    fn prev_file(&mut self, cx: &mut Context<Self>) {
        if self.selected_file_index > 0 {
            self.select_file(self.selected_file_index - 1, cx);
        }
    }

    fn next_file(&mut self, cx: &mut Context<Self>) {
        if self.selected_file_index + 1 < self.files.len() {
            self.select_file(self.selected_file_index + 1, cx);
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(DiffViewerEvent::Close);
    }

    fn get_selected_text(&self) -> Option<String> {
        let file = self.files.get(self.selected_file_index)?;
        let ((start_line, start_col), (end_line, end_col)) = self.selection.normalized()?;

        let mut result = String::new();

        for line_idx in start_line..=end_line {
            if line_idx >= file.lines.len() {
                break;
            }

            let line = &file.lines[line_idx];
            let text = &line.plain_text;

            if start_line == end_line {
                let start = start_col.min(text.len());
                let end = end_col.min(text.len());
                result.push_str(&text[start..end]);
            } else if line_idx == start_line {
                let start = start_col.min(text.len());
                result.push_str(&text[start..]);
                result.push('\n');
            } else if line_idx == end_line {
                let end = end_col.min(text.len());
                result.push_str(&text[..end]);
            } else {
                result.push_str(text);
                result.push('\n');
            }
        }

        if result.is_empty() { None } else { Some(result) }
    }

    fn copy_selection(&self, cx: &mut Context<Self>) {
        copy_to_clipboard(cx, self.get_selected_text());
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        if let Some(file) = self.files.get(self.selected_file_index) {
            if file.lines.is_empty() {
                return;
            }
            let last_line = file.lines.len() - 1;
            let last_col = file.lines[last_line].plain_text.len();
            self.selection.start = Some((0, 0));
            self.selection.end = Some((last_line, last_col));
            cx.notify();
        }
    }
}

/// Events emitted by the diff viewer.
#[derive(Clone, Debug)]
pub enum DiffViewerEvent {
    Close,
}

impl EventEmitter<DiffViewerEvent> for DiffViewer {}

impl Render for DiffViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let has_error = self.error_message.is_some();
        let error_message = self.error_message.clone();
        let diff_mode = self.diff_mode;
        let is_working = diff_mode == DiffMode::WorkingTree;
        let has_files = !self.files.is_empty();
        let has_selection = self.selection.normalized().is_some();

        let gutter_width = (self.line_num_width * 8 * 2 + 8 + 16) as f32;

        let current_file = self.files.get(self.selected_file_index);
        let file_path = current_file.map(|f| f.path.clone()).unwrap_or_default();
        let is_binary = current_file.map(|f| f.is_binary).unwrap_or(false);
        let line_count = current_file.map(|f| f.lines.len()).unwrap_or(0);

        let tree_elements = if has_files {
            self.render_tree_node(&self.file_tree.clone(), 0, &t, cx)
        } else {
            Vec::new()
        };

        let total_added: usize = self.files.iter().map(|f| f.added).sum();
        let total_removed: usize = self.files.iter().map(|f| f.removed).sum();

        let theme_colors = Arc::new(t.clone());

        window.focus(&focus_handle, cx);

        modal_backdrop("diff-viewer-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("DiffViewer")
            .items_center()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                let key = event.keystroke.key.as_str();
                let modifiers = &event.keystroke.modifiers;

                match key {
                    "escape" => {
                        if this.selection.normalized().is_some() {
                            this.selection.clear();
                            cx.notify();
                        } else {
                            this.close(cx);
                        }
                    }
                    "tab" => this.toggle_mode(cx),
                    "s" => this.toggle_view_mode(cx),
                    "up" => this.prev_file(cx),
                    "down" => this.next_file(cx),
                    "c" if modifiers.platform || modifiers.control => this.copy_selection(cx),
                    "a" if modifiers.platform || modifiers.control => this.select_all(cx),
                    _ => {}
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    if this.scrollbar_drag.is_none() {
                        this.close(cx);
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.scrollbar_drag.is_some() {
                    let y = f32::from(event.position.y);
                    this.update_scrollbar_drag(y, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    if this.scrollbar_drag.is_some() {
                        this.end_scrollbar_drag(cx);
                    }
                }),
            )
            .child(
                modal_content("diff-viewer-modal", &t)
                    .w(relative(0.92))
                    .max_w(px(1400.0))
                    .h(relative(0.88))
                    .max_h(px(950.0))
                    .child(self.render_header(&t, has_files, total_added, total_removed, is_working, cx))
                    .child(self.render_content(&t, has_error, error_message, has_files, is_binary, file_path, line_count, gutter_width, tree_elements, theme_colors, cx))
                    .child(self.render_footer(&t, has_selection)),
            )
    }
}

impl_focusable!(DiffViewer);
