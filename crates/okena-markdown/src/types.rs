//! AST types and utility functions for the markdown renderer.

use okena_highlight::syntax::HighlightedSpan;

/// A node in the markdown AST.
#[derive(Clone)]
pub(crate) enum Node {
    Heading {
        level: u8,
        children: Vec<Inline>,
    },
    Paragraph {
        children: Vec<Inline>,
    },
    CodeBlock {
        language: Option<String>,
        code: String,
        /// Syntax-highlighted spans, one entry per line of `code`. Empty until
        /// `MarkdownDocument::highlight_code_blocks` runs — it needs the theme,
        /// which parsing does not have — and for languages syntect can't place.
        /// Each line's spans hold exactly the characters of that line, so
        /// selection offsets map onto them unchanged.
        highlighted: Vec<Vec<HighlightedSpan>>,
    },
    List {
        ordered: bool,
        items: Vec<Vec<Inline>>,
    },
    Table {
        headers: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
        /// Per-column display width (in characters), precomputed at parse time so
        /// rendering does not re-measure every cell on every frame.
        col_widths: Vec<usize>,
    },
    Blockquote {
        children: Vec<Inline>,
    },
    HorizontalRule,
    /// YAML frontmatter at the top of the document, rendered as a metadata card
    /// rather than fed to the markdown parser (where it degrades into a heading).
    /// `text_len` is the flat-text length (in chars), precomputed at parse time
    /// so selection-offset accounting does not rebuild the flat text on every
    /// frame — same rationale as `Table`'s `col_widths`.
    Frontmatter {
        block: Frontmatter,
        text_len: usize,
    },
}

/// Parsed YAML frontmatter block.
#[derive(Clone)]
pub(crate) enum Frontmatter {
    /// Well-formed mapping: ordered key/value pairs.
    Parsed(Vec<(String, FmValue)>),
    /// Content that isn't a YAML mapping (invalid, or a bare scalar/sequence).
    /// Shown verbatim so nothing is silently dropped.
    Raw(String),
}

/// A frontmatter value, mirrored from `serde_yaml_ng::Value` but order-preserving
/// and shaped for display (scalars flattened to strings).
#[derive(Clone)]
pub(crate) enum FmValue {
    Scalar(String),
    List(Vec<FmValue>),
    Map(Vec<(String, FmValue)>),
    /// null or an empty value.
    Empty,
}

impl FmValue {
    /// Convert a parsed YAML value into a display-oriented `FmValue`.
    pub(crate) fn from_yaml(value: serde_yaml_ng::Value) -> Self {
        use serde_yaml_ng::Value;
        match value {
            Value::Null => FmValue::Empty,
            Value::Bool(b) => FmValue::Scalar(b.to_string()),
            Value::Number(n) => FmValue::Scalar(n.to_string()),
            Value::String(s) if s.is_empty() => FmValue::Empty,
            Value::String(s) => FmValue::Scalar(s),
            Value::Sequence(seq) => {
                FmValue::List(seq.into_iter().map(FmValue::from_yaml).collect())
            }
            Value::Mapping(map) => FmValue::Map(
                map.into_iter()
                    .map(|(k, v)| (yaml_key_to_string(&k), FmValue::from_yaml(v)))
                    .collect(),
            ),
            Value::Tagged(tagged) => FmValue::from_yaml(tagged.value),
        }
    }
}

/// Stringify a YAML mapping key for display.
pub(crate) fn yaml_key_to_string(key: &serde_yaml_ng::Value) -> String {
    use serde_yaml_ng::Value;
    match key {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => String::new(),
        other => serde_yaml_ng::to_string(other)
            .ok()
            .map(|s| s.trim().to_string())
            .unwrap_or_default(),
    }
}

/// Flat-text representation of a frontmatter block. Used both for selection
/// offset accounting and copy output, and mirrors the on-screen layout so a
/// copied selection matches what is shown.
pub(crate) fn frontmatter_flat_text(fm: &Frontmatter) -> String {
    let mut out = String::new();
    match fm {
        Frontmatter::Raw(raw) => {
            for line in raw.lines() {
                out.push_str(line);
                out.push('\n');
            }
        }
        Frontmatter::Parsed(entries) => fm_entries_to_text(entries, 0, &mut out),
    }
    out
}

fn fm_indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn fm_entries_to_text(entries: &[(String, FmValue)], depth: usize, out: &mut String) {
    for (key, value) in entries {
        fm_indent(out, depth);
        out.push_str(key);
        match value {
            FmValue::Scalar(s) => {
                out.push_str(": ");
                out.push_str(s);
                out.push('\n');
            }
            FmValue::Empty => {
                out.push_str(":\n");
            }
            FmValue::List(items) => {
                out.push_str(":\n");
                fm_list_to_text(items, depth + 1, out);
            }
            FmValue::Map(sub) => {
                out.push_str(":\n");
                fm_entries_to_text(sub, depth + 1, out);
            }
        }
    }
}

fn fm_list_to_text(items: &[FmValue], depth: usize, out: &mut String) {
    for item in items {
        fm_indent(out, depth);
        out.push_str("\u{2022} ");
        match item {
            FmValue::Scalar(s) => {
                out.push_str(s);
                out.push('\n');
            }
            FmValue::Empty => {
                out.push('\n');
            }
            FmValue::List(inner) => {
                out.push('\n');
                fm_list_to_text(inner, depth + 1, out);
            }
            FmValue::Map(sub) => {
                out.push('\n');
                fm_entries_to_text(sub, depth + 1, out);
            }
        }
    }
}

/// Inline content within a block.
#[derive(Clone)]
pub(crate) enum Inline {
    Text(String),
    Code(String),
    Bold(Vec<Inline>),
    Italic(Vec<Inline>),
    Link { _url: String, children: Vec<Inline> },
}

/// Get character count of a string (not byte count).
pub(crate) fn char_len(s: &str) -> usize {
    s.chars().count()
}
