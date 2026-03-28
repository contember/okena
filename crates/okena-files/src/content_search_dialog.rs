//! Content search dialog ("Find in Files") overlay.
//!
//! Provides a searchable overlay for finding text content across project files,
//! with syntax-highlighted results grouped by file.

use crate::content_search::{
    ContentSearchConfig, FileSearchResult, SearchHandle, SearchMode,
};
use crate::code_view::{CodeSelection, build_styled_text_with_backgrounds, extract_selected_text, selection_bg_ranges};
use crate::selection::copy_to_clipboard;
use crate::file_tree::{build_file_tree, expandable_folder_row, expandable_file_row, FileTreeNode};
use crate::list_overlay::ListOverlayConfig;
use crate::syntax::{
    HighlightedLine, highlight_content, load_syntax_set,
};
use crate::theme::theme;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::h_flex;
use gpui_component::tooltip::Tooltip;
use okena_ui::badge::keyboard_hint;
use okena_ui::empty_state::empty_state;
use okena_ui::file_icon::file_icon;
use okena_ui::modal::{fullscreen_overlay, modal_backdrop, modal_content, modal_header};
use okena_ui::selectable_list::selectable_list_item;
use okena_ui::simple_input::{InputChangedEvent, SimpleInput, SimpleInputState};
use okena_ui::text_utils::find_word_boundaries;
use okena_ui::tokens::{ui_text, ui_text_ms, ui_text_sm};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use syntect::parsing::SyntaxSet;

// Local action for closing the dialog
gpui::actions!(okena_files_content_search, [Cancel]);

/// Remembered state from the last content search session.
#[derive(Default)]
struct ContentSearchMemory {
    query: String,
    case_sensitive: bool,
    regex: bool,
    fuzzy: bool,
    file_glob: Option<String>,
    glob_input: String,
    expanded: bool,
    show_ignored: bool,
    show_hidden: bool,
}

impl Global for ContentSearchMemory {}

/// A flattened result row for display in the list.
#[derive(Clone)]
enum ResultRow {
    /// File header row (file path, match count).
    FileHeader {
        file_path: PathBuf,
        relative_path: String,
        match_count: usize,
    },
    /// Match row within a file, with optional context lines.
    Match {
        file_path: PathBuf,
        line_number: usize,
        line_content: String,
        match_ranges: Vec<std::ops::Range<usize>>,
        /// Context lines before the match (line_number, content).
        context_before: Vec<(usize, String)>,
        /// Context lines after the match (line_number, content).
        context_after: Vec<(usize, String)>,
    },
}

/// Content search dialog for finding text in project files.
pub struct ContentSearchDialog {
    focus_handle: FocusHandle,
    scroll_handle: UniformListScrollHandle,
    search_input: Entity<SimpleInputState>,
    project_fs: std::sync::Arc<dyn crate::project_fs::ProjectFs>,
    config: ListOverlayConfig,
    /// Flattened result rows for display.
    rows: Vec<ResultRow>,
    selected_index: usize,
    /// Total number of matches across all files.
    total_matches: usize,
    /// Whether a search is currently running.
    searching: bool,
    /// Handle to cancel running search.
    search_handle: Option<SearchHandle>,
    /// Search config toggles.
    case_sensitive: bool,
    regex_mode: bool,
    fuzzy_mode: bool,
    show_ignored: bool,
    show_hidden: bool,
    filter_popover_open: bool,
    filter_button_bounds: Option<Bounds<Pixels>>,
    file_glob: Option<String>,
    /// Glob filter input entity.
    glob_input: Entity<SimpleInputState>,
    /// Whether the glob input row is visible.
    glob_editing: bool,
    /// Whether the overlay is in expanded (full) mode.
    expanded: bool,
    /// Cached syntax-highlighted lines per file path.
    highlight_cache: HashMap<PathBuf, Vec<HighlightedLine>>,
    /// Shared syntax set.
    syntax_set: SyntaxSet,
    /// Whether the theme is dark.
    is_dark: bool,
    /// Debounce task for search.
    debounce_task: Option<Task<()>>,
    /// Scroll handle for the preview panel.
    preview_scroll_handle: UniformListScrollHandle,
    /// Expanded folder paths in the sidebar.
    expanded_folders: HashSet<String>,
    /// Scroll handle for the sidebar tree.
    tree_scroll_handle: ScrollHandle,
    /// Currently active scope path (folder or file) shown in sidebar.
    scope_path: Option<String>,
    /// Text selection state in the preview panel.
    preview_selection: CodeSelection,
    /// File path currently shown in preview (for selection reset).
    preview_file: Option<PathBuf>,
}

impl ContentSearchDialog {
    pub fn new(project_fs: std::sync::Arc<dyn crate::project_fs::ProjectFs>, is_dark: bool, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let scroll_handle = UniformListScrollHandle::new();
        let syntax_set = load_syntax_set();

        let config = ListOverlayConfig::new("Find in Files")
            .searchable("Search file contents...")
            .size(700.0, 550.0)
            .key_context("ContentSearchDialog");

        // Restore from previous session
        let memory = cx.try_global::<ContentSearchMemory>();
        let (query, case_sensitive, regex_mode, fuzzy_mode, file_glob, glob_input_text, expanded, show_ignored, show_hidden) =
            memory
                .map(|m| {
                    (
                        m.query.clone(),
                        m.case_sensitive,
                        m.regex,
                        m.fuzzy,
                        m.file_glob.clone(),
                        m.glob_input.clone(),
                        m.expanded,
                        m.show_ignored,
                        m.show_hidden,
                    )
                })
                .unwrap_or_default();

        // Create search input entity
        let search_input = cx.new(|cx| {
            let mut input = SimpleInputState::new(cx)
                .placeholder("Search file contents...");
            if !query.is_empty() {
                input.set_value(&query, cx);
                input.select_all(cx);
            }
            input
        });

        // Subscribe to search input changes
        cx.subscribe(&search_input, |this: &mut Self, _, _: &InputChangedEvent, cx| {
            this.trigger_search(cx);
        })
        .detach();

        // Create glob filter input entity
        let glob_input = cx.new(|cx| {
            let mut input = SimpleInputState::new(cx)
                .placeholder("e.g. *.rs, src/**/*.ts");
            if !glob_input_text.is_empty() {
                input.set_value(&glob_input_text, cx);
            }
            input
        });

        // Subscribe to glob input changes
        cx.subscribe(&glob_input, |this: &mut Self, _, _: &InputChangedEvent, cx| {
            let value = this.glob_input.read(cx).value().to_string();
            this.file_glob = if value.is_empty() { None } else { Some(value) };
            this.trigger_search(cx);
        })
        .detach();

        let has_query = !query.is_empty();

        let mut dialog = Self {
            focus_handle,
            scroll_handle,
            search_input,
            project_fs,
            config,
            rows: Vec::new(),
            selected_index: 0,
            total_matches: 0,
            searching: false,
            search_handle: None,
            case_sensitive,
            regex_mode,
            fuzzy_mode,
            show_ignored,
            show_hidden,
            filter_popover_open: false,
            filter_button_bounds: None,
            file_glob,
            glob_input,
            glob_editing: false,
            expanded,
            highlight_cache: HashMap::new(),
            syntax_set,
            is_dark,
            debounce_task: None,
            preview_scroll_handle: UniformListScrollHandle::new(),
            expanded_folders: HashSet::new(),
            tree_scroll_handle: ScrollHandle::new(),
            scope_path: None,
            preview_selection: CodeSelection::default(),
            preview_file: None,
        };

        // Run initial search if we have a restored query
        if has_query {
            dialog.trigger_search(cx);
        }

        dialog
    }

    /// Save current state for next open.
    fn save_memory(&self, cx: &mut Context<Self>) {
        cx.set_global(ContentSearchMemory {
            query: self.search_input.read(cx).value().to_string(),
            case_sensitive: self.case_sensitive,
            regex: self.regex_mode,
            fuzzy: self.fuzzy_mode,
            file_glob: self.file_glob.clone(),
            glob_input: self.glob_input.read(cx).value().to_string(),
            expanded: self.expanded,
            show_ignored: self.show_ignored,
            show_hidden: self.show_hidden,
        });
    }

    fn close(&self, cx: &mut Context<Self>) {
        if let Some(handle) = &self.search_handle {
            handle.cancel();
        }
        self.save_memory(cx);
        cx.emit(ContentSearchDialogEvent::Close);
    }

    /// Open file viewer at the selected match.
    fn open_selected(&self, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get(self.selected_index) {
            let (path, line) = match row {
                ResultRow::Match { file_path, line_number, .. } => (file_path.clone(), *line_number),
                ResultRow::FileHeader { file_path, .. } => (file_path.clone(), 1),
            };
            self.save_memory(cx);
            cx.emit(ContentSearchDialogEvent::FileSelected { path, line });
        }
    }

    fn select_prev(&mut self) -> bool {
        crate::list_overlay::select_prev(&mut self.selected_index, &self.scroll_handle)
    }

    fn select_next(&mut self) -> bool {
        crate::list_overlay::select_next(&mut self.selected_index, self.rows.len(), &self.scroll_handle)
    }

    /// Trigger a debounced search.
    fn trigger_search(&mut self, cx: &mut Context<Self>) {
        // Cancel any running search
        if let Some(handle) = self.search_handle.take() {
            handle.cancel();
        }

        let query = self.search_input.read(cx).value().to_string();
        if query.is_empty() {
            self.rows.clear();
            self.total_matches = 0;
            self.searching = false;
            self.selected_index = 0;
            cx.notify();
            return;
        }

        // Debounce: wait 200ms before starting search
        self.debounce_task = Some(cx.spawn(async move |this: WeakEntity<ContentSearchDialog>, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(200)).await;
            this.update(cx, |this, cx| {
                this.run_search(cx);
            }).ok();
        }));
    }

    /// Actually run the search on a background thread.
    fn run_search(&mut self, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).value().to_string();
        if query.is_empty() {
            return;
        }

        let handle = SearchHandle::new();
        self.search_handle = Some(handle.clone());
        self.searching = true;
        self.highlight_cache.clear();
        cx.notify();

        let mode = if self.fuzzy_mode {
            SearchMode::Fuzzy
        } else if self.regex_mode {
            SearchMode::Regex
        } else {
            SearchMode::Literal
        };

        let config = ContentSearchConfig {
            case_sensitive: self.case_sensitive,
            mode,
            max_results: 1000,
            file_glob: self.file_glob.clone(),
            context_lines: 0,
            show_ignored: self.show_ignored,
            show_hidden: self.show_hidden,
        };

        let project_fs = self.project_fs.clone();
        let cancelled = handle.flag();

        cx.spawn(async move |entity: WeakEntity<ContentSearchDialog>, cx| {
            let results = cx
                .background_executor()
                .spawn(async move {
                    let mut results: Vec<FileSearchResult> = Vec::new();
                    project_fs.search_content(
                        &query,
                        &config,
                        &cancelled,
                        &mut |result| {
                            results.push(result);
                        },
                    );
                    // Sort files by best match score (highest first) for fuzzy mode
                    results.sort_by(|a, b| b.best_score.cmp(&a.best_score));
                    results
                })
                .await;

            entity
                .update(cx, |this, cx| {
                    if this
                        .search_handle
                        .as_ref()
                        .is_some_and(|h| !h.is_cancelled())
                    {
                        this.apply_results(results);
                        this.searching = false;
                        cx.notify();
                    }
                })
                .ok();
        })
        .detach();
    }

    /// Convert search results into flattened display rows.
    fn apply_results(&mut self, results: Vec<FileSearchResult>) {
        self.rows.clear();
        self.total_matches = 0;

        for file_result in &results {
            self.rows.push(ResultRow::FileHeader {
                file_path: file_result.file_path.clone(),
                relative_path: file_result.relative_path.clone(),
                match_count: file_result.matches.len(),
            });

            for m in &file_result.matches {
                self.total_matches += 1;
                self.rows.push(ResultRow::Match {
                    file_path: file_result.file_path.clone(),
                    line_number: m.line_number,
                    line_content: m.line_content.clone(),
                    match_ranges: m.match_ranges.clone(),
                    context_before: m.context_before.clone(),
                    context_after: m.context_after.clone(),
                });
            }
        }

        self.selected_index = if self.rows.is_empty() { 0 } else { 1.min(self.rows.len() - 1) };
    }

    /// Get or compute syntax-highlighted lines for a file.
    fn get_highlighted_line(
        &mut self,
        file_path: &Path,
        line_number: usize,
    ) -> Option<HighlightedLine> {
        if !self.highlight_cache.contains_key(file_path) {
            let content = std::fs::read_to_string(file_path).ok()?;
            let lines = highlight_content(
                &content,
                file_path,
                &self.syntax_set,
                5000, // skip syntax highlighting for very large files
                self.is_dark,
            );
            self.highlight_cache.insert(file_path.to_path_buf(), lines);
        }

        let lines = self.highlight_cache.get(file_path)?;
        // line_number is 1-based
        lines.get(line_number.saturating_sub(1)).cloned()
    }

    /// Render a file header row.
    fn render_file_header(
        &self,
        idx: usize,
        relative_path: &str,
        match_count: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx);
        let is_selected = idx == self.selected_index;
        let filename = Path::new(relative_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| relative_path.to_string());

        selectable_list_item(
            ElementId::Name(format!("file-header-{}", idx).into()),
            is_selected,
            &t,
        )
        .w_full()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _window, cx| {
                this.selected_index = idx;
                this.open_selected(cx);
            }),
        )
        .gap(px(8.0))
        .child(file_icon(&filename, &t, cx))
        .child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .gap(px(8.0))
                .overflow_hidden()
                .child(
                    div()
                        .text_size(ui_text(13.0, cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(t.text_primary))
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(relative_path.to_string()),
                )
                .child(
                    div()
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.text_muted))
                        .child(format!("{} match{}", match_count, if match_count == 1 { "" } else { "es" })),
                ),
        )
    }

    /// Render a single styled code line (used for both match and context lines).
    fn render_code_line(
        &mut self,
        file_path: &Path,
        line_number: usize,
        line_content: &str,
        match_ranges: Option<&[std::ops::Range<usize>]>,
        t: &okena_core::theme::ThemeColors,
        cx: &App,
    ) -> Div {
        let styled_text = if let Some(highlighted) = self.get_highlighted_line(file_path, line_number) {
            if let Some(ranges) = match_ranges {
                let match_bg = search_match_bg(t.search_match_bg);
                let bg_ranges: Vec<(std::ops::Range<usize>, Hsla)> = ranges
                    .iter()
                    .filter(|r| r.end <= highlighted.plain_text.len())
                    .map(|r| (r.clone(), match_bg))
                    .collect();
                build_styled_text_with_backgrounds(&highlighted.spans, &bg_ranges)
            } else {
                build_styled_text_with_backgrounds(&highlighted.spans, &[])
            }
        } else if let Some(ranges) = match_ranges {
            let match_bg = search_match_bg(t.search_match_bg);
            let highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = ranges
                .iter()
                .filter(|r| r.end <= line_content.len())
                .map(|r| (r.clone(), HighlightStyle {
                    background_color: Some(match_bg),
                    ..Default::default()
                }))
                .collect();
            StyledText::new(line_content.to_string()).with_highlights(highlights)
        } else {
            StyledText::new(line_content.to_string())
        };

        let is_context = match_ranges.is_none();

        div()
            .flex()
            .gap(px(8.0))
            .when(is_context, |d| d.opacity(0.5))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .min_w(px(40.0))
                    .flex_shrink_0()
                    .child(format!("{:>4}", line_number)),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_size(ui_text_ms(cx))
                    .font_family("monospace")
                    .text_color(rgb(if is_context { t.text_muted } else { t.text_primary }))
                    .child(styled_text),
            )
    }

    /// Render a match result row with optional context lines as one selectable block.
    fn render_match_row(
        &mut self,
        idx: usize,
        file_path: &Path,
        line_number: usize,
        line_content: &str,
        match_ranges: &[std::ops::Range<usize>],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let is_selected = idx == self.selected_index;

        selectable_list_item(
            ElementId::Name(format!("match-{}", idx).into()),
            is_selected,
            &t,
        )
        .w_full()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                this.selected_index = idx;
                if event.click_count >= 2 {
                    this.open_selected(cx);
                }
                cx.notify();
            }),
        )
        .gap(px(8.0))
        .pl(px(28.0))
        .child(self.render_code_line(file_path, line_number, line_content, Some(match_ranges), &t, cx))
        .into_any_element()
    }

    /// Render the file preview panel showing the selected match's file.
    fn render_preview_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);

        // Get the currently selected match info
        let selected_match = self.rows.get(self.selected_index).and_then(|row| match row {
            ResultRow::Match {
                file_path,
                line_number,
                match_ranges,
                ..
            } => Some((file_path.clone(), *line_number, match_ranges.clone())),
            ResultRow::FileHeader { file_path, .. } => Some((file_path.clone(), 1, vec![])),
        });

        let Some((file_path, match_line, match_ranges)) = selected_match else {
            return div()
                .flex_1()
                .h_full()
                .bg(rgb(t.bg_primary))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.text_muted))
                        .child("Select a match to preview"),
                );
        };

        // Reset selection when preview file changes
        if self.preview_file.as_ref() != Some(&file_path) {
            self.preview_selection = CodeSelection::default();
            self.preview_file = Some(file_path.clone());
        }

        // Ensure file is in highlight cache
        if !self.highlight_cache.contains_key(&file_path) {
            let fp_str = file_path.to_string_lossy();
            if let Ok(content) = self.project_fs.read_file(&fp_str) {
                let lines = highlight_content(
                    &content,
                    &file_path,
                    &self.syntax_set,
                    5000,
                    self.is_dark,
                );
                self.highlight_cache.insert(file_path.clone(), lines);
            }
        }

        let lines = self.highlight_cache.get(&file_path).cloned().unwrap_or_default();
        let line_count = lines.len();
        let match_bg = search_match_bg(t.search_match_bg);
        let current_match_bg = Hsla::from(Rgba {
            r: ((t.search_current_bg >> 16) & 0xFF) as f32 / 255.0,
            g: ((t.search_current_bg >> 8) & 0xFF) as f32 / 255.0,
            b: (t.search_current_bg & 0xFF) as f32 / 255.0,
            a: 0.4,
        });

        // Find all matches in this file to highlight them all (current brighter)
        let all_matches_in_file: Vec<(usize, Vec<std::ops::Range<usize>>)> = self
            .rows
            .iter()
            .filter_map(|row| match row {
                ResultRow::Match {
                    file_path: fp,
                    line_number,
                    match_ranges,
                    ..
                } if *fp == file_path => Some((*line_number, match_ranges.clone())),
                _ => None,
            })
            .collect();

        let relative_path = file_path.to_string_lossy().to_string();

        // Scroll to the match line
        let scroll_to = match_line.saturating_sub(5); // 5 lines above for context
        self.preview_scroll_handle
            .scroll_to_item(scroll_to, ScrollStrategy::Top);

        let view = cx.entity().clone();

        div()
            .flex_1()
            .h_full()
            .bg(rgb(t.bg_primary))
            .border_l_1()
            .border_color(rgb(t.border))
            .flex()
            .flex_col()
            // File path header
            .child(
                div()
                    .px(px(12.0))
                    .py(px(8.0))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(relative_path),
            )
            // File content
            .child(
                uniform_list(
                    "preview-lines",
                    line_count,
                    move |range, _window, cx| {
                        view.update(cx, |this, cx| {
                            let t = theme(cx);
                            range
                                .map(|line_idx| {
                                    let line_number = line_idx + 1;
                                    let line_num_str = format!("{:>4}", line_number);

                                    // Check if this line has matches
                                    let line_match = all_matches_in_file
                                        .iter()
                                        .find(|(ln, _)| *ln == line_number);

                                    let is_current_match = line_number == match_line;

                                    // Combine match highlights with selection highlights
                                    let line_len = lines.get(line_idx).map_or(0, |hl| hl.plain_text.len());
                                    let sel_bg_ranges = selection_bg_ranges(&this.preview_selection, line_idx, line_len);

                                    let styled_text = if let Some(hl) =
                                        lines.get(line_idx)
                                    {
                                        let mut bg_ranges: Vec<(std::ops::Range<usize>, Hsla)> = Vec::new();
                                        if let Some((_, ranges)) = line_match {
                                            let bg = if is_current_match {
                                                current_match_bg
                                            } else {
                                                match_bg
                                            };
                                            bg_ranges.extend(
                                                ranges
                                                    .iter()
                                                    .filter(|r| r.end <= hl.plain_text.len())
                                                    .map(|r| (r.clone(), bg)),
                                            );
                                        }
                                        bg_ranges.extend(sel_bg_ranges);
                                        build_styled_text_with_backgrounds(
                                            &hl.spans, &bg_ranges,
                                        )
                                    } else {
                                        StyledText::new(String::new())
                                    };

                                    let text_layout = styled_text.layout().clone();
                                    let plain_text = lines.get(line_idx).map(|hl| hl.plain_text.clone()).unwrap_or_default();

                                    let row_bg = if is_current_match {
                                        Some(current_match_bg)
                                    } else if line_match.is_some() {
                                        Some(match_bg)
                                    } else {
                                        None
                                    };

                                    div()
                                        .id(ElementId::Name(format!("preview-line-{}", line_idx).into()))
                                        .flex()
                                        .items_center()
                                        .px(px(8.0))
                                        .h(px(24.0))
                                        .text_size(ui_text(13.0, cx))
                                        .font_family("monospace")
                                        .when_some(row_bg, |d, bg| d.bg(bg))
                                        .on_mouse_down(MouseButton::Left, {
                                            let text_layout = text_layout.clone();
                                            let plain_text = plain_text.clone();
                                            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                                let col = text_layout
                                                    .index_for_position(event.position)
                                                    .unwrap_or_else(|ix| ix)
                                                    .min(line_len);
                                                if event.click_count >= 3 {
                                                    this.preview_selection.start = Some((line_idx, 0));
                                                    this.preview_selection.end = Some((line_idx, line_len));
                                                    this.preview_selection.finish();
                                                } else if event.click_count == 2 {
                                                    let (start, end) = find_word_boundaries(&plain_text, col);
                                                    this.preview_selection.start = Some((line_idx, start));
                                                    this.preview_selection.end = Some((line_idx, end));
                                                    this.preview_selection.finish();
                                                } else {
                                                    this.preview_selection.start = Some((line_idx, col));
                                                    this.preview_selection.end = Some((line_idx, col));
                                                    this.preview_selection.is_selecting = true;
                                                }
                                                cx.notify();
                                            })
                                        })
                                        .on_mouse_move({
                                            let text_layout = text_layout.clone();
                                            cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                                                if this.preview_selection.is_selecting {
                                                    let col = text_layout
                                                        .index_for_position(event.position)
                                                        .unwrap_or_else(|ix| ix)
                                                        .min(line_len);
                                                    this.preview_selection.end = Some((line_idx, col));
                                                    cx.notify();
                                                }
                                            })
                                        })
                                        .on_mouse_up(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _window, cx| {
                                                this.preview_selection.finish();
                                                cx.notify();
                                            }),
                                        )
                                        .child(
                                            div()
                                                .text_color(rgb(t.text_muted))
                                                .min_w(px(44.0))
                                                .flex_shrink_0()
                                                .text_size(ui_text_ms(cx))
                                                .child(line_num_str),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .overflow_hidden()
                                                .text_color(rgb(t.text_primary))
                                                .child(styled_text),
                                        )
                                        .into_any_element()
                                })
                                .collect()
                        })
                    },
                )
                .flex_1()
                .track_scroll(&self.preview_scroll_handle),
            )
    }

    /// Set scope to a folder or file path, updating the glob filter and re-searching.
    fn set_scope(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        self.scope_path = path.clone();
        // Determine if path is a folder (exists in expanded_folders or has children in tree)
        // by checking if any file's relative_path starts with it + "/"
        let glob = path.map(|p| {
            let prefix = format!("{p}/");
            let is_folder = self.rows.iter().any(|r| matches!(r, ResultRow::FileHeader { relative_path, .. } if relative_path.starts_with(&prefix)));
            if is_folder { format!("{p}/**") } else { p }
        });
        self.file_glob = glob.clone();
        self.glob_input.update(cx, |input, cx| {
            input.set_value(glob.as_deref().unwrap_or(""), cx);
        });
        self.trigger_search(cx);
        cx.notify();
    }

    /// Toggle folder expansion in the sidebar tree.
    fn toggle_folder(&mut self, folder_path: &str, cx: &mut Context<Self>) {
        if !self.expanded_folders.remove(folder_path) {
            self.expanded_folders.insert(folder_path.to_string());
        }
        cx.notify();
    }

    /// Render the sidebar file tree for expanded mode.
    /// Shows only files/folders that have search results.
    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);

        // Build tree from matched files only
        let matched_files: Vec<(usize, &str)> = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(i, row)| match row {
                ResultRow::FileHeader { relative_path, .. } => Some((i, relative_path.as_str())),
                _ => None,
            })
            .collect();
        let result_tree = build_file_tree(matched_files.into_iter());
        let tree_elements = self.render_tree_node(&result_tree, 0, "", &t, cx);

        div()
            .w(px(240.0))
            .h_full()
            .border_r_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_primary))
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(16.0))
                    .py(px(10.0))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(t.text_secondary))
                            .child("Scope"),
                    )
                    .when_some(self.scope_path.clone(), |d, _| {
                        d.child(
                            div()
                                .id("clear-scope")
                                .cursor_pointer()
                                .text_size(ui_text_sm(cx))
                                .text_color(rgb(t.text_muted))
                                .hover(|s| s.text_color(rgb(t.text_primary)))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _window, cx| {
                                        this.set_scope(None, cx);
                                    }),
                                )
                                .child("clear"),
                        )
                    }),
            )
            .child(
                div()
                    .id("scope-tree")
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.tree_scroll_handle)
                    .py(px(6.0))
                    .children(tree_elements),
            )
    }

    /// Recursively render file tree nodes for the sidebar.
    /// Matches FileViewer's tree style (chevrons, folder icons, sizing).
    fn render_tree_node(
        &self,
        node: &FileTreeNode,
        depth: usize,
        parent_path: &str,
        t: &okena_core::theme::ThemeColors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut elements = Vec::new();

        for (name, child) in &node.children {
            let folder_path = if parent_path.is_empty() {
                name.clone()
            } else {
                format!("{parent_path}/{name}")
            };
            let is_expanded = self.expanded_folders.contains(&folder_path);
            let is_scoped = self.scope_path.as_ref() == Some(&folder_path);
            let fp_toggle = folder_path.clone();
            let fp_scope = folder_path.clone();

            elements.push(
                expandable_folder_row(name, depth, is_expanded, t, cx)
                    .id(ElementId::Name(format!("cs-folder-{}", folder_path).into()))
                    .when(is_scoped, |d| d.bg(rgb(t.bg_selection)))
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.toggle_folder(&fp_toggle, cx);
                    }))
                    // Scope button
                    .child(
                        div()
                            .id(ElementId::Name(format!("scope-folder-{}", folder_path).into()))
                            .cursor_pointer()
                            .px(px(4.0))
                            .py(px(2.0))
                            .rounded(px(3.0))
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(if is_scoped { t.text_primary } else { t.text_muted }))
                            .when(is_scoped, |d| d.bg(rgb(t.border_active)))
                            .hover(|s| s.bg(rgb(t.bg_hover)).text_color(rgb(t.text_primary)))
                            .flex_shrink_0()
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _window, cx| {
                                if this.scope_path.as_ref() == Some(&fp_scope) {
                                    this.set_scope(None, cx);
                                } else {
                                    this.set_scope(Some(fp_scope.clone()), cx);
                                }
                            }))
                            .child(if is_scoped { "scoped" } else { "scope" }),
                    )
                    .into_any_element(),
            );

            if is_expanded {
                elements.extend(self.render_tree_node(child, depth + 1, &folder_path, t, cx));
            }
        }

        for &row_index in &node.files {
            if let Some(ResultRow::FileHeader {
                relative_path,
                match_count,
                ..
            }) = self.rows.get(row_index)
            {
                let filename = std::path::Path::new(relative_path.as_str())
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| relative_path.clone());
                let rel = relative_path.clone();
                let is_scoped = self.scope_path.as_ref() == Some(&rel);
                let count = *match_count;

                elements.push(
                    expandable_file_row(&filename, depth, None, t, cx)
                        .id(ElementId::Name(format!("cs-file-{}", row_index).into()))
                        .when(is_scoped, |d| d.bg(rgb(t.bg_selection)))
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            if this.scope_path.as_ref() == Some(&rel) {
                                this.set_scope(None, cx);
                            } else {
                                this.set_scope(Some(rel.clone()), cx);
                            }
                        }))
                        .child(
                            div()
                                .text_size(ui_text_sm(cx))
                                .text_color(rgb(t.text_muted))
                                .flex_shrink_0()
                                .ml(px(4.0))
                                .child(count.to_string()),
                        )
                        .into_any_element(),
                );
            }
        }

        elements
    }

    fn render_toggles(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);

        let glob_value = self.glob_input.read(cx).value().to_string();
        let has_glob = !glob_value.is_empty();

        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .px(px(12.0))
            .py(px(6.0))
            .border_b_1()
            .border_color(rgb(t.border))
            .child(self.render_toggle_button("Aa", self.case_sensitive, "Case Sensitive", "case", cx))
            .child(self.render_toggle_button(".*", self.regex_mode, "Regular Expression", "regex", cx))
            .child(self.render_toggle_button("~", self.fuzzy_mode, "Fuzzy Match", "fuzzy", cx))
            .child(self.render_file_filter_button(cx))
            // Glob filter input
            .child(
                div()
                    .id("glob-filter")
                    .cursor_pointer()
                    .px(px(8.0))
                    .py(px(3.0))
                    .rounded(px(4.0))
                    .text_size(ui_text_sm(cx))
                    .bg(rgb(if has_glob { t.border_active } else { t.bg_secondary }))
                    .text_color(rgb(if has_glob { t.text_primary } else { t.text_muted }))
                    .child(if has_glob {
                        format!("filter: {}", glob_value)
                    } else {
                        "filter".to_string()
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.glob_editing = !this.glob_editing;
                            if this.glob_editing {
                                this.glob_input.update(cx, |input, cx| input.focus(window, cx));
                            } else {
                                this.search_input.update(cx, |input, cx| input.focus(window, cx));
                            }
                            cx.notify();
                        }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .justify_end()
                    .child(
                        div()
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_muted))
                            .child(if self.searching {
                                "Searching...".to_string()
                            } else if self.total_matches > 0 {
                                format!(
                                    "{} match{} in {} file{}",
                                    self.total_matches,
                                    if self.total_matches == 1 { "" } else { "es" },
                                    self.rows.iter().filter(|r| matches!(r, ResultRow::FileHeader { .. })).count(),
                                    if self.rows.iter().filter(|r| matches!(r, ResultRow::FileHeader { .. })).count() == 1 { "" } else { "s" },
                                )
                            } else if !self.search_input.read(cx).value().is_empty() {
                                "No results".to_string()
                            } else {
                                String::new()
                            }),
                    ),
            )
    }

    /// Render a single toggle button with tooltip.
    fn render_toggle_button(
        &self,
        label: &str,
        active: bool,
        tooltip: &str,
        id: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx);
        let id_owned = id.to_string();
        let tooltip_text: SharedString = tooltip.to_string().into();

        div()
            .id(ElementId::Name(format!("toggle-{}", id).into()))
            .cursor_pointer()
            .px(px(8.0))
            .py(px(3.0))
            .rounded(px(4.0))
            .text_size(ui_text_sm(cx))
            .font_weight(FontWeight::MEDIUM)
            .tooltip(move |_window, cx| Tooltip::new(tooltip_text.clone()).build(_window, cx))
            .when(active, |d: Stateful<Div>| {
                d.bg(rgb(t.border_active))
                    .text_color(rgb(t.text_primary))
            })
            .when(!active, |d: Stateful<Div>| {
                d.bg(rgb(t.bg_secondary))
                    .text_color(rgb(t.text_muted))
            })
            .hover(|s: StyleRefinement| s.bg(rgb(t.bg_hover)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _window, cx| {
                    match id_owned.as_str() {
                        "case" => this.case_sensitive = !this.case_sensitive,
                        "regex" => {
                            this.regex_mode = !this.regex_mode;
                            if this.regex_mode { this.fuzzy_mode = false; }
                        }
                        "fuzzy" => {
                            this.fuzzy_mode = !this.fuzzy_mode;
                            if this.fuzzy_mode { this.regex_mode = false; }
                        }
                        _ => {}
                    }
                    this.trigger_search(cx);
                    cx.notify();
                }),
            )
            .child(label.to_string())
    }

    fn render_file_filter_button(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = theme(cx);
        let active_count = self.show_ignored as u8 + self.show_hidden as u8;

        let entity = cx.entity().downgrade();
        let entity2 = entity.clone();

        crate::list_overlay::file_filter_button(
            "cs-filter-btn", active_count, &t, cx,
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
    }
}

/// Events emitted by the content search dialog.
#[derive(Clone, Debug)]
pub enum ContentSearchDialogEvent {
    Close,
    FileSelected { path: PathBuf, line: usize },
}

impl EventEmitter<ContentSearchDialogEvent> for ContentSearchDialog {}

impl okena_ui::overlay::CloseEvent for ContentSearchDialogEvent {
    fn is_close(&self) -> bool {
        matches!(self, Self::Close)
    }
}

impl Render for ContentSearchDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let project_name = self.project_fs.project_name();

        // Focus search input on first render
        let search_input_focus = self.search_input.read(cx).focus_handle(cx);
        if !search_input_focus.is_focused(window) && !self.glob_editing {
            self.search_input.update(cx, |input, cx| input.focus(window, cx));
        }

        // Shared key handler for both modes
        let key_handler = cx.listener(|this, event: &KeyDownEvent, _window, cx| {
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
                "tab" if !event.keystroke.modifiers.shift => {
                    this.expanded = !this.expanded;
                    if !this.search_input.read(cx).value().is_empty() {
                        this.trigger_search(cx);
                    }
                    cx.notify();
                }
                "escape" => this.close(cx),
                "c" if event.keystroke.modifiers.platform => {
                    if let Some(file_path) = &this.preview_file {
                        if let Some(lines) = this.highlight_cache.get(file_path) {
                            let text = extract_selected_text(
                                &this.preview_selection,
                                lines.len(),
                                |i| &lines[i].plain_text,
                            );
                            copy_to_clipboard(cx, text);
                        }
                    }
                }
                _ => {}
            }
        });

        let search_row = crate::list_overlay::search_input_row(&self.search_input, &t, cx);

        // Toggles row
        let toggles = self.render_toggles(cx);

        // Glob filter row
        let glob_row = if self.glob_editing {
            Some(
                div()
                    .px(px(12.0))
                    .py(px(4.0))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_muted))
                            .child("Filter:"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(SimpleInput::new(&self.glob_input).text_size(ui_text_sm(cx))),
                    ),
            )
        } else {
            None
        };

        // Results list
        let results_area: AnyElement = if self.rows.is_empty() {
            div()
                .flex_1()
                .child(empty_state(
                    if self.searching {
                        "Searching..."
                    } else if self.search_input.read(cx).value().is_empty() {
                        "Type to search file contents"
                    } else {
                        "No matching results"
                    },
                    &t,
                    cx,
                ))
                .into_any_element()
        } else {
            let rows = self.rows.clone();
            let has_context = self.expanded;
            let view = cx.entity().clone();

            uniform_list("content-search-list", rows.len(), move |range, _window, cx| {
                view.update(cx, |this, cx| {
                    range
                        .map(|i| {
                            let row = &rows[i];
                            match row {
                                ResultRow::FileHeader {
                                    relative_path,
                                    match_count,
                                    ..
                                } => this
                                    .render_file_header(i, relative_path, *match_count, cx)
                                    .into_any_element(),
                                ResultRow::Match {
                                    file_path,
                                    line_number,
                                    line_content,
                                    match_ranges,
                                    ..
                                } => this.render_match_row(
                                    i, file_path, *line_number, line_content, match_ranges, cx,
                                ),
                            }
                        })
                        .collect()
                })
            })
            .flex_1()
            .track_scroll(&self.scroll_handle)
            .into_any_element()
        };

        // Footer
        let footer = div()
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
                    .child(keyboard_hint(
                        "Tab",
                        if self.expanded { "compact" } else { "expand" },
                        &t,
                    ))
                    .child(keyboard_hint("Esc", "to close", &t)),
            )
            .child(
                div()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(if self.total_matches > 0 {
                        format!("{} results", self.total_matches)
                    } else {
                        String::new()
                    }),
            );

        // Shared content children
        let header = modal_header(
            &self.config.title,
            Some(format!("Searching in {}", project_name)),
            &t,
            cx,
            cx.listener(|this, _, _window, cx| this.close(cx)),
        );

        if self.expanded {
            // Fullscreen mode: file tree | results | file preview
            let sidebar = self.render_sidebar(cx);
            let preview = self.render_preview_panel(cx);

            fullscreen_overlay("content-search-fullscreen", &t)
                .track_focus(&focus_handle)
                .key_context(self.config.key_context.as_str())
                .on_action(cx.listener(|this, _: &Cancel, _window, cx| this.close(cx)))
                .on_key_down(key_handler)
                .child(header)
                .child(
                    // 3-column layout: sidebar | search+results | preview
                    div()
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .child(sidebar)
                        .child(
                            div()
                                .w(px(450.0))
                                .flex()
                                .flex_col()
                                .h_full()
                                .min_w_0()
                                .child(search_row)
                                .child(toggles)
                                .children(glob_row)
                                .child(results_area)
                                .child(footer),
                        )
                        .child(preview),
                )
                .when(self.filter_popover_open, |d| {
                    d.child(
                        div()
                            .id("cs-filter-popover-backdrop")
                            .absolute()
                            .inset_0()
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                this.filter_popover_open = false;
                                cx.notify();
                            }))
                    )
                })
                .when(self.filter_popover_open && self.filter_button_bounds.is_some(), |d| {
                    let bounds = self.filter_button_bounds.unwrap();
                    let entity = cx.entity().downgrade();
                    d.child(crate::list_overlay::file_filter_popover(
                        bounds, self.show_ignored, self.show_hidden, &t, cx,
                        move |filter, _, cx| {
                            if let Some(e) = entity.upgrade() {
                                e.update(cx, |this, cx| {
                                    match filter {
                                        "ignored" => this.show_ignored = !this.show_ignored,
                                        "hidden" => this.show_hidden = !this.show_hidden,
                                        _ => {}
                                    }
                                    this.trigger_search(cx);
                                    cx.notify();
                                });
                            }
                        },
                    ))
                })
                .into_any_element()
        } else {
            // Compact modal mode
            modal_backdrop("content-search-backdrop", &t)
                .track_focus(&focus_handle)
                .key_context(self.config.key_context.as_str())
                .items_start()
                .pt(px(80.0))
                .on_action(cx.listener(|this, _: &Cancel, _window, cx| this.close(cx)))
                .on_key_down(key_handler)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.close(cx)),
                )
                .child(
                    modal_content("content-search-modal", &t)
                        .relative()
                        .w(px(self.config.width))
                        .h(px(self.config.max_height))
                        .child(header)
                        .child(search_row)
                        .child(toggles)
                        .children(glob_row)
                        .child(results_area)
                        .child(footer)
                        .when(self.filter_popover_open, |modal| {
                            modal.child(
                                div()
                                    .id("cs-filter-popover-backdrop-compact")
                                    .absolute()
                                    .inset_0()
                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                        this.filter_popover_open = false;
                                        cx.notify();
                                    }))
                            )
                        })
                        .when(self.filter_popover_open && self.filter_button_bounds.is_some(), |modal| {
                            let bounds = self.filter_button_bounds.unwrap();
                            let entity = cx.entity().downgrade();
                            modal.child(crate::list_overlay::file_filter_popover(
                                bounds, self.show_ignored, self.show_hidden, &t, cx,
                                move |filter, _, cx| {
                                    if let Some(e) = entity.upgrade() {
                                        e.update(cx, |this, cx| {
                                            match filter {
                                                "ignored" => this.show_ignored = !this.show_ignored,
                                                "hidden" => this.show_hidden = !this.show_hidden,
                                                _ => {}
                                            }
                                            this.trigger_search(cx);
                                            cx.notify();
                                        });
                                    }
                                },
                            ))
                        }),
                )
                .into_any_element()
        }
    }
}

impl Focusable for ContentSearchDialog {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Convert a u32 theme color to Hsla with alpha for search match background.
fn search_match_bg(color: u32) -> Hsla {
    Hsla::from(Rgba {
        r: ((color >> 16) & 0xFF) as f32 / 255.0,
        g: ((color >> 8) & 0xFF) as f32 / 255.0,
        b: (color & 0xFF) as f32 / 255.0,
        a: 0.5,
    })
}
