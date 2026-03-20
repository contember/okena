//! File search dialog for quick file lookup.
//!
//! Provides a searchable list of files in the active project,
//! similar to VS Code's Cmd+P file picker.

use crate::keybindings::Cancel;
use crate::theme::theme;
use crate::views::components::{
    keyboard_hint, modal_backdrop, modal_content, modal_header, search_input_area_selected,
    ListOverlayConfig,
};
use gpui::*;
use gpui_component::h_flex;
use okena_ui::empty_state::empty_state;
use okena_ui::selectable_list::selectable_list_item;
use std::path::PathBuf;

/// Binary/non-openable file extensions that get pushed to the bottom of results.
const BINARY_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "svg", "webp",
    "mp3", "mp4", "wav", "avi", "mov",
    "zip", "tar", "gz", "rar", "7z",
    "pdf", "woff", "woff2", "ttf", "eot", "exe", "bin",
];

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

/// Characters allowed in search queries
const SEARCH_CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 -_./";

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

/// Remembered state from the last file search session.
#[derive(Default)]
struct FileSearchMemory {
    query: String,
    selected_index: usize,
}

impl Global for FileSearchMemory {}

/// File search dialog for finding files in a project
pub struct FileSearchDialog {
    focus_handle: FocusHandle,
    scroll_handle: UniformListScrollHandle,
    search_query: String,
    files: Vec<FileEntry>,
    filtered_files: Vec<(usize, Vec<usize>)>,
    selected_index: usize,
    project_path: PathBuf,
    config: ListOverlayConfig,
    /// When true, the entire query is "selected" — first keystroke replaces it.
    select_all: bool,
}

impl FileSearchDialog {
    /// Create a new file search dialog, restoring the last query if available.
    pub fn new(project_path: PathBuf, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let scroll_handle = UniformListScrollHandle::new();

        let files = Self::scan_files(&project_path);

        let config = ListOverlayConfig::new("Go to File")
            .searchable("Type to search files...")
            .size(650.0, 550.0)
            .key_context("FileSearchDialog");

        // Restore from previous session
        let memory = cx.try_global::<FileSearchMemory>();
        let (query, restored_index) = memory
            .map(|m| (m.query.clone(), m.selected_index))
            .unwrap_or_default();
        let select_all = !query.is_empty();

        let mut dialog = Self {
            focus_handle,
            scroll_handle,
            search_query: query,
            files,
            filtered_files: vec![],
            selected_index: 0,
            project_path,
            config,
            select_all,
        };

        dialog.filter_files();
        if restored_index < dialog.filtered_files.len() {
            dialog.selected_index = restored_index;
        }

        dialog
    }

    /// Scan files in the project directory.
    pub fn scan_files(project_path: &PathBuf) -> Vec<FileEntry> {
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

    /// Save current state for next open.
    fn save_memory(&self, cx: &mut Context<Self>) {
        cx.set_global(FileSearchMemory {
            query: self.search_query.clone(),
            selected_index: self.selected_index,
        });
    }

    /// Close the dialog, saving state for next open.
    fn close(&self, cx: &mut Context<Self>) {
        self.save_memory(cx);
        cx.emit(FileSearchDialogEvent::Close);
    }

    /// Open the currently selected file.
    fn open_selected(&self, cx: &mut Context<Self>) {
        if let Some(&(file_index, _)) = self.filtered_files.get(self.selected_index) {
            let file = &self.files[file_index];
            self.save_memory(cx);
            cx.emit(FileSearchDialogEvent::FileSelected(file.path.clone()));
        }
    }

    /// Scroll to keep the selected item visible.
    fn scroll_to_selected(&self) {
        if !self.filtered_files.is_empty() {
            self.scroll_handle.scroll_to_item(self.selected_index, ScrollStrategy::Top);
        }
    }

    /// Move selection up.
    fn select_prev(&mut self) -> bool {
        if self.selected_index > 0 {
            self.selected_index -= 1;
            self.scroll_to_selected();
            true
        } else {
            false
        }
    }

    /// Move selection down.
    fn select_next(&mut self) -> bool {
        if self.selected_index < self.filtered_files.len().saturating_sub(1) {
            self.selected_index += 1;
            self.scroll_to_selected();
            true
        } else {
            false
        }
    }

    /// Filter files based on the search query using fuzzy matching with scoring.
    fn filter_files(&mut self) {
        let query = self.search_query.to_lowercase();

        if query.is_empty() {
            self.filtered_files = (0..self.files.len()).map(|i| (i, vec![])).collect();
        } else {
            let mut scored: Vec<(usize, i32, Vec<usize>)> = self.files
                .iter()
                .enumerate()
                .filter_map(|(i, file)| {
                    let text = file.relative_path.to_lowercase();
                    Self::fuzzy_score(&text, &query, &file.filename, &file.relative_path)
                        .map(|(score, positions)| (i, score, positions))
                })
                .collect();

            scored.sort_by(|a, b| b.1.cmp(&a.1));
            self.filtered_files = scored.into_iter().map(|(i, _, pos)| (i, pos)).collect();
        }

        self.selected_index = 0;
    }

    /// Greedy character matching within a substring. Returns byte positions or None.
    fn find_positions(text: &str, query_chars: &[char]) -> Option<Vec<usize>> {
        let mut positions = Vec::with_capacity(query_chars.len());
        let mut text_idx = 0;
        let text_bytes: Vec<(usize, char)> = text.char_indices().collect();
        for &qc in query_chars {
            let mut found = false;
            while text_idx < text_bytes.len() {
                if text_bytes[text_idx].1 == qc {
                    positions.push(text_bytes[text_idx].0);
                    text_idx += 1;
                    found = true;
                    break;
                }
                text_idx += 1;
            }
            if !found {
                return None;
            }
        }
        Some(positions)
    }

    /// Fuzzy match with scoring. Returns (score, matched_byte_positions) or None.
    fn fuzzy_score(text: &str, query: &str, filename: &str, relative_path: &str) -> Option<(i32, Vec<usize>)> {
        let query_chars: Vec<char> = query.chars().collect();

        if query_chars.is_empty() {
            return Some((0, vec![]));
        }

        // Try to match entirely within the filename first for better highlights
        let filename_lower = filename.to_lowercase();
        let filename_start_byte = text.len().saturating_sub(filename_lower.len());
        let positions = if let Some(fn_positions) =
            Self::find_positions(&text[filename_start_byte..], &query_chars)
        {
            fn_positions.iter().map(|p| p + filename_start_byte).collect()
        } else if let Some(path_positions) = Self::find_positions(text, &query_chars) {
            path_positions
        } else {
            return None;
        };

        let text_bytes: Vec<(usize, char)> = text.char_indices().collect();

        // Calculate score
        let mut score: i32 = 0;

        // Consecutive matches bonus + gap penalty (multiplier 2)
        for w in positions.windows(2) {
            let p0_text_idx = text_bytes.iter().position(|(bi, _)| *bi == w[0])?;
            let p1_text_idx = text_bytes.iter().position(|(bi, _)| *bi == w[1])?;
            if p1_text_idx == p0_text_idx + 1 {
                score += 5;
            } else {
                score -= (p1_text_idx - p0_text_idx - 1) as i32 * 2;
            }
        }

        // Best contiguous streak bonus
        if positions.len() > 1 {
            let mut best_streak: usize = 1;
            let mut current_streak: usize = 1;
            for w in positions.windows(2) {
                let p0 = text_bytes.iter().position(|(bi, _)| *bi == w[0])?;
                let p1 = text_bytes.iter().position(|(bi, _)| *bi == w[1])?;
                if p1 == p0 + 1 {
                    current_streak += 1;
                    best_streak = best_streak.max(current_streak);
                } else {
                    current_streak = 1;
                }
            }
            if best_streak > 1 {
                score += (best_streak as i32 - 1) * 15;
            }
        }

        // Start-of-word bonus
        let word_separators = ['/', '.', '-', '_', '\\'];
        for &pos in &positions {
            if pos == 0 {
                score += 10;
            } else if let Some(prev_char) = text[..pos].chars().last() {
                if word_separators.contains(&prev_char) {
                    score += 10;
                }
            }
        }

        // Filename match bonus: matches in the filename portion score higher
        for &pos in &positions {
            if pos >= filename_start_byte {
                score += 25;
            }
        }

        // Exact filename match bonus
        if filename_lower == query {
            score += 200;
        } else if filename_lower.starts_with(query) {
            score += 100;
        } else if filename_lower.contains(query) {
            score += 50;
        }

        // Shorter path bonus
        score -= (relative_path.len() / 8) as i32;

        // Binary extension penalty
        if let Some(ext) = std::path::Path::new(relative_path)
            .extension()
            .and_then(|e| e.to_str())
        {
            if BINARY_EXTENSIONS.contains(&ext.to_lowercase().as_str()) {
                score -= 1000;
            }
        }

        Some((score, positions))
    }

    /// Build a `StyledText` with highlighted match positions.
    fn styled_text_with_highlights(
        text: &str,
        positions: &[usize],
        accent_color: u32,
    ) -> StyledText {
        let highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = positions
            .iter()
            .filter_map(|&pos| {
                // Find the byte length of the char at this position
                let ch = text.get(pos..)?.chars().next()?;
                Some((
                    pos..pos + ch.len_utf8(),
                    HighlightStyle {
                        color: Some(rgb(accent_color).into()),
                        font_weight: Some(FontWeight::BOLD),
                        ..Default::default()
                    },
                ))
            })
            .collect();

        StyledText::new(text.to_string()).with_highlights(highlights)
    }

    /// Render a single file row.
    fn render_file_row(
        &self,
        filtered_index: usize,
        file_index: usize,
        match_positions: &[usize],
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx);
        let file = &self.files[file_index];
        let is_selected = filtered_index == self.selected_index;

        let filename = &file.filename;
        let relative_path = &file.relative_path;

        // Get directory portion of the path
        let dir_path = if relative_path.contains('/') || relative_path.contains('\\') {
            let path = std::path::Path::new(relative_path.as_str());
            path.parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };

        // Split match positions into dir vs filename ranges
        let filename_start = if relative_path.len() >= filename.len() {
            relative_path.len() - filename.len()
        } else {
            0
        };

        let dir_positions: Vec<usize> = match_positions
            .iter()
            .filter(|&&p| p < filename_start)
            .copied()
            .collect();

        let filename_positions: Vec<usize> = match_positions
            .iter()
            .filter(|&&p| p >= filename_start)
            .map(|&p| p - filename_start)
            .collect();

        let filename_element = Self::styled_text_with_highlights(filename, &filename_positions, t.border_active);
        let dir_element = if dir_path.is_empty() {
            StyledText::new("\u{00A0}".to_string())
        } else {
            Self::styled_text_with_highlights(&dir_path, &dir_positions, t.border_active)
        };

        selectable_list_item(
                ElementId::Name(format!("file-{}", filtered_index).into()),
                is_selected,
                &t,
            )
            .w_full()
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
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(t.text_primary))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(filename_element),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(t.text_muted))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(dir_element),
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
        let search_query = self.search_query.clone();
        let project_name = self.project_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Project".to_string());

        // Focus on first render
        if !focus_handle.is_focused(window) {
            window.focus(&focus_handle, cx);
        }

        modal_backdrop("file-search-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context(self.config.key_context.as_str())
            .items_start()
            .pt(px(80.0))
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                match event.keystroke.key.as_str() {
                    "up" => {
                        this.select_all = false;
                        if this.select_prev() {
                            cx.notify();
                        }
                    }
                    "down" => {
                        this.select_all = false;
                        if this.select_next() {
                            cx.notify();
                        }
                    }
                    "enter" => this.open_selected(cx),
                    "backspace" => {
                        if this.select_all {
                            this.search_query.clear();
                            this.select_all = false;
                            this.filter_files();
                            cx.notify();
                        } else if !this.search_query.is_empty() {
                            this.search_query.pop();
                            this.filter_files();
                            cx.notify();
                        }
                    }
                    key if key.len() == 1 => {
                        let Some(ch) = key.chars().next() else { return };

                        if SEARCH_CHARS.contains(ch) {
                            if this.select_all {
                                this.search_query.clear();
                                this.select_all = false;
                            }
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
                    .w(px(self.config.width))
                    .h(px(self.config.max_height))
                    .child(modal_header(
                        &self.config.title,
                        Some(format!("Searching in {}", project_name)),
                        &t,
                        cx.listener(|this, _, _window, cx| this.close(cx)),
                    ))
                    .child(search_input_area_selected(&search_query, self.config.search_placeholder.as_ref().map(|s| s.as_str()).unwrap_or(""), self.select_all, &t))
                    .child(if self.filtered_files.is_empty() {
                        div()
                            .flex_1()
                            .child(empty_state(
                                if self.files.is_empty() { "No files found in project" } else { "No matching files" },
                                &t,
                            ))
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
                                            let (file_index, positions) = &filtered[i];
                                            this.render_file_row(i, *file_index, positions, cx)
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
                                h_flex()
                                    .gap(px(16.0))
                                    .child(keyboard_hint("Enter", "to open", &t))
                                    .child(keyboard_hint("Esc", "to close", &t)),
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
