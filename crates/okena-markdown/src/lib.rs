#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

//! Markdown renderer for GPUI.
//!
//! Parses markdown content and renders it as GPUI elements.

mod parser;
mod render;
mod style;
mod types;

use gpui::*;

use okena_core::selection::SelectionState;
use types::Node;

pub use style::DOC_MAX_WIDTH;

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
    /// Cumulative start offset (in characters) of each node, parallel to `nodes`.
    /// Precomputed at parse time so rendering does not re-walk node text lengths.
    node_offsets: Vec<usize>,
    /// Flat text representation of all visible content
    pub plain_text: String,
}

impl MarkdownDocument {
    /// Syntax-highlight every fenced code block for the given theme.
    ///
    /// Kept out of `parse` because the colours come from the syntax theme, which
    /// the parser has no business knowing: the viewer calls this after parsing
    /// and again whenever the theme flips between dark and light. A document
    /// that never gets the call renders its code blocks in the document text
    /// colour, exactly as before.
    pub fn highlight_code_blocks(&mut self, is_dark: bool) {
        for node in &mut self.nodes {
            if let Node::CodeBlock {
                language,
                code,
                highlighted,
            } = node
            {
                *highlighted = okena_highlight::syntax::highlight_code_block(
                    code,
                    language.as_deref(),
                    is_dark,
                )
                .into_iter()
                .map(|line| line.spans)
                .collect();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Named imports, not a glob: `use super::*` would pull in gpui's own `test`
    // macro and shadow the one these tests need.
    use super::{MarkdownDocument, Node};

    /// The spans of a line must hold exactly that line's characters — tabs and
    /// all. Rendering maps a character-offset selection onto them, so a span set
    /// that drops or rewrites characters silently shifts the selection.
    #[test]
    fn spans_reproduce_each_line_verbatim() {
        let mut doc = MarkdownDocument::parse("```rust\nfn main() {\n\tlet x = 1;\n}\n```\n");
        doc.highlight_code_blocks(true);

        let Some(Node::CodeBlock {
            code, highlighted, ..
        }) = doc.nodes.first()
        else {
            panic!("expected a code block");
        };

        let lines: Vec<&str> = code.lines().collect();
        assert_eq!(highlighted.len(), lines.len());
        for (spans, line) in highlighted.iter().zip(&lines) {
            let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
            assert_eq!(&joined, line);
        }
        // More than one colour, or nothing was actually highlighted.
        let colors: Vec<_> = highlighted[0].iter().map(|s| s.color.r).collect();
        assert!(colors.len() > 1, "expected `fn main() {{` to be coloured");
    }

    /// A fence with no language, or one syntect cannot place, is left alone so
    /// it keeps the document's text colour instead of the syntax theme's.
    #[test]
    fn unknown_and_missing_languages_stay_unhighlighted() {
        for md in [
            "```\nplain text\n```\n",
            "```notalanguage\nplain text\n```\n",
        ] {
            let mut doc = MarkdownDocument::parse(md);
            doc.highlight_code_blocks(true);

            let Some(Node::CodeBlock { highlighted, .. }) = doc.nodes.first() else {
                panic!("expected a code block");
            };
            assert!(highlighted.is_empty(), "{md:?} should not be highlighted");
        }
    }
}
