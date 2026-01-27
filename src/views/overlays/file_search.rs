//! File search dialog for quick file lookup.
//!
//! Provides a searchable list of files in the active project,
//! similar to VS Code's Cmd+P file picker.

use crate::theme::{theme, with_alpha};
use crate::views::components::{modal_backdrop, modal_content, modal_header};
use gpui::*;
use gpui::prelude::*;
use std::path::PathBuf;

/// Maximum number of files to scan
const MAX_FILES: usize = 10000;

/// Maximum directory depth to scan
const MAX_DEPTH: usize = 10;

/// Directories to ignore during scan
const IGNORED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "__pycache__",
    ".venv",
    "venv",
    ".idea",
    ".vscode",
    "dist",
    "build",
    ".next",
    ".nuxt",
];

/// File patterns to ignore
const IGNORED_FILES: &[&str] = &[
    ".DS_Store",
    "Thumbs.db",
    ".gitignore",
];

/// File extensions to ignore
const IGNORED_EXTENSIONS: &[&str] = &[
    "pyc",
    "pyo",
    "class",
    "o",
    "obj",
    "dll",
    "so",
    "dylib",
];

/// A file entry in the search list
#[derive(Clone, Debug)]
pub struct FileEntry {
    /// Full path to the file
    pub path: PathBuf,
    /// Path relative to project root
    pub relative_path: String,
    /// Just the filename
    pub filename: String,
}

/// File search dialog for finding files in a project
pub struct FileSearchDialog {
    focus_handle: FocusHandle,
    scroll_handle: UniformListScrollHandle,
    search_query: String,
    files: Vec<FileEntry>,
    filtered_files: Vec<usize>,
    selected_index: usize,
    project_path: PathBuf,
}

impl FileSearchDialog {
    /// Create a new file search dialog for the given project path.
    pub fn new(project_path: PathBuf, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let scroll_handle = UniformListScrollHandle::new();

        // Scan files in the project
        let files = Self::scan_files(&project_path);
        let filtered_files: Vec<usize> = (0..files.len()).collect();

        Self {
            focus_handle,
            scroll_handle,
            search_query: String::new(),
            files,
            filtered_files,
            selected_index: 0,
            project_path,
        }
    }

    /// Scan files in the project directory.
    fn scan_files(project_path: &PathBuf) -> Vec<FileEntry> {
        let mut files = Vec::new();
        Self::scan_dir(project_path, project_path, 0, &mut files);

        // Sort by relative path for consistent ordering
        files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

        files
    }

    /// Recursively scan a directory for files.
    fn scan_dir(
        root: &PathBuf,
        dir: &PathBuf,
        depth: usize,
        files: &mut Vec<FileEntry>,
    ) {
        if depth > MAX_DEPTH || files.len() >= MAX_FILES {
            return;
        }

        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            if files.len() >= MAX_FILES {
                break;
            }

            let path = entry.path();
            let file_name = match entry.file_name().into_string() {
                Ok(name) => name,
                Err(_) => continue,
            };

            // Skip hidden files (except common config files)
            if file_name.starts_with('.') && !file_name.starts_with(".env") {
                continue;
            }

            if path.is_dir() {
                // Skip ignored directories
                if IGNORED_DIRS.contains(&file_name.as_str()) {
                    continue;
                }
                Self::scan_dir(root, &path, depth + 1, files);
            } else {
                // Skip ignored files
                if IGNORED_FILES.contains(&file_name.as_str()) {
                    continue;
                }

                // Skip ignored extensions
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if IGNORED_EXTENSIONS.contains(&ext) {
                        continue;
                    }
                }

                // Calculate relative path
                let relative_path = path
                    .strip_prefix(root)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| file_name.clone());

                files.push(FileEntry {
                    path,
                    relative_path,
                    filename: file_name,
                });
            }
        }
    }

    /// Close the dialog.
    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(FileSearchDialogEvent::Close);
    }

    /// Open the currently selected file.
    fn open_selected(&self, cx: &mut Context<Self>) {
        if let Some(&file_index) = self.filtered_files.get(self.selected_index) {
            let file = &self.files[file_index];
            cx.emit(FileSearchDialogEvent::FileSelected(file.path.clone()));
        }
    }

    /// Scroll to keep the selected item visible.
    fn scroll_to_selected(&self) {
        if !self.filtered_files.is_empty() {
            self.scroll_handle.scroll_to_item(self.selected_index, ScrollStrategy::Top);
        }
    }

    /// Filter files based on the search query using fuzzy matching.
    fn filter_files(&mut self) {
        let query = self.search_query.to_lowercase();

        if query.is_empty() {
            self.filtered_files = (0..self.files.len()).collect();
        } else {
            // Simple fuzzy matching: check if all query characters appear in order
            self.filtered_files = self.files
                .iter()
                .enumerate()
                .filter(|(_, file)| {
                    Self::fuzzy_match(&file.relative_path.to_lowercase(), &query)
                })
                .map(|(i, _)| i)
                .collect();
        }

        // Reset selection to first item
        self.selected_index = 0;
    }

    /// Simple fuzzy matching: all query characters must appear in order.
    fn fuzzy_match(text: &str, query: &str) -> bool {
        let mut text_chars = text.chars().peekable();

        for query_char in query.chars() {
            loop {
                match text_chars.next() {
                    Some(text_char) if text_char == query_char => break,
                    Some(_) => continue,
                    None => return false,
                }
            }
        }

        true
    }

    /// Render a single file row.
    fn render_file_row(&self, filtered_index: usize, file_index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let file = &self.files[file_index];
        let is_selected = filtered_index == self.selected_index;

        let filename = file.filename.clone();
        let relative_path = file.relative_path.clone();

        // Get directory portion of the path
        let dir_path = if relative_path.contains('/') || relative_path.contains('\\') {
            let path = std::path::Path::new(&relative_path);
            path.parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };

        div()
            .id(ElementId::Name(format!("file-{}", filtered_index).into()))
            .w_full()
            .cursor_pointer()
            .flex()
            .items_center()
            .px(px(12.0))
            .py(px(8.0))
            .when(is_selected, |d| d.bg(with_alpha(t.border_active, 0.15)))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    this.selected_index = filtered_index;
                    this.open_selected(cx);
                }),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .overflow_hidden()
                    .child(
                        // Filename
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(t.text_primary))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(filename),
                    )
                    .child(
                        // Directory path (always rendered for uniform height)
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(t.text_muted))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(if dir_path.is_empty() { "\u{00A0}".to_string() } else { dir_path }),
                    ),
            )
    }
}

/// Events emitted by the file search dialog.
#[derive(Clone, Debug)]
pub enum FileSearchDialogEvent {
    /// Dialog was closed without selection.
    Close,
    /// A file was selected.
    FileSelected(PathBuf),
}

impl EventEmitter<FileSearchDialogEvent> for FileSearchDialog {}

impl Render for FileSearchDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let filtered_files = self.filtered_files.clone();
        let search_query = self.search_query.clone();
        let project_name = self.project_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Project".to_string());

        // Focus on first render
        window.focus(&focus_handle, cx);

        modal_backdrop("file-search-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("FileSearchDialog")
            .items_start()
            .pt(px(80.0))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => {
                        this.close(cx);
                    }
                    "up" => {
                        if this.selected_index > 0 {
                            this.selected_index -= 1;
                            this.scroll_to_selected();
                            cx.notify();
                        }
                    }
                    "down" => {
                        if this.selected_index < this.filtered_files.len().saturating_sub(1) {
                            this.selected_index += 1;
                            this.scroll_to_selected();
                            cx.notify();
                        }
                    }
                    "enter" => {
                        this.open_selected(cx);
                    }
                    "backspace" => {
                        if !this.search_query.is_empty() {
                            this.search_query.pop();
                            this.filter_files();
                            cx.notify();
                        }
                    }
                    key if key.len() == 1 => {
                        // Single character - add to search
                        let ch = key.chars().next().unwrap();
                        if ch.is_alphanumeric() || ch == ' ' || ch == '-' || ch == '_' || ch == '.' || ch == '/' {
                            this.search_query.push(ch);
                            this.filter_files();
                            cx.notify();
                        }
                    }
                    _ => {}
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    this.close(cx);
                }),
            )
            .child(
                modal_content("file-search-modal", &t)
                    .w(px(650.0))
                    .h(px(550.0))
                    .child(modal_header(
                        "Go to File",
                        Some(format!("Searching in {}", project_name)),
                        &t,
                        cx.listener(|this, _, _window, cx| this.close(cx)),
                    ))
                    .child(
                        // Search input area
                        div()
                            .px(px(12.0))
                            .py(px(10.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .border_b_1()
                            .border_color(rgb(t.border))
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .text_color(rgb(t.text_muted))
                                    .child(">"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(14.0))
                                    .text_color(if search_query.is_empty() {
                                        rgb(t.text_muted)
                                    } else {
                                        rgb(t.text_primary)
                                    })
                                    .child(if search_query.is_empty() {
                                        "Type to search files...".to_string()
                                    } else {
                                        search_query
                                    }),
                            ),
                    )
                    .child(if filtered_files.is_empty() {
                        div()
                            .flex_1()
                            .child(
                                div()
                                    .px(px(12.0))
                                    .py(px(20.0))
                                    .text_size(px(13.0))
                                    .text_color(rgb(t.text_muted))
                                    .child(if self.files.is_empty() {
                                        "No files found in project"
                                    } else {
                                        "No matching files"
                                    }),
                            )
                            .into_any_element()
                    } else {
                        let filtered = self.filtered_files.clone();
                        let view = cx.entity().clone();
                        uniform_list(
                            "file-list",
                            filtered.len(),
                            move |range, _window, cx| {
                                view.update(cx, |this, cx| {
                                    range
                                        .map(|i| {
                                            let file_index = filtered[i];
                                            this.render_file_row(i, file_index, cx)
                                        })
                                        .collect()
                                })
                            },
                        )
                        .flex_1()
                        .track_scroll(&self.scroll_handle)
                        .into_any_element()
                    })
                    .child(
                        // Footer with hints
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
                            .border_t_1()
                            .border_color(rgb(t.border))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(16.0))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(4.0))
                                            .child(
                                                div()
                                                    .px(px(4.0))
                                                    .py(px(1.0))
                                                    .rounded(px(3.0))
                                                    .bg(rgb(t.bg_secondary))
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(t.text_muted))
                                                    .child("Enter"),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(t.text_muted))
                                                    .child("to open"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(4.0))
                                            .child(
                                                div()
                                                    .px(px(4.0))
                                                    .py(px(1.0))
                                                    .rounded(px(3.0))
                                                    .bg(rgb(t.bg_secondary))
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(t.text_muted))
                                                    .child("Esc"),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.0))
                                                    .text_color(rgb(t.text_muted))
                                                    .child("to close"),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(rgb(t.text_muted))
                                    .child(format!("{} files", self.files.len())),
                            ),
                    ),
            )
    }
}

impl_focusable!(FileSearchDialog);
