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

/// A laid-out text run and its character offset in [`MarkdownDocument::plain_text`].
///
/// GPUI reports hit-test positions as UTF-8 byte offsets. Markdown selection is
/// character-based, so this type owns the conversion at the rendering boundary.
#[derive(Clone)]
pub struct MarkdownTextRun {
    layout: TextLayout,
    text: SharedString,
    start_offset: usize,
}

impl MarkdownTextRun {
    pub(crate) fn new(
        layout: TextLayout,
        text: impl Into<SharedString>,
        start_offset: usize,
    ) -> Self {
        Self {
            layout,
            text: text.into(),
            start_offset,
        }
    }

    /// Map a window position to a document character offset.
    ///
    /// `Err` carries the closest offset when the position is outside this run,
    /// mirroring [`TextLayout::index_for_position`].
    pub fn index_for_position(&self, position: Point<Pixels>) -> Result<usize, usize> {
        match self.layout.index_for_position(position) {
            Ok(byte_offset) => Ok(self.document_offset(byte_offset)),
            Err(byte_offset) => Err(self.document_offset(byte_offset)),
        }
    }

    /// Bounds populated by GPUI after the run has been laid out.
    pub fn bounds(&self) -> Bounds<Pixels> {
        self.layout.bounds()
    }

    fn document_offset(&self, byte_offset: usize) -> usize {
        let byte_offset = byte_offset.min(self.text.len());
        let char_offset = self
            .text
            .char_indices()
            .take_while(|(index, _)| *index < byte_offset)
            .count();
        self.start_offset + char_offset
    }
}

/// One independently laid-out markdown unit: a block, code line, or table row.
pub struct RenderedTextUnit {
    pub div: Div,
    pub start_offset: usize,
    pub end_offset: usize,
    pub text_runs: Vec<MarkdownTextRun>,
}

/// A rendered node that can be either a simple block or a code block with selectable lines.
pub enum RenderedNode {
    /// A simple block (heading, paragraph, list, etc.) - single selectable unit
    Simple {
        div: Div,
        start_offset: usize,
        end_offset: usize,
        text_runs: Vec<MarkdownTextRun>,
    },
    /// A code block with individually selectable lines
    CodeBlock {
        language: Option<String>,
        lines: Vec<RenderedTextUnit>,
    },
    /// A table with individually selectable rows
    Table {
        header: Option<RenderedTextUnit>,
        rows: Vec<RenderedTextUnit>,
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
