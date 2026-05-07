//! File viewer overlay for displaying file contents with syntax highlighting.
//!
//! Provides a read-only view of files with syntax highlighting via syntect.
//! Markdown files can be viewed in rendered preview mode.

mod context_menu;
mod loading;
mod render;
mod search;
mod selection;

use crate::code_view::ScrollbarDrag;
use crate::list_directory::DirEntry;
use crate::selection::SelectionState;
use crate::syntax::{load_syntax_set, HighlightedLine};
use context_menu::{DeleteConfirmState, FileRenameState, FileTreeContextMenu, TabContextMenu};
use gpui::*;
use okena_markdown::{MarkdownDocument, MarkdownSelection};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;
use syntect::parsing::SyntaxSet;

/// Maximum file size to load (5MB)
const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024;

/// Maximum number of lines to display
const MAX_LINES: usize = 10000;

/// Maximum number of open tabs
const MAX_TABS: usize = 20;

/// Maximum navigation history stack size
const MAX_HISTORY: usize = 50;

/// Display mode for file viewer.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum DisplayMode {
    #[default]
    Source,
    Preview,
}

/// Type alias for source view selection (line, column).
type Selection = SelectionState<(usize, usize)>;

/// Width of file tree sidebar.
const SIDEBAR_WIDTH: f32 = 240.0;

/// Per-file state for a single tab in the file viewer.
///
/// `relative_path` is the canonical identifier (project-relative, used for
/// `fs.read_file`, tab equality, history, and tree highlighting). `file_path`
/// is the derived absolute path used for filesystem-level context-menu
/// operations (rename/delete) — it's only meaningful for local projects.
pub(super) struct FileViewerTab {
    pub file_path: PathBuf,
    pub relative_path: String,
    pub content: String,
    pub highlighted_lines: Vec<HighlightedLine>,
    pub line_count: usize,
    pub line_num_width: usize,
    pub error_message: Option<String>,
    pub selection: Selection,
    pub display_mode: DisplayMode,
    pub is_markdown: bool,
    pub markdown_doc: Option<MarkdownDocument>,
    pub markdown_selection: MarkdownSelection,
    pub markdown_scroll_handle: ScrollHandle,
    pub source_scroll_handle: UniformListScrollHandle,
    pub scrollbar_drag: Option<ScrollbarDrag>,
    /// Last known modification time of the file (for detecting external changes).
    pub modified_at: Option<SystemTime>,
    /// Whether the tab content is still being loaded asynchronously.
    pub loading: bool,
}

impl FileViewerTab {
    /// Create a new tab for browsing (no file loaded).
    pub(super) fn new_empty() -> Self {
        Self {
            file_path: PathBuf::new(),
            relative_path: String::new(),
            content: String::new(),
            highlighted_lines: Vec::new(),
            line_count: 0,
            line_num_width: 3,
            error_message: None,
            selection: Selection::default(),
            display_mode: DisplayMode::Source,
            is_markdown: false,
            markdown_doc: None,
            markdown_selection: MarkdownSelection::default(),
            markdown_scroll_handle: ScrollHandle::new(),
            source_scroll_handle: UniformListScrollHandle::new(),
            scrollbar_drag: None,
            modified_at: None,
            loading: false,
        }
    }

    /// Create a tab in loading state (content will be filled asynchronously).
    fn new_loading(relative_path: String, file_path: PathBuf) -> Self {
        let is_markdown = Self::is_markdown_file(&file_path);
        Self {
            file_path,
            relative_path,
            content: String::new(),
            highlighted_lines: Vec::new(),
            line_count: 0,
            line_num_width: 3,
            error_message: None,
            selection: Selection::default(),
            display_mode: if is_markdown {
                DisplayMode::Preview
            } else {
                DisplayMode::Source
            },
            is_markdown,
            markdown_doc: None,
            markdown_selection: MarkdownSelection::default(),
            markdown_scroll_handle: ScrollHandle::new(),
            source_scroll_handle: UniformListScrollHandle::new(),
            scrollbar_drag: None,
            modified_at: None,
            loading: true,
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
    /// Syntax set for highlighting
    syntax_set: SyntaxSet,
    /// File font size from settings
    file_font_size: f32,
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
    /// Which folder paths are currently expanded
    expanded_folders: HashSet<String>,
    /// Scroll handle for the file tree sidebar
    tree_scroll_handle: ScrollHandle,
    /// Whether the sidebar is visible
    sidebar_visible: bool,
    /// Open tabs
    pub(super) tabs: Vec<FileViewerTab>,
    /// Index of the active tab
    pub(super) active_tab: usize,
    /// Navigation history
    pub(super) history: NavigationHistory,
    /// Last time we checked files for external modifications
    last_change_check: std::time::Instant,
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
    pub(super) search_state: Option<search::FileSearchState>,
    /// True when this viewer is hosted inside a detached window.
    /// Hides the "detach" button and is set by the detached host.
    pub(super) is_detached: bool,
}

impl FileViewer {
    /// Resolve a project-relative path to an absolute `PathBuf` for filesystem
    /// ops. For remote projects the absolute path doesn't exist locally;
    /// callers fall back to the relative path wrapped as a `PathBuf`.
    fn resolve_absolute(
        fs: &std::sync::Arc<dyn crate::project_fs::ProjectFs>,
        relative_path: &str,
    ) -> PathBuf {
        match fs.project_root() {
            Some(root) => root.join(relative_path),
            None => PathBuf::from(relative_path),
        }
    }

    /// Create a new file viewer with `relative_path` (project-relative) opened
    /// in the first tab.
    pub fn new(
        relative_path: String,
        project_fs: std::sync::Arc<dyn crate::project_fs::ProjectFs>,
        font_size: f32,
        is_dark: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let expanded_folders = Self::compute_expanded_for_relative(&relative_path);
        let syntax_set = load_syntax_set();

        let file_path = Self::resolve_absolute(&project_fs, &relative_path);
        let tab = FileViewerTab::new_loading(relative_path.clone(), file_path.clone());

        let mut viewer = Self {
            focus_handle,
            project_fs,
            syntax_set,
            file_font_size: font_size,
            measured_char_width: font_size * 0.6,
            is_dark,
            loading: true,
            loaded_dirs: HashMap::new(),
            loading_dirs: HashSet::new(),
            expanded_folders,
            tree_scroll_handle: ScrollHandle::new(),
            sidebar_visible: true,
            tabs: vec![tab],
            active_tab: 0,
            history: NavigationHistory::new(),
            last_change_check: std::time::Instant::now(),
            show_ignored: false,
            filter_popover_open: false,
            filter_button_bounds: None,
            context_menu: None,
            tab_context_menu: None,
            rename_state: None,
            delete_confirm: None,
            search_state: None,
            is_detached: false,
        };

        // Kick off the root directory listing and any expanded ancestors so
        // the tree fills in around the opened file.
        viewer.fetch_initial_dirs(cx);
        viewer.spawn_tab_load(relative_path, cx);
        viewer
    }

    /// Create a file viewer for browsing a project without a pre-selected file.
    ///
    /// Opens the sidebar file tree with no file loaded.
    pub fn new_browse(
        project_fs: std::sync::Arc<dyn crate::project_fs::ProjectFs>,
        font_size: f32,
        is_dark: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        let mut viewer = Self {
            focus_handle,
            project_fs,
            syntax_set: load_syntax_set(),
            file_font_size: font_size,
            measured_char_width: font_size * 0.6,
            is_dark,
            loading: true,
            loaded_dirs: HashMap::new(),
            loading_dirs: HashSet::new(),
            expanded_folders: HashSet::new(),
            tree_scroll_handle: ScrollHandle::new(),
            sidebar_visible: true,
            tabs: vec![FileViewerTab::new_empty()],
            active_tab: 0,
            history: NavigationHistory::new(),
            last_change_check: std::time::Instant::now(),
            show_ignored: false,
            filter_popover_open: false,
            filter_button_bounds: None,
            context_menu: None,
            tab_context_menu: None,
            rename_state: None,
            delete_confirm: None,
            search_state: None,
            is_detached: false,
        };
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

    /// Request to detach the viewer into a separate OS window.
    pub(super) fn request_detach(&self, cx: &mut Context<Self>) {
        cx.emit(FileViewerEvent::Detach);
    }

    /// Update configuration (font size and dark mode) from the host app.
    /// Also refreshes the file tree and all tabs that were modified externally.
    pub fn update_config(&mut self, font_size: f32, is_dark: bool, cx: &mut Context<Self>) {
        let rehighlight = is_dark != self.is_dark;
        self.file_font_size = font_size;
        self.is_dark = is_dark;

        // Re-fetch directory listings so the sidebar reflects added/removed files
        self.refresh_file_tree_async(cx);

        for tab in &mut self.tabs {
            if tab.is_empty() {
                continue;
            }
            // Reload externally modified files (also re-highlights)
            if tab.reload_if_changed(&self.syntax_set, self.is_dark) {
                continue;
            }
            // Theme changed — re-highlight without reloading
            if rehighlight {
                tab.do_highlight_content(
                    &tab.file_path.clone(),
                    &self.syntax_set,
                    self.is_dark,
                );
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
        // Always re-fetch the target dir even if it wasn't loaded before — the
        // caller asked us to refresh it.
        self.fetch_directory(relative_path.to_string(), cx);
        for path in to_refetch {
            if path != relative_path {
                self.fetch_directory(path, cx);
            }
        }
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
        let fs = self.project_fs.clone();
        let show_ignored = self.show_ignored;
        let path_for_task = relative_path.clone();
        cx.spawn(async move |entity: WeakEntity<Self>, cx| {
            let result: Result<Vec<DirEntry>, String> = cx
                .background_executor()
                .spawn(async move { fs.list_directory(&path_for_task, show_ignored) })
                .await;
            let _ = entity.update(cx, |this, cx| {
                this.loading_dirs.remove(&relative_path);
                match result {
                    Ok(entries) => {
                        this.loaded_dirs.insert(relative_path.clone(), entries);
                    }
                    Err(e) => {
                        log::warn!("list_directory({}) failed: {}", relative_path, e);
                        // Cache an empty vec so we don't retry on every render.
                        this.loaded_dirs.insert(relative_path.clone(), Vec::new());
                    }
                }
                if relative_path.is_empty() {
                    this.loading = false;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Check if the active tab's file was modified externally and reload if so.
    /// Throttled to at most once per second.
    pub(super) fn check_active_tab_freshness(&mut self) {
        if self.last_change_check.elapsed() < std::time::Duration::from_secs(1) {
            return;
        }
        self.last_change_check = std::time::Instant::now();

        let tab = &mut self.tabs[self.active_tab];
        if !tab.is_empty() {
            tab.reload_if_changed(&self.syntax_set, self.is_dark);
        }
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
        if let Some(idx) = self.tabs.iter().position(|t| t.relative_path == relative_path) {
            if idx != self.active_tab {
                let current = self.active_tab().relative_path.clone();
                self.history.push(&current);
                self.active_tab = idx;
            }
            self.expand_ancestors_and_fetch(&relative_path, cx);
            cx.notify();
            return;
        }

        self.expand_ancestors_and_fetch(&relative_path, cx);

        let file_path = Self::resolve_absolute(&self.project_fs, &relative_path);
        let new_tab = FileViewerTab::new_loading(relative_path.clone(), file_path);

        // If current tab is empty (no file loaded), replace it
        if self.active_tab().is_empty() {
            self.tabs[self.active_tab] = new_tab;
            self.spawn_tab_load(relative_path, cx);
            cx.notify();
            return;
        }

        // Push history for the current file
        let current = self.active_tab().relative_path.clone();
        self.history.push(&current);

        if self.tabs.len() >= MAX_TABS {
            // At limit: replace the active tab
            self.tabs[self.active_tab] = new_tab;
        } else {
            // Insert new tab after active
            let insert_at = self.active_tab + 1;
            self.tabs.insert(insert_at, new_tab);
            self.active_tab = insert_at;
        }

        self.spawn_tab_load(relative_path, cx);
        cx.notify();
    }

    /// Mark all ancestor folders of `relative_path` as expanded and ensure
    /// their listings are loaded so the tree reveals down to the file.
    fn expand_ancestors_and_fetch(&mut self, relative_path: &str, cx: &mut Context<Self>) {
        let expanded = Self::compute_expanded_for_relative(relative_path);
        for path in &expanded {
            self.fetch_directory(path.clone(), cx);
        }
        self.expanded_folders.extend(expanded);
    }

    /// Close a tab by index.
    pub(super) fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.tabs.len() <= 1 {
            cx.emit(FileViewerEvent::Close);
            return;
        }

        self.tabs.remove(index);

        if index == self.active_tab {
            // Closed the active tab: prefer the tab to the right (same index),
            // or the last tab if we were at the end
            self.active_tab = index.min(self.tabs.len() - 1);
        } else if self.active_tab > index {
            // Closed a tab before the active one: shift index left
            self.active_tab -= 1;
        }
        // If closed tab was after active tab, active_tab stays the same

        cx.notify();
    }

    /// Close all tabs except the one at `index`.
    pub(super) fn close_other_tabs(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            let kept = self.tabs.remove(index);
            self.tabs.clear();
            self.tabs.push(kept);
            self.active_tab = 0;
            cx.notify();
        }
    }

    /// Close all tabs, leaving an empty viewer state.
    pub(super) fn close_all_tabs(&mut self, cx: &mut Context<Self>) {
        self.tabs.clear();
        self.tabs.push(FileViewerTab::new_empty());
        self.active_tab = 0;
        cx.notify();
    }

    /// Switch to a tab by index.
    pub(super) fn set_active_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.tabs.len() && index != self.active_tab {
            let current = self.active_tab().relative_path.clone();
            self.history.push(&current);
            self.active_tab = index;
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
        if let Some(idx) = self.tabs.iter().position(|t| t.relative_path == relative_path) {
            self.active_tab = idx;
            cx.notify();
            return;
        }

        self.expand_ancestors_and_fetch(&relative_path, cx);

        let file_path = Self::resolve_absolute(&self.project_fs, &relative_path);
        let new_tab = FileViewerTab::new_loading(relative_path.clone(), file_path);
        self.tabs[self.active_tab] = new_tab;
        self.spawn_tab_load(relative_path, cx);
        cx.notify();
    }

    /// Spawn a background task to load file content for a tab. The tab is
    /// identified by `relative_path` so concurrent reorders don't bind us to
    /// a stale index.
    fn spawn_tab_load(&self, relative_path: String, cx: &mut Context<Self>) {
        let fs = self.project_fs.clone();
        let rel = relative_path.clone();
        cx.spawn(async move |entity: WeakEntity<Self>, cx| {
            let result: Result<String, String> = cx
                .background_executor()
                .spawn(async move {
                    let size = fs.file_size(&rel)?;
                    if size > MAX_FILE_SIZE {
                        return Err(format!(
                            "File too large ({:.1} MB). Maximum size is 5 MB.",
                            size as f64 / 1024.0 / 1024.0
                        ));
                    }
                    fs.read_file(&rel)
                })
                .await;
            let _ = entity.update(cx, |this, cx| {
                if let Some(tab) =
                    this.tabs.iter_mut().find(|t| t.relative_path == relative_path)
                {
                    tab.apply_loaded_content(result, &this.syntax_set, this.is_dark);
                    cx.notify();
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
    /// User requested to detach the viewer into a separate OS window.
    Detach,
}

impl EventEmitter<FileViewerEvent> for FileViewer {}

impl okena_ui::overlay::CloseEvent for FileViewerEvent {
    fn is_close(&self) -> bool {
        matches!(self, Self::Close)
    }
}

impl Focusable for FileViewer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{FileViewer, NavigationHistory};

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
}
