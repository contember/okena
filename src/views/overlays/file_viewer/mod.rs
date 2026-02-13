//! File viewer overlay for displaying file contents with syntax highlighting.
//!
//! Provides a read-only view of files with syntax highlighting via syntect.
//! Markdown files can be viewed in rendered preview mode.

mod loading;
mod render;
mod selection;

use crate::settings::settings_entity;
use crate::theme::{theme, theme_entity};
use crate::ui::SelectionState;
use crate::views::components::{build_file_tree, load_syntax_set, FileTreeNode, HighlightedLine, ScrollbarDrag};
use crate::views::overlays::file_search::{FileSearchDialog, FileEntry};
use super::markdown_renderer::{MarkdownDocument, MarkdownSelection};
use super::markdown_renderer;
use gpui::*;
use std::collections::HashSet;
use std::path::PathBuf;
use syntect::parsing::SyntaxSet;

/// Maximum file size to load (5MB)
const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024;

/// Maximum number of lines to display
const MAX_LINES: usize = 10000;

/// Display mode for file viewer.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum DisplayMode {
    #[default]
    Source,
    Preview,
}

/// Type alias for source view selection (line, column).
type Selection = SelectionState<(usize, usize)>;

/// Width of file tree sidebar.
const SIDEBAR_WIDTH: f32 = 240.0;

/// File viewer overlay for displaying file contents.
pub struct FileViewer {
    focus_handle: FocusHandle,
    file_path: PathBuf,
    project_path: PathBuf,
    content: String,
    highlighted_lines: Vec<HighlightedLine>,
    line_count: usize,
    line_num_width: usize,
    error_message: Option<String>,
    selection: Selection,
    /// Current display mode (source or preview)
    display_mode: DisplayMode,
    /// Whether the file is a markdown file
    is_markdown: bool,
    /// Parsed markdown document for preview mode
    markdown_doc: Option<MarkdownDocument>,
    /// Selection state for markdown preview mode
    markdown_selection: MarkdownSelection,
    /// Scroll handle for markdown preview (to track scroll offset)
    markdown_scroll_handle: ScrollHandle,
    /// Scroll handle for virtualized source view
    source_scroll_handle: UniformListScrollHandle,
    /// Scrollbar drag state
    scrollbar_drag: Option<ScrollbarDrag>,
    /// Syntax set for highlighting
    syntax_set: SyntaxSet,
    /// File font size from settings
    file_font_size: f32,
    /// Measured monospace character width (from font metrics)
    measured_char_width: f32,
    /// All files in the project (from file search scan)
    files: Vec<FileEntry>,
    /// File tree for sidebar navigation
    file_tree: FileTreeNode,
    /// Which folder paths are currently expanded
    expanded_folders: HashSet<String>,
    /// Index of the currently selected file in `files`
    selected_file_index: Option<usize>,
    /// Scroll handle for the file tree sidebar
    tree_scroll_handle: ScrollHandle,
    /// Whether the sidebar is visible
    sidebar_visible: bool,
    /// Whether the current theme is dark (for syntax highlighting)
    is_dark: bool,
}

impl FileViewer {
    /// Create a new file viewer for the given file path.
    pub fn new(file_path: PathBuf, project_path: PathBuf, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let is_markdown = Self::is_markdown_file(&file_path);
        let file_font_size = settings_entity(cx).read(cx).settings.file_font_size;
        let is_dark = theme(cx).is_dark();

        // Re-highlight when theme changes
        let theme_entity = theme_entity(cx);
        cx.observe(&theme_entity, |this: &mut Self, _, cx| {
            let new_is_dark = theme(cx).is_dark();
            if new_is_dark != this.is_dark {
                this.is_dark = new_is_dark;
                this.do_highlight_content(&this.file_path.clone());
                cx.notify();
            }
        }).detach();

        // Scan project files and build tree
        let files = FileSearchDialog::scan_files(&project_path);
        let file_tree = build_file_tree(
            files.iter().enumerate().map(|(i, f)| (i, f.relative_path.as_str()))
        );
        let selected_file_index = files.iter().position(|f| f.path == file_path);
        let expanded_folders = Self::compute_expanded_for_path(&file_path, &project_path);

        let mut viewer = Self {
            focus_handle,
            file_path: file_path.clone(),
            project_path,
            content: String::new(),
            highlighted_lines: Vec::new(),
            line_count: 0,
            line_num_width: 3,
            error_message: None,
            selection: Selection::default(),
            display_mode: if is_markdown { DisplayMode::Preview } else { DisplayMode::Source },
            is_markdown,
            markdown_doc: None,
            markdown_selection: MarkdownSelection::default(),
            markdown_scroll_handle: ScrollHandle::new(),
            source_scroll_handle: UniformListScrollHandle::new(),
            scrollbar_drag: None,
            syntax_set: load_syntax_set(),
            file_font_size,
            measured_char_width: file_font_size * 0.6,
            files,
            file_tree,
            expanded_folders,
            selected_file_index,
            tree_scroll_handle: ScrollHandle::new(),
            sidebar_visible: false,
            is_dark,
        };

        // Load and highlight the file
        viewer.load_file(&file_path);

        viewer
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

impl_focusable!(FileViewer);

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::FileViewer;

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
}
