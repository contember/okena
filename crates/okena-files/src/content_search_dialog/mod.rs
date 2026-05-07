//! Content search dialog ("Find in Files") overlay.
//!
//! Provides a searchable overlay for finding text content across project files,
//! with syntax-highlighted results grouped by file.

mod preview;
mod render;
mod rows;
mod search;
mod sidebar;
mod toggles;

use crate::code_view::CodeSelection;
use crate::content_search::SearchHandle;
use crate::list_overlay::ListOverlayConfig;
use crate::syntax::{HighlightedLine, load_syntax_set};
use gpui::*;
use okena_ui::simple_input::{InputChangedEvent, SimpleInputState};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
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
}

impl Global for ContentSearchMemory {}

/// A flattened result row for display in the list.
#[derive(Clone)]
pub(super) enum ResultRow {
    /// File header row (file path, match count).
    FileHeader {
        file_path: PathBuf,
        relative_path: String,
        match_count: usize,
    },
    /// Match row within a file, with optional context lines.
    Match {
        file_path: PathBuf,
        relative_path: String,
        line_number: usize,
        line_content: String,
        match_ranges: Vec<std::ops::Range<usize>>,
        /// Context lines before the match (line_number, content).
        _context_before: Vec<(usize, String)>,
        /// Context lines after the match (line_number, content).
        _context_after: Vec<(usize, String)>,
    },
}

/// Content search dialog for finding text in project files.
pub struct ContentSearchDialog {
    pub(super) focus_handle: FocusHandle,
    pub(super) scroll_handle: UniformListScrollHandle,
    pub(super) search_input: Entity<SimpleInputState>,
    pub(super) project_fs: std::sync::Arc<dyn crate::project_fs::ProjectFs>,
    pub(super) config: ListOverlayConfig,
    /// Flattened result rows for display.
    pub(super) rows: Vec<ResultRow>,
    pub(super) selected_index: usize,
    /// Total number of matches across all files.
    pub(super) total_matches: usize,
    /// Whether a search is currently running.
    pub(super) searching: bool,
    /// Handle to cancel running search.
    pub(super) search_handle: Option<SearchHandle>,
    /// Search config toggles.
    pub(super) case_sensitive: bool,
    pub(super) regex_mode: bool,
    pub(super) fuzzy_mode: bool,
    pub(super) show_ignored: bool,
    pub(super) filter_popover_open: bool,
    pub(super) filter_button_bounds: Option<Bounds<Pixels>>,
    pub(super) file_glob: Option<String>,
    /// Glob filter input entity.
    pub(super) glob_input: Entity<SimpleInputState>,
    /// Whether the glob input row is visible.
    pub(super) glob_editing: bool,
    /// Whether the overlay is in expanded (full) mode.
    pub(super) expanded: bool,
    /// Cached syntax-highlighted lines per file path.
    pub(super) highlight_cache: HashMap<PathBuf, Vec<HighlightedLine>>,
    /// Files currently being loaded for highlighting (to avoid duplicate loads).
    pub(super) loading_files: HashSet<PathBuf>,
    /// Shared syntax set.
    pub(super) syntax_set: SyntaxSet,
    /// Whether the theme is dark.
    pub(super) is_dark: bool,
    /// Debounce task for search.
    pub(super) debounce_task: Option<Task<()>>,
    /// Scroll handle for the preview panel.
    pub(super) preview_scroll_handle: UniformListScrollHandle,
    /// Expanded folder paths in the sidebar.
    pub(super) expanded_folders: HashSet<String>,
    /// Scroll handle for the sidebar tree.
    pub(super) tree_scroll_handle: ScrollHandle,
    /// Currently active scope path (folder or file) shown in sidebar.
    pub(super) scope_path: Option<String>,
    /// Text selection state in the preview panel.
    pub(super) preview_selection: CodeSelection,
    /// File path currently shown in preview (for selection reset).
    pub(super) preview_file: Option<PathBuf>,
    /// (file_path, match_line) the preview was last auto-scrolled to.
    /// Prevents re-anchoring the scroll on every render — without this,
    /// the user can't scroll past the highlighted row.
    pub(super) last_scrolled_match: Option<(PathBuf, usize)>,
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
        let (query, case_sensitive, regex_mode, fuzzy_mode, file_glob, glob_input_text, expanded, show_ignored) =
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
            filter_popover_open: false,
            filter_button_bounds: None,
            file_glob,
            glob_input,
            glob_editing: false,
            expanded,
            highlight_cache: HashMap::new(),
            loading_files: HashSet::new(),
            syntax_set,
            is_dark,
            debounce_task: None,
            preview_scroll_handle: UniformListScrollHandle::new(),
            expanded_folders: HashSet::new(),
            tree_scroll_handle: ScrollHandle::new(),
            scope_path: None,
            preview_selection: CodeSelection::default(),
            preview_file: None,
            last_scrolled_match: None,
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
        });
    }

    pub(super) fn close(&self, cx: &mut Context<Self>) {
        if let Some(handle) = &self.search_handle {
            handle.cancel();
        }
        self.save_memory(cx);
        cx.emit(ContentSearchDialogEvent::Close);
    }

    /// Open file viewer at the selected match.
    pub(super) fn open_selected(&self, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get(self.selected_index) {
            let (relative_path, line) = match row {
                ResultRow::Match { relative_path, line_number, .. } => {
                    (relative_path.clone(), *line_number)
                }
                ResultRow::FileHeader { relative_path, .. } => (relative_path.clone(), 1),
            };
            self.save_memory(cx);
            cx.emit(ContentSearchDialogEvent::FileSelected { relative_path, line });
        }
    }

    pub(super) fn select_prev(&mut self) -> bool {
        crate::list_overlay::select_prev(&mut self.selected_index, &self.scroll_handle)
    }

    pub(super) fn select_next(&mut self) -> bool {
        crate::list_overlay::select_next(&mut self.selected_index, self.rows.len(), &self.scroll_handle)
    }

    /// Set scope to a folder or file path, updating the glob filter and re-searching.
    pub(super) fn set_scope(&mut self, path: Option<String>, cx: &mut Context<Self>) {
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
    pub(super) fn toggle_folder(&mut self, folder_path: &str, cx: &mut Context<Self>) {
        if !self.expanded_folders.remove(folder_path) {
            self.expanded_folders.insert(folder_path.to_string());
        }
        cx.notify();
    }
}

/// Events emitted by the content search dialog.
#[derive(Clone, Debug)]
pub enum ContentSearchDialogEvent {
    Close,
    /// A match was opened. `relative_path` is project-relative; callers don't
    /// need to handle absolute path semantics (which differ between local and
    /// remote projects).
    FileSelected { relative_path: String, line: usize },
}

impl EventEmitter<ContentSearchDialogEvent> for ContentSearchDialog {}

impl okena_ui::overlay::CloseEvent for ContentSearchDialogEvent {
    fn is_close(&self) -> bool {
        matches!(self, Self::Close)
    }
}

impl Focusable for ContentSearchDialog {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Convert a u32 theme color to Hsla with alpha for search match background.
pub(super) fn search_match_bg(color: u32) -> Hsla {
    Hsla::from(Rgba {
        r: ((color >> 16) & 0xFF) as f32 / 255.0,
        g: ((color >> 8) & 0xFF) as f32 / 255.0,
        b: (color & 0xFF) as f32 / 255.0,
        a: 0.5,
    })
}
