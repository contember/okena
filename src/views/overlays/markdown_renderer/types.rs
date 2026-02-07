//! AST types and utility functions for the markdown renderer.

/// A node in the markdown AST.
#[derive(Clone)]
pub(super) enum Node {
    Heading { level: u8, children: Vec<Inline> },
    Paragraph { children: Vec<Inline> },
    CodeBlock { language: Option<String>, code: String },
    List { ordered: bool, items: Vec<Vec<Inline>> },
    Table { headers: Vec<Vec<Inline>>, rows: Vec<Vec<Vec<Inline>>> },
    Blockquote { children: Vec<Inline> },
    HorizontalRule,
}

/// Inline content within a block.
#[derive(Clone)]
pub(super) enum Inline {
    Text(String),
    Code(String),
    Bold(Vec<Inline>),
    Italic(Vec<Inline>),
    Link { #[allow(dead_code)] url: String, children: Vec<Inline> },
}

/// Slice a string by character indices (not byte indices).
/// Returns (before, selected, after) parts.
pub(super) fn slice_by_chars(s: &str, start: usize, end: usize) -> (String, String, String) {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let start = start.min(len);
    let end = end.min(len);

    let before: String = chars[..start].iter().collect();
    let selected: String = chars[start..end].iter().collect();
    let after: String = chars[end..].iter().collect();

    (before, selected, after)
}

/// Get character count of a string (not byte count).
pub(super) fn char_len(s: &str) -> usize {
    s.chars().count()
}
