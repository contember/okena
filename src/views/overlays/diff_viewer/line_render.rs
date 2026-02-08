//! Line rendering for the diff viewer.

use super::types::{DisplayLine, HighlightedSpan};
use super::{DiffViewer, SIDEBAR_WIDTH};
use crate::git::DiffLineType;
use crate::theme::ThemeColors;
use crate::ui::Selection2DExtension;
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;

/// Helper to create rgba from u32 color and alpha.
fn rgba(color: u32, alpha: f32) -> Rgba {
    let r = ((color >> 16) & 0xFF) as f32 / 255.0;
    let g = ((color >> 8) & 0xFF) as f32 / 255.0;
    let b = (color & 0xFF) as f32 / 255.0;
    Rgba { r, g, b, a: alpha }
}

impl DiffViewer {
    /// Calculate column position from x coordinate.
    pub(super) fn x_to_column(&self, x: f32, gutter_width: f32) -> usize {
        // Approximate char width based on font size (monospace fonts are ~0.6 of font size)
        let char_width = self.file_font_size * 0.6;
        let text_x = (x - gutter_width - SIDEBAR_WIDTH).max(0.0);
        (text_x / char_width) as usize
    }

    /// Render a single diff line with syntax highlighting.
    pub(super) fn render_line(
        &self,
        line_index: usize,
        line: &DisplayLine,
        t: &ThemeColors,
        gutter_width: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let has_selection = self.selection.line_has_selection(line_index);
        let font_size = self.file_font_size;
        let line_height = font_size * 1.6;
        let is_header = line.line_type == DiffLineType::Header;

        // Chunk header is rendered completely differently - full width separator
        if is_header {
            return self.render_chunk_header(line, t, font_size, line_height, line_index);
        }

        let old_num = line
            .old_line_num
            .map(|n| format!("{:>width$}", n, width = self.line_num_width))
            .unwrap_or_else(|| " ".repeat(self.line_num_width));
        let new_num = line
            .new_line_num
            .map(|n| format!("{:>width$}", n, width = self.line_num_width))
            .unwrap_or_else(|| " ".repeat(self.line_num_width));

        // Two-level background: light tint for the line, context has no tint
        let (indicator, line_bg, indicator_color) = match line.line_type {
            DiffLineType::Added => ("+", Some(rgba(t.diff_added_bg, 0.4)), t.diff_added_fg),
            DiffLineType::Removed => ("-", Some(rgba(t.diff_removed_bg, 0.4)), t.diff_removed_fg),
            DiffLineType::Context => (" ", None, t.text_muted),
            DiffLineType::Header => unreachable!(),
        };

        let spans = line.spans.clone();
        let plain_text = line.plain_text.clone();

        // Character width for monospace font (approximately 0.6 of font size)
        let char_width = font_size * 0.6;
        let num_col_width = (self.line_num_width as f32) * char_width + 8.0;

        div()
            .id(ElementId::Name(format!("diff-line-{}", line_index).into()))
            .flex()
            .h(px(line_height))
            .text_size(px(font_size))
            .font_family("monospace")
            .when(line_bg.is_some(), |d| d.bg(line_bg.unwrap()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    let col = this.x_to_column(f32::from(event.position.x), gutter_width);
                    this.selection.start = Some((line_index, col));
                    this.selection.end = Some((line_index, col));
                    this.selection.is_selecting = true;
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                if this.selection.is_selecting {
                    let col = this.x_to_column(f32::from(event.position.x), gutter_width);
                    this.selection.end = Some((line_index, col));
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    this.selection.finish();
                    cx.notify();
                }),
            )
            // Gutter with line numbers
            .child(
                h_flex()
                    .flex_shrink_0()
                    .h_full()
                    .child(
                        div()
                            .w(px(num_col_width))
                            .pr(px(4.0))
                            .text_color(rgb(t.text_muted))
                            .text_right()
                            .child(old_num),
                    )
                    .child(
                        div()
                            .w(px(num_col_width))
                            .pr(px(4.0))
                            .text_color(rgb(t.text_muted))
                            .text_right()
                            .child(new_num),
                    )
                    .child(
                        div()
                            .w(px(20.0))
                            .text_center()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(indicator_color))
                            .child(indicator),
                    ),
            )
            // Content
            .child(if has_selection {
                self.render_line_with_selection(line_index, &plain_text, &spans, false)
            } else {
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .pl(px(4.0))
                    .overflow_hidden()
                    .children(spans.iter().map(|span| {
                        div().text_color(span.color).child(span.text.clone())
                    }))
            })
    }

    /// Render a chunk header (@@ ... @@) as a full-width separator.
    fn render_chunk_header(
        &self,
        line: &DisplayLine,
        t: &ThemeColors,
        font_size: f32,
        line_height: f32,
        line_index: usize,
    ) -> Stateful<Div> {
        div()
            .id(ElementId::Name(format!("diff-header-{}", line_index).into()))
            .w_full()
            .h(px(line_height))
            .flex()
            .items_center()
            .bg(rgba(t.diff_hunk_header_bg, 0.6))
            .border_y_1()
            .border_color(rgb(t.border))
            .px(px(12.0))
            .child(
                div()
                    .text_size(px(font_size * 0.85))
                    .text_color(rgb(t.diff_hunk_header_fg))
                    .children(line.spans.iter().map(|span| {
                        div().text_color(span.color).child(span.text.clone())
                    })),
            )
    }

    /// Render a line with selection highlighting.
    pub(super) fn render_line_with_selection(
        &self,
        line_index: usize,
        plain_text: &str,
        spans: &[HighlightedSpan],
        _is_header: bool,
    ) -> Div {
        let selection_bg = Rgba {
            r: 0.25,
            g: 0.45,
            b: 0.75,
            a: 0.35,
        };

        let ((start_line, start_col), (end_line, end_col)) = match self.selection.normalized() {
            Some(range) => range,
            None => {
                return div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .pl(px(4.0))
                    .overflow_hidden()
                    .children(spans.iter().map(|span| {
                        div().text_color(span.color).child(span.text.clone())
                    }));
            }
        };

        let line_len = plain_text.len();
        let sel_start = if line_index == start_line {
            start_col.min(line_len)
        } else {
            0
        };
        let sel_end = if line_index == end_line {
            end_col.min(line_len)
        } else {
            line_len
        };

        let mut elements: Vec<Div> = Vec::new();
        let mut current_col = 0;

        for span in spans {
            let span_len = span.text.len();
            let span_end = current_col + span_len;

            let span_sel_start = sel_start.max(current_col);
            let span_sel_end = sel_end.min(span_end);

            if span_sel_start < span_sel_end
                && span_sel_start < span_end
                && span_sel_end > current_col
            {
                let rel_sel_start = span_sel_start - current_col;
                let rel_sel_end = span_sel_end - current_col;

                if rel_sel_start > 0 {
                    elements.push(
                        div()
                            .text_color(span.color)
                            .child(span.text[..rel_sel_start].to_string()),
                    );
                }

                elements.push(
                    div()
                        .bg(selection_bg)
                        .text_color(span.color)
                        .child(span.text[rel_sel_start..rel_sel_end].to_string()),
                );

                if rel_sel_end < span_len {
                    elements.push(
                        div()
                            .text_color(span.color)
                            .child(span.text[rel_sel_end..].to_string()),
                    );
                }
            } else {
                elements.push(div().text_color(span.color).child(span.text.clone()));
            }

            current_col = span_end;
        }

        div()
            .flex_1()
            .flex()
            .items_center()
            .pl(px(4.0))
            .overflow_hidden()
            .children(elements)
    }

    /// Render visible lines for the virtualized list.
    pub(super) fn render_visible_lines(
        &self,
        range: std::ops::Range<usize>,
        t: &ThemeColors,
        gutter_width: f32,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(file) = &self.current_file else {
            return Vec::new();
        };

        range
            .filter_map(|i| {
                file.lines
                    .get(i)
                    .map(|line| self.render_line(i, line, t, gutter_width, cx).into_any_element())
            })
            .collect()
    }
}
