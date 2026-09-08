//! File viewer overlay for displaying file contents with syntax highlighting.
//!
//! Provides a read-only view of files with syntax highlighting via syntect.
//! Markdown files can be viewed in rendered preview mode.

mod blame_load;
mod blame_render;
mod context_menu;
mod history;
mod loading;
mod render;
mod search;
mod selection;
mod tree;

use crate::blame::{BlameError, BlameLine, BlameProvider};
use crate::code_view::ScrollbarDrag;
use crate::file_tree::FileTreeRow;
use crate::history::{FileHistoryEntry, FileHistoryProvider};
use crate::list_directory::DirEntry;
use crate::selection::SelectionState;
use crate::syntax::{HighlightedLine, load_syntax_set};
use context_menu::{DeleteConfirmState, FileRenameState, FileTreeContextMenu, TabContextMenu};
use gpui::*;
use okena_markdown::{MarkdownDocument, MarkdownSelection};
use okena_ui::resizable_sidebar::ResizableSidebarState;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use syntect::parsing::SyntaxSet;

/// Maximum file size to load for text/markdown (5MB)
const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024;

/// Maximum number of lines to display
const MAX_LINES: usize = 10000;

/// Maximum number of open tabs
const MAX_TABS: usize = 50;

/// Maximum navigation history stack size
const MAX_HISTORY: usize = 50;

fn is_missing_directory_error(error: &str) -> bool {
    error.contains(crate::list_directory::DIRECTORY_NOT_FOUND_ERROR)
        // Keep recovery working while a client is connected to an older daemon.
        || error.contains("(os error 2)")
        || error.contains("(os error 3)")
}

fn is_path_or_descendant(path: &str, ancestor: &str) -> bool {
    path == ancestor
        || path
            .strip_prefix(ancestor)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn prune_missing_directory(
    relative_path: &str,
    loaded_dirs: &mut HashMap<String, Vec<DirEntry>>,
    loading_dirs: &mut HashSet<String>,
    expanded_folders: &mut HashSet<String>,
) -> String {
    loaded_dirs.retain(|path, _| !is_path_or_descendant(path, relative_path));
    loading_dirs.retain(|path| !is_path_or_descendant(path, relative_path));
    expanded_folders.retain(|path| !is_path_or_descendant(path, relative_path));

    relative_path
        .rsplit_once('/')
        .map_or_else(String::new, |(parent, _)| parent.to_string())
}

/// Display mode for file viewer.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum DisplayMode {
    #[default]
    Source,
    Preview,
}

/// Type alias for source view selection (line, column).
type Selection = SelectionState<(usize, usize)>;

/// Per-file state for a single tab in the file viewer.
///
/// `relative_path` is the canonical identifier (project-relative, used for
/// `fs.read_file`, tab equality, history, and tree highlighting). `file_path`
/// is a client-only identity rooted at the project id; daemon paths come from
/// `ProjectFs::absolute_path`.
pub(super) struct FileViewerTab {
    pub file_path: PathBuf,
    pub relative_path: String,
    pub content: String,
    pub highlighted_lines: Vec<HighlightedLine>,
    pub source_rows: Vec<SourceRow>,
    pub line_count: usize,
    pub line_num_width: usize,
    pub longest_source_row: usize,
    pub wrap_lines: bool,
    pub wrap_columns: usize,
    pub json_pretty: bool,
    pub json_alternate: Option<JsonAlternateView>,
    pub error_message: Option<String>,
    pub selection: Selection,
    pub display_mode: DisplayMode,
    pub is_markdown: bool,
    pub markdown_doc: Option<MarkdownDocument>,
    pub markdown_selection: MarkdownSelection,
    /// Virtualized list state for the markdown preview. One list item per
    /// top-level block, so only visible blocks are built per frame. Lazily
    /// (re)created in render when the node count or font size changes.
    pub markdown_list_state: Option<ListState>,
    /// Node count `markdown_list_state` was built for (to detect doc reloads).
    pub markdown_list_nodes: usize,
    /// Font size the list heights were measured at (to trigger a remeasure).
    pub markdown_list_font: f32,
    /// Independent horizontal scroll state for each rendered markdown table.
    pub markdown_table_scroll_handles: HashMap<usize, ScrollHandle>,
    pub source_scroll_handle: UniformListScrollHandle,
    pub scrollbar_drag: Option<ScrollbarDrag>,
    /// Last daemon-reported modification time in Unix milliseconds.
    pub modified_at: Option<u64>,
    /// Whether the tab content is still being loaded asynchronously.
    pub loading: bool,
    /// Per-line git blame for this file. Lazy-loaded when the user toggles
    /// the blame gutter on.
    pub blame: BlameLoadState,
    /// Commit history for this file. Loaded only when the history rail opens.
    pub history: FileHistoryLoadState,
    /// Historical revision currently shown in place of the working tree.
    pub revision: Option<FileHistoryEntry>,
    /// Content source for `revision`; kept separately because direct opens do
    /// not necessarily have a matching history entry.
    pub revision_source: Option<FileSource>,
    /// Monotonic token for history-list requests targeting this tab.
    pub history_generation: u64,
    /// Monotonic counter bumped each time `spawn_tab_load` schedules a fresh
    /// async load for this tab. The bg task captures the generation it was
    /// scheduled at and `apply_loaded_content` is skipped if a newer load
    /// has been queued in the meantime, so a slow earlier load can't
    /// clobber a faster later one with stale content.
    pub load_generation: u64,
    /// True for files previewed as images (png/jpg/gif/webp/svg/...).
    pub is_image: bool,
    /// True for SVG files specifically. SVG is the one image format that
    /// also has a meaningful source view — the loader keeps the raw XML in
    /// `content`/`highlighted_lines` so the user can flip between Preview
    /// (rendered) and Source (highlighted XML) via the same toggle markdown
    /// uses.
    pub is_svg: bool,
    /// Shared complete-file renderer used by image/font tabs and binary diffs.
    pub file_renderer: Option<Entity<crate::file_renderer::FileRenderer>>,
    /// True for font files (otf/ttf/woff/woff2).
    pub is_font: bool,
    /// One-based source position requested by the link that opened this tab.
    pub target_line: Option<usize>,
    pub target_column: Option<usize>,
}

pub(super) struct SourceRow {
    pub logical_line: usize,
    pub byte_range: Range<usize>,
    pub columns: usize,
}

pub(super) struct JsonAlternateView {
    pub content: String,
    pub highlighted_lines: Option<Vec<HighlightedLine>>,
}

/// Lifecycle of a tab's blame data.
#[derive(Clone, Debug, Default)]
pub enum BlameLoadState {
    #[default]
    NotLoaded,
    Loading,
    Loaded(std::sync::Arc<Vec<BlameLine>>),
    Error(BlameError),
}

#[derive(Clone, Debug, Default)]
pub enum FileHistoryLoadState {
    #[default]
    NotLoaded,
    Loading,
    Loaded(std::sync::Arc<Vec<FileHistoryEntry>>),
    Error(String),
}

impl FileViewerTab {
    fn source_row_for_line(&self, line: usize) -> usize {
        let logical_line = line.saturating_sub(1);
        self.source_rows
            .partition_point(|row| row.logical_line < logical_line)
            .min(self.source_rows.len().saturating_sub(1))
    }

    /// Create a new tab for browsing (no file loaded).
    pub(super) fn new_empty() -> Self {
        Self {
            file_path: PathBuf::new(),
            relative_path: String::new(),
            content: String::new(),
            highlighted_lines: Vec::new(),
            source_rows: Vec::new(),
            line_count: 0,
            line_num_width: 3,
            longest_source_row: 0,
            wrap_lines: false,
            wrap_columns: 120,
            json_pretty: false,
            json_alternate: None,
            error_message: None,
            selection: Selection::default(),
            display_mode: DisplayMode::Source,
            is_markdown: false,
            markdown_doc: None,
            markdown_selection: MarkdownSelection::default(),
            markdown_list_state: None,
            markdown_list_nodes: 0,
            markdown_list_font: 0.0,
            markdown_table_scroll_handles: HashMap::new(),
            source_scroll_handle: UniformListScrollHandle::new(),
            scrollbar_drag: None,
            modified_at: None,
            loading: false,
            blame: BlameLoadState::NotLoaded,
            history: FileHistoryLoadState::NotLoaded,
            revision: None,
            revision_source: None,
            history_generation: 0,
            load_generation: 0,
            is_image: false,
            is_svg: false,
            file_renderer: None,
            is_font: false,
            target_line: None,
            target_column: None,
        }
    }

    /// Create a tab in loading state (content will be filled asynchronously).
    fn new_loading(relative_path: String, file_path: PathBuf) -> Self {
        let renderer_kind = crate::file_renderer::FileRenderer::kind_for_path(&file_path);
        let is_image = matches!(
            renderer_kind,
            Some(crate::file_renderer::FileRendererKind::Image { .. })
        );
        let is_svg = matches!(
            renderer_kind,
            Some(crate::file_renderer::FileRendererKind::Image { is_svg: true })
        );
        let is_font = renderer_kind == Some(crate::file_renderer::FileRendererKind::Font);
        let is_markdown = !is_image && !is_font && Self::is_markdown_file(&file_path);
        Self {
            file_path,
            relative_path,
            content: String::new(),
            highlighted_lines: Vec::new(),
            source_rows: Vec::new(),
            line_count: 0,
            line_num_width: 3,
            longest_source_row: 0,
            wrap_lines: false,
            wrap_columns: 120,
            json_pretty: false,
            json_alternate: None,
            error_message: None,
            selection: Selection::default(),
            display_mode: if is_markdown || is_svg {
                DisplayMode::Preview
            } else {
                DisplayMode::Source
            },
            is_markdown,
            markdown_doc: None,
            markdown_selection: MarkdownSelection::default(),
            markdown_list_state: None,
            markdown_list_nodes: 0,
            markdown_list_font: 0.0,
            markdown_table_scroll_handles: HashMap::new(),
            source_scroll_handle: UniformListScrollHandle::new(),
            scrollbar_drag: None,
            modified_at: None,
            loading: true,
            blame: BlameLoadState::NotLoaded,
            history: FileHistoryLoadState::NotLoaded,
            revision: None,
            revision_source: None,
            history_generation: 0,
            load_generation: 0,
            is_image,
            is_svg,
            file_renderer: None,
            is_font,
            target_line: None,
            target_column: None,
        }
    }

    /// Get the filename for display in the tab bar.
    pub fn filename(&self) -> String {
        if let Some(name) = self.file_path.file_name() {
            name.to_string_lossy().to_string()
        } else if let Some(idx) = self.relative_path.rfind(['/', '\\']) {
            self.relative_path[idx + 1..].to_string()
        } else if !self.relative_path.is_empty() {
            self.relative_path.clone()
        } else {
            "Untitled".to_string()
        }
    }

    /// Check if this tab has no file loaded.
    pub fn is_empty(&self) -> bool {
        self.relative_path.is_empty()
    }
}

/// A single entry in the navigation history.
struct HistoryEntry {
    relative_path: String,
}

/// Back/forward navigation history.
pub(super) struct NavigationHistory {
    back_stack: Vec<HistoryEntry>,
    forward_stack: Vec<HistoryEntry>,
}

impl NavigationHistory {
    fn new() -> Self {
        Self {
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
        }
    }

    /// Record a navigation from `current` to a new file.
    fn push(&mut self, current: &str) {
        if current.is_empty() {
            return;
        }
        self.back_stack.push(HistoryEntry {
            relative_path: current.to_string(),
        });
        self.forward_stack.clear();
        if self.back_stack.len() > MAX_HISTORY {
            self.back_stack.remove(0);
        }
    }

    /// Go back. Returns the relative path to navigate to.
    fn go_back(&mut self, current: &str) -> Option<String> {
        let entry = self.back_stack.pop()?;
        if !current.is_empty() {
            self.forward_stack.push(HistoryEntry {
                relative_path: current.to_string(),
            });
        }
        Some(entry.relative_path)
    }

    /// Go forward. Returns the relative path to navigate to.
    fn go_forward(&mut self, current: &str) -> Option<String> {
        let entry = self.forward_stack.pop()?;
        if !current.is_empty() {
            self.back_stack.push(HistoryEntry {
                relative_path: current.to_string(),
            });
        }
        Some(entry.relative_path)
    }

    fn can_go_back(&self) -> bool {
        !self.back_stack.is_empty()
    }

    fn can_go_forward(&self) -> bool {
        !self.forward_stack.is_empty()
    }
}

/// File viewer overlay for displaying file contents.
pub struct FileViewer {
    focus_handle: FocusHandle,
    project_fs: std::sync::Arc<dyn crate::project_fs::ProjectFs>,
    /// Syntax set for highlighting (shared via `Arc`, large to clone)
    syntax_set: std::sync::Arc<SyntaxSet>,
    /// File font size from settings
    file_font_size: f32,
    /// Monospace font used for source measurement and rendering.
    file_font: Font,
    /// Measured monospace character width (from font metrics)
    measured_char_width: f32,
    /// Whether the current theme is dark (for syntax highlighting)
    is_dark: bool,
    /// True until the project root directory listing arrives.
    loading: bool,
    /// Cache of directory listings keyed by project-relative folder path
    /// (`""` = project root). Populated lazily as folders are expanded.
    pub(super) loaded_dirs: HashMap<String, Vec<DirEntry>>,
    /// Folder paths whose listing is currently in flight.
    pub(super) loading_dirs: HashSet<String>,
    pub(super) tree_error_message: Option<String>,
    /// Which folder paths are currently expanded
    expanded_folders: HashSet<String>,
    /// Cached flattened rows for the currently visible file tree.
    pub(super) visible_tree_rows: RefCell<Option<Arc<Vec<FileTreeRow<String>>>>>,
    /// Scroll handle for the virtualized file tree sidebar.
    tree_scroll_handle: UniformListScrollHandle,
    /// Active drag gesture for the file tree scrollbar.
    tree_scrollbar_drag: Option<ScrollbarDrag>,
    /// Whether the sidebar is visible
    sidebar_visible: bool,
    /// Width and active resize gesture for the file tree sidebar.
    pub(super) sidebar_resize: ResizableSidebarState,
    /// Open tabs
    pub(super) tabs: Vec<FileViewerTab>,
    /// Index of the active tab
    pub(super) active_tab: usize,
    /// Navigation history
    pub(super) history: NavigationHistory,
    /// Last time we checked files for external modifications
    last_change_check: std::time::Instant,
    /// True while a background freshness check (stat + possible reload) is in
    /// flight, so we don't spawn overlapping checks if a stat outlives the
    /// once-per-second throttle window (e.g. on a slow network mount).
    freshness_check_in_flight: bool,
    /// Whether to include gitignored files in the file tree
    pub(super) show_ignored: bool,
    /// Whether the filter popover is open
    pub(super) filter_popover_open: bool,
    /// Bounds of the filter button for popover positioning
    pub(super) filter_button_bounds: Option<Bounds<Pixels>>,
    /// Context menu state for file tree right-click
    pub(super) context_menu: Option<FileTreeContextMenu>,
    /// Context menu state for tab right-click
    pub(super) tab_context_menu: Option<TabContextMenu>,
    /// Inline rename state
    pub(super) rename_state: Option<FileRenameState>,
    /// Delete confirmation dialog state
    pub(super) delete_confirm: Option<DeleteConfirmState>,
    /// In-file search state (Ctrl+F)
    pub(super) search_state: Option<crate::in_page_search::InPageSearch>,
    /// True when this viewer is hosted inside a detached window.
    /// Hides the "detach" button and is set by the detached host.
    pub(super) is_detached: bool,
    /// Whether this viewer is a drill-down that can return to another screen.
    pub(super) can_go_back: bool,
    /// Optional provider for per-file git blame. `None` for projects that
    /// can't supply blame (no host wiring, non-git filesystems, etc).
    pub(super) blame_provider: Option<std::sync::Arc<dyn BlameProvider>>,
    /// Whether the blame gutter column is visible. Persisted in settings.
    pub(super) blame_visible: bool,
    /// Optional provider for per-file commit history and historical contents.
    pub(super) history_provider: Option<std::sync::Arc<dyn FileHistoryProvider>>,
    /// Whether the active file's revision rail is visible.
    pub(super) history_visible: bool,
    /// Right-click context menu over a non-empty text selection.
    pub(super) selection_context_menu: Option<Point<Pixels>>,
    /// Monotonic counter used to stamp each `spawn_tab_load` invocation.
    /// Each background task captures the value it was scheduled at and only
    /// applies its result if the tab's recorded generation still matches,
    /// so a slow earlier load can't overwrite a faster later one.
    next_load_generation: u64,
    next_history_generation: u64,
    /// Canonical daemon scope and its breadcrumb ancestry.
    pub(super) scope: Option<okena_core::api::ResolvedPath>,
    pub(super) scope_navigation_in_flight: bool,
    scope_generation: u64,
    pub(super) transfer_in_progress: bool,
    pub(super) transfer_status: Option<String>,
}

/// The project a viewer serves: its filesystem plus the optional git providers
/// behind the blame gutter and the revision rail.
#[derive(Clone)]
pub struct FileViewerScope {
    pub project_fs: std::sync::Arc<dyn crate::project_fs::ProjectFs>,
    pub blame_provider: Option<std::sync::Arc<dyn BlameProvider>>,
    pub history_provider: Option<std::sync::Arc<dyn FileHistoryProvider>>,
}

impl FileViewerScope {
    /// A scope with no git providers, for plain filesystem browsing.
    pub fn plain(project_fs: std::sync::Arc<dyn crate::project_fs::ProjectFs>) -> Self {
        Self {
            project_fs,
            blame_provider: None,
            history_provider: None,
        }
    }
}

/// Presentation state, read from the user's settings and the active theme.
#[derive(Clone)]
pub struct FileViewerConfig {
    pub font_size: f32,
    pub font_family: SharedString,
    pub is_dark: bool,
    pub blame_visible: bool,
}

/// One-based caret target inside the opened file. Default means "no target".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FilePosition {
    pub line: Option<usize>,
    pub column: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileSource {
    WorkingTree,
    GitRevision(String),
    Index,
    BranchMergeBase { base: String, head: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileTarget {
    pub relative_path: String,
    pub source: FileSource,
    pub position: FilePosition,
}

impl FileTarget {
    pub fn working_tree(relative_path: String, position: FilePosition) -> Self {
        Self {
            relative_path,
            source: FileSource::WorkingTree,
            position,
        }
    }
}

impl FileViewer {
    /// Build a stable client-side identity path without touching local disk.
    fn tree_path(
        fs: &std::sync::Arc<dyn crate::project_fs::ProjectFs>,
        relative_path: &str,
    ) -> PathBuf {
        PathBuf::from(fs.project_id()).join(relative_path)
    }

    pub fn is_scope(&self, fs: &std::sync::Arc<dyn crate::project_fs::ProjectFs>) -> bool {
        self.project_fs.scope_path() == fs.scope_path()
    }

    pub fn rebind_scope(
        &mut self,
        scope: FileViewerScope,
        blame_visible: bool,
        relative_path: Option<String>,
        position: FilePosition,
        cx: &mut Context<Self>,
    ) {
        let FileViewerScope {
            project_fs,
            blame_provider,
            history_provider,
        } = scope;
        for tab in &mut self.tabs {
            release_tab_renderer(tab, cx);
        }
        self.project_fs = project_fs;
        self.scope = None;
        self.scope_generation = self.scope_generation.wrapping_add(1);
        self.scope_navigation_in_flight = false;
        self.loaded_dirs.clear();
        self.loading_dirs.clear();
        self.tree_error_message = None;
        self.expanded_folders = relative_path
            .as_deref()
            .map(Self::compute_expanded_for_relative)
            .unwrap_or_default();
        self.invalidate_visible_tree_rows();
        self.tree_scroll_handle
            .scroll_to_item(0, ScrollStrategy::Top);
        self.tree_scrollbar_drag = None;
        self.tabs = vec![match &relative_path {
            Some(relative_path) => {
                let mut tab = FileViewerTab::new_loading(
                    relative_path.clone(),
                    Self::tree_path(&self.project_fs, relative_path),
                );
                tab.target_line = position.line;
                tab.target_column = position.column;
                if position.line.is_some() {
                    tab.display_mode = DisplayMode::Source;
                }
                tab
            }
            None => FileViewerTab::new_empty(),
        }];
        self.active_tab = 0;
        self.history = NavigationHistory::new();
        self.loading = true;
        self.freshness_check_in_flight = false;
        self.sidebar_visible = true;
        self.blame_provider = blame_provider;
        self.history_provider = history_provider;
        self.blame_visible = blame_visible;
        self.history_visible = false;
        self.fetch_scope_info(cx);
        self.fetch_initial_dirs(cx);
        if let Some(relative_path) = relative_path {
            self.spawn_tab_load(relative_path, cx);
            if self.blame_visible {
                self.spawn_blame_load_for_active(cx);
            }
        }
        cx.notify();
    }

    /// Create a new file viewer with `relative_path` (project-relative) opened
    /// in the first tab.
    pub fn new(
        scope: FileViewerScope,
        config: FileViewerConfig,
        relative_path: String,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_at(scope, config, relative_path, FilePosition::default(), cx)
    }

    /// Create a project file viewer and focus an optional one-based position.
    pub fn new_at(
        scope: FileViewerScope,
        config: FileViewerConfig,
        relative_path: String,
        position: FilePosition,
        cx: &mut Context<Self>,
    ) -> Self {
        let FileViewerScope {
            project_fs,
            blame_provider,
            history_provider,
        } = scope;
        let FileViewerConfig {
            font_size,
            font_family,
            is_dark,
            blame_visible,
        } = config;
        let focus_handle = cx.focus_handle();
        let expanded_folders = Self::compute_expanded_for_relative(&relative_path);
        let syntax_set = load_syntax_set();

        let file_path = Self::tree_path(&project_fs, &relative_path);
        let mut tab = FileViewerTab::new_loading(relative_path.clone(), file_path.clone());
        tab.target_line = position.line;
        tab.target_column = position.column;
        if position.line.is_some() {
            tab.display_mode = DisplayMode::Source;
        }

        let mut viewer = Self {
            focus_handle,
            project_fs,
            syntax_set,
            file_font_size: font_size,
            file_font: okena_ui::tokens::file_font_for_family(font_family, cx),
            measured_char_width: font_size * 0.6,
            is_dark,
            loading: true,
            loaded_dirs: HashMap::new(),
            loading_dirs: HashSet::new(),
            tree_error_message: None,
            expanded_folders,
            visible_tree_rows: RefCell::new(None),
            tree_scroll_handle: UniformListScrollHandle::new(),
            tree_scrollbar_drag: None,
            sidebar_visible: true,
            sidebar_resize: ResizableSidebarState::default(),
            tabs: vec![tab],
            active_tab: 0,
            history: NavigationHistory::new(),
            last_change_check: std::time::Instant::now(),
            freshness_check_in_flight: false,
            show_ignored: false,
            filter_popover_open: false,
            filter_button_bounds: None,
            context_menu: None,
            tab_context_menu: None,
            rename_state: None,
            delete_confirm: None,
            search_state: None,
            is_detached: false,
            can_go_back: false,
            blame_provider,
            blame_visible,
            history_provider,
            history_visible: false,
            selection_context_menu: None,
            next_load_generation: 0,
            next_history_generation: 0,
            scope: None,
            scope_navigation_in_flight: false,
            scope_generation: 0,
            transfer_in_progress: false,
            transfer_status: None,
        };

        // Kick off the root directory listing and any expanded ancestors so
        // the tree fills in around the opened file.
        viewer.fetch_scope_info(cx);
        viewer.fetch_initial_dirs(cx);
        viewer.spawn_tab_load(relative_path, cx);
        if viewer.blame_visible {
            viewer.spawn_blame_load_for_active(cx);
        }
        viewer
    }

    /// Create a file viewer for browsing a project without a pre-selected file.
    ///
    /// Opens the sidebar file tree with no file loaded.
    pub fn new_browse(
        scope: FileViewerScope,
        config: FileViewerConfig,
        cx: &mut Context<Self>,
    ) -> Self {
        let FileViewerScope {
            project_fs,
            blame_provider,
            history_provider,
        } = scope;
        let FileViewerConfig {
            font_size,
            font_family,
            is_dark,
            blame_visible,
        } = config;
        let focus_handle = cx.focus_handle();

        let mut viewer = Self {
            focus_handle,
            project_fs,
            syntax_set: load_syntax_set(),
            file_font_size: font_size,
            file_font: okena_ui::tokens::file_font_for_family(font_family, cx),
            measured_char_width: font_size * 0.6,
            is_dark,
            loading: true,
            loaded_dirs: HashMap::new(),
            loading_dirs: HashSet::new(),
            tree_error_message: None,
            expanded_folders: HashSet::new(),
            visible_tree_rows: RefCell::new(None),
            tree_scroll_handle: UniformListScrollHandle::new(),
            tree_scrollbar_drag: None,
            sidebar_visible: true,
            sidebar_resize: ResizableSidebarState::default(),
            tabs: vec![FileViewerTab::new_empty()],
            active_tab: 0,
            history: NavigationHistory::new(),
            last_change_check: std::time::Instant::now(),
            freshness_check_in_flight: false,
            show_ignored: false,
            filter_popover_open: false,
            filter_button_bounds: None,
            context_menu: None,
            tab_context_menu: None,
            rename_state: None,
            delete_confirm: None,
            search_state: None,
            is_detached: false,
            can_go_back: false,
            blame_provider,
            blame_visible,
            history_provider,
            history_visible: false,
            selection_context_menu: None,
            next_load_generation: 0,
            next_history_generation: 0,
            scope: None,
            scope_navigation_in_flight: false,
            scope_generation: 0,
            transfer_in_progress: false,
            transfer_status: None,
        };
        viewer.fetch_scope_info(cx);
        viewer.fetch_initial_dirs(cx);
        viewer
    }

    /// Mark this viewer as hosted in a detached window so the detach button
    /// is hidden and the viewer renders for that context.
    pub fn set_detached(&mut self, detached: bool, cx: &mut Context<Self>) {
        if self.is_detached != detached {
            self.is_detached = detached;
            cx.notify();
        }
    }

    /// Whether this viewer is hosted in a detached window.
    pub fn is_detached(&self) -> bool {
        self.is_detached
    }

    pub fn set_can_go_back(&mut self, can_go_back: bool, cx: &mut Context<Self>) {
        if self.can_go_back != can_go_back {
            self.can_go_back = can_go_back;
            cx.notify();
        }
    }

    /// Request to detach the viewer into a separate OS window.
    pub(super) fn request_detach(&self, cx: &mut Context<Self>) {
        cx.emit(FileViewerEvent::Detach);
    }

    pub(super) fn request_source_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::project_fs::FileSourceAction;
        let tab = self.active_tab();
        match self.project_fs.source_action() {
            FileSourceAction::OpenExternally => {
                let Some(path) = self.project_fs.absolute_path(&tab.relative_path) else {
                    self.transfer_status = Some("The daemon did not provide a local path".into());
                    cx.notify();
                    return;
                };
                cx.emit(FileViewerEvent::OpenExternally {
                    path,
                    line: tab.target_line,
                    column: tab.target_column,
                });
            }
            FileSourceAction::Download => {
                if self.transfer_in_progress {
                    return;
                }
                let initial_dir = dirs::download_dir()
                    .or_else(|| std::env::current_dir().ok())
                    .unwrap_or_else(std::env::temp_dir);
                let filename = tab.filename();
                let relative_path = tab.relative_path.clone();
                let provider = self.project_fs.clone();
                let receiver = cx.prompt_for_new_path(&initial_dir, Some(&filename));
                self.transfer_status = None;
                cx.spawn_in(window, async move |entity: WeakEntity<Self>, cx| {
                    let destination = match receiver.await {
                        Ok(Ok(Some(destination))) => destination,
                        Ok(Ok(None)) => return,
                        Ok(Err(error)) => {
                            let _ = entity.update(cx, |this, cx| {
                                this.transfer_status =
                                    Some(format!("Cannot open save dialog: {error}"));
                                cx.notify();
                            });
                            return;
                        }
                        Err(error) => {
                            let _ = entity.update(cx, |this, cx| {
                                this.transfer_status =
                                    Some(format!("Save dialog closed unexpectedly: {error}"));
                                cx.notify();
                            });
                            return;
                        }
                    };
                    let _ = entity.update(cx, |this, cx| {
                        this.transfer_in_progress = true;
                        this.transfer_status = Some("Downloading…".to_string());
                        cx.notify();
                    });
                    let saved_path = destination.clone();
                    let result = cx
                        .background_executor()
                        .spawn(async move {
                            use std::io::Write as _;
                            let parent = destination.parent().ok_or_else(|| {
                                "Selected path has no parent directory".to_string()
                            })?;
                            let mut temporary =
                                tempfile::NamedTempFile::new_in(parent).map_err(|error| {
                                    format!("Cannot create temporary file: {error}")
                                })?;
                            provider.download_file(&relative_path, temporary.as_file_mut())?;
                            temporary.as_file_mut().flush().map_err(|error| {
                                format!("Cannot flush downloaded file: {error}")
                            })?;
                            temporary.persist(&destination).map_err(|error| {
                                format!("Cannot save downloaded file: {}", error.error)
                            })?;
                            Ok::<(), String>(())
                        })
                        .await;
                    let _ = entity.update(cx, |this, cx| {
                        this.transfer_in_progress = false;
                        this.transfer_status = Some(match result {
                            Ok(()) => format!("Saved to {}", saved_path.display()),
                            Err(error) => error,
                        });
                        cx.notify();
                    });
                })
                .detach();
            }
        }
    }

    /// Update configuration (font and dark mode) from the host app.
    /// Also refreshes the daemon-backed file tree.
    pub fn update_config(
        &mut self,
        font_size: f32,
        font_family: SharedString,
        is_dark: bool,
        cx: &mut Context<Self>,
    ) {
        let rehighlight = is_dark != self.is_dark;
        self.file_font_size = font_size;
        self.file_font = okena_ui::tokens::file_font_for_family(font_family, cx);
        self.is_dark = is_dark;

        // Re-fetch directory listings so the sidebar reflects added/removed files
        self.refresh_file_tree_async(cx);

        for tab in &mut self.tabs {
            if tab.is_empty() {
                continue;
            }
            // Theme changed — re-highlight without reloading. Raster image
            // and font tabs have no highlighted content; SVG tabs do (the
            // source-view XML), so they need the rehighlight too.
            if rehighlight && !tab.is_font && (!tab.is_image || tab.is_svg) {
                tab.do_highlight_content(&tab.file_path.clone(), &self.syntax_set, self.is_dark);
                if let Some(alternate) = tab.json_alternate.as_mut() {
                    alternate.highlighted_lines = None;
                }
                // The rendered markdown view carries its own highlighted code
                // blocks, separate from the source view's lines.
                if let Some(doc) = tab.markdown_doc.as_mut() {
                    doc.highlight_code_blocks(self.is_dark);
                }
            }
        }
    }

    /// Invalidate the cached directory listings and re-fetch the ones that are
    /// currently expanded. Called when settings change or after file ops that
    /// might affect multiple folders (e.g. rename across hierarchies).
    pub(super) fn refresh_file_tree_async(&mut self, cx: &mut Context<Self>) {
        let to_refetch: Vec<String> = std::iter::once(String::new())
            .chain(self.expanded_folders.iter().cloned())
            .collect();
        self.loaded_dirs.clear();
        self.loading_dirs.clear();
        self.tree_error_message = None;
        self.invalidate_visible_tree_rows();
        for path in to_refetch {
            self.fetch_directory(path, cx);
        }
    }

    /// Re-fetch listings for a single directory and any of its loaded
    /// descendants. Use after a targeted file op (create/delete/rename of one
    /// entry) where a global rescan would be wasteful.
    pub(super) fn invalidate_directory(&mut self, relative_path: &str, cx: &mut Context<Self>) {
        let prefix = if relative_path.is_empty() {
            String::new()
        } else {
            format!("{}/", relative_path)
        };
        let to_refetch: Vec<String> = self
            .loaded_dirs
            .keys()
            .filter(|k| k.as_str() == relative_path || k.starts_with(&prefix))
            .cloned()
            .collect();
        for path in &to_refetch {
            self.loaded_dirs.remove(path);
            self.loading_dirs.remove(path);
        }
        self.invalidate_visible_tree_rows();
        // Always re-fetch the target dir even if it wasn't loaded before — the
        // caller asked us to refresh it.
        self.fetch_directory(relative_path.to_string(), cx);
        for path in to_refetch {
            if path != relative_path {
                self.fetch_directory(path, cx);
            }
        }
    }

    fn fetch_scope_info(&mut self, cx: &mut Context<Self>) {
        let fs = self.project_fs.clone();
        let path = fs.scope_path();
        let generation = self.scope_generation;
        cx.spawn(async move |entity: WeakEntity<Self>, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { fs.resolve_path(&path) })
                .await;
            let _ = entity.update(cx, |this, cx| {
                if this.scope_generation != generation {
                    return;
                }
                match result {
                    Ok(scope) if scope.kind == okena_core::api::ResolvedPathKind::Directory => {
                        this.scope = Some(scope);
                    }
                    Ok(_) => {
                        this.tree_error_message =
                            Some("Browser scope is not a directory".to_string());
                    }
                    Err(error) => this.tree_error_message = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn navigate_up(&mut self, cx: &mut Context<Self>) {
        let Some(parent) = self
            .scope
            .as_ref()
            .and_then(|scope| scope.breadcrumbs.iter().rev().nth(1))
        else {
            return;
        };
        self.navigate_to_scope(parent.canonical_path.clone(), cx);
    }

    pub(super) fn navigate_to_scope(&mut self, path: String, cx: &mut Context<Self>) {
        if self.scope_navigation_in_flight
            || self
                .scope
                .as_ref()
                .is_some_and(|scope| scope.canonical_path == path)
        {
            return;
        }
        let old_fs = self.project_fs.clone();
        let old_scope_path = self
            .scope
            .as_ref()
            .map(|scope| scope.canonical_path.clone())
            .unwrap_or_else(|| old_fs.scope_path());
        let absolute_tabs: Vec<Option<String>> = self
            .tabs
            .iter()
            .map(|tab| {
                (!tab.is_empty()).then(|| {
                    crate::project_fs::join_daemon_path(&old_scope_path, &tab.relative_path)
                })
            })
            .collect();
        self.scope_navigation_in_flight = true;
        self.transfer_status = None;
        cx.notify();

        cx.spawn(async move |entity: WeakEntity<Self>, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let scope = old_fs.resolve_path(&path)?;
                    if scope.kind != okena_core::api::ResolvedPathKind::Directory {
                        return Err("Browser scope is not a directory".to_string());
                    }
                    let fs = old_fs.scoped_to(scope.clone());
                    Ok((scope, fs))
                })
                .await;
            let _ = entity.update(cx, |this, cx| {
                this.scope_navigation_in_flight = false;
                match result {
                    Ok((scope, fs)) => {
                        for (tab, absolute_path) in this.tabs.iter_mut().zip(absolute_tabs) {
                            let Some(absolute_path) = absolute_path else {
                                continue;
                            };
                            let Some(relative_path) = fs.relative_path(&absolute_path) else {
                                continue;
                            };
                            tab.relative_path = relative_path.clone();
                            tab.file_path = Self::tree_path(&fs, &relative_path);
                        }
                        this.project_fs = fs;
                        this.scope = Some(scope);
                        this.scope_generation = this.scope_generation.wrapping_add(1);
                        this.loaded_dirs.clear();
                        this.loading_dirs.clear();
                        this.expanded_folders.clear();
                        this.tree_error_message = None;
                        this.invalidate_visible_tree_rows();
                        this.tree_scroll_handle
                            .scroll_to_item(0, ScrollStrategy::Top);
                        this.tree_scrollbar_drag = None;
                        this.loading = true;
                        this.freshness_check_in_flight = false;
                        this.sidebar_visible = true;
                        this.history = NavigationHistory::new();
                        this.blame_provider = None;
                        for tab in &mut this.tabs {
                            tab.blame = BlameLoadState::NotLoaded;
                        }
                        this.fetch_initial_dirs(cx);
                    }
                    Err(error) => this.tree_error_message = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Fetch the initial directory listings: the project root plus any
    /// ancestor folders that are expanded (so a viewer opened for
    /// `a/b/c.rs` shows the path expanded out to that file).
    fn fetch_initial_dirs(&mut self, cx: &mut Context<Self>) {
        self.fetch_directory(String::new(), cx);
        let dirs: Vec<String> = self.expanded_folders.iter().cloned().collect();
        for dir in dirs {
            self.fetch_directory(dir, cx);
        }
    }

    /// Spawn a background task to load `relative_path`'s immediate children
    /// and stash them in `loaded_dirs`. No-op if already loaded or in flight.
    pub(super) fn fetch_directory(&mut self, relative_path: String, cx: &mut Context<Self>) {
        if self.loaded_dirs.contains_key(&relative_path)
            || !self.loading_dirs.insert(relative_path.clone())
        {
            return;
        }
        self.invalidate_visible_tree_rows();
        let fs = self.project_fs.clone();
        let show_ignored = self.show_ignored;
        let path_for_task = relative_path.clone();
        let generation = self.scope_generation;
        cx.spawn(async move |entity: WeakEntity<Self>, cx| {
            let result: Result<Vec<DirEntry>, String> = cx
                .background_executor()
                .spawn(async move { fs.list_directory(&path_for_task, show_ignored) })
                .await;
            let _ = entity.update(cx, |this, cx| {
                if this.scope_generation != generation {
                    return;
                }
                this.loading_dirs.remove(&relative_path);
                match result {
                    Ok(entries) => {
                        this.loaded_dirs.insert(relative_path.clone(), entries);
                        if relative_path.is_empty() {
                            this.tree_error_message = None;
                        }
                    }
                    Err(error)
                        if !relative_path.is_empty() && is_missing_directory_error(&error) =>
                    {
                        let parent = prune_missing_directory(
                            &relative_path,
                            &mut this.loaded_dirs,
                            &mut this.loading_dirs,
                            &mut this.expanded_folders,
                        );
                        let parent_is_current = parent.is_empty()
                            || this.loaded_dirs.contains_key(&parent)
                            || this.loading_dirs.contains(&parent)
                            || this.expanded_folders.contains(&parent);
                        if parent_is_current {
                            this.loaded_dirs.remove(&parent);
                            this.fetch_directory(parent, cx);
                        }
                    }
                    Err(error) => {
                        this.tree_error_message = Some(error);
                        // Cache an empty vec so we don't retry on every render.
                        this.loaded_dirs.insert(relative_path.clone(), Vec::new());
                    }
                }
                if relative_path.is_empty() {
                    this.loading = false;
                }
                this.invalidate_visible_tree_rows();
                cx.notify();
            });
        })
        .detach();
    }

    /// Poll daemon metadata and reload the active tab when it changes.
    pub(super) fn check_active_tab_freshness(&mut self, cx: &mut Context<Self>) {
        if self.freshness_check_in_flight
            || self.last_change_check.elapsed() < std::time::Duration::from_secs(1)
        {
            return;
        }
        self.last_change_check = std::time::Instant::now();

        let tab = &self.tabs[self.active_tab];
        if tab.is_empty() || tab.revision.is_some() {
            return;
        }

        let relative_path = tab.relative_path.clone();
        let old_mtime = tab.modified_at;
        let fs = self.project_fs.clone();
        let path_for_request = relative_path.clone();
        let generation = self.scope_generation;

        self.freshness_check_in_flight = true;
        cx.spawn(async move |entity: WeakEntity<Self>, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { fs.file_metadata(&path_for_request) })
                .await;
            let _ = entity.update(cx, |this, cx| {
                if this.scope_generation != generation {
                    return;
                }
                this.freshness_check_in_flight = false;
                match result {
                    Ok(metadata)
                        if metadata.modified_at_millis != old_mtime
                            && this
                                .tabs
                                .iter()
                                .find(|tab| tab.relative_path == relative_path)
                                .is_some_and(|tab| tab.revision.is_none()) =>
                    {
                        this.spawn_tab_load(relative_path, cx);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        log::warn!("File freshness check failed: {error}");
                    }
                }
            });
        })
        .detach();
    }

    /// Get the active tab.
    pub(super) fn active_tab(&self) -> &FileViewerTab {
        &self.tabs[self.active_tab]
    }

    /// Get the active tab mutably.
    pub(super) fn active_tab_mut(&mut self) -> &mut FileViewerTab {
        &mut self.tabs[self.active_tab]
    }

    /// Open a file in a tab (VS Code style).
    /// - If already open in a tab, switches to it.
    /// - If current tab is empty, replaces it.
    /// - Otherwise creates a new tab after the active one.
    pub fn open_file_in_tab(&mut self, relative_path: String, cx: &mut Context<Self>) {
        // Already open? Switch to it.
        if let Some(idx) = self
            .tabs
            .iter()
            .position(|t| t.relative_path == relative_path)
        {
            if idx != self.active_tab {
                let current = self.active_tab().relative_path.clone();
                self.history.push(&current);
                self.active_tab = idx;
            }
            self.expand_ancestors_and_fetch(&relative_path, cx);
            if self.history_visible {
                self.spawn_history_load_for_active(cx);
            }
            cx.notify();
            return;
        }

        self.expand_ancestors_and_fetch(&relative_path, cx);

        let file_path = Self::tree_path(&self.project_fs, &relative_path);
        let new_tab = FileViewerTab::new_loading(relative_path.clone(), file_path);

        // If current tab is empty (no file loaded), replace it
        if self.active_tab().is_empty() {
            release_tab_renderer(&mut self.tabs[self.active_tab], cx);
            self.tabs[self.active_tab] = new_tab;
            self.spawn_tab_load(relative_path, cx);
            if self.history_visible {
                self.spawn_history_load_for_active(cx);
            }
            cx.notify();
            return;
        }

        // Push history for the current file
        let current = self.active_tab().relative_path.clone();
        self.history.push(&current);

        let (new_active, evicted) =
            Self::insert_tab_after_active(&mut self.tabs, self.active_tab, new_tab);
        self.active_tab = new_active;
        // Release any image assets owned by the evicted tab — otherwise
        // its sprite-atlas tile / decoded asset cache entry would linger
        // after the tab is gone (an SVG that had been zoomed to 16× is
        // tens of MB of GPU memory).
        if let Some(mut tab) = evicted {
            release_tab_renderer(&mut tab, cx);
        }

        self.spawn_tab_load(relative_path, cx);
        if self.history_visible {
            self.spawn_history_load_for_active(cx);
        }
        cx.notify();
    }

    pub fn open_file_in_tab_at(
        &mut self,
        relative_path: String,
        position: FilePosition,
        cx: &mut Context<Self>,
    ) {
        self.open_file_in_tab(relative_path, cx);
        let tab = self.active_tab_mut();
        tab.target_line = position.line;
        tab.target_column = position.column;
        if let Some(line) = position.line {
            tab.display_mode = DisplayMode::Source;
            let row = tab.source_row_for_line(line);
            tab.source_scroll_handle
                .scroll_to_item(row, ScrollStrategy::Center);
        }
        cx.notify();
    }

    pub fn open_target(&mut self, target: FileTarget, cx: &mut Context<Self>) {
        let FileTarget {
            relative_path,
            source,
            position,
        } = target;
        self.open_file_in_tab_at(relative_path.clone(), position, cx);
        match source {
            FileSource::WorkingTree if self.active_tab().revision.is_some() => {
                self.spawn_tab_load(relative_path, cx);
            }
            FileSource::WorkingTree => {}
            source => self.show_source(relative_path, source, cx),
        }
    }

    /// Insert `new_tab` directly after the active tab and return
    /// `(new_active_index, evicted_tab)`. When already at `MAX_TABS`, the
    /// oldest tab is evicted first to make room — never the active tab,
    /// so the file the user is looking at is preserved. Evicting a tab
    /// before the active one shifts the active index left by one.
    ///
    /// The evicted tab is returned (not dropped here) so the caller can
    /// release any GPU-side image assets it owned before letting it drop.
    fn insert_tab_after_active(
        tabs: &mut Vec<FileViewerTab>,
        active: usize,
        new_tab: FileViewerTab,
    ) -> (usize, Option<FileViewerTab>) {
        let mut active = active;
        let mut evicted = None;
        if tabs.len() >= MAX_TABS {
            // Oldest tab is index 0; skip it only if it's the active tab.
            let evict = if active == 0 { 1 } else { 0 };
            evicted = Some(tabs.remove(evict));
            if evict < active {
                active -= 1;
            }
        }
        let insert_at = active + 1;
        tabs.insert(insert_at, new_tab);
        (insert_at, evicted)
    }

    /// Mark all ancestor folders of `relative_path` as expanded and ensure
    /// their listings are loaded so the tree reveals down to the file.
    fn expand_ancestors_and_fetch(&mut self, relative_path: &str, cx: &mut Context<Self>) {
        let expanded = Self::compute_expanded_for_relative(relative_path);
        for path in &expanded {
            self.fetch_directory(path.clone(), cx);
        }
        self.expanded_folders.extend(expanded);
        self.invalidate_visible_tree_rows();
    }

    /// Close a tab by index.
    pub(super) fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.tabs.len() <= 1 {
            cx.emit(FileViewerEvent::Close);
            return;
        }

        let mut removed = self.tabs.remove(index);
        release_tab_renderer(&mut removed, cx);

        if index == self.active_tab {
            // Closed the active tab: prefer the tab to the right (same index),
            // or the last tab if we were at the end
            self.active_tab = index.min(self.tabs.len() - 1);
        } else if self.active_tab > index {
            // Closed a tab before the active one: shift index left
            self.active_tab -= 1;
        }
        // If closed tab was after active tab, active_tab stays the same

        if self.history_visible {
            self.spawn_history_load_for_active(cx);
        }

        cx.notify();
    }

    /// Close all tabs except the one at `index`.
    pub(super) fn close_other_tabs(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            let kept = self.tabs.remove(index);
            let mut dropped: Vec<FileViewerTab> = self.tabs.drain(..).collect();
            self.tabs.push(kept);
            self.active_tab = 0;
            for mut tab in dropped.drain(..) {
                release_tab_renderer(&mut tab, cx);
            }
            cx.notify();
        }
    }

    /// Close all tabs, leaving an empty viewer state.
    pub(super) fn close_all_tabs(&mut self, cx: &mut Context<Self>) {
        let mut dropped: Vec<FileViewerTab> = self.tabs.drain(..).collect();
        self.tabs.push(FileViewerTab::new_empty());
        self.active_tab = 0;
        for mut tab in dropped.drain(..) {
            release_tab_renderer(&mut tab, cx);
        }
        cx.notify();
    }

    /// Release every tab's GPU-side image asset before the viewer is dropped.
    pub fn release_all_image_assets(&mut self, cx: &mut App) {
        for tab in &mut self.tabs {
            release_tab_renderer(tab, cx);
        }
    }

    /// Switch to a tab by index.
    pub(super) fn set_active_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.tabs.len() && index != self.active_tab {
            let current = self.active_tab().relative_path.clone();
            self.history.push(&current);
            self.active_tab = index;
            if self.blame_visible {
                self.spawn_blame_load_for_active(cx);
            }
            if self.history_visible {
                self.spawn_history_load_for_active(cx);
            }
            // Update expanded folders to reveal active tab's file
            let tab_rel = self.tabs[self.active_tab].relative_path.clone();
            self.expand_ancestors_and_fetch(&tab_rel, cx);
            // Re-run search for the new tab's content
            if self.search_state.is_some() {
                self.perform_file_search(cx);
            }
            cx.notify();
        }
    }

    /// Navigate back in history.
    pub(super) fn go_back(&mut self, cx: &mut Context<Self>) {
        let current = self.active_tab().relative_path.clone();
        if let Some(target) = self.history.go_back(&current) {
            self.navigate_to_file_no_history(target, cx);
        }
    }

    /// Navigate forward in history.
    pub(super) fn go_forward(&mut self, cx: &mut Context<Self>) {
        let current = self.active_tab().relative_path.clone();
        if let Some(target) = self.history.go_forward(&current) {
            self.navigate_to_file_no_history(target, cx);
        }
    }

    /// Navigate to a file without pushing history (used by back/forward).
    fn navigate_to_file_no_history(&mut self, relative_path: String, cx: &mut Context<Self>) {
        // If file is open in a tab, switch to it
        if let Some(idx) = self
            .tabs
            .iter()
            .position(|t| t.relative_path == relative_path)
        {
            self.active_tab = idx;
            if self.history_visible {
                self.spawn_history_load_for_active(cx);
            }
            cx.notify();
            return;
        }

        self.expand_ancestors_and_fetch(&relative_path, cx);

        let file_path = Self::tree_path(&self.project_fs, &relative_path);
        let new_tab = FileViewerTab::new_loading(relative_path.clone(), file_path);
        release_tab_renderer(&mut self.tabs[self.active_tab], cx);
        self.tabs[self.active_tab] = new_tab;
        self.spawn_tab_load(relative_path, cx);
        if self.history_visible {
            self.spawn_history_load_for_active(cx);
        }
        cx.notify();
    }

    /// Spawn a background task to load file content for a tab. The tab is
    /// identified by `relative_path` so concurrent reorders don't bind us to
    /// a stale index, AND by a per-load generation token so a slow earlier
    /// load can't overwrite a faster later one for the same path.
    fn spawn_tab_load(&mut self, relative_path: String, cx: &mut Context<Self>) {
        self.next_load_generation = self.next_load_generation.wrapping_add(1);
        let generation = self.next_load_generation;
        if let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|t| t.relative_path == relative_path)
        {
            tab.load_generation = generation;
            tab.revision = None;
            tab.revision_source = None;
            tab.loading = true;
            tab.error_message = None;
        }
        let fs = self.project_fs.clone();
        let rel = relative_path.clone();
        // Image / font detection is driven purely by extension, so we can
        // decide the load strategy off-thread without holding the tab borrow.
        let asset_path = PathBuf::from(&relative_path);
        let renderer_kind = crate::file_renderer::FileRenderer::kind_for_path(&asset_path);
        let is_image = matches!(
            renderer_kind,
            Some(crate::file_renderer::FileRendererKind::Image { .. })
        );
        let is_font = renderer_kind == Some(crate::file_renderer::FileRendererKind::Font);
        let svg_renderer = cx.svg_renderer();
        let syntax_set = self.syntax_set.clone();
        let is_dark = self.is_dark;
        cx.spawn(async move |entity: WeakEntity<Self>, cx| {
            let result: Result<(loading::LoadedContent, Option<u64>), String> = cx
                .background_executor()
                .spawn(async move {
                    let metadata = fs.file_metadata(&rel)?;
                    let content = if is_image || is_font {
                        if metadata.size > crate::file_renderer::MAX_RENDERED_FILE_SIZE {
                            return Err(format!(
                                "File too large ({} bytes). Maximum size is 20 MB.",
                                metadata.size
                            ));
                        }
                        let bytes = fs.read_file_bytes(&rel)?;
                        if is_image {
                            loading::build_image_content(&asset_path, bytes, &svg_renderer)?
                        } else {
                            loading::build_font_content(&asset_path, bytes, &svg_renderer)?
                        }
                    } else {
                        if metadata.size > MAX_FILE_SIZE {
                            return Err(format!(
                                "File too large ({} bytes). Maximum size is 5 MB.",
                                metadata.size
                            ));
                        }
                        loading::build_text_content(
                            &asset_path,
                            fs.read_file(&rel)?,
                            &syntax_set,
                            is_dark,
                        )
                    };
                    Ok((content, metadata.modified_at_millis))
                })
                .await;
            let Some(entity) = entity.upgrade() else {
                if let Ok((content, _)) = result {
                    cx.update(|cx| content.release(cx));
                }
                return;
            };
            entity.update(cx, |this, cx| {
                let tab_index = this
                    .tabs
                    .iter()
                    .position(|tab| tab.relative_path == relative_path);
                let Some(tab_index) = tab_index else {
                    if let Ok((content, _)) = result {
                        content.release(cx);
                    }
                    return;
                };
                if this.tabs[tab_index].load_generation != generation {
                    if let Ok((content, _)) = result {
                        content.release(cx);
                    }
                    return;
                }
                let modified_at = result
                    .as_ref()
                    .ok()
                    .and_then(|(_, modified_at)| *modified_at);
                let tab = &mut this.tabs[tab_index];
                tab.apply_loaded_content(
                    result.map(|(content, _)| content),
                    modified_at,
                    &this.syntax_set,
                    this.is_dark,
                    cx,
                );
                let target_row = tab.target_line.map(|line| tab.source_row_for_line(line));
                tab.blame = BlameLoadState::NotLoaded;
                if tab_index == this.active_tab {
                    this.perform_file_search(cx);
                }
                cx.notify();
                if let Some(row) = target_row {
                    this.active_tab()
                        .source_scroll_handle
                        .scroll_to_item(row, ScrollStrategy::Center);
                }
                if this.blame_visible {
                    this.spawn_blame_load_for_active(cx);
                }
                if this.history_visible {
                    this.spawn_history_load_for_active(cx);
                }
            });
        })
        .detach();
    }

    /// Compute which folder paths should be expanded to reveal a file.
    fn compute_expanded_for_relative(relative_path: &str) -> HashSet<String> {
        let mut expanded = HashSet::new();
        let parts: Vec<&str> = relative_path.split(['/', '\\']).collect();
        // Expand all ancestor directories (not the file itself)
        let mut path_so_far = String::new();
        for part in &parts[..parts.len().saturating_sub(1)] {
            if !path_so_far.is_empty() {
                path_so_far.push('/');
            }
            path_so_far.push_str(part);
            expanded.insert(path_so_far.clone());
        }
        expanded
    }
}

/// Events emitted by the file viewer.
#[derive(Clone, Debug)]
pub enum FileViewerEvent {
    /// Viewer was closed.
    Close,
    /// Return to the screen that opened this file.
    Back,
    /// User requested to detach the viewer into a separate OS window.
    Detach,
    /// User clicked a blame entry — open the named commit in the diff viewer.
    OpenCommit(String),
    /// Open the selected file's diff in the named commit.
    OpenFileDiff { hash: String, relative_path: String },
    /// User toggled the blame gutter — host persists the preference.
    BlamePreferenceChanged(bool),
    /// User clicked "Send to terminal" on a selection. Carries the structured
    /// payload; the host formats it (relative to terminal CWD) before pasting.
    SendToTerminal(okena_core::send_payload::SendPayload),
    /// Open the daemon-side path with a same-host external application.
    OpenExternally {
        path: String,
        line: Option<usize>,
        column: Option<usize>,
    },
}

fn release_tab_renderer(tab: &mut FileViewerTab, cx: &mut App) {
    if let Some(renderer) = tab.file_renderer.take() {
        renderer.update(cx, |renderer, cx| renderer.release_assets(cx));
    }
}

impl EventEmitter<FileViewerEvent> for FileViewer {}

impl okena_ui::overlay::CloseEvent for FileViewerEvent {
    fn is_close(&self) -> bool {
        matches!(self, Self::Close | Self::Back)
    }
}

impl Focusable for FileViewer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FileViewer, FileViewerTab, MAX_TABS, NavigationHistory, is_missing_directory_error,
        prune_missing_directory,
    };
    use crate::list_directory::{DIRECTORY_NOT_FOUND_ERROR, DirEntry};
    use std::collections::{HashMap, HashSet};

    fn tab(name: &str) -> FileViewerTab {
        FileViewerTab::new_loading(name.to_string(), name.into())
    }

    fn paths(tabs: &[FileViewerTab]) -> Vec<&str> {
        tabs.iter().map(|t| t.relative_path.as_str()).collect()
    }

    #[::core::prelude::v1::test]
    fn insert_tab_below_limit_inserts_after_active() {
        let mut tabs = vec![tab("a"), tab("b"), tab("c")];
        let (active, evicted) = FileViewer::insert_tab_after_active(&mut tabs, 0, tab("new"));
        assert_eq!(active, 1);
        assert!(evicted.is_none());
        assert_eq!(paths(&tabs), ["a", "new", "b", "c"]);
    }

    #[::core::prelude::v1::test]
    fn insert_tab_at_limit_evicts_oldest_and_keeps_active() {
        let mut tabs: Vec<FileViewerTab> = (0..MAX_TABS).map(|i| tab(&format!("f{i}"))).collect();
        // Active is somewhere in the middle.
        let (active, evicted) = FileViewer::insert_tab_after_active(&mut tabs, 10, tab("new"));
        assert_eq!(tabs.len(), MAX_TABS);
        // Oldest (index 0) was evicted; everything shifted left by one, so the
        // active file f10 stays active and the new tab lands right after it.
        assert_eq!(evicted.expect("oldest tab returned").relative_path, "f0");
        assert_eq!(tabs[0].relative_path, "f1");
        assert_eq!(tabs[active - 1].relative_path, "f10");
        assert_eq!(tabs[active].relative_path, "new");
    }

    #[::core::prelude::v1::test]
    fn insert_tab_at_limit_skips_active_when_active_is_oldest() {
        let mut tabs: Vec<FileViewerTab> = (0..MAX_TABS).map(|i| tab(&format!("f{i}"))).collect();
        // Active IS the oldest tab — must not evict it.
        let (active, evicted) = FileViewer::insert_tab_after_active(&mut tabs, 0, tab("new"));
        assert_eq!(tabs.len(), MAX_TABS);
        assert_eq!(active, 1);
        // f0 (active) preserved at index 0; f1 (next oldest) evicted.
        assert_eq!(
            evicted.expect("next-oldest tab returned").relative_path,
            "f1"
        );
        assert_eq!(tabs[0].relative_path, "f0");
        assert_eq!(tabs[1].relative_path, "new");
        assert_eq!(tabs[2].relative_path, "f2");
    }

    #[::core::prelude::v1::test]
    fn test_compute_expanded_root_file() {
        let expanded = FileViewer::compute_expanded_for_relative("README.md");
        assert!(expanded.is_empty());
    }

    #[::core::prelude::v1::test]
    fn test_compute_expanded_nested_file() {
        let expanded = FileViewer::compute_expanded_for_relative("src/views/mod.rs");
        assert_eq!(expanded.len(), 2);
        assert!(expanded.contains("src"));
        assert!(expanded.contains("src/views"));
    }

    #[::core::prelude::v1::test]
    fn test_compute_expanded_empty_string() {
        let expanded = FileViewer::compute_expanded_for_relative("");
        assert!(expanded.is_empty());
    }

    #[::core::prelude::v1::test]
    fn test_compute_expanded_no_slash() {
        let expanded = FileViewer::compute_expanded_for_relative("Cargo.toml");
        assert!(expanded.is_empty());
    }

    #[::core::prelude::v1::test]
    fn test_history_back_forward() {
        let mut history = NavigationHistory::new();

        // Navigate a -> b -> c
        history.push("a.rs");
        history.push("b.rs");

        assert!(history.can_go_back());
        assert!(!history.can_go_forward());

        // Go back from c
        let target = history.go_back("c.rs").unwrap();
        assert_eq!(target, "b.rs");
        assert!(history.can_go_forward());

        // Go back again
        let target = history.go_back("b.rs").unwrap();
        assert_eq!(target, "a.rs");

        // Go forward
        let target = history.go_forward("a.rs").unwrap();
        assert_eq!(target, "b.rs");

        let target = history.go_forward("b.rs").unwrap();
        assert_eq!(target, "c.rs");

        assert!(!history.can_go_forward());
    }

    #[::core::prelude::v1::test]
    fn test_history_new_navigation_clears_forward() {
        let mut history = NavigationHistory::new();

        history.push("a.rs");
        history.push("b.rs");

        // Go back from c to b
        history.go_back("c.rs");

        // New navigation from b
        history.push("b.rs");

        // Forward should be empty
        assert!(!history.can_go_forward());

        // Back should give b then a
        let target = history.go_back("d.rs").unwrap();
        assert_eq!(target, "b.rs");
        let target = history.go_back("b.rs").unwrap();
        assert_eq!(target, "a.rs");
    }

    #[::core::prelude::v1::test]
    fn test_history_limit() {
        let mut history = NavigationHistory::new();

        for i in 0..60 {
            history.push(&format!("file_{}.rs", i));
        }

        assert_eq!(history.back_stack.len(), 50);

        // First entry should be file_59 (0-9 were trimmed)
        let mut target = history.go_back("current.rs").unwrap();
        assert_eq!(target, "file_59.rs");

        // Drain remaining
        let mut count = 1;
        while let Some(t) = history.go_back(&target) {
            target = t;
            count += 1;
        }
        assert_eq!(count, 50);
    }

    #[::core::prelude::v1::test]
    fn recognizes_missing_directory_errors_from_current_and_older_daemons() {
        assert!(is_missing_directory_error(DIRECTORY_NOT_FOUND_ERROR));
        assert!(is_missing_directory_error(
            "Server returned 400 Bad Request: {\"error\":\"Cannot read directory: No such file or directory (os error 2)\"}"
        ));
        assert!(!is_missing_directory_error(
            "Server returned 500 Internal Server Error"
        ));
    }

    #[::core::prelude::v1::test]
    fn missing_directory_prunes_only_its_subtree() {
        let mut loaded_dirs: HashMap<String, Vec<DirEntry>> = [
            (String::new(), Vec::new()),
            ("apps".to_string(), Vec::new()),
            ("apps/deleted".to_string(), Vec::new()),
            ("apps/deleted/nested".to_string(), Vec::new()),
            ("apps/kept".to_string(), Vec::new()),
        ]
        .into_iter()
        .collect();
        let mut loading_dirs = HashSet::from([
            "apps/deleted/other".to_string(),
            "apps/kept/other".to_string(),
        ]);
        let mut expanded_folders = HashSet::from([
            "apps".to_string(),
            "apps/deleted".to_string(),
            "apps/deleted/nested".to_string(),
            "apps/kept".to_string(),
        ]);

        let parent = prune_missing_directory(
            "apps/deleted",
            &mut loaded_dirs,
            &mut loading_dirs,
            &mut expanded_folders,
        );

        assert_eq!(parent, "apps");
        assert_eq!(
            loaded_dirs.keys().cloned().collect::<HashSet<_>>(),
            HashSet::from([String::new(), "apps".to_string(), "apps/kept".to_string(),])
        );
        assert_eq!(loading_dirs, HashSet::from(["apps/kept/other".to_string()]));
        assert_eq!(
            expanded_folders,
            HashSet::from(["apps".to_string(), "apps/kept".to_string()])
        );
    }
}
