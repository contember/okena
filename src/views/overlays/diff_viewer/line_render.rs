//! Line rendering for the diff viewer.

use super::types::{DisplayLine, HighlightedSpan};
use super::{DiffViewer, SIDEBAR_WIDTH};
use crate::git::DiffLineType;
use crate::theme::ThemeColors;
use crate::ui::Selection2DExtension;
use gpui::prelude::*;
use gpui::*;

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
    ) -> impl IntoElement {
        let has_selection = self.selection.line_has_selection(line_index);

        let old_num = line
            .old_line_num
            .map(|n| format!("{:>width$}", n, width = self.line_num_width))
            .unwrap_or_else(|| " ".repeat(self.line_num_width));
        let new_num = line
            .new_line_num
            .map(|n| format!("{:>width$}", n, width = self.line_num_width))
            .unwrap_or_else(|| " ".repeat(self.line_num_width));

        let (indicator, bg_color, indicator_color) = match line.line_type {
            DiffLineType::Added => ("+", Some(t.diff_added_bg), t.diff_added_fg),
            DiffLineType::Removed => ("-", Some(t.diff_removed_bg), t.diff_removed_fg),
            DiffLineType::Header => ("", Some(t.diff_hunk_header_bg), t.diff_hunk_header_fg),
            DiffLineType::Context => (" ", None, t.text_secondary),
        };

        let is_header = line.line_type == DiffLineType::Header;
        let spans = line.spans.clone();
        let plain_text = line.plain_text.clone();
        let font_size = self.file_font_size;
        let line_height = font_size * 1.5;

        div()
            .id(ElementId::Name(format!("diff-line-{}", line_index).into()))
            .flex()
            .h(px(line_height))
            .text_size(px(font_size))
            .font_family("monospace")
            .when(bg_color.is_some(), |d| d.bg(rgb(bg_color.unwrap())))
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
            .when(!is_header, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .child(
                            div()
                                .w(px((self.line_num_width * 8) as f32))
                                .text_color(rgb(t.text_muted))
                                .text_right()
                                .child(old_num),
                        )
                        .child(
                            div()
                                .w(px((self.line_num_width * 8 + 8) as f32))
                                .pl(px(8.0))
                                .text_color(rgb(t.text_muted))
                                .text_right()
                                .child(new_num),
                        )
                        .child(
                            div()
                                .w(px(16.0))
                                .text_center()
                                .text_color(rgb(indicator_color))
                                .child(indicator),
                        ),
                )
            })
            .child(if has_selection {
                self.render_line_with_selection(line_index, &plain_text, &spans, is_header)
            } else {
                div()
                    .flex_1()
                    .flex()
                    .overflow_hidden()
                    .when(is_header, |d| d.font_weight(FontWeight::MEDIUM).pl(px(8.0)))
                    .children(spans.iter().map(|span| {
                        div().text_color(span.color).child(span.text.clone())
                    }))
            })
    }

    /// Render a line with selection highlighting.
    pub(super) fn render_line_with_selection(
        &self,
        line_index: usize,
        plain_text: &str,
        spans: &[HighlightedSpan],
        is_header: bool,
    ) -> Div {
        let selection_bg = Rgba {
            r: 0.2,
            g: 0.4,
            b: 0.7,
            a: 0.4,
        };

        let ((start_line, start_col), (end_line, end_col)) = match self.selection.normalized() {
            Some(range) => range,
            None => {
                return div()
                    .flex_1()
                    .flex()
                    .overflow_hidden()
                    .when(is_header, |d| d.font_weight(FontWeight::MEDIUM).pl(px(8.0)))
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
            .overflow_hidden()
            .when(is_header, |d| d.font_weight(FontWeight::MEDIUM).pl(px(8.0)))
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
        let Some(file) = self.files.get(self.selected_file_index) else {
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
