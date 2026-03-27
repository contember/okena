//! File viewer overlay for displaying file contents with syntax highlighting.
//!
//! Provides a read-only view of files with syntax highlighting via syntect.
//! Markdown files can be viewed in rendered preview mode.

mod loading;
mod render;
mod selection;

use crate::code_view::ScrollbarDrag;
use crate::file_search::{FileEntry, FileSearchDialog};
use crate::file_tree::{build_file_tree, FileTreeNode};
use crate::selection::SelectionState;
use crate::syntax::{load_syntax_set, HighlightedLine};
use gpui::*;
use okena_markdown::{MarkdownDocument, MarkdownSelection};
use std::collections::HashSet;
use std::path::PathBuf;
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
pub(super) struct FileViewerTab {
    pub file_path: PathBuf,
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
    pub selected_file_index: Option<usize>,
}

impl FileViewerTab {
    /// Create a new tab for browsing (no file loaded).
    fn new_empty() -> Self {
        Self {
            file_path: PathBuf::new(),
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
            selected_file_index: None,
        }
    }

    /// Create a new tab with a file loaded.
    fn new_with_file(
        file_path: PathBuf,
        file_index: Option<usize>,
        syntax_set: &SyntaxSet,
        is_dark: bool,
    ) -> Self {
        let is_markdown = Self::is_markdown_file(&file_path);
        let mut tab = Self {
            file_path: file_path.clone(),
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
            selected_file_index: file_index,
        };
        tab.load_file(&file_path, syntax_set, is_dark);
        tab
    }

    /// Get the filename for display in the tab bar.
    pub fn filename(&self) -> String {
        self.file_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".to_string())
    }

    /// Check if this tab has no file loaded.
    pub fn is_empty(&self) -> bool {
        self.file_path.as_os_str().is_empty()
    }
}

/// A single entry in the navigation history.
struct HistoryEntry {
    file_path: PathBuf,
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

    /// Record a navigation from `current_file` to a new file.
    fn push(&mut self, current_file: &PathBuf) {
        if current_file.as_os_str().is_empty() {
            return;
        }
        self.back_stack.push(HistoryEntry {
            file_path: current_file.clone(),
        });
        self.forward_stack.clear();
        if self.back_stack.len() > MAX_HISTORY {
            self.back_stack.remove(0);
        }
    }

    /// Go back. Returns the file path to navigate to.
    fn go_back(&mut self, current_file: &PathBuf) -> Option<PathBuf> {
        let entry = self.back_stack.pop()?;
        if !current_file.as_os_str().is_empty() {
            self.forward_stack.push(HistoryEntry {
                file_path: current_file.clone(),
            });
        }
        Some(entry.file_path)
    }

    /// Go forward. Returns the file path to navigate to.
    fn go_forward(&mut self, current_file: &PathBuf) -> Option<PathBuf> {
        let entry = self.forward_stack.pop()?;
        if !current_file.as_os_str().is_empty() {
            self.back_stack.push(HistoryEntry {
                file_path: current_file.clone(),
            });
        }
        Some(entry.file_path)
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
    project_path: PathBuf,
    /// Syntax set for highlighting
    syntax_set: SyntaxSet,
    /// File font size from settings
    file_font_size: f32,
    /// Measured monospace character width (from font metrics)
    measured_char_width: f32,
    /// Whether the current theme is dark (for syntax highlighting)
    is_dark: bool,
    /// All files in the project (from file search scan)
    files: Vec<FileEntry>,
    /// File tree for sidebar navigation
    file_tree: FileTreeNode,
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
}

impl FileViewer {
    /// Create a new file viewer for the given file path.
    pub fn new(
        file_path: PathBuf,
        project_path: PathBuf,
        font_size: f32,
        is_dark: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        // Scan project files and build tree
        let files = FileSearchDialog::scan_files(&project_path);
        let file_tree = build_file_tree(
            files
                .iter()
                .enumerate()
                .map(|(i, f)| (i, f.relative_path.as_str())),
        );
        let file_index = files.iter().position(|f| f.path == file_path);
        let expanded_folders = Self::compute_expanded_for_path(&file_path, &project_path);

        let syntax_set = load_syntax_set();
        let tab = FileViewerTab::new_with_file(file_path, file_index, &syntax_set, is_dark);

        Self {
            focus_handle,
            project_path,
            syntax_set,
            file_font_size: font_size,
            measured_char_width: font_size * 0.6,
            is_dark,
            files,
            file_tree,
            expanded_folders,
            tree_scroll_handle: ScrollHandle::new(),
            sidebar_visible: true,
            tabs: vec![tab],
            active_tab: 0,
            history: NavigationHistory::new(),
        }
    }

    /// Create a file viewer for browsing a project without a pre-selected file.
    ///
    /// Opens the sidebar file tree with no file loaded.
    pub fn new_browse(
        project_path: PathBuf,
        font_size: f32,
        is_dark: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        let files = FileSearchDialog::scan_files(&project_path);
        let file_tree = build_file_tree(
            files
                .iter()
                .enumerate()
                .map(|(i, f)| (i, f.relative_path.as_str())),
        );

        Self {
            focus_handle,
            project_path,
            syntax_set: load_syntax_set(),
            file_font_size: font_size,
            measured_char_width: font_size * 0.6,
            is_dark,
            files,
            file_tree,
            expanded_folders: HashSet::new(),
            tree_scroll_handle: ScrollHandle::new(),
            sidebar_visible: true,
            tabs: vec![FileViewerTab::new_empty()],
            active_tab: 0,
            history: NavigationHistory::new(),
        }
    }

    /// Update configuration (font size and dark mode) from the host app.
    pub fn update_config(&mut self, font_size: f32, is_dark: bool) {
        let rehighlight = is_dark != self.is_dark;
        self.file_font_size = font_size;
        self.is_dark = is_dark;
        if rehighlight {
            for tab in &mut self.tabs {
                if !tab.is_empty() {
                    tab.do_highlight_content(
                        &tab.file_path.clone(),
                        &self.syntax_set,
                        self.is_dark,
                    );
                }
            }
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
    pub fn open_file_in_tab(&mut self, file_path: PathBuf, cx: &mut Context<Self>) {
        // Already open? Switch to it.
        if let Some(idx) = self.tabs.iter().position(|t| t.file_path == file_path) {
            if idx != self.active_tab {
                let current_file = self.active_tab().file_path.clone();
                self.history.push(&current_file);
                self.active_tab = idx;
            }
            // Expand ancestors so sidebar highlights this file
            let expanded = Self::compute_expanded_for_path(&file_path, &self.project_path);
            self.expanded_folders.extend(expanded);
            cx.notify();
            return;
        }

        let file_index = self.files.iter().position(|f| f.path == file_path);
        let expanded = Self::compute_expanded_for_path(&file_path, &self.project_path);
        self.expanded_folders.extend(expanded);

        let new_tab =
            FileViewerTab::new_with_file(file_path, file_index, &self.syntax_set, self.is_dark);

        // If current tab is empty (no file loaded), replace it
        if self.active_tab().is_empty() {
            self.tabs[self.active_tab] = new_tab;
            cx.notify();
            return;
        }

        // Push history for the current file
        let current_file = self.active_tab().file_path.clone();
        self.history.push(&current_file);

        if self.tabs.len() >= MAX_TABS {
            // At limit: replace the active tab
            self.tabs[self.active_tab] = new_tab;
        } else {
            // Insert new tab after active
            let insert_at = self.active_tab + 1;
            self.tabs.insert(insert_at, new_tab);
            self.active_tab = insert_at;
        }

        cx.notify();
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

    /// Switch to a tab by index.
    pub(super) fn set_active_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.tabs.len() && index != self.active_tab {
            let current_file = self.active_tab().file_path.clone();
            self.history.push(&current_file);
            self.active_tab = index;
            // Update expanded folders to reveal active tab's file
            let expanded = Self::compute_expanded_for_path(
                &self.tabs[self.active_tab].file_path,
                &self.project_path,
            );
            self.expanded_folders.extend(expanded);
            cx.notify();
        }
    }

    /// Navigate back in history.
    pub(super) fn go_back(&mut self, cx: &mut Context<Self>) {
        let current_file = self.active_tab().file_path.clone();
        if let Some(target) = self.history.go_back(&current_file) {
            self.navigate_to_file_no_history(target, cx);
        }
    }

    /// Navigate forward in history.
    pub(super) fn go_forward(&mut self, cx: &mut Context<Self>) {
        let current_file = self.active_tab().file_path.clone();
        if let Some(target) = self.history.go_forward(&current_file) {
            self.navigate_to_file_no_history(target, cx);
        }
    }

    /// Navigate to a file without pushing history (used by back/forward).
    fn navigate_to_file_no_history(&mut self, file_path: PathBuf, cx: &mut Context<Self>) {
        // If file is open in a tab, switch to it
        if let Some(idx) = self.tabs.iter().position(|t| t.file_path == file_path) {
            self.active_tab = idx;
            cx.notify();
            return;
        }

        // Replace the current tab with a new one for the target file
        let file_index = self.files.iter().position(|f| f.path == file_path);
        let expanded = Self::compute_expanded_for_path(&file_path, &self.project_path);
        self.expanded_folders.extend(expanded);

        let new_tab =
            FileViewerTab::new_with_file(file_path, file_index, &self.syntax_set, self.is_dark);
        self.tabs[self.active_tab] = new_tab;
        cx.notify();
    }

    /// Compute which folder paths should be expanded to reveal a file.
    fn compute_expanded_for_path(file_path: &PathBuf, project_path: &PathBuf) -> HashSet<String> {
        let mut expanded = HashSet::new();
        if let Ok(relative) = file_path.strip_prefix(project_path) {
            let rel_str = relative.to_string_lossy();
            let parts: Vec<&str> = rel_str.split('/').collect();
            // Expand all ancestor directories (not the file itself)
            let mut path_so_far = String::new();
            for part in &parts[..parts.len().saturating_sub(1)] {
                if !path_so_far.is_empty() {
                    path_so_far.push('/');
                }
                path_so_far.push_str(part);
                expanded.insert(path_so_far.clone());
            }
        }
        expanded
    }
}

/// Events emitted by the file viewer.
#[derive(Clone, Debug)]
pub enum FileViewerEvent {
    /// Viewer was closed.
    Close,
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
    use std::path::PathBuf;

    use super::{FileViewer, NavigationHistory};

    #[::core::prelude::v1::test]
    fn test_compute_expanded_root_file() {
        let project = PathBuf::from("/projects/myapp");
        let file = PathBuf::from("/projects/myapp/README.md");
        let expanded = FileViewer::compute_expanded_for_path(&file, &project);
        assert!(expanded.is_empty());
    }

    #[::core::prelude::v1::test]
    fn test_compute_expanded_nested_file() {
        let project = PathBuf::from("/projects/myapp");
        let file = PathBuf::from("/projects/myapp/src/views/mod.rs");
        let expanded = FileViewer::compute_expanded_for_path(&file, &project);
        assert_eq!(expanded.len(), 2);
        assert!(expanded.contains("src"));
        assert!(expanded.contains("src/views"));
    }

    #[::core::prelude::v1::test]
    fn test_compute_expanded_outside_project() {
        let project = PathBuf::from("/projects/myapp");
        let file = PathBuf::from("/other/place/file.rs");
        let expanded = FileViewer::compute_expanded_for_path(&file, &project);
        assert!(expanded.is_empty());
    }

    #[::core::prelude::v1::test]
    fn test_history_back_forward() {
        let mut history = NavigationHistory::new();
        let a = PathBuf::from("/a.rs");
        let b = PathBuf::from("/b.rs");
        let c = PathBuf::from("/c.rs");

        // Navigate a -> b -> c
        history.push(&a);
        history.push(&b);

        assert!(history.can_go_back());
        assert!(!history.can_go_forward());

        // Go back from c
        let target = history.go_back(&c).unwrap();
        assert_eq!(target, b);
        assert!(history.can_go_forward());

        // Go back again
        let target = history.go_back(&b).unwrap();
        assert_eq!(target, a);

        // Go forward
        let target = history.go_forward(&a).unwrap();
        assert_eq!(target, b);

        let target = history.go_forward(&b).unwrap();
        assert_eq!(target, c);

        assert!(!history.can_go_forward());
    }

    #[::core::prelude::v1::test]
    fn test_history_new_navigation_clears_forward() {
        let mut history = NavigationHistory::new();
        let a = PathBuf::from("/a.rs");
        let b = PathBuf::from("/b.rs");
        let c = PathBuf::from("/c.rs");
        let d = PathBuf::from("/d.rs");

        history.push(&a);
        history.push(&b);

        // Go back from c to b
        history.go_back(&c);

        // New navigation from b
        history.push(&b);

        // Forward should be empty
        assert!(!history.can_go_forward());

        // Back should give b then a
        let target = history.go_back(&d).unwrap();
        assert_eq!(target, b);
        let target = history.go_back(&b).unwrap();
        assert_eq!(target, a);
    }

    #[::core::prelude::v1::test]
    fn test_history_limit() {
        let mut history = NavigationHistory::new();
        let current = PathBuf::from("/current.rs");

        for i in 0..60 {
            history.push(&PathBuf::from(format!("/file_{}.rs", i)));
        }

        assert_eq!(history.back_stack.len(), 50);

        // First entry should be file_10 (0-9 were trimmed)
        let mut target = history.go_back(&current).unwrap();
        assert_eq!(target, PathBuf::from("/file_59.rs"));

        // Drain remaining
        let mut count = 1;
        while let Some(t) = history.go_back(&target) {
            target = t;
            count += 1;
        }
        assert_eq!(count, 50);
    }
}
