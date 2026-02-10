//! File viewer overlay for displaying file contents with syntax highlighting.
//!
//! Provides a read-only view of files with syntax highlighting via syntect.
//! Markdown files can be viewed in rendered preview mode.

mod loading;
mod render;
mod selection;

use crate::settings::settings_entity;
use crate::ui::SelectionState;
use crate::views::components::{load_syntax_set, HighlightedLine, ScrollbarDrag};
use super::markdown_renderer::{MarkdownDocument, MarkdownSelection};
use super::markdown_renderer;
use gpui::*;
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

/// File viewer overlay for displaying file contents.
pub struct FileViewer {
    focus_handle: FocusHandle,
    file_path: PathBuf,
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
}

impl FileViewer {
    /// Create a new file viewer for the given file path.
    pub fn new(file_path: PathBuf, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let is_markdown = Self::is_markdown_file(&file_path);
        let file_font_size = settings_entity(cx).read(cx).settings.file_font_size;

        let mut viewer = Self {
            focus_handle,
            file_path: file_path.clone(),
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
        };

        // Load and highlight the file
        viewer.load_file(&file_path);

        viewer
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
