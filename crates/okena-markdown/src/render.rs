//! Rendering logic for markdown nodes and inline elements.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::theme::ThemeColors;
use okena_highlight::styled::build_styled_text_with_backgrounds;
use okena_highlight::syntax::HighlightedSpan;
use okena_ui::tokens::ui_text_md;

use super::style::{
    MdColors, body_line_height, body_size, heading_style, inline_code_size, node_spacing,
    table_line_height,
};
use super::types::{FmValue, Frontmatter, Inline, Node, char_len};
use super::{MarkdownDocument, MarkdownTextRun, RenderedNode, RenderedTextUnit};

/// Height of one code line. Code blocks are laid out line by line (each line is
/// its own selectable element), so this stands in for `line_height`.
const CODE_LINE_HEIGHT: Pixels = px(20.0);

/// One syntax-highlighted code line, drawn as a single text run so indentation
/// and wide glyphs measure the same as the source.
///
/// `selection` is a character range within the line; the spans hold the line's
/// characters unchanged (tabs included), so the range converts straight to the
/// byte offsets `build_styled_text_with_backgrounds` expects.
fn highlighted_code_line(
    line: &str,
    spans: &[HighlightedSpan],
    selection: Option<(usize, usize)>,
    selection_bg: Rgba,
) -> StyledText {
    let byte_at = |char_idx: usize| {
        line.char_indices()
            .nth(char_idx)
            .map(|(byte, _)| byte)
            .unwrap_or(line.len())
    };
    let bg_ranges = match selection {
        Some((start, end)) if start < end => {
            vec![(byte_at(start)..byte_at(end), selection_bg.into())]
        }
        _ => Vec::new(),
    };
    build_styled_text_with_backgrounds(spans, &bg_ranges)
}

/// Render one stable text element and apply selection as a text highlight.
/// Keeping the element whole makes its `TextLayout` usable throughout a drag.
fn plain_text_run(text: &str, selection: Option<(usize, usize)>, selection_bg: Rgba) -> StyledText {
    let byte_at = |char_idx: usize| {
        text.char_indices()
            .nth(char_idx)
            .map(|(byte, _)| byte)
            .unwrap_or(text.len())
    };
    let highlights = selection
        .filter(|(start, end)| start < end)
        .map(|(start, end)| {
            vec![(
                byte_at(start)..byte_at(end),
                HighlightStyle {
                    background_color: Some(selection_bg.into()),
                    ..Default::default()
                },
            )]
        })
        .unwrap_or_default();
    StyledText::new(text.to_string()).with_highlights(highlights)
}

fn push_text_run(
    text: &str,
    selection: Option<(usize, usize)>,
    start_offset: usize,
    selection_bg: Rgba,
    targets: &mut Vec<MarkdownTextRun>,
) -> StyledText {
    let styled = plain_text_run(text, selection, selection_bg);
    targets.push(MarkdownTextRun::new(
        styled.layout().clone(),
        text.to_string(),
        start_offset,
    ));
    styled
}

/// Emphasis a word token inherits from the inline elements enclosing it.
///
/// Inline elements are flattened into one wrapping row rather than nested into
/// containers: a container holding a run that wraps measures as a full-width
/// box, so whatever follows it starts on a new line instead of continuing the
/// current one. Emphasis rides down to the tokens instead.
#[derive(Clone, Copy, Default)]
struct InlineStyle {
    bold: bool,
    italic: bool,
    link: bool,
}

impl InlineStyle {
    fn apply(self, el: Div, c: &MdColors) -> Div {
        el.when(self.bold, |el| el.font_weight(FontWeight::BOLD))
            .when(self.italic, |el| el.italic())
            .when(self.link, |el| el.text_color(rgb(c.link)).underline())
    }
}

/// Split a text run into word tokens, each keeping its trailing whitespace.
///
/// One element per word is what lets the row wrap between words. Leading
/// whitespace becomes its own token, so a break there leaves the space behind
/// on the previous line.
fn word_tokens(text: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = 0usize;
    let mut prev_ws = false;
    for (i, ch) in text.char_indices() {
        let ws = ch.is_whitespace();
        if prev_ws && !ws && i > start {
            tokens.push(&text[start..i]);
            start = i;
        }
        prev_ws = ws;
    }
    if start < text.len() {
        tokens.push(&text[start..]);
    }
    tokens
}

/// Narrow a character selection range to the `len` characters at `offset`.
fn sub_selection(
    selection: Option<(usize, usize)>,
    offset: usize,
    len: usize,
) -> Option<(usize, usize)> {
    selection.and_then(|(s, e)| {
        if e <= offset || s >= offset + len {
            None
        } else {
            Some((s.saturating_sub(offset), (e - offset).min(len)))
        }
    })
}

impl MarkdownDocument {
    /// Number of top-level blocks in the document. Each maps to one list item.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Vertical space `(above, below)` the block at `idx`, for the caller to put
    /// on its per-block wrapper. Out-of-range indices get the paragraph rhythm.
    pub fn node_spacing(&self, idx: usize) -> (Pixels, Pixels) {
        match self.nodes.get(idx) {
            Some(node) => node_spacing(node, idx == 0),
            None => (px(0.0), px(12.0)),
        }
    }

    /// Render a single top-level node by index, ready for the caller to wrap
    /// with mouse handlers. Code blocks/tables are returned with their
    /// individual selectable lines/rows. Returns None if `idx` is out of range.
    pub fn render_node(
        &self,
        idx: usize,
        t: &ThemeColors,
        cx: &App,
        selection: Option<(usize, usize)>,
    ) -> Option<RenderedNode> {
        let node = self.nodes.get(idx)?;
        let offset = self.node_offsets.get(idx).copied().unwrap_or(0);
        let node_len = Self::node_text_length(node);
        let node_selection = selection.and_then(|(start, end)| {
            if end <= offset || start >= offset + node_len {
                None
            } else {
                Some((start.saturating_sub(offset), (end - offset).min(node_len)))
            }
        });

        let rendered = match node {
            Node::CodeBlock {
                language,
                code,
                highlighted,
            } => {
                // Return code blocks with individual lines for per-line selection
                let selection_bg = rgba(0x3390ff40);
                let mut lines = Vec::new();
                let mut line_offset = offset;

                for (line_idx, line) in code.lines().enumerate() {
                    let line_len = char_len(line);
                    let line_end = line_offset + line_len + 1; // +1 for newline

                    let line_sel = node_selection.and_then(|(s, e)| {
                        let rel_offset = line_offset - offset;
                        let rel_end = rel_offset + line_len + 1;
                        if e <= rel_offset || s >= rel_end {
                            None
                        } else {
                            Some((s.saturating_sub(rel_offset), (e - rel_offset).min(line_len)))
                        }
                    });

                    let spans = highlighted.get(line_idx).map(Vec::as_slice).unwrap_or(&[]);
                    let display_line = if line.is_empty() { " " } else { line };
                    let styled = if !spans.is_empty() {
                        highlighted_code_line(line, spans, line_sel, selection_bg)
                    } else {
                        plain_text_run(display_line, line_sel, selection_bg)
                    };
                    let text_runs = vec![MarkdownTextRun::new(
                        styled.layout().clone(),
                        line.to_string(),
                        line_offset,
                    )];
                    let line_div = div().h(CODE_LINE_HEIGHT).child(styled);

                    lines.push(RenderedTextUnit {
                        div: line_div,
                        start_offset: line_offset,
                        end_offset: line_end,
                        text_runs,
                    });
                    line_offset = line_end;
                }

                RenderedNode::CodeBlock {
                    language: language.clone(),
                    lines,
                }
            }
            Node::Table {
                headers,
                rows,
                col_widths,
            } => {
                // Return tables with individual rows for per-row selection.
                // Column widths are precomputed at parse time.
                let c = MdColors::new(t);
                let mut row_offset = offset;
                let mut rendered_rows = Vec::new();
                let mut rendered_header = None;

                // Header row
                if !headers.is_empty() {
                    let header_len: usize = headers
                        .iter()
                        .map(|h| Self::inlines_text_length(h))
                        .sum::<usize>()
                        + headers.len().saturating_sub(1)
                        + 1; // tabs + newline
                    let header_end = row_offset + header_len;

                    let header_sel = node_selection.and_then(|(s, e)| {
                        let rel_start = row_offset - offset;
                        let rel_end = rel_start + header_len;
                        if e <= rel_start || s >= rel_end {
                            None
                        } else {
                            Some((s.saturating_sub(rel_start), (e - rel_start).min(header_len)))
                        }
                    });

                    let mut header_row = h_flex();
                    let mut header_runs = Vec::new();
                    let mut cell_offset = 0usize;
                    for (i, header) in headers.iter().enumerate() {
                        let cell_len =
                            Self::inlines_text_length(header) + if i > 0 { 1 } else { 0 };
                        let cell_sel = header_sel.and_then(|(s, e)| {
                            let cell_start = cell_offset + if i > 0 { 1 } else { 0 };
                            let cell_end = cell_offset + cell_len;
                            if e <= cell_start || s >= cell_end {
                                None
                            } else {
                                Some((
                                    s.saturating_sub(cell_start),
                                    (e - cell_start).min(Self::inlines_text_length(header)),
                                ))
                            }
                        });

                        let width = col_widths.get(i).copied().unwrap_or(10);
                        let min_w = ((width * 8) + 24).max(80) as f32;
                        header_row = header_row.child(
                            div().min_w(px(min_w)).px(px(12.0)).py(px(8.0)).child(
                                Self::render_inlines_with_selection_and_targets(
                                    header,
                                    t,
                                    cx,
                                    cell_sel,
                                    row_offset + cell_offset + if i > 0 { 1 } else { 0 },
                                    &mut header_runs,
                                )
                                .text_size(ui_text_md(cx))
                                .line_height(table_line_height(cx))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(c.heading)),
                            ),
                        );
                        cell_offset += cell_len;
                    }

                    let header_div = header_row
                        .bg(rgb(c.surface))
                        .border_b_1()
                        .border_color(rgb(c.surface_border));
                    rendered_header = Some(RenderedTextUnit {
                        div: header_div,
                        start_offset: row_offset,
                        end_offset: header_end,
                        text_runs: header_runs,
                    });
                    row_offset = header_end;
                }

                // Data rows
                for (row_idx, row) in rows.iter().enumerate() {
                    let row_len: usize = row
                        .iter()
                        .map(|cell| Self::inlines_text_length(cell))
                        .sum::<usize>()
                        + row.len().saturating_sub(1)
                        + 1; // tabs + newline
                    let row_end = row_offset + row_len;

                    let row_sel = node_selection.and_then(|(s, e)| {
                        let rel_start = row_offset - offset;
                        let rel_end = rel_start + row_len;
                        if e <= rel_start || s >= rel_end {
                            None
                        } else {
                            Some((s.saturating_sub(rel_start), (e - rel_start).min(row_len)))
                        }
                    });

                    let mut row_div = h_flex();
                    let mut row_runs = Vec::new();
                    if row_idx % 2 == 1 {
                        row_div = row_div.bg(rgb(c.surface));
                    }
                    if row_idx < rows.len() - 1 {
                        row_div = row_div.border_b_1().border_color(rgb(c.surface_border));
                    }

                    let mut cell_offset = 0usize;
                    for (i, cell) in row.iter().enumerate() {
                        let cell_len = Self::inlines_text_length(cell) + if i > 0 { 1 } else { 0 };
                        let cell_sel = row_sel.and_then(|(s, e)| {
                            let cell_start = cell_offset + if i > 0 { 1 } else { 0 };
                            let cell_end = cell_offset + cell_len;
                            if e <= cell_start || s >= cell_end {
                                None
                            } else {
                                Some((
                                    s.saturating_sub(cell_start),
                                    (e - cell_start).min(Self::inlines_text_length(cell)),
                                ))
                            }
                        });

                        let width = col_widths.get(i).copied().unwrap_or(10);
                        let min_w = ((width * 8) + 24).max(80) as f32;
                        row_div = row_div.child(
                            div().min_w(px(min_w)).px(px(12.0)).py(px(6.0)).child(
                                Self::render_inlines_with_selection_and_targets(
                                    cell,
                                    t,
                                    cx,
                                    cell_sel,
                                    row_offset + cell_offset + if i > 0 { 1 } else { 0 },
                                    &mut row_runs,
                                )
                                .text_size(ui_text_md(cx))
                                .line_height(table_line_height(cx))
                                .text_color(rgb(c.body)),
                            ),
                        );
                        cell_offset += cell_len;
                    }

                    rendered_rows.push(RenderedTextUnit {
                        div: row_div,
                        start_offset: row_offset,
                        end_offset: row_end,
                        text_runs: row_runs,
                    });
                    row_offset = row_end;
                }

                RenderedNode::Table {
                    header: rendered_header,
                    rows: rendered_rows,
                }
            }
            _ => {
                // Other nodes are simple blocks
                let mut text_runs = Vec::new();
                let node_div = Self::render_node_with_selection(
                    node,
                    t,
                    cx,
                    node_selection,
                    offset,
                    &mut text_runs,
                );
                RenderedNode::Simple {
                    div: node_div,
                    start_offset: offset,
                    end_offset: offset + node_len,
                    text_runs,
                }
            }
        };

        Some(rendered)
    }

    /// Calculate the text length of a node (for selection offset tracking, in characters).
    pub(crate) fn node_text_length(node: &Node) -> usize {
        match node {
            Node::Heading { level: _, children }
            | Node::Paragraph { children }
            | Node::Blockquote { children } => {
                Self::inlines_text_length(children) + 1 // +1 for newline
            }
            Node::CodeBlock { code, .. } => {
                // Sum of character lengths of each line + 1 newline per line
                code.lines()
                    .map(|line| char_len(line) + 1)
                    .sum::<usize>()
                    .max(1)
            }
            Node::List { items, .. } => items
                .iter()
                .map(|item| Self::inlines_text_length(item) + 1)
                .sum(),
            Node::Table { headers, rows, .. } => {
                let header_len: usize = headers.iter().map(|h| Self::inlines_text_length(h)).sum::<usize>()
                    + headers.len().saturating_sub(1) // tabs
                    + 1; // newline
                let rows_len: usize = rows
                    .iter()
                    .map(|row| {
                        row.iter().map(|cell| Self::inlines_text_length(cell)).sum::<usize>()
                        + row.len().saturating_sub(1) // tabs
                        + 1 // newline
                    })
                    .sum();
                header_len + rows_len
            }
            Node::HorizontalRule => 1, // newline
            Node::Frontmatter { text_len, .. } => *text_len,
        }
    }

    /// Calculate the text length of inline elements (in characters, not bytes).
    pub(crate) fn inlines_text_length(inlines: &[Inline]) -> usize {
        inlines
            .iter()
            .map(|inline| match inline {
                Inline::Text(t) => char_len(t),
                Inline::Code(c) => char_len(c),
                Inline::Bold(children) | Inline::Italic(children) => {
                    Self::inlines_text_length(children)
                }
                Inline::Link { children, .. } => Self::inlines_text_length(children),
            })
            .sum()
    }

    /// Render a node with selection highlighting.
    fn render_node_with_selection(
        node: &Node,
        t: &ThemeColors,
        cx: &App,
        selection: Option<(usize, usize)>,
        base_offset: usize,
        text_runs: &mut Vec<MarkdownTextRun>,
    ) -> Div {
        let c = MdColors::new(t);
        match node {
            Node::Heading { level, children } => {
                let (size, line_height) = heading_style(*level, cx);
                let text = Self::render_inlines_as_text(children);
                let content =
                    push_text_run(&text, selection, base_offset, rgba(0x3390ff40), text_runs);

                div()
                    .w_full()
                    .text_size(size)
                    .line_height(line_height)
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(c.heading))
                    .child(content)
            }
            // `w_full` gives the inline flow a definite width to wrap against.
            // Without it the run is sized from its own content and spills past
            // the reading column instead of breaking at its edge.
            Node::Paragraph { children } => Self::render_inlines_with_selection_and_targets(
                children,
                t,
                cx,
                selection,
                base_offset,
                text_runs,
            )
            .w_full(),
            Node::List { ordered, items } => {
                // One marker column for both list kinds, right-aligned in it, so
                // the text hangs at the same indent whatever the marker is —
                // including two-digit numbers.
                let marker_width = px(22.0);
                let mut list = v_flex().w_full().gap(px(6.0)).pl(px(4.0));
                let mut offset = 0usize;

                for (i, item_inlines) in items.iter().enumerate() {
                    let item_len = Self::inlines_text_length(item_inlines) + 1;
                    let item_sel = selection.and_then(|(s, e)| {
                        if e <= offset || s >= offset + item_len {
                            None
                        } else {
                            Some((
                                s.saturating_sub(offset),
                                (e - offset).min(item_len - 1), // -1 to exclude newline
                            ))
                        }
                    });

                    let marker = if *ordered {
                        format!("{}.", i + 1)
                    } else {
                        "\u{2022}".to_string()
                    };
                    list = list.child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .text_size(body_size(cx))
                                    // Match the body leading so the marker sits
                                    // on the item's first line, not above it.
                                    .line_height(body_line_height(cx))
                                    .text_color(rgb(c.muted))
                                    .w(marker_width)
                                    .flex_shrink_0()
                                    .text_right()
                                    .child(marker),
                            )
                            .child(
                                Self::render_inlines_with_selection_and_targets(
                                    item_inlines,
                                    t,
                                    cx,
                                    item_sel,
                                    base_offset + offset,
                                    text_runs,
                                )
                                .flex_1(),
                            ),
                    );
                    offset += item_len;
                }
                list
            }
            Node::Blockquote { children } => div()
                .pl(px(14.0))
                .border_l_2()
                .border_color(rgb(c.surface_border))
                .child(
                    Self::render_inlines_with_selection_and_targets(
                        children,
                        t,
                        cx,
                        selection,
                        base_offset,
                        text_runs,
                    )
                    .w_full()
                    .text_color(rgb(c.muted))
                    .italic(),
                ),
            // Whitespace is what separates sections here, so an explicit rule
            // stays as a hairline that barely registers.
            Node::HorizontalRule => div().w_full().h(px(1.0)).bg(rgb(c.rule)),
            Node::Frontmatter { block, .. } => {
                Self::render_frontmatter(block, t, cx, selection, base_offset, text_runs)
            }
            // These are rendered by the specialized branches in `render_node`.
            Node::CodeBlock { .. } | Node::Table { .. } => div(),
        }
    }

    /// Render a frontmatter block as a bordered metadata card.
    fn render_frontmatter(
        fm: &Frontmatter,
        t: &ThemeColors,
        cx: &App,
        selection: Option<(usize, usize)>,
        base_offset: usize,
        text_runs: &mut Vec<MarkdownTextRun>,
    ) -> Div {
        let c = MdColors::new(t);
        let card = v_flex()
            .gap(px(4.0))
            .w_full()
            .px(px(14.0))
            .py(px(12.0))
            .rounded(px(6.0))
            .bg(rgb(c.surface))
            .border_1()
            .border_color(rgb(c.surface_border))
            .text_size(ui_text_md(cx))
            .line_height(table_line_height(cx));

        match fm {
            Frontmatter::Raw(raw) => {
                let mut cursor = 0usize;
                let mut lines = Vec::new();
                for line in raw.lines() {
                    let line_len = char_len(line);
                    let line_selection = sub_selection(selection, cursor, line_len);
                    let display_line = if line.is_empty() { " " } else { line };
                    let styled = plain_text_run(display_line, line_selection, rgba(0x3390ff40));
                    text_runs.push(MarkdownTextRun::new(
                        styled.layout().clone(),
                        line.to_string(),
                        base_offset + cursor,
                    ));
                    lines.push(div().text_color(rgb(c.body)).child(styled));
                    cursor += line_len + 1;
                }
                card.font_family("monospace").children(lines)
            }
            Frontmatter::Parsed(entries) => {
                let mut cursor = 0usize;
                card.children(Self::render_fm_entries(
                    entries,
                    0,
                    t,
                    cx,
                    selection,
                    base_offset,
                    &mut cursor,
                    text_runs,
                ))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_fm_entries(
        entries: &[(String, FmValue)],
        depth: usize,
        t: &ThemeColors,
        cx: &App,
        selection: Option<(usize, usize)>,
        base_offset: usize,
        cursor: &mut usize,
        text_runs: &mut Vec<MarkdownTextRun>,
    ) -> Vec<Div> {
        entries
            .iter()
            .map(|(key, value)| {
                Self::render_fm_entry(
                    key,
                    value,
                    depth,
                    t,
                    cx,
                    selection,
                    base_offset,
                    cursor,
                    text_runs,
                )
            })
            .collect()
    }

    /// Render a single `key: value` frontmatter entry. Scalars sit inline next
    /// to the key; lists and nested maps stack below it, indented.
    #[allow(clippy::too_many_arguments)]
    fn render_fm_entry(
        key: &str,
        value: &FmValue,
        depth: usize,
        t: &ThemeColors,
        cx: &App,
        selection: Option<(usize, usize)>,
        base_offset: usize,
        cursor: &mut usize,
        text_runs: &mut Vec<MarkdownTextRun>,
    ) -> Div {
        let c = MdColors::new(t);
        *cursor += depth * 2;
        let key_len = char_len(key);
        let key_start = *cursor;
        let key_text = push_text_run(
            key,
            sub_selection(selection, key_start, key_len),
            base_offset + key_start,
            rgba(0x3390ff40),
            text_runs,
        );
        *cursor += key_len;
        let key_label = div()
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(c.muted))
            .child(key_text);

        match value {
            // `min-width: 0` lets the value column shrink below its unwrapped
            // width so a long scalar wraps inside the card instead of painting
            // past its edge.
            FmValue::Scalar(s) => {
                *cursor += 2; // `: `
                let value_len = char_len(s);
                let value_start = *cursor;
                let value_text = push_text_run(
                    s,
                    sub_selection(selection, value_start, value_len),
                    base_offset + value_start,
                    rgba(0x3390ff40),
                    text_runs,
                );
                *cursor += value_len + 1; // newline
                h_flex()
                    .w_full()
                    .gap(px(8.0))
                    .items_baseline()
                    .child(key_label.min_w(px(120.0)).flex_shrink_0())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(rgb(c.body))
                            .child(value_text),
                    )
            }
            FmValue::Empty => {
                *cursor += 2; // `:` and newline
                h_flex()
                    .w_full()
                    .gap(px(8.0))
                    .items_baseline()
                    .child(key_label.min_w(px(120.0)).flex_shrink_0())
                    .child(div().italic().text_color(rgb(c.muted)).child("\u{2014}"))
            }
            FmValue::List(items) => {
                *cursor += 2; // `:` and newline
                v_flex()
                    .w_full()
                    .gap(px(2.0))
                    .child(key_label)
                    .child(Self::render_fm_list(
                        items,
                        depth + 1,
                        t,
                        cx,
                        selection,
                        base_offset,
                        cursor,
                        text_runs,
                    ))
            }
            FmValue::Map(sub) => {
                *cursor += 2; // `:` and newline
                v_flex().w_full().gap(px(2.0)).child(key_label).child(
                    v_flex()
                        .w_full()
                        .gap(px(4.0))
                        .pl(px(16.0))
                        .children(Self::render_fm_entries(
                            sub,
                            depth + 1,
                            t,
                            cx,
                            selection,
                            base_offset,
                            cursor,
                            text_runs,
                        )),
                )
            }
        }
    }

    /// Render a frontmatter sequence as a bulleted, indented list.
    #[allow(clippy::too_many_arguments)]
    fn render_fm_list(
        items: &[FmValue],
        depth: usize,
        t: &ThemeColors,
        cx: &App,
        selection: Option<(usize, usize)>,
        base_offset: usize,
        cursor: &mut usize,
        text_runs: &mut Vec<MarkdownTextRun>,
    ) -> Div {
        let c = MdColors::new(t);
        let mut list = v_flex().w_full().gap(px(2.0)).pl(px(16.0));
        for item in items {
            *cursor += depth * 2;
            let marker_start = *cursor;
            let marker_text = push_text_run(
                "\u{2022} ",
                sub_selection(selection, marker_start, 2),
                base_offset + marker_start,
                rgba(0x3390ff40),
                text_runs,
            );
            *cursor += 2;
            list = list.child(match item {
                FmValue::Scalar(s) => {
                    let value_len = char_len(s);
                    let value_start = *cursor;
                    let value_text = push_text_run(
                        s,
                        sub_selection(selection, value_start, value_len),
                        base_offset + value_start,
                        rgba(0x3390ff40),
                        text_runs,
                    );
                    *cursor += value_len + 1;
                    h_flex()
                        .w_full()
                        .gap(px(8.0))
                        .items_baseline()
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_color(rgb(c.muted))
                                .child(marker_text),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(rgb(c.body))
                                .child(value_text),
                        )
                }
                FmValue::Empty => {
                    *cursor += 1;
                    h_flex()
                        .gap(px(8.0))
                        .child(div().text_color(rgb(c.muted)).child(marker_text))
                }
                FmValue::List(inner) => {
                    *cursor += 1;
                    v_flex()
                        .w_full()
                        .child(div().text_color(rgb(c.muted)).child(marker_text))
                        .child(Self::render_fm_list(
                            inner,
                            depth + 1,
                            t,
                            cx,
                            selection,
                            base_offset,
                            cursor,
                            text_runs,
                        ))
                }
                FmValue::Map(sub) => {
                    *cursor += 1;
                    v_flex()
                        .w_full()
                        .gap(px(4.0))
                        .child(div().text_color(rgb(c.muted)).child(marker_text))
                        .child(v_flex().w_full().gap(px(4.0)).pl(px(16.0)).children(
                            Self::render_fm_entries(
                                sub,
                                depth + 1,
                                t,
                                cx,
                                selection,
                                base_offset,
                                cursor,
                                text_runs,
                            ),
                        ))
                }
            });
        }
        list
    }

    fn render_inlines_with_selection_and_targets(
        inlines: &[Inline],
        t: &ThemeColors,
        cx: &App,
        selection: Option<(usize, usize)>,
        base_offset: usize,
        text_runs: &mut Vec<MarkdownTextRun>,
    ) -> Div {
        let mut elements: Vec<Div> = Vec::new();
        Self::push_inlines(
            inlines,
            InlineStyle::default(),
            t,
            cx,
            selection,
            base_offset,
            &mut elements,
            text_runs,
        );

        div()
            .flex()
            .flex_wrap()
            // `min-width: 0` lets this inline-flow container shrink below its
            // min-content size when it's a flex child (e.g. a list item's content
            // next to the bullet). Without it, `min-width: auto` pins the flex
            // item to its widest word and the wrapping height is measured at that
            // narrow width — inflating the block with a huge vertical gap.
            .min_w_0()
            .items_baseline()
            .text_size(body_size(cx))
            .line_height(body_line_height(cx))
            .text_color(rgb(MdColors::new(t).body))
            .children(elements)
    }

    /// Text length of one inline element, in characters.
    fn inline_text_length(inline: &Inline) -> usize {
        match inline {
            Inline::Text(text) => char_len(text),
            Inline::Code(code) => char_len(code),
            Inline::Bold(children) | Inline::Italic(children) => {
                Self::inlines_text_length(children)
            }
            Inline::Link { children, .. } => Self::inlines_text_length(children),
        }
    }

    /// Flatten inline elements into `out`, handing each one the slice of the
    /// selection range that falls inside it.
    #[allow(clippy::too_many_arguments)]
    fn push_inlines(
        inlines: &[Inline],
        style: InlineStyle,
        t: &ThemeColors,
        cx: &App,
        selection: Option<(usize, usize)>,
        base_offset: usize,
        out: &mut Vec<Div>,
        text_runs: &mut Vec<MarkdownTextRun>,
    ) {
        let mut offset = 0usize;
        for inline in inlines {
            let len = Self::inline_text_length(inline);
            let inline_sel = sub_selection(selection, offset, len);
            Self::push_inline(
                inline,
                style,
                t,
                cx,
                inline_sel,
                base_offset + offset,
                out,
                text_runs,
            );
            offset += len;
        }
    }

    /// Append one inline element to `out`: text as word tokens, code as a chip,
    /// emphasis as style handed down to the tokens inside it.
    #[allow(clippy::too_many_arguments)]
    fn push_inline(
        inline: &Inline,
        style: InlineStyle,
        t: &ThemeColors,
        cx: &App,
        selection: Option<(usize, usize)>,
        base_offset: usize,
        out: &mut Vec<Div>,
        text_runs: &mut Vec<MarkdownTextRun>,
    ) {
        let selection_bg = rgba(0x3390ff40);
        let c = MdColors::new(t);

        match inline {
            Inline::Text(text) => {
                let mut offset = 0usize;
                for token in word_tokens(text) {
                    let token_len = char_len(token);
                    let styled = push_text_run(
                        token,
                        sub_selection(selection, offset, token_len),
                        base_offset + offset,
                        selection_bg,
                        text_runs,
                    );
                    // `min-width: 0` lets one very long word (a bare URL,
                    // say) shrink to the column width and wrap inside it.
                    let el = div().min_w_0().child(styled);
                    out.push(style.apply(el, &c));
                    offset += token_len;
                }
            }
            Inline::Code(code) => {
                // Emphasis, not a badge: the monospace face and a barely-there
                // tint carry it, so a paragraph full of code reads as prose.
                let chip = |d: Div| {
                    d.font_family("monospace")
                        .text_size(inline_code_size(cx))
                        .px(px(3.0))
                        .rounded(px(3.0))
                        .bg(rgb(c.inline_code_bg))
                        .text_color(rgb(c.body))
                };
                let styled = push_text_run(code, selection, base_offset, selection_bg, text_runs);
                let el = chip(div()).child(styled);
                out.push(style.apply(el, &c));
            }
            Inline::Bold(children) => Self::push_inlines(
                children,
                InlineStyle {
                    bold: true,
                    ..style
                },
                t,
                cx,
                selection,
                base_offset,
                out,
                text_runs,
            ),
            Inline::Italic(children) => Self::push_inlines(
                children,
                InlineStyle {
                    italic: true,
                    ..style
                },
                t,
                cx,
                selection,
                base_offset,
                out,
                text_runs,
            ),
            Inline::Link { children, .. } => Self::push_inlines(
                children,
                InlineStyle {
                    link: true,
                    ..style
                },
                t,
                cx,
                selection,
                base_offset,
                out,
                text_runs,
            ),
        }
    }

    /// Render inlines as plain text (for measuring, headings, etc.).
    pub(crate) fn render_inlines_as_text(inlines: &[Inline]) -> String {
        let mut result = String::new();
        for inline in inlines {
            Self::inline_to_text(inline, &mut result);
        }
        result
    }

    fn inline_to_text(inline: &Inline, out: &mut String) {
        match inline {
            Inline::Text(text) => out.push_str(text),
            Inline::Code(code) => out.push_str(code),
            Inline::Bold(children) | Inline::Italic(children) => {
                for child in children {
                    Self::inline_to_text(child, out);
                }
            }
            Inline::Link { children, .. } => {
                for child in children {
                    Self::inline_to_text(child, out);
                }
            }
        }
    }
}
