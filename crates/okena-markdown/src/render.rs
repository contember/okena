//! Rendering logic for markdown nodes and inline elements.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::theme::ThemeColors;
use okena_ui::code_block::code_block_container;
use okena_ui::tokens::ui_text_md;

use super::style::{
    MdColors, body_line_height, body_size, heading_style, inline_code_size, node_spacing,
    table_line_height,
};
use super::types::{FmValue, Frontmatter, Inline, Node, char_len, slice_by_chars};
use super::{MarkdownDocument, RenderedNode};

/// Height of one code line. Code blocks are laid out line by line (each line is
/// its own selectable element), so this stands in for `line_height`.
const CODE_LINE_HEIGHT: Pixels = px(20.0);

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
            Node::CodeBlock { language, code } => {
                // Return code blocks with individual lines for per-line selection
                let selection_bg = rgba(0x3390ff40);
                let mut lines = Vec::new();
                let mut line_offset = offset;

                for line in code.lines() {
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

                    let line_div = if let Some((sel_start, sel_end)) = line_sel {
                        let (before, selected, after) = slice_by_chars(line, sel_start, sel_end);
                        div()
                            .h(CODE_LINE_HEIGHT)
                            .flex()
                            .child(div().child(before))
                            .child(div().bg(selection_bg).child(selected))
                            .child(div().child(after))
                    } else {
                        div().h(CODE_LINE_HEIGHT).child(if line.is_empty() {
                            " ".to_string()
                        } else {
                            line.to_string()
                        })
                    };

                    lines.push((line_div, line_offset, line_end));
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
                                Self::render_inlines_with_selection(header, t, cx, cell_sel)
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
                    rendered_header = Some((header_div, row_offset, header_end));
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
                                Self::render_inlines_with_selection(cell, t, cx, cell_sel)
                                    .text_size(ui_text_md(cx))
                                    .line_height(table_line_height(cx))
                                    .text_color(rgb(c.body)),
                            ),
                        );
                        cell_offset += cell_len;
                    }

                    rendered_rows.push((row_div, row_offset, row_end));
                    row_offset = row_end;
                }

                RenderedNode::Table {
                    header: rendered_header,
                    rows: rendered_rows,
                }
            }
            _ => {
                // Other nodes are simple blocks
                let node_div = Self::render_node_with_selection(node, t, cx, node_selection);
                RenderedNode::Simple {
                    div: node_div,
                    start_offset: offset,
                    end_offset: offset + node_len,
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
    ) -> Div {
        let c = MdColors::new(t);
        match node {
            Node::Heading { level, children } => {
                let (size, line_height) = heading_style(*level, cx);

                // For headings, render inline content with selection support
                // but apply heading styles to the container
                let content = if let Some((start, end)) = selection {
                    // Render with selection highlighting
                    Self::render_heading_text_with_selection(children, start, end)
                } else {
                    // No selection - render as plain text for proper styling
                    div().child(Self::render_inlines_as_text(children))
                };

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
            Node::Paragraph { children } => {
                Self::render_inlines_with_selection(children, t, cx, selection).w_full()
            }
            Node::CodeBlock { language, code } => {
                let selection_bg = rgba(0x3390ff40);

                // Render code lines with selection
                let mut code_lines: Vec<Div> = Vec::new();
                let mut offset = 0usize;

                for line in code.lines() {
                    let line_len = char_len(line);
                    let line_end = offset + line_len + 1; // +1 for newline

                    let line_sel = selection.and_then(|(s, e)| {
                        if e <= offset || s >= line_end {
                            None
                        } else {
                            Some((s.saturating_sub(offset), (e - offset).min(line_len)))
                        }
                    });

                    let line_div = if let Some((sel_start, sel_end)) = line_sel {
                        let (before, selected, after) = slice_by_chars(line, sel_start, sel_end);
                        div()
                            .h(CODE_LINE_HEIGHT)
                            .flex()
                            .child(div().child(before))
                            .child(div().bg(selection_bg).child(selected))
                            .child(div().child(after))
                    } else {
                        div().h(CODE_LINE_HEIGHT).child(if line.is_empty() {
                            " ".to_string()
                        } else {
                            line.to_string()
                        })
                    };

                    code_lines.push(line_div);
                    offset = line_end;
                }

                code_block_container(language.as_deref(), t, cx).child(
                    div()
                        .px(px(14.0))
                        .py(px(10.0))
                        .font_family("monospace")
                        .text_size(ui_text_md(cx))
                        .text_color(rgb(c.body))
                        .flex()
                        .flex_col()
                        .children(code_lines),
                )
            }
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
                                Self::render_inlines_with_selection(item_inlines, t, cx, item_sel)
                                    .flex_1(),
                            ),
                    );
                    offset += item_len;
                }
                list
            }
            Node::Table {
                headers,
                rows,
                col_widths,
            } => Self::render_table_with_selection(headers, rows, col_widths, t, cx, selection),
            Node::Blockquote { children } => div()
                .pl(px(14.0))
                .border_l_2()
                .border_color(rgb(c.surface_border))
                .child(
                    Self::render_inlines_with_selection(children, t, cx, selection)
                        .w_full()
                        .text_color(rgb(c.muted))
                        .italic(),
                ),
            // Whitespace is what separates sections here, so an explicit rule
            // stays as a hairline that barely registers.
            Node::HorizontalRule => div().w_full().h(px(1.0)).bg(rgb(c.rule)),
            // Frontmatter renders as a self-contained metadata card. Partial
            // (inline) selection highlighting is intentionally omitted; block
            // selection and copy still work through the flat-text offsets.
            Node::Frontmatter { block, .. } => Self::render_frontmatter(block, t, cx),
        }
    }

    /// Render a frontmatter block as a bordered metadata card.
    fn render_frontmatter(fm: &Frontmatter, t: &ThemeColors, cx: &App) -> Div {
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
                card.font_family("monospace")
                    .children(raw.lines().map(|line| {
                        div().text_color(rgb(c.body)).child(if line.is_empty() {
                            " ".to_string()
                        } else {
                            line.to_string()
                        })
                    }))
            }
            Frontmatter::Parsed(entries) => card.children(
                entries
                    .iter()
                    .map(|(key, value)| Self::render_fm_entry(key, value, t, cx)),
            ),
        }
    }

    /// Render a single `key: value` frontmatter entry. Scalars sit inline next
    /// to the key; lists and nested maps stack below it, indented.
    fn render_fm_entry(key: &str, value: &FmValue, t: &ThemeColors, cx: &App) -> Div {
        let c = MdColors::new(t);
        let key_label = || {
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(c.muted))
                .child(key.to_string())
        };

        match value {
            FmValue::Scalar(s) => h_flex()
                .gap(px(8.0))
                .items_baseline()
                .child(key_label().min_w(px(120.0)).flex_shrink_0())
                .child(div().flex_1().text_color(rgb(c.body)).child(s.clone())),
            FmValue::Empty => h_flex()
                .gap(px(8.0))
                .items_baseline()
                .child(key_label().min_w(px(120.0)).flex_shrink_0())
                .child(div().italic().text_color(rgb(c.muted)).child("\u{2014}")),
            FmValue::List(items) => v_flex()
                .gap(px(2.0))
                .child(key_label())
                .child(Self::render_fm_list(items, t, cx)),
            FmValue::Map(sub) => v_flex().gap(px(2.0)).child(key_label()).child(
                v_flex()
                    .gap(px(4.0))
                    .pl(px(16.0))
                    .children(sub.iter().map(|(k, v)| Self::render_fm_entry(k, v, t, cx))),
            ),
        }
    }

    /// Render a frontmatter sequence as a bulleted, indented list.
    fn render_fm_list(items: &[FmValue], t: &ThemeColors, cx: &App) -> Div {
        let c = MdColors::new(t);
        let mut list = v_flex().gap(px(2.0)).pl(px(16.0));
        for item in items {
            list = list.child(match item {
                FmValue::Scalar(s) => h_flex()
                    .gap(px(8.0))
                    .items_baseline()
                    .child(div().text_color(rgb(c.muted)).child("\u{2022}"))
                    .child(div().text_color(rgb(c.body)).child(s.clone())),
                FmValue::Empty => h_flex()
                    .gap(px(8.0))
                    .child(div().text_color(rgb(c.muted)).child("\u{2022}")),
                FmValue::List(inner) => v_flex()
                    .child(div().text_color(rgb(c.muted)).child("\u{2022}"))
                    .child(Self::render_fm_list(inner, t, cx)),
                FmValue::Map(sub) => v_flex()
                    .gap(px(4.0))
                    .child(div().text_color(rgb(c.muted)).child("\u{2022}"))
                    .child(
                        v_flex()
                            .gap(px(4.0))
                            .pl(px(16.0))
                            .children(sub.iter().map(|(k, v)| Self::render_fm_entry(k, v, t, cx))),
                    ),
            });
        }
        list
    }

    /// Render inline elements with selection highlighting.
    pub(crate) fn render_inlines_with_selection(
        inlines: &[Inline],
        t: &ThemeColors,
        cx: &App,
        selection: Option<(usize, usize)>,
    ) -> Div {
        let mut elements: Vec<Div> = Vec::new();
        let mut offset = 0usize;

        for inline in inlines {
            let inline_len = match inline {
                Inline::Text(text) => char_len(text),
                Inline::Code(code) => char_len(code),
                Inline::Bold(children) | Inline::Italic(children) => {
                    Self::inlines_text_length(children)
                }
                Inline::Link { children, .. } => Self::inlines_text_length(children),
            };

            let inline_sel = selection.and_then(|(s, e)| {
                if e <= offset || s >= offset + inline_len {
                    None
                } else {
                    Some((s.saturating_sub(offset), (e - offset).min(inline_len)))
                }
            });

            elements.push(Self::render_inline_with_selection(
                inline, t, cx, inline_sel,
            ));
            offset += inline_len;
        }

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

    /// Render a single inline element with selection.
    fn render_inline_with_selection(
        inline: &Inline,
        t: &ThemeColors,
        cx: &App,
        selection: Option<(usize, usize)>,
    ) -> Div {
        let selection_bg = rgba(0x3390ff40);
        let c = MdColors::new(t);

        match inline {
            Inline::Text(text) => {
                if let Some((start, end)) = selection {
                    let (before, selected, after) = slice_by_chars(text, start, end);
                    div()
                        .flex()
                        .min_w_0()
                        .child(div().min_w_0().child(before))
                        .child(div().min_w_0().bg(selection_bg).child(selected))
                        .child(div().min_w_0().child(after))
                } else {
                    // `min-width: 0` lets a long run shrink to the column width
                    // and wrap inside it. On `min-width: auto` the run keeps its
                    // unwrapped width and paints past the edge of the column.
                    div().min_w_0().child(text.clone())
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
                if let Some((start, end)) = selection {
                    let (before, selected, after) = slice_by_chars(code, start, end);
                    chip(div())
                        .flex()
                        .child(div().child(before))
                        .child(div().bg(selection_bg).child(selected))
                        .child(div().child(after))
                } else {
                    chip(div()).child(code.clone())
                }
            }
            Inline::Bold(children) => {
                let mut container = div().font_weight(FontWeight::BOLD).flex().flex_wrap();
                let mut offset = 0usize;
                for child in children {
                    let child_len = match child {
                        Inline::Text(t) => char_len(t),
                        Inline::Code(c) => char_len(c),
                        Inline::Bold(ch) | Inline::Italic(ch) => Self::inlines_text_length(ch),
                        Inline::Link { children: ch, .. } => Self::inlines_text_length(ch),
                    };
                    let child_sel = selection.and_then(|(s, e)| {
                        if e <= offset || s >= offset + child_len {
                            None
                        } else {
                            Some((s.saturating_sub(offset), (e - offset).min(child_len)))
                        }
                    });
                    container = container
                        .child(Self::render_inline_with_selection(child, t, cx, child_sel));
                    offset += child_len;
                }
                container
            }
            Inline::Italic(children) => {
                let mut container = div().italic().flex().flex_wrap();
                let mut offset = 0usize;
                for child in children {
                    let child_len = match child {
                        Inline::Text(t) => char_len(t),
                        Inline::Code(c) => char_len(c),
                        Inline::Bold(ch) | Inline::Italic(ch) => Self::inlines_text_length(ch),
                        Inline::Link { children: ch, .. } => Self::inlines_text_length(ch),
                    };
                    let child_sel = selection.and_then(|(s, e)| {
                        if e <= offset || s >= offset + child_len {
                            None
                        } else {
                            Some((s.saturating_sub(offset), (e - offset).min(child_len)))
                        }
                    });
                    container = container
                        .child(Self::render_inline_with_selection(child, t, cx, child_sel));
                    offset += child_len;
                }
                container
            }
            Inline::Link { children, .. } => {
                let mut container = div().text_color(rgb(c.link)).underline().flex().flex_wrap();
                let mut offset = 0usize;
                for child in children {
                    let child_len = match child {
                        Inline::Text(t) => char_len(t),
                        Inline::Code(c) => char_len(c),
                        Inline::Bold(ch) | Inline::Italic(ch) => Self::inlines_text_length(ch),
                        Inline::Link { children: ch, .. } => Self::inlines_text_length(ch),
                    };
                    let child_sel = selection.and_then(|(s, e)| {
                        if e <= offset || s >= offset + child_len {
                            None
                        } else {
                            Some((s.saturating_sub(offset), (e - offset).min(child_len)))
                        }
                    });
                    container = container
                        .child(Self::render_inline_with_selection(child, t, cx, child_sel));
                    offset += child_len;
                }
                container
            }
        }
    }

    /// Render a table with selection highlighting.
    fn render_table_with_selection(
        headers: &[Vec<Inline>],
        rows: &[Vec<Vec<Inline>>],
        col_widths: &[usize],
        t: &ThemeColors,
        cx: &App,
        selection: Option<(usize, usize)>,
    ) -> Div {
        // Column widths are precomputed at parse time.
        let c = MdColors::new(t);
        let mut table = v_flex()
            .rounded(px(6.0))
            .border_1()
            .border_color(rgb(c.surface_border))
            .overflow_hidden();

        let mut offset = 0usize;

        // Header row
        if !headers.is_empty() {
            let mut header_row = div()
                .flex()
                .bg(rgb(c.surface))
                .border_b_1()
                .border_color(rgb(c.surface_border));

            for (i, header) in headers.iter().enumerate() {
                let cell_len = Self::inlines_text_length(header) + if i > 0 { 1 } else { 0 }; // +1 for tab
                let cell_sel = selection.and_then(|(s, e)| {
                    let cell_start = offset + if i > 0 { 1 } else { 0 }; // skip tab
                    let cell_end = offset + cell_len;
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
                        Self::render_inlines_with_selection(header, t, cx, cell_sel)
                            .text_size(ui_text_md(cx))
                            .line_height(table_line_height(cx))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(c.heading)),
                    ),
                );
                offset += cell_len;
            }
            offset += 1; // newline
            table = table.child(header_row);
        }

        // Data rows
        for (row_idx, row) in rows.iter().enumerate() {
            let mut row_div = div()
                .flex()
                .when(row_idx % 2 == 1, |d| d.bg(rgb(c.surface)));

            if row_idx < rows.len() - 1 {
                row_div = row_div.border_b_1().border_color(rgb(c.surface_border));
            }

            for (i, cell) in row.iter().enumerate() {
                let cell_len = Self::inlines_text_length(cell) + if i > 0 { 1 } else { 0 };
                let cell_sel = selection.and_then(|(s, e)| {
                    let cell_start = offset + if i > 0 { 1 } else { 0 };
                    let cell_end = offset + cell_len;
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
                        Self::render_inlines_with_selection(cell, t, cx, cell_sel)
                            .text_size(ui_text_md(cx))
                            .line_height(table_line_height(cx))
                            .text_color(rgb(c.body)),
                    ),
                );
                offset += cell_len;
            }
            offset += 1; // newline
            table = table.child(row_div);
        }

        table
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

    /// Render heading text with selection highlighting.
    /// Returns a Div with flex layout containing the text split by selection.
    fn render_heading_text_with_selection(
        inlines: &[Inline],
        sel_start: usize,
        sel_end: usize,
    ) -> Div {
        let selection_bg = rgba(0x3390ff40);
        let text = Self::render_inlines_as_text(inlines);
        let (before, selected, after) = slice_by_chars(&text, sel_start, sel_end);

        div()
            .flex()
            .child(div().child(before))
            .child(div().bg(selection_bg).child(selected))
            .child(div().child(after))
    }
}
