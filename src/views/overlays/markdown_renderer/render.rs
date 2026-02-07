//! Rendering logic for markdown nodes and inline elements.

use crate::theme::ThemeColors;
use gpui::*;
use gpui::prelude::FluentBuilder;
use gpui_component::{h_flex, v_flex};

use super::types::{char_len, slice_by_chars, Inline, Node};
use super::{MarkdownDocument, RenderedNode};

impl MarkdownDocument {
    /// Render the document as a list of RenderedNode items.
    /// This allows the caller to wrap each node/line with mouse handlers.
    /// Code blocks are returned with individual lines for per-line selection.
    pub fn render_nodes_with_offsets(
        &self,
        t: &ThemeColors,
        selection: Option<(usize, usize)>,
    ) -> Vec<RenderedNode> {
        let mut result = Vec::new();
        let mut offset = 0usize;

        for node in &self.nodes {
            let node_len = Self::node_text_length(node);
            let node_selection = selection.and_then(|(start, end)| {
                if end <= offset || start >= offset + node_len {
                    None
                } else {
                    Some((
                        start.saturating_sub(offset),
                        (end - offset).min(node_len),
                    ))
                }
            });

            match node {
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
                                Some((
                                    s.saturating_sub(rel_offset),
                                    (e - rel_offset).min(line_len),
                                ))
                            }
                        });

                        let line_div = if let Some((sel_start, sel_end)) = line_sel {
                            let (before, selected, after) = slice_by_chars(line, sel_start, sel_end);
                            div()
                                .h(px(18.0))
                                .flex()
                                .child(div().child(before))
                                .child(div().bg(selection_bg).child(selected))
                                .child(div().child(after))
                        } else {
                            div()
                                .h(px(18.0))
                                .child(if line.is_empty() { " ".to_string() } else { line.to_string() })
                        };

                        lines.push((line_div, line_offset, line_end));
                        line_offset = line_end;
                    }

                    result.push(RenderedNode::CodeBlock {
                        language: language.clone(),
                        lines,
                    });
                }
                Node::Table { headers, rows } => {
                    // Return tables with individual rows for per-row selection

                    // Calculate column widths
                    let mut col_widths: Vec<usize> = headers
                        .iter()
                        .map(|h| char_len(&Self::render_inlines_as_text(h)))
                        .collect();
                    for row in rows {
                        for (i, cell) in row.iter().enumerate() {
                            let len = char_len(&Self::render_inlines_as_text(cell));
                            if i < col_widths.len() {
                                col_widths[i] = col_widths[i].max(len);
                            }
                        }
                    }

                    let mut row_offset = offset;
                    let mut rendered_rows = Vec::new();
                    let mut rendered_header = None;

                    // Header row
                    if !headers.is_empty() {
                        let header_len: usize = headers.iter().map(|h| Self::inlines_text_length(h)).sum::<usize>()
                            + headers.len().saturating_sub(1) + 1; // tabs + newline
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
                            let cell_len = Self::inlines_text_length(header) + if i > 0 { 1 } else { 0 };
                            let cell_sel = header_sel.and_then(|(s, e)| {
                                let cell_start = cell_offset + if i > 0 { 1 } else { 0 };
                                let cell_end = cell_offset + cell_len;
                                if e <= cell_start || s >= cell_end {
                                    None
                                } else {
                                    Some((s.saturating_sub(cell_start), (e - cell_start).min(Self::inlines_text_length(header))))
                                }
                            });

                            let width = col_widths.get(i).copied().unwrap_or(10);
                            let min_w = ((width * 8) + 24).max(80) as f32;
                            header_row = header_row.child(
                                div()
                                    .min_w(px(min_w))
                                    .px(px(12.0))
                                    .py(px(8.0))
                                    .child(
                                        Self::render_inlines_with_selection(header, t, cell_sel)
                                            .text_size(px(12.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(t.text_primary))
                                    )
                            );
                            cell_offset += cell_len;
                        }

                        let header_div = header_row.bg(rgb(t.bg_header)).border_b_1().border_color(rgb(t.border));
                        rendered_header = Some((header_div, row_offset, header_end));
                        row_offset = header_end;
                    }

                    // Data rows
                    for (row_idx, row) in rows.iter().enumerate() {
                        let row_len: usize = row.iter().map(|cell| Self::inlines_text_length(cell)).sum::<usize>()
                            + row.len().saturating_sub(1) + 1; // tabs + newline
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
                            row_div = row_div.bg(rgb(t.bg_secondary));
                        }
                        if row_idx < rows.len() - 1 {
                            row_div = row_div.border_b_1().border_color(rgb(t.border));
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
                                    Some((s.saturating_sub(cell_start), (e - cell_start).min(Self::inlines_text_length(cell))))
                                }
                            });

                            let width = col_widths.get(i).copied().unwrap_or(10);
                            let min_w = ((width * 8) + 24).max(80) as f32;
                            row_div = row_div.child(
                                div()
                                    .min_w(px(min_w))
                                    .px(px(12.0))
                                    .py(px(6.0))
                                    .child(
                                        Self::render_inlines_with_selection(cell, t, cell_sel)
                                            .text_size(px(12.0))
                                            .text_color(rgb(t.text_secondary))
                                    )
                            );
                            cell_offset += cell_len;
                        }

                        rendered_rows.push((row_div, row_offset, row_end));
                        row_offset = row_end;
                    }

                    result.push(RenderedNode::Table {
                        header: rendered_header,
                        rows: rendered_rows,
                    });
                }
                _ => {
                    // Other nodes are simple blocks
                    let node_div = Self::render_node_with_selection(node, t, node_selection);
                    result.push(RenderedNode::Simple {
                        div: node_div,
                        start_offset: offset,
                        end_offset: offset + node_len,
                    });
                }
            }

            offset += node_len;
        }

        result
    }


    /// Calculate the text length of a node (for selection offset tracking, in characters).
    pub(super) fn node_text_length(node: &Node) -> usize {
        match node {
            Node::Heading { children, .. } |
            Node::Paragraph { children } |
            Node::Blockquote { children } => {
                Self::inlines_text_length(children) + 1 // +1 for newline
            }
            Node::CodeBlock { code, .. } => {
                // Sum of character lengths of each line + 1 newline per line
                code.lines().map(|line| char_len(line) + 1).sum::<usize>().max(1)
            }
            Node::List { items, .. } => {
                items.iter().map(|item| Self::inlines_text_length(item) + 1).sum()
            }
            Node::Table { headers, rows } => {
                let header_len: usize = headers.iter().map(|h| Self::inlines_text_length(h)).sum::<usize>()
                    + headers.len().saturating_sub(1) // tabs
                    + 1; // newline
                let rows_len: usize = rows.iter().map(|row| {
                    row.iter().map(|cell| Self::inlines_text_length(cell)).sum::<usize>()
                        + row.len().saturating_sub(1) // tabs
                        + 1 // newline
                }).sum();
                header_len + rows_len
            }
            Node::HorizontalRule => 1, // newline
        }
    }

    /// Calculate the text length of inline elements (in characters, not bytes).
    pub(super) fn inlines_text_length(inlines: &[Inline]) -> usize {
        inlines.iter().map(|inline| {
            match inline {
                Inline::Text(t) => char_len(t),
                Inline::Code(c) => char_len(c),
                Inline::Bold(children) | Inline::Italic(children) => {
                    Self::inlines_text_length(children)
                }
                Inline::Link { children, .. } => {
                    Self::inlines_text_length(children)
                }
            }
        }).sum()
    }

    /// Render a node with selection highlighting.
    fn render_node_with_selection(node: &Node, t: &ThemeColors, selection: Option<(usize, usize)>) -> Div {
        match node {
            Node::Heading { level, children } => {
                let (size, weight) = match level {
                    1 => (px(28.0), FontWeight::BOLD),
                    2 => (px(24.0), FontWeight::BOLD),
                    3 => (px(20.0), FontWeight::SEMIBOLD),
                    4 => (px(18.0), FontWeight::SEMIBOLD),
                    5 => (px(16.0), FontWeight::MEDIUM),
                    _ => (px(14.0), FontWeight::MEDIUM),
                };

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
                    .text_size(size)
                    .font_weight(weight)
                    .text_color(rgb(t.text_primary))
                    .pb(px(4.0))
                    .when(*level <= 2, |d| {
                        d.border_b_1()
                            .border_color(rgb(t.border))
                            .mb(px(4.0))
                    })
                    .child(content)
            }
            Node::Paragraph { children } => {
                Self::render_inlines_with_selection(children, t, selection)
            }
            Node::CodeBlock { language, code } => {
                let lang_label = language.as_deref().unwrap_or("");
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
                            Some((
                                s.saturating_sub(offset),
                                (e - offset).min(line_len),
                            ))
                        }
                    });

                    let line_div = if let Some((sel_start, sel_end)) = line_sel {
                        let (before, selected, after) = slice_by_chars(line, sel_start, sel_end);
                        div()
                            .h(px(18.0))
                            .flex()
                            .child(div().child(before))
                            .child(div().bg(selection_bg).child(selected))
                            .child(div().child(after))
                    } else {
                        div()
                            .h(px(18.0))
                            .child(if line.is_empty() { " ".to_string() } else { line.to_string() })
                    };

                    code_lines.push(line_div);
                    offset = line_end;
                }

                v_flex()
                    .rounded(px(6.0))
                    .bg(rgb(t.bg_primary))
                    .border_1()
                    .border_color(rgb(t.border))
                    .overflow_hidden()
                    .when(!lang_label.is_empty(), |d| {
                        d.child(
                            div()
                                .px(px(12.0))
                                .py(px(4.0))
                                .bg(rgb(t.bg_header))
                                .border_b_1()
                                .border_color(rgb(t.border))
                                .text_size(px(10.0))
                                .text_color(rgb(t.text_muted))
                                .child(lang_label.to_string())
                        )
                    })
                    .child(
                        div()
                            .p(px(12.0))
                            .font_family("monospace")
                            .text_size(px(12.0))
                            .text_color(rgb(t.text_secondary))
                            .flex()
                            .flex_col()
                            .children(code_lines)
                    )
            }
            Node::List { ordered, items } => {
                let mut list = v_flex().gap(px(4.0)).pl(px(16.0));
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
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .text_color(rgb(t.text_muted))
                                    .w(px(16.0))
                                    .flex_shrink_0()
                                    .child(marker)
                            )
                            .child(Self::render_inlines_with_selection(item_inlines, t, item_sel).flex_1())
                    );
                    offset += item_len;
                }
                list
            }
            Node::Table { headers, rows } => {
                Self::render_table_with_selection(headers, rows, t, selection)
            }
            Node::Blockquote { children } => {
                div()
                    .pl(px(12.0))
                    .border_l_2()
                    .border_color(rgb(t.text_muted))
                    .child(
                        Self::render_inlines_with_selection(children, t, selection)
                            .text_color(rgb(t.text_muted))
                            .italic()
                    )
            }
            Node::HorizontalRule => {
                div()
                    .w_full()
                    .h(px(1.0))
                    .bg(rgb(t.border))
                    .my(px(8.0))
            }
        }
    }

    /// Render inline elements with selection highlighting.
    pub(super) fn render_inlines_with_selection(inlines: &[Inline], t: &ThemeColors, selection: Option<(usize, usize)>) -> Div {
        let mut elements: Vec<Div> = Vec::new();
        let mut offset = 0usize;

        for inline in inlines {
            let inline_len = match inline {
                Inline::Text(text) => char_len(text),
                Inline::Code(code) => char_len(code),
                Inline::Bold(children) | Inline::Italic(children) => Self::inlines_text_length(children),
                Inline::Link { children, .. } => Self::inlines_text_length(children),
            };

            let inline_sel = selection.and_then(|(s, e)| {
                if e <= offset || s >= offset + inline_len {
                    None
                } else {
                    Some((
                        s.saturating_sub(offset),
                        (e - offset).min(inline_len),
                    ))
                }
            });

            elements.push(Self::render_inline_with_selection(inline, t, inline_sel));
            offset += inline_len;
        }

        div()
            .flex()
            .flex_wrap()
            .items_baseline()
            .text_size(px(14.0))
            .line_height(px(22.0))
            .text_color(rgb(t.text_secondary))
            .children(elements)
    }

    /// Render a single inline element with selection.
    fn render_inline_with_selection(inline: &Inline, t: &ThemeColors, selection: Option<(usize, usize)>) -> Div {
        let selection_bg = rgba(0x3390ff40);

        match inline {
            Inline::Text(text) => {
                if let Some((start, end)) = selection {
                    let (before, selected, after) = slice_by_chars(text, start, end);
                    div()
                        .flex()
                        .child(div().child(before))
                        .child(div().bg(selection_bg).child(selected))
                        .child(div().child(after))
                } else {
                    div().child(text.clone())
                }
            }
            Inline::Code(code) => {
                if let Some((start, end)) = selection {
                    let (before, selected, after) = slice_by_chars(code, start, end);
                    div()
                        .font_family("monospace")
                        .text_size(px(13.0))
                        .px(px(4.0))
                        .rounded(px(3.0))
                        .bg(rgb(t.bg_primary))
                        .text_color(rgb(t.text_primary))
                        .flex()
                        .child(div().child(before))
                        .child(div().bg(selection_bg).child(selected))
                        .child(div().child(after))
                } else {
                    div()
                        .font_family("monospace")
                        .text_size(px(13.0))
                        .px(px(4.0))
                        .rounded(px(3.0))
                        .bg(rgb(t.bg_primary))
                        .text_color(rgb(t.text_primary))
                        .child(code.clone())
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
                    container = container.child(Self::render_inline_with_selection(child, t, child_sel));
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
                    container = container.child(Self::render_inline_with_selection(child, t, child_sel));
                    offset += child_len;
                }
                container
            }
            Inline::Link { children, .. } => {
                let mut container = div()
                    .text_color(rgb(t.term_blue))
                    .underline()
                    .flex()
                    .flex_wrap();
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
                    container = container.child(Self::render_inline_with_selection(child, t, child_sel));
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
        t: &ThemeColors,
        selection: Option<(usize, usize)>,
    ) -> Div {
        // Calculate column widths based on content (using character count)
        let mut col_widths: Vec<usize> = headers
            .iter()
            .map(|h| char_len(&Self::render_inlines_as_text(h)))
            .collect();

        for row in rows {
            for (i, cell) in row.iter().enumerate() {
                let len = char_len(&Self::render_inlines_as_text(cell));
                if i < col_widths.len() {
                    col_widths[i] = col_widths[i].max(len);
                }
            }
        }

        let mut table = v_flex()
            .rounded(px(4.0))
            .border_1()
            .border_color(rgb(t.border))
            .overflow_hidden();

        let mut offset = 0usize;

        // Header row
        if !headers.is_empty() {
            let mut header_row = div()
                .flex()
                .bg(rgb(t.bg_header))
                .border_b_1()
                .border_color(rgb(t.border));

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
                    div()
                        .min_w(px(min_w))
                        .px(px(12.0))
                        .py(px(8.0))
                        .child(
                            Self::render_inlines_with_selection(header, t, cell_sel)
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(t.text_primary))
                        )
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
                .when(row_idx % 2 == 1, |d| d.bg(rgb(t.bg_secondary)));

            if row_idx < rows.len() - 1 {
                row_div = row_div.border_b_1().border_color(rgb(t.border));
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
                    div()
                        .min_w(px(min_w))
                        .px(px(12.0))
                        .py(px(6.0))
                        .child(
                            Self::render_inlines_with_selection(cell, t, cell_sel)
                                .text_size(px(12.0))
                                .text_color(rgb(t.text_secondary))
                        )
                );
                offset += cell_len;
            }
            offset += 1; // newline
            table = table.child(row_div);
        }

        table
    }

    /// Render inlines as plain text (for measuring, headings, etc.).
    pub(super) fn render_inlines_as_text(inlines: &[Inline]) -> String {
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
    fn render_heading_text_with_selection(inlines: &[Inline], sel_start: usize, sel_end: usize) -> Div {
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
