//! Git diff viewer overlay.
//!
//! Provides a read-only view of git diffs with working/staged toggle,
//! file tree sidebar, syntax highlighting, and selection support.

mod line_render;
mod render;
mod scrollbar;
mod syntax;
mod types;

use crate::git::{get_diff_with_options, is_git_repo, DiffMode, DiffResult, FileDiff};
use crate::keybindings::Cancel;
use crate::settings::settings_entity;
use crate::theme::theme;
use crate::ui::{copy_to_clipboard, SelectionState};
use crate::views::components::{extract_selected_text, modal_backdrop, modal_content, syntax::load_syntax_set};
use gpui::prelude::*;
use gpui::*;
use std::sync::Arc;
use syntect::parsing::SyntaxSet;

use syntax::process_file;
use types::{DiffDisplayFile, FileStats, FileTreeNode, HScrollbarDrag, ScrollbarDrag, SideBySideLine, SideBySideSide};

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
    /// Ignore whitespace changes in diff.
    ignore_whitespace: bool,
    project_path: String,
    /// Raw diff data for all files (not syntax highlighted).
    raw_files: Vec<FileDiff>,
    /// Lightweight file stats for sidebar display.
    file_stats: Vec<FileStats>,
    /// Currently processed file with syntax highlighting (lazy loaded).
    current_file: Option<DiffDisplayFile>,
    file_tree: FileTreeNode,
    selected_file_index: usize,
    selection: Selection,
    scroll_handle: UniformListScrollHandle,
    tree_scroll_handle: ScrollHandle,
    error_message: Option<String>,
    line_num_width: usize,
    syntax_set: SyntaxSet,
    scrollbar_drag: Option<ScrollbarDrag>,
    file_font_size: f32,
    /// Cached side-by-side lines for current file.
    side_by_side_lines: Vec<SideBySideLine>,
    /// Horizontal scroll offset in pixels.
    scroll_x: f32,
    /// Maximum line length in characters (for horizontal scroll range).
    max_line_chars: usize,
    /// Cached diff pane viewport width (updated from scroll handle geometry).
    diff_pane_width: f32,
    /// Horizontal scrollbar drag state.
    h_scrollbar_drag: Option<HScrollbarDrag>,
    /// Which side of the side-by-side view the current selection belongs to.
    selection_side: Option<SideBySideSide>,
}

impl DiffViewer {
    /// Create a new diff viewer for the given project path, optionally selecting a specific file.
    pub fn new(project_path: String, select_file: Option<String>, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let settings = settings_entity(cx).read(cx);
        let file_font_size = settings.settings.file_font_size;
        let view_mode = settings.settings.diff_view_mode;
        let ignore_whitespace = settings.settings.diff_ignore_whitespace;

        let mut viewer = Self {
            focus_handle,
            diff_mode: DiffMode::WorkingTree,
            view_mode,
            ignore_whitespace,
            project_path: project_path.clone(),
            raw_files: Vec::new(),
            file_stats: Vec::new(),
            current_file: None,
            file_tree: FileTreeNode::default(),
            selected_file_index: 0,
            selection: Selection::default(),
            scroll_handle: UniformListScrollHandle::new(),
            tree_scroll_handle: ScrollHandle::new(),
            error_message: None,
            line_num_width: 4,
            syntax_set: load_syntax_set(),
            scrollbar_drag: None,
            file_font_size,
            side_by_side_lines: Vec::new(),
            scroll_x: 0.0,
            max_line_chars: 0,
            diff_pane_width: 0.0,
            h_scrollbar_drag: None,
            selection_side: None,
        };

        if !is_git_repo(std::path::Path::new(&project_path)) {
            viewer.error_message = Some("Not a git repository".to_string());
            return viewer;
        }

        viewer.load_diff(DiffMode::WorkingTree);

        // Select specific file if requested
        if let Some(file_path) = select_file {
            if let Some(index) = viewer.file_stats.iter().position(|f| f.path == file_path) {
                viewer.selected_file_index = index;
                viewer.process_current_file();
                viewer.update_side_by_side_cache();
            }
        }

        viewer
    }

    fn load_diff(&mut self, mode: DiffMode) {
        self.diff_mode = mode;
        self.error_message = None;
        self.raw_files.clear();
        self.file_stats.clear();
        self.current_file = None;
        self.file_tree = FileTreeNode::default();
        self.selected_file_index = 0;
        self.selection.clear();
        self.selection_side = None;
        self.side_by_side_lines.clear();
        self.scroll_x = 0.0;
        self.max_line_chars = 0;

        let path = std::path::Path::new(&self.project_path);
        match get_diff_with_options(path, mode, self.ignore_whitespace) {
            Ok(result) => {
                if result.is_empty() {
                    self.error_message =
                        Some(format!("No {} changes", mode.display_name().to_lowercase()));
                } else {
                    self.store_diff_result(result);
                    self.build_file_tree();
                    // Process the first file for display
                    self.process_current_file();
                    self.update_side_by_side_cache();
                }
            }
            Err(e) => {
                self.error_message = Some(e);
            }
        }
    }

    /// Store raw diff data and extract lightweight stats (no syntax highlighting).
    fn store_diff_result(&mut self, result: DiffResult) {
        for file in result.files {
            self.file_stats.push(FileStats::from(&file));
            self.raw_files.push(file);
        }
    }

    /// Process the currently selected file with syntax highlighting.
    fn process_current_file(&mut self) {
        if let Some(raw_file) = self.raw_files.get(self.selected_file_index) {
            let repo_path = std::path::Path::new(&self.project_path);
            let mut max_line_num = 0usize;

            let display_file = process_file(
                raw_file,
                &mut max_line_num,
                &self.syntax_set,
                repo_path,
                self.diff_mode,
            );

            self.line_num_width = max_line_num.to_string().len().max(3);
            self.max_line_chars = display_file
                .lines
                .iter()
                .map(|l| l.plain_text.chars().count())
                .max()
                .unwrap_or(0);
            self.current_file = Some(display_file);
        } else {
            self.current_file = None;
        }
    }

    fn build_file_tree(&mut self) {
        self.file_tree = FileTreeNode::default();

        for (index, file) in self.file_stats.iter().enumerate() {
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
        self.selection.clear();
        self.selection_side = None;
        self.update_side_by_side_cache();
        // Save to global settings
        settings_entity(cx).update(cx, |settings, cx| {
            settings.set_diff_view_mode(self.view_mode, cx);
        });
        cx.notify();
    }

    fn toggle_ignore_whitespace(&mut self, cx: &mut Context<Self>) {
        self.ignore_whitespace = !self.ignore_whitespace;
        self.load_diff(self.diff_mode);
        // Save to global settings
        settings_entity(cx).update(cx, |settings, cx| {
            settings.set_diff_ignore_whitespace(self.ignore_whitespace, cx);
        });
        cx.notify();
    }

    fn update_side_by_side_cache(&mut self) {
        if self.view_mode == DiffViewMode::SideBySide {
            if let Some(file) = &self.current_file {
                self.side_by_side_lines = side_by_side::to_side_by_side(&file.lines);
            } else {
                self.side_by_side_lines.clear();
            }
        } else {
            self.side_by_side_lines.clear();
        }
    }

    fn select_file(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.file_stats.len() {
            self.selected_file_index = index;
            self.selection.clear();
            self.selection_side = None;
            self.scroll_x = 0.0;
            self.process_current_file();
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
        if self.selected_file_index + 1 < self.file_stats.len() {
            self.select_file(self.selected_file_index + 1, cx);
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(DiffViewerEvent::Close);
    }

    fn get_selected_text(&self) -> Option<String> {
        if let Some(side) = self.selection_side {
            let lines = &self.side_by_side_lines;
            extract_selected_text(&self.selection, lines.len(), |i| {
                let sbs_line = &lines[i];
                let content = match side {
                    SideBySideSide::Left => &sbs_line.left,
                    SideBySideSide::Right => &sbs_line.right,
                };
                content.as_ref().map(|c| c.plain_text.as_str()).unwrap_or("")
            })
        } else {
            let file = self.current_file.as_ref()?;
            extract_selected_text(&self.selection, file.lines.len(), |i| {
                &file.lines[i].plain_text
            })
        }
    }

    fn copy_selection(&self, cx: &mut Context<Self>) {
        copy_to_clipboard(cx, self.get_selected_text());
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
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
                    if file.lines.is_empty() {
                        return;
                    }
                    let last_line = file.lines.len() - 1;
                    let last_col = file.lines[last_line].plain_text.len();
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
        let has_files = !self.file_stats.is_empty();
        let has_selection = self.selection.normalized().is_some();

        let gutter_width = (self.line_num_width * 8 * 2 + 8 + 16) as f32;

        let current_stats = self.file_stats.get(self.selected_file_index);
        let file_path = current_stats.map(|f| f.path.clone()).unwrap_or_default();
        let is_binary = current_stats.map(|f| f.is_binary).unwrap_or(false);
        let line_count = self.current_file.as_ref().map(|f| f.lines.len()).unwrap_or(0);

        let tree_elements = if has_files {
            self.render_tree_node(&self.file_tree.clone(), 0, &t, cx)
        } else {
            Vec::new()
        };

        let total_added: usize = self.file_stats.iter().map(|f| f.added).sum();
        let total_removed: usize = self.file_stats.iter().map(|f| f.removed).sum();

        let theme_colors = Arc::new(t.clone());

        if !focus_handle.is_focused(window) {
            window.focus(&focus_handle, cx);
        }

        modal_backdrop("diff-viewer-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("DiffViewer")
            .items_center()
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                if this.selection.normalized().is_some() {
                    this.selection.clear();
                    this.selection_side = None;
                    cx.notify();
                } else {
                    this.close(cx);
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                let key = event.keystroke.key.as_str();
                let modifiers = &event.keystroke.modifiers;

                match key {
                    "tab" => this.toggle_mode(cx),
                    "s" => this.toggle_view_mode(cx),
                    "w" => this.toggle_ignore_whitespace(cx),
                    "up" => this.prev_file(cx),
                    "down" => this.next_file(cx),
                    "left" => {
                        this.scroll_x = (this.scroll_x - 40.0).max(0.0);
                        cx.notify();
                    }
                    "right" => {
                        let max = this.max_scroll_x();
                        this.scroll_x = (this.scroll_x + 40.0).min(max);
                        cx.notify();
                    }
                    "c" if modifiers.platform || modifiers.control => this.copy_selection(cx),
                    "a" if modifiers.platform || modifiers.control => this.select_all(cx),
                    _ => {}
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    if this.scrollbar_drag.is_none() && this.h_scrollbar_drag.is_none() {
                        this.close(cx);
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.scrollbar_drag.is_some() {
                    let y = f32::from(event.position.y);
                    this.update_scrollbar_drag(y, cx);
                }
                if let Some(drag) = this.h_scrollbar_drag {
                    let x = f32::from(event.position.x);
                    let delta_x = x - drag.start_x;
                    let max = this.max_scroll_x();
                    let text_w = this.max_text_width();
                    let avail_w = this.available_text_width();
                    let scale = if avail_w > 0.0 { text_w / avail_w } else { 1.0 };
                    this.scroll_x = (drag.start_scroll_x + delta_x * scale).clamp(0.0, max);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    if this.scrollbar_drag.is_some() {
                        this.end_scrollbar_drag(cx);
                    }
                    if this.h_scrollbar_drag.is_some() {
                        this.h_scrollbar_drag = None;
                        cx.notify();
                    }
                }),
            )
            .child(
                modal_content("diff-viewer-modal", &t)
                    .w(relative(0.92))
                    .max_w(px(1400.0))
                    .h(relative(0.88))
                    .max_h(px(950.0))
                    .child(self.render_header(&t, has_files, self.file_stats.len(), total_added, total_removed, is_working, self.ignore_whitespace, cx))
                    .child(self.render_content(&t, has_error, error_message, has_files, is_binary, file_path, line_count, gutter_width, tree_elements, theme_colors, cx))
                    .child(self.render_footer(&t, has_selection)),
            )
    }
}

impl_focusable!(DiffViewer);
