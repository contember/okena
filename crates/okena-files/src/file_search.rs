//! File search dialog for quick file lookup.
//!
//! Provides a searchable list of files in the active project,
//! similar to VS Code's Cmd+P file picker.

use crate::list_overlay::ListOverlayConfig;
use crate::theme::theme;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::h_flex;
use ignore::WalkBuilder;
use okena_ui::badge::keyboard_hint;
use okena_ui::tokens::{ui_text_sm, ui_text_ms, ui_text};
use okena_ui::empty_state::empty_state;
use okena_ui::file_icon::file_icon;
use okena_ui::modal::{modal_backdrop, modal_content, modal_header};
use okena_ui::selectable_list::selectable_list_item;
use okena_ui::simple_input::{InputChangedEvent, SimpleInputState};
use std::path::PathBuf;

// Define Cancel action locally so we don't depend on the main app's keybindings
gpui::actions!(okena_files, [Cancel]);

/// Binary/non-openable file extensions that get pushed to the bottom of results.
const BINARY_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "svg", "webp",
    "mp3", "mp4", "wav", "avi", "mov",
    "zip", "tar", "gz", "rar", "7z",
    "pdf", "woff", "woff2", "ttf", "eot", "exe", "bin",
];

/// Maximum number of files to scan
const MAX_FILES: usize = 10000;

/// A file entry in the search list
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FileEntry {
    /// Full path to the file
    pub path: PathBuf,
    /// Path relative to project root
    pub relative_path: String,
    /// Just the filename
    pub filename: String,
}

/// Remembered state from the last file search session.
///
/// The `show_ignored` filter toggle is NOT stored here — it lives in
/// `AppSettings.file_finder` so it persists across app restarts and acts as
/// a global default. The caller seeds the dialog with that value and listens
/// for `FiltersChanged` events to write it back.
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
    search_input: Entity<SimpleInputState>,
    fs: std::sync::Arc<dyn crate::project_fs::ProjectFs>,
    files: Vec<FileEntry>,
    filtered_files: Vec<(usize, Vec<usize>)>,
    selected_index: usize,
    project_name: String,
    config: ListOverlayConfig,
    show_ignored: bool,
    filter_popover_open: bool,
    filter_button_bounds: Option<Bounds<Pixels>>,
    loading: bool,
}

impl FileSearchDialog {
    /// Create a new file search dialog, restoring the last query if available.
    ///
    /// `show_ignored` comes from `AppSettings.file_finder` — the caller
    /// (overlay manager) reads it from the global settings and passes it in.
    /// When the user toggles the filter, the dialog emits
    /// [`FileSearchDialogEvent::FiltersChanged`] so the caller can persist
    /// the new value back to settings.
    pub fn new(
        fs: std::sync::Arc<dyn crate::project_fs::ProjectFs>,
        show_ignored: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let scroll_handle = UniformListScrollHandle::new();

        let project_name = fs.project_name();

        let config = ListOverlayConfig::new("Go to File")
            .searchable("Type to search files...")
            .size(650.0, 550.0)
            .key_context("FileSearchDialog");

        // Restore in-session memory (query + selected_index only — the
        // show_ignored filter lives in AppSettings and was passed in as a param).
        let memory = cx.try_global::<FileSearchMemory>();
        let (query, restored_index) = memory
            .map(|m| (m.query.clone(), m.selected_index))
            .unwrap_or_default();

        // Create search input entity
        let search_input = cx.new(|cx| {
            let mut input = SimpleInputState::new(cx)
                .placeholder("Type to search files...");
            if !query.is_empty() {
                input.set_value(&query, cx);
                input.select_all(cx);
            }
            input
        });

        // Subscribe to input changes for filtering
        cx.subscribe(&search_input, |this: &mut Self, _, _: &InputChangedEvent, cx| {
            this.filter_files(cx);
            cx.notify();
        })
        .detach();

        // Load files asynchronously to avoid blocking the UI thread (important for remote projects)
        let fs_for_scan = fs.clone();
        cx.spawn(async move |entity: WeakEntity<Self>, cx| {
            let files = cx
                .background_executor()
                .spawn(async move { fs_for_scan.list_files(show_ignored) })
                .await;
            let _ = entity.update(cx, |this, cx| {
                this.files = files;
                this.loading = false;
                this.filter_files(cx);
                if restored_index < this.filtered_files.len() {
                    this.selected_index = restored_index;
                }
                cx.notify();
            });
        })
        .detach();

        Self {
            focus_handle,
            scroll_handle,
            search_input,
            fs,
            files: Vec::new(),
            filtered_files: vec![],
            selected_index: 0,
            project_name,
            config,
            show_ignored,
            filter_popover_open: false,
            filter_button_bounds: None,
            loading: true,
        }
    }

    /// Scan files in the project directory using the `ignore` crate.
    ///
    /// `show_ignored` is additive: regular (non-gitignored) files are scanned
    /// first, then gitignored files are appended up to `MAX_FILES`. Without
    /// this two-pass split, a single huge gitignored directory (e.g. an
    /// Android `build/` tree) can fill the cap alphabetically and crowd out
    /// real project files later in the walk.
    pub fn scan_files(project_path: &PathBuf, show_ignored: bool) -> Vec<FileEntry> {
        let mut files = Vec::new();
        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

        Self::collect_files(project_path, false, &mut files, &mut seen);
        if show_ignored {
            Self::collect_files(project_path, true, &mut files, &mut seen);
        }

        files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        files
    }

    /// Walk the project, appending entries to `files` until `MAX_FILES` is
    /// reached. `seen` tracks already-collected paths so the gitignored pass
    /// doesn't duplicate the regular pass.
    fn collect_files(
        project_path: &PathBuf,
        include_ignored: bool,
        files: &mut Vec<FileEntry>,
        seen: &mut std::collections::HashSet<PathBuf>,
    ) {
        let mut walk_builder = WalkBuilder::new(project_path);
        walk_builder
            .hidden(false)
            .git_ignore(!include_ignored)
            .git_global(!include_ignored)
            .git_exclude(!include_ignored)
            .max_depth(Some(15));

        let mut override_builder = ignore::overrides::OverrideBuilder::new(project_path);
        for pattern in crate::content_search::ALWAYS_IGNORE {
            let _ = override_builder.add(pattern);
        }
        if let Ok(overrides) = override_builder.build() {
            walk_builder.overrides(overrides);
        }

        for entry in walk_builder.build().flatten() {
            if files.len() >= MAX_FILES {
                break;
            }

            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if !seen.insert(path.to_path_buf()) {
                continue;
            }

            let filename = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name.to_string(),
                None => continue,
            };

            let relative_path = path
                .strip_prefix(project_path)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| filename.clone());

            files.push(FileEntry {
                path: path.to_path_buf(),
                relative_path,
                filename,
            });
        }
    }

    /// Save current state for next open.
    fn save_memory(&self, cx: &mut Context<Self>) {
        cx.set_global(FileSearchMemory {
            query: self.search_input.read(cx).value().to_string(),
            selected_index: self.selected_index,
        });
    }

    /// Toggle the gitignore filter, re-scan, and emit `FiltersChanged` so
    /// the caller can persist the new value to settings.
    fn toggle_filter(&mut self, filter: &str, cx: &mut Context<Self>) {
        if filter == "ignored" {
            self.show_ignored = !self.show_ignored;
        } else {
            return;
        }
        cx.emit(FileSearchDialogEvent::FiltersChanged {
            show_ignored: self.show_ignored,
        });
        self.rescan(cx);
        cx.notify();
    }

    /// Re-scan files in the background using current filter flags.
    fn rescan(&mut self, cx: &mut Context<Self>) {
        let fs = self.fs.clone();
        let show_ignored = self.show_ignored;
        self.loading = true;
        self.files.clear();
        self.filtered_files.clear();
        self.selected_index = 0;
        cx.spawn(async move |entity: WeakEntity<Self>, cx| {
            let files = cx
                .background_executor()
                .spawn(async move { fs.list_files(show_ignored) })
                .await;
            let _ = entity.update(cx, |this, cx| {
                // Discard if flag changed during scan (newer rescan in flight)
                if this.show_ignored != show_ignored {
                    return;
                }
                this.files = files;
                this.loading = false;
                this.filter_files(cx);
                cx.notify();
            });
        })
        .detach();
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
            cx.emit(FileSearchDialogEvent::FileSelected(file.relative_path.clone()));
        }
    }

    fn select_prev(&mut self) -> bool {
        crate::list_overlay::select_prev(&mut self.selected_index, &self.scroll_handle)
    }

    fn select_next(&mut self) -> bool {
        crate::list_overlay::select_next(&mut self.selected_index, self.filtered_files.len(), &self.scroll_handle)
    }

    /// Filter files based on the search query using fuzzy matching with scoring.
    fn filter_files(&mut self, cx: &Context<Self>) {
        let query = self.search_input.read(cx).value().to_lowercase();

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

    /// Fuzzy match with scoring using nucleo-matcher. Returns (score, matched_byte_positions) or None.
    fn fuzzy_score(text: &str, query: &str, filename: &str, relative_path: &str) -> Option<(i32, Vec<usize>)> {
        if query.is_empty() {
            return Some((0, vec![]));
        }

        use nucleo_matcher::{Config, Matcher, Utf32Str};

        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let mut haystack_buf = Vec::new();
        let mut needle_buf = Vec::new();
        let mut indices: Vec<u32> = Vec::new();

        let haystack = Utf32Str::new(text, &mut haystack_buf);
        let needle = Utf32Str::new(query, &mut needle_buf);

        // Run nucleo fuzzy matching
        let nucleo_score = matcher.fuzzy_indices(haystack, needle, &mut indices)?;
        let mut score = nucleo_score as i32;

        // Convert char indices to byte positions
        let char_to_byte: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        let positions: Vec<usize> = indices
            .iter()
            .filter_map(|&idx| char_to_byte.get(idx as usize).copied())
            .collect();

        // Additional bonuses on top of nucleo's score

        // Filename match bonus
        let filename_lower = filename.to_lowercase();
        let filename_start_byte = text.len().saturating_sub(filename_lower.len());
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
            .gap(px(8.0))
            .child(file_icon(filename, &t, cx))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .text_size(ui_text(13.0, cx))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(t.text_primary))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(filename_element),
                    )
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(dir_element),
                    ),
            )
    }

    fn render_filter_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);
        let active_count = self.show_ignored as u8;

        let entity = cx.entity().downgrade();
        let entity2 = entity.clone();

        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(12.0))
            .py(px(6.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .child(
                crate::list_overlay::file_filter_button(
                    "filter-btn", active_count, &t, cx,
                    move |_, _, cx| {
                        if let Some(e) = entity.upgrade() {
                            e.update(cx, |this, cx| {
                                this.filter_popover_open = !this.filter_popover_open;
                                cx.notify();
                            });
                        }
                    },
                    move |bounds, _, cx| {
                        if let Some(e) = entity2.upgrade() {
                            e.update(cx, |this, _| this.filter_button_bounds = Some(bounds));
                        }
                    },
                )
            )
    }
}

/// Events emitted by the file search dialog.
#[derive(Clone, Debug)]
pub enum FileSearchDialogEvent {
    /// Dialog was closed without selection.
    Close,
    /// A file was selected. Carries the project-relative path so callers don't
    /// need to know about absolute path semantics (which differ between local
    /// and remote projects).
    FileSelected(String),
    /// User toggled the gitignore filter. The caller persists this to
    /// settings so the new state becomes the default for future opens.
    FiltersChanged {
        show_ignored: bool,
    },
}

impl EventEmitter<FileSearchDialogEvent> for FileSearchDialog {}

impl okena_ui::overlay::CloseEvent for FileSearchDialogEvent {
    fn is_close(&self) -> bool { matches!(self, Self::Close) }
}

impl Render for FileSearchDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let project_name = self.project_name.clone();

        // Focus search input on first render
        let input_focus = self.search_input.read(cx).focus_handle(cx);
        if !input_focus.is_focused(window) {
            self.search_input.update(cx, |input, cx| input.focus(window, cx));
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
                        if this.select_prev() {
                            cx.notify();
                        }
                    }
                    "down" => {
                        if this.select_next() {
                            cx.notify();
                        }
                    }
                    "enter" => this.open_selected(cx),
                    "escape" => this.close(cx),
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
                    .relative()
                    .w(px(self.config.width))
                    .h(px(self.config.max_height))
                    .child(modal_header(
                        &self.config.title,
                        Some(format!("Searching in {}", project_name)),
                        &t,
                        cx,
                        cx.listener(|this, _, _window, cx| this.close(cx)),
                    ))
                    .child(crate::list_overlay::search_input_row(&self.search_input, &t, cx))
                    .child(self.render_filter_bar(cx))
                    .child(if self.loading {
                        div()
                            .flex_1()
                            .child(empty_state("Loading files…", &t, cx))
                            .into_any_element()
                    } else if self.filtered_files.is_empty() {
                        div()
                            .flex_1()
                            .child(empty_state(
                                if self.files.is_empty() { "No files found in project" } else { "No matching files" },
                                &t,
                                cx,
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
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(format!("{} files", self.files.len())),
                            ),
                    )
                    // Filter popover backdrop + overlay (at modal level, like settings dropdowns)
                    .when(self.filter_popover_open, |modal| {
                        modal.child(
                            div()
                                .id("filter-popover-backdrop")
                                .absolute()
                                .inset_0()
                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                    this.filter_popover_open = false;
                                    cx.notify();
                                }))
                        )
                    })
                    .when_some(
                        self.filter_popover_open
                            .then_some(self.filter_button_bounds)
                            .flatten(),
                        |modal, bounds| {
                            let entity = cx.entity().downgrade();
                            modal.child(crate::list_overlay::file_filter_popover(
                                bounds, self.show_ignored, &t, cx,
                                move |filter, _, cx| {
                                    if let Some(e) = entity.upgrade() {
                                        e.update(cx, |this, cx| this.toggle_filter(filter, cx));
                                    }
                                },
                            ))
                        },
                    ),
            )
    }
}

impl Focusable for FileSearchDialog {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
