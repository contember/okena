//! Fast Markdown/MDX syntax highlighting backed by tree-sitter.
//!
//! syntect's Sublime Markdown grammar is pathologically slow — on the order of
//! ~1 ms per line (≈20× slower than e.g. the Rust grammar on the same bytes,
//! ≈2000× slower than plain text), and the diff viewer highlights the full old
//! *and* new file on every selection. A 700-line `.mdx` therefore costs well
//! over a second just to colour, regardless of how small the diff is.
//!
//! tree-sitter parses the whole document in ~30 ms instead, and — because it
//! parses the entire file cheaply — it also sidesteps syntect's "carry parse
//! state to a mid-file hunk" problem entirely.
//!
//! Strategy (hybrid): tree-sitter handles the Markdown *structure* (headings,
//! emphasis, links, inline code, fence delimiters), while fenced code blocks
//! keep being highlighted by the existing syntect path for their embedded
//! language (graphql/json/ts/…). Those grammars are fast in syntect; only
//! Markdown itself was the problem. The output mirrors [`crate::syntax`]'s
//! per-line `HashMap<line, Vec<HighlightedSpan>>` so the diff viewer is unchanged.

use crate::syntax::{
    HighlightedLine, HighlightedSpan, default_text_color, highlight_line, load_syntax_theme,
    map_extension_to_syntax,
};
use gpui::Rgba;
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::OnceLock;
use streaming_iterator::StreamingIterator;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color, Highlighter, Theme};
use syntect::parsing::{ScopeStack, SyntaxSet};
use syntect::util::LinesWithEndings;
use tree_sitter::{Language, Query, QueryCursor};
use tree_sitter_md::{
    HIGHLIGHT_QUERY_BLOCK, HIGHLIGHT_QUERY_INLINE, INLINE_LANGUAGE, LANGUAGE, MarkdownParser,
};

/// Captures only the fenced code content plus its declared language, so the
/// embedded language can be handed to syntect.
const FENCED_CODE_QUERY: &str =
    "(fenced_code_block (info_string (language) @language) (code_fence_content) @content)";

/// Whether a path should be highlighted as Markdown/MDX.
pub fn is_markdown_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("md" | "mdx" | "markdown")
    )
}

/// Pre-highlight a full Markdown/MDX file for the **diff viewer**, returning
/// 1-based line number -> spans (same shape and line numbering as
/// [`crate::syntax`]'s syntect path). All lines are highlighted, since diff
/// line lookups are by arbitrary line number.
pub fn highlight_markdown_file(
    content: &str,
    syntax_set: &SyntaxSet,
    is_dark: bool,
) -> HashMap<usize, Vec<HighlightedSpan>> {
    markdown_line_spans(content, syntax_set, is_dark, 0)
        .into_iter()
        .enumerate()
        .map(|(idx, spans)| (idx + 1, spans))
        .collect()
}

/// Pre-highlight Markdown/MDX for the **file viewer / content-search preview**,
/// returning ordered [`HighlightedLine`]s (spans + plain text), capped at
/// `max_lines` (0 = unlimited). Same output shape as
/// [`crate::syntax::highlight_content`].
pub fn highlight_markdown_content(
    content: &str,
    syntax_set: &SyntaxSet,
    max_lines: usize,
    is_dark: bool,
) -> Vec<HighlightedLine> {
    markdown_line_spans(content, syntax_set, is_dark, max_lines)
        .into_iter()
        .map(|spans| {
            let plain_text = spans.iter().map(|s| s.text.as_str()).collect();
            HighlightedLine { spans, plain_text }
        })
        .collect()
}

/// Shared core: produce per-line spans in document order. `max_lines == 0`
/// means all lines. The whole document is always parsed (tree-sitter is cheap
/// and parse state is global); only the emitted line count is capped.
fn markdown_line_spans(
    content: &str,
    syntax_set: &SyntaxSet,
    is_dark: bool,
    max_lines: usize,
) -> Vec<Vec<HighlightedSpan>> {
    let default = default_text_color(is_dark);

    // Parsing can in principle fail (it cannot in practice for these grammars);
    // degrade to uncoloured text rather than panicking.
    let Some(tree) = MarkdownParser::default().parse(content.as_bytes(), None) else {
        return plain_line_spans(content, default, max_lines);
    };
    let src = content.as_bytes();

    // Markdown colours are derived from the active syntect theme so they match
    // how that theme renders Markdown elsewhere; captures the theme has no rule
    // for fall back to a hand-picked palette.
    let palette = MarkdownPalette::from_theme(load_syntax_theme(is_dark), is_dark);

    // 1. Collect Markdown structural captures from the block tree and every
    //    inline sub-tree. Inline trees use document-absolute byte offsets.
    let mut caps: Vec<(usize, usize, Rgba)> = Vec::new();
    if let Some(q) = block_query() {
        collect_captures(q, tree.block_tree().root_node(), src, &palette, &mut caps);
    }
    if let Some(q) = inline_query() {
        for inline in tree.inline_trees() {
            collect_captures(q, inline.root_node(), src, &palette, &mut caps);
        }
    }

    // 2. Lay captures onto a per-byte colour buffer. Larger ranges are applied
    //    first so smaller, more specific captures (e.g. emphasis inside a
    //    heading) win.
    caps.sort_by_key(|&(start, end, _)| std::cmp::Reverse(end - start));
    let mut colors: Vec<Option<Rgba>> = vec![None; content.len()];
    for (start, end, color) in caps {
        if let Some(slice) = colors.get_mut(start..end.min(content.len())) {
            for slot in slice {
                *slot = Some(color);
            }
        }
    }

    // 3. Fenced code blocks: highlight their content with the embedded
    //    language via the existing (fast) syntect path, keyed by line number.
    let line_starts = build_line_starts(content);
    let mut code_lines: HashMap<usize, Vec<HighlightedSpan>> = HashMap::new();
    if let Some(q) = fenced_code_query() {
        collect_code_blocks(
            q,
            tree.block_tree().root_node(),
            content,
            &line_starts,
            syntax_set,
            is_dark,
            &mut code_lines,
        );
    }

    // 4. Assemble per-line spans. Code-content lines use their syntect spans;
    //    everything else is built from the colour buffer.
    let mut result = Vec::with_capacity(line_starts.len());
    let mut offset = 0usize;
    for (idx, line) in LinesWithEndings::from(content).enumerate() {
        if max_lines > 0 && idx >= max_lines {
            break;
        }
        if let Some(spans) = code_lines.remove(&(idx + 1)) {
            result.push(spans);
        } else {
            result.push(line_spans(line, offset, &colors, default));
        }
        offset += line.len();
    }
    result
}

/// Run a highlight query over a tree and push coloured byte ranges.
fn collect_captures(
    query: &Query,
    root: tree_sitter::Node,
    src: &[u8],
    palette: &MarkdownPalette,
    out: &mut Vec<(usize, usize, Rgba)>,
) {
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, src);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            if let Some(color) = names
                .get(cap.index as usize)
                .and_then(|name| palette.color(name))
            {
                let range = cap.node.byte_range();
                out.push((range.start, range.end, color));
            }
        }
    }
}

/// Highlight each fenced code block's content with its embedded language.
fn collect_code_blocks(
    query: &Query,
    root: tree_sitter::Node,
    content: &str,
    line_starts: &[usize],
    syntax_set: &SyntaxSet,
    is_dark: bool,
    out: &mut HashMap<usize, Vec<HighlightedSpan>>,
) {
    let names = query.capture_names();
    let theme = load_syntax_theme(is_dark);
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, content.as_bytes());
    while let Some(m) = matches.next() {
        let mut lang = "";
        let mut content_range: Option<(usize, usize)> = None;
        for cap in m.captures {
            match names.get(cap.index as usize).copied() {
                Some("language") => {
                    let r = cap.node.byte_range();
                    lang = content.get(r.start..r.end).unwrap_or("");
                }
                Some("content") => {
                    let r = cap.node.byte_range();
                    content_range = Some((r.start, r.end));
                }
                _ => {}
            }
        }
        let Some((start, end)) = content_range else {
            continue;
        };
        let Some(code) = content.get(start..end.min(content.len())) else {
            continue;
        };
        let syntax = syntax_for_lang(lang, syntax_set);
        let mut highlighter = HighlightLines::new(syntax, theme);
        let first_line = line_of(line_starts, start);
        for (i, line) in LinesWithEndings::from(code).enumerate() {
            let spans = highlight_line(line, &mut highlighter, syntax_set, is_dark);
            out.insert(first_line + i, spans);
        }
    }
}

/// Resolve a fenced-code info-string language to a syntect syntax, falling back
/// to plain text. Mirrors [`crate::syntax::get_syntax_for_path`]'s mapping.
fn syntax_for_lang<'a>(
    lang: &str,
    syntax_set: &'a SyntaxSet,
) -> &'a syntect::parsing::SyntaxReference {
    map_extension_to_syntax(lang)
        .and_then(|mapped| syntax_set.find_syntax_by_extension(mapped))
        .or_else(|| syntax_set.find_syntax_by_token(lang))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text())
}

/// Build spans for one (non-code) line from the per-byte colour buffer,
/// coalescing equal-coloured runs, expanding tabs, and dropping line endings.
fn line_spans(line: &str, base: usize, colors: &[Option<Rgba>], default: Rgba) -> Vec<HighlightedSpan> {
    let mut spans: Vec<HighlightedSpan> = Vec::new();
    for (i, ch) in line.char_indices() {
        if ch == '\n' || ch == '\r' {
            continue;
        }
        let color = colors.get(base + i).copied().flatten().unwrap_or(default);
        match spans.last_mut() {
            Some(last) if color_eq(last.color, color) => push_char(&mut last.text, ch),
            _ => {
                let mut text = String::new();
                push_char(&mut text, ch);
                spans.push(HighlightedSpan { color, text });
            }
        }
    }
    spans
}

fn push_char(buf: &mut String, ch: char) {
    if ch == '\t' {
        buf.push_str("    ");
    } else {
        buf.push(ch);
    }
}

fn color_eq(a: Rgba, b: Rgba) -> bool {
    a.r == b.r && a.g == b.g && a.b == b.b && a.a == b.a
}

/// Fallback when parsing fails: one default-coloured span per non-empty line.
fn plain_line_spans(content: &str, default: Rgba, max_lines: usize) -> Vec<Vec<HighlightedSpan>> {
    let mut result = Vec::new();
    for (idx, line) in LinesWithEndings::from(content).enumerate() {
        if max_lines > 0 && idx >= max_lines {
            break;
        }
        let text = line.trim_end_matches(['\n', '\r']).replace('\t', "    ");
        let spans = if text.is_empty() {
            Vec::new()
        } else {
            vec![HighlightedSpan { color: default, text }]
        };
        result.push(spans);
    }
    result
}

/// Byte offsets of each line start (line 1 starts at byte 0).
fn build_line_starts(content: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in content.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// 1-based line number containing the given byte offset.
fn line_of(line_starts: &[usize], byte: usize) -> usize {
    line_starts.partition_point(|&s| s <= byte).max(1)
}

/// Markdown element colours, resolved from the active syntect theme with
/// hand-picked fallbacks for elements the theme defines no rule for.
struct MarkdownPalette {
    title: Rgba,
    strong: Rgba,
    emphasis: Rgba,
    literal: Rgba,
    uri: Rgba,
    reference: Rgba,
    punctuation: Rgba,
    escape: Rgba,
}

impl MarkdownPalette {
    fn from_theme(theme: &Theme, is_dark: bool) -> Self {
        let highlighter = Highlighter::new(theme);
        let default_fg = theme.settings.foreground;
        // Each Markdown element maps to the TextMate scope a Sublime/TextMate
        // grammar would assign it; the theme's rule for that scope (if any)
        // wins, otherwise the fallback hex keeps elements visually distinct.
        let pick = |scope: &str, fallback: u32| {
            resolve_scope(&highlighter, default_fg, scope).unwrap_or_else(|| rgb(fallback))
        };
        if is_dark {
            // Fallbacks: Dracula palette.
            Self {
                title: pick("markup.heading.markdown", 0xbd93f9),
                strong: pick("markup.bold.markdown", 0xffb86c),
                emphasis: pick("markup.italic.markdown", 0xf1fa8c),
                literal: pick("markup.raw.inline.markdown", 0x50fa7b),
                uri: pick("markup.underline.link.markdown", 0x8be9fd),
                reference: pick("string.other.link.title.markdown", 0xff79c6),
                punctuation: pick("punctuation.definition.markdown", 0x6272a4),
                escape: pick("constant.character.escape.markdown", 0xffb86c),
            }
        } else {
            // Fallbacks: GitHub palette.
            Self {
                title: pick("markup.heading.markdown", 0x6f42c1),
                strong: pick("markup.bold.markdown", 0xb31d28),
                emphasis: pick("markup.italic.markdown", 0xe36209),
                literal: pick("markup.raw.inline.markdown", 0x22863a),
                uri: pick("markup.underline.link.markdown", 0x032f62),
                reference: pick("string.other.link.title.markdown", 0x005cc5),
                punctuation: pick("punctuation.definition.markdown", 0x6a737d),
                escape: pick("constant.character.escape.markdown", 0xe36209),
            }
        }
    }

    /// Colour for a tree-sitter Markdown capture, or `None` for captures that
    /// should use the default text colour (e.g. `@none`).
    fn color(&self, capture: &str) -> Option<Rgba> {
        Some(match capture {
            "text.title" => self.title,
            "text.strong" => self.strong,
            "text.emphasis" => self.emphasis,
            "text.literal" => self.literal,
            "text.uri" => self.uri,
            "text.reference" => self.reference,
            "punctuation.special" | "punctuation.delimiter" => self.punctuation,
            "string.escape" => self.escape,
            _ => return None,
        })
    }
}

/// Resolve a TextMate scope through a theme. Returns `None` when the theme has
/// no specific rule (the highlighter then yields the default text colour), so
/// the caller can fall back.
fn resolve_scope(highlighter: &Highlighter, default_fg: Option<Color>, scope: &str) -> Option<Rgba> {
    let stack = ScopeStack::from_str(scope).ok()?;
    let fg = highlighter.style_for_stack(stack.as_slice()).foreground;
    if let Some(d) = default_fg
        && fg.r == d.r
        && fg.g == d.g
        && fg.b == d.b
    {
        return None;
    }
    Some(color_to_rgba(fg))
}

fn color_to_rgba(c: Color) -> Rgba {
    Rgba {
        r: c.r as f32 / 255.0,
        g: c.g as f32 / 255.0,
        b: c.b as f32 / 255.0,
        a: c.a as f32 / 255.0,
    }
}

fn rgb(hex: u32) -> Rgba {
    Rgba {
        r: ((hex >> 16) & 0xff) as f32 / 255.0,
        g: ((hex >> 8) & 0xff) as f32 / 255.0,
        b: (hex & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

fn block_query() -> Option<&'static Query> {
    static Q: OnceLock<Option<Query>> = OnceLock::new();
    Q.get_or_init(|| Query::new(&Language::new(LANGUAGE), HIGHLIGHT_QUERY_BLOCK).ok())
        .as_ref()
}

fn inline_query() -> Option<&'static Query> {
    static Q: OnceLock<Option<Query>> = OnceLock::new();
    Q.get_or_init(|| Query::new(&Language::new(INLINE_LANGUAGE), HIGHLIGHT_QUERY_INLINE).ok())
        .as_ref()
}

fn fenced_code_query() -> Option<&'static Query> {
    static Q: OnceLock<Option<Query>> = OnceLock::new();
    Q.get_or_init(|| Query::new(&Language::new(LANGUAGE), FENCED_CODE_QUERY).ok())
        .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::load_syntax_set;

    fn spans_text(spans: &[HighlightedSpan]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn highlights_heading_and_reconstructs_text() {
        let set = load_syntax_set();
        let content = "# Title\n\nSome **bold** text.\n";
        let map = highlight_markdown_file(content, &set, true);

        // Source text is preserved verbatim (this is a source diff, not rendered
        // Markdown), including the literal `#` and `**` markers, sans line endings.
        assert_eq!(spans_text(&map[&1]), "# Title");
        assert_eq!(spans_text(&map[&3]), "Some **bold** text.");

        // The heading body is coloured as a title (not default text).
        let default = default_text_color(true);
        let title_colored = map[&1].iter().any(|s| !color_eq(s.color, default));
        assert!(title_colored, "heading should be coloured");
    }

    #[test]
    fn fenced_code_uses_embedded_language() {
        let set = load_syntax_set();
        let content = "Intro\n\n```json\n{\"a\": 1}\n```\n";
        let map = highlight_markdown_file(content, &set, true);

        // The JSON content line is reconstructed and coloured by syntect.
        let json_line = &map[&4];
        assert_eq!(spans_text(json_line), "{\"a\": 1}");
        let default = default_text_color(true);
        assert!(
            json_line.iter().any(|s| !color_eq(s.color, default)),
            "embedded JSON should be syntax-coloured"
        );
    }

    #[test]
    fn line_count_matches_input() {
        let set = load_syntax_set();
        let content = "a\nb\nc\n";
        let map = highlight_markdown_file(content, &set, false);
        // Three content lines (trailing newline does not create a 4th).
        assert!(map.contains_key(&1) && map.contains_key(&2) && map.contains_key(&3));
    }

    #[test]
    fn palette_derives_from_theme_with_fallback() {
        // Dark = Dracula, which defines `markup.heading` -> cyan (#8be9fd):
        // the heading colour must come from the theme, not the fallback purple.
        let dark = MarkdownPalette::from_theme(load_syntax_theme(true), true);
        assert!(
            color_eq(dark.title, rgb(0x8be9fd)),
            "dark heading should use Dracula's markup.heading colour"
        );

        // Light = GitHub, which has no `markup.heading` rule: the heading
        // colour must fall back to the hand-picked palette.
        let light = MarkdownPalette::from_theme(load_syntax_theme(false), false);
        assert!(
            color_eq(light.title, rgb(0x6f42c1)),
            "light heading should fall back to the palette"
        );
    }

    #[test]
    fn content_shape_has_plain_text_and_respects_max_lines() {
        let set = load_syntax_set();
        let content = "# A\n\nb\nc\nd\n";

        let lines = highlight_markdown_content(content, &set, 0, true);
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0].plain_text, "# A");
        // plain_text is exactly the concatenation of span texts on each line.
        for line in &lines {
            let joined: String = line.spans.iter().map(|s| s.text.as_str()).collect();
            assert_eq!(joined, line.plain_text);
        }

        // max_lines caps the emitted line count.
        let capped = highlight_markdown_content(content, &set, 2, true);
        assert_eq!(capped.len(), 2);
    }

    #[test]
    fn is_markdown_path_matches_extensions() {
        assert!(is_markdown_path(Path::new("a/b/c.md")));
        assert!(is_markdown_path(Path::new("README.MDX")));
        assert!(is_markdown_path(Path::new("x.markdown")));
        assert!(!is_markdown_path(Path::new("x.rs")));
        assert!(!is_markdown_path(Path::new("x")));
    }
}
