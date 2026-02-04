//! Data types for the diff viewer.

use crate::git::DiffLineType;
use gpui::Rgba;
use std::collections::BTreeMap;

/// A highlighted span with color.
#[derive(Clone)]
pub struct HighlightedSpan {
    pub color: Rgba,
    pub text: String,
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

/// Processed file for display.
pub struct DiffDisplayFile {
    /// Full path.
    pub path: String,
    /// Lines added count.
    pub added: usize,
    /// Lines removed count.
    pub removed: usize,
    /// Processed lines for display.
    pub lines: Vec<DisplayLine>,
    /// Whether this is a binary file.
    pub is_binary: bool,
    /// Whether this is a new file.
    pub is_new: bool,
    /// Whether this is a deleted file.
    pub is_deleted: bool,
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
