//! Data types for the diff viewer.

use crate::git::{DiffLineType, FileDiff};
pub use crate::views::components::syntax::HighlightedSpan;
use std::collections::BTreeMap;

pub use crate::workspace::persistence::DiffViewMode;

/// Lightweight file stats for sidebar display (no syntax highlighting).
pub struct FileStats {
    pub path: String,
    pub added: usize,
    pub removed: usize,
    pub is_binary: bool,
    pub is_new: bool,
    pub is_deleted: bool,
}

impl From<&FileDiff> for FileStats {
    fn from(file: &FileDiff) -> Self {
        Self {
            path: file.display_name().to_string(),
            added: file.lines_added,
            removed: file.lines_removed,
            is_binary: file.is_binary,
            is_new: file.old_path.is_none(),
            is_deleted: file.new_path.is_none(),
        }
    }
}

/// A range of characters that changed within a line.
#[derive(Clone, Debug)]
pub struct ChangedRange {
    /// Start column (character index).
    pub start: usize,
    /// End column (exclusive).
    pub end: usize,
}

/// Content for one side of a side-by-side line.
#[derive(Clone)]
pub struct SideContent {
    pub line_num: usize,
    pub line_type: DiffLineType,
    pub spans: Vec<HighlightedSpan>,
    /// Plain text content (for selection/copy - future use).
    #[allow(dead_code)]
    pub plain_text: String,
    /// Ranges of characters that actually changed (for word-level highlighting).
    pub changed_ranges: Vec<ChangedRange>,
}

/// A paired line for side-by-side view.
#[derive(Clone)]
pub struct SideBySideLine {
    pub left: Option<SideContent>,
    pub right: Option<SideContent>,
    pub is_header: bool,
    /// Header text content (for copy - future use).
    #[allow(dead_code)]
    pub header_text: String,
    pub header_spans: Vec<HighlightedSpan>,
}

/// A processed line ready for display with syntax highlighting.
#[derive(Clone)]
pub struct DisplayLine {
    /// Type of the line.
    pub line_type: DiffLineType,
    /// Old line number (for display).
    pub old_line_num: Option<usize>,
    /// New line number (for display).
    pub new_line_num: Option<usize>,
    /// Highlighted spans for display.
    pub spans: Vec<HighlightedSpan>,
    /// Plain text content (for selection/copy).
    pub plain_text: String,
}

/// Processed file for display (lines with syntax highlighting).
pub struct DiffDisplayFile {
    /// Processed lines for display.
    pub lines: Vec<DisplayLine>,
}

/// A node in the file tree.
#[derive(Default, Clone)]
pub struct FileTreeNode {
    /// Files at this level (index into files vec).
    pub files: Vec<usize>,
    /// Subdirectories.
    pub children: BTreeMap<String, FileTreeNode>,
}

/// State for scrollbar dragging.
#[derive(Clone, Copy)]
pub struct ScrollbarDrag {
    /// Initial mouse Y position.
    pub start_y: f32,
    /// Initial scroll offset.
    pub start_scroll_y: f32,
}
