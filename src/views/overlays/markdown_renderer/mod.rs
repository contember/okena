//! Markdown renderer for GPUI.
//!
//! Parses markdown content and renders it as GPUI elements.

mod parser;
mod render;
mod types;

use gpui::*;

use crate::ui::SelectionState;
use types::Node;

/// Type alias for markdown selection (1D character offset).
pub type MarkdownSelection = SelectionState<usize>;

/// A rendered node that can be either a simple block or a code block with selectable lines.
pub enum RenderedNode {
    /// A simple block (heading, paragraph, list, etc.) - single selectable unit
    Simple {
        div: Div,
        start_offset: usize,
        end_offset: usize,
    },
    /// A code block with individually selectable lines
    CodeBlock {
        language: Option<String>,
        /// Each line as (div, start_offset, end_offset)
        lines: Vec<(Div, usize, usize)>,
    },
    /// A table with individually selectable rows
    Table {
        /// Header row (div, start_offset, end_offset)
        header: Option<(Div, usize, usize)>,
        /// Data rows as (div, start_offset, end_offset)
        rows: Vec<(Div, usize, usize)>,
    },
}

/// Parsed markdown document ready for rendering.
pub struct MarkdownDocument {
    nodes: Vec<Node>,
    /// Flat text representation of all visible content
    pub plain_text: String,
}
