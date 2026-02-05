//! Side-by-side diff view transformation and rendering.

use super::types::{DisplayLine, SideBySideLine, SideContent};
use super::DiffViewer;
use crate::git::DiffLineType;
use crate::theme::ThemeColors;
use gpui::prelude::*;
use gpui::*;

/// Transform unified diff lines into side-by-side format.
///
/// Algorithm:
/// - Context lines appear on both sides with the same content
/// - Header lines span both sides as a separator
/// - Removed/Added lines are paired: removed on left, added on right
/// - If counts differ, extra lines have None on the opposite side
pub fn to_side_by_side(lines: &[DisplayLine]) -> Vec<SideBySideLine> {
    let mut result = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = &lines[i];

        match line.line_type {
            DiffLineType::Header => {
                result.push(SideBySideLine {
                    left: None,
                    right: None,
                    is_header: true,
                    header_text: line.plain_text.clone(),
                    header_spans: line.spans.clone(),
                });
                i += 1;
            }
            DiffLineType::Context => {
                let content = SideContent {
                    line_num: line.old_line_num.unwrap_or(0),
                    line_type: DiffLineType::Context,
                    spans: line.spans.clone(),
                    plain_text: line.plain_text.clone(),
                };
                result.push(SideBySideLine {
                    left: Some(content.clone()),
                    right: Some(SideContent {
                        line_num: line.new_line_num.unwrap_or(0),
                        ..content
                    }),
                    is_header: false,
                    header_text: String::new(),
                    header_spans: Vec::new(),
                });
                i += 1;
            }
            DiffLineType::Removed => {
                // Collect consecutive removed lines
                let mut removed_lines = Vec::new();
                while i < lines.len() && lines[i].line_type == DiffLineType::Removed {
                    removed_lines.push(&lines[i]);
                    i += 1;
                }

                // Collect following consecutive added lines
                let mut added_lines = Vec::new();
                while i < lines.len() && lines[i].line_type == DiffLineType::Added {
                    added_lines.push(&lines[i]);
                    i += 1;
                }

                // Pair them up
                let max_len = removed_lines.len().max(added_lines.len());
                for j in 0..max_len {
                    let left = removed_lines.get(j).map(|l| SideContent {
                        line_num: l.old_line_num.unwrap_or(0),
                        line_type: DiffLineType::Removed,
                        spans: l.spans.clone(),
                        plain_text: l.plain_text.clone(),
                    });
                    let right = added_lines.get(j).map(|l| SideContent {
                        line_num: l.new_line_num.unwrap_or(0),
                        line_type: DiffLineType::Added,
                        spans: l.spans.clone(),
                        plain_text: l.plain_text.clone(),
                    });
                    result.push(SideBySideLine {
                        left,
                        right,
                        is_header: false,
                        header_text: String::new(),
                        header_spans: Vec::new(),
                    });
                }
            }
            DiffLineType::Added => {
                // Pure addition without preceding removal
                result.push(SideBySideLine {
                    left: None,
                    right: Some(SideContent {
                        line_num: line.new_line_num.unwrap_or(0),
                        line_type: DiffLineType::Added,
                        spans: line.spans.clone(),
                        plain_text: line.plain_text.clone(),
                    }),
                    is_header: false,
                    header_text: String::new(),
                    header_spans: Vec::new(),
                });
                i += 1;
            }
        }
    }

    result
}

impl DiffViewer {
    /// Render visible lines for side-by-side view.
    pub(super) fn render_side_by_side_lines(
        &self,
        range: std::ops::Range<usize>,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        range
            .filter_map(|i| {
                self.side_by_side_lines
                    .get(i)
                    .map(|line| self.render_side_by_side_line(i, line, t, cx).into_any_element())
            })
            .collect()
    }

    /// Render a single side-by-side line.
    fn render_side_by_side_line(
        &self,
        _idx: usize,
        line: &SideBySideLine,
        t: &ThemeColors,
        _cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let font_size = self.file_font_size;
        let line_height = font_size * 1.5;

        if line.is_header {
            // Header spans both columns
            div()
                .flex()
                .h(px(line_height))
                .text_size(px(font_size))
                .font_family("monospace")
                .bg(rgb(t.diff_hunk_header_bg))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .pl(px(8.0))
                        .font_weight(FontWeight::MEDIUM)
                        .children(line.header_spans.iter().map(|span| {
                            div().text_color(span.color).child(span.text.clone())
                        })),
                )
        } else {
            // Two-column layout using a table-like structure
            let left = line.left.clone();
            let right = line.right.clone();
            let line_num_width = self.line_num_width;
            let border_color = t.border;

            div()
                .w_full()
                .h(px(line_height))
                .text_size(px(font_size))
                .font_family("monospace")
                .flex()
                .child(self.render_side_column_content(&left, t, true, line_num_width, line_height))
                .child(
                    div()
                        .w(px(1.0))
                        .h(px(line_height))
                        .bg(rgb(border_color))
                        .flex_shrink_0(),
                )
                .child(self.render_side_column_content(&right, t, false, line_num_width, line_height))
        }
    }

    /// Render one column (left or right) of a side-by-side line.
    fn render_side_column_content(
        &self,
        content: &Option<SideContent>,
        t: &ThemeColors,
        is_left: bool,
        line_num_width: usize,
        line_height: f32,
    ) -> Div {
        match content {
            Some(c) => {
                let (indicator, bg_color, indicator_color) = match c.line_type {
                    DiffLineType::Added => ("+", Some(t.diff_added_bg), t.diff_added_fg),
                    DiffLineType::Removed => ("-", Some(t.diff_removed_bg), t.diff_removed_fg),
                    DiffLineType::Context => (" ", None, t.text_secondary),
                    DiffLineType::Header => ("", None, t.text_secondary),
                };

                let line_num = format!("{:>width$}", c.line_num, width = line_num_width);

                let mut column = div()
                    .flex_1()
                    .h(px(line_height))
                    .flex()
                    .items_center()
                    .overflow_hidden();

                if let Some(bg) = bg_color {
                    column = column.bg(rgb(bg));
                }

                // Gutter with line number and indicator
                let gutter = div()
                    .flex_shrink_0()
                    .flex()
                    .child(
                        div()
                            .w(px((line_num_width * 8) as f32))
                            .text_color(rgb(t.text_muted))
                            .text_right()
                            .child(line_num),
                    )
                    .child(
                        div()
                            .w(px(16.0))
                            .text_center()
                            .text_color(rgb(indicator_color))
                            .child(indicator),
                    );

                // Content with syntax highlighted spans
                let mut content_div = div().flex_1().flex().overflow_hidden();
                for span in &c.spans {
                    content_div = content_div.child(
                        div().text_color(span.color).child(span.text.clone())
                    );
                }

                column.child(gutter).child(content_div)
            }
            None => {
                // Empty side - show muted background
                let bg = if is_left {
                    t.diff_removed_bg
                } else {
                    t.diff_added_bg
                };
                div()
                    .flex_1()
                    .h(px(line_height))
                    .bg(rgba(bg, 0.3))
            }
        }
    }
}

/// Helper to create rgba from u32 color and alpha.
fn rgba(color: u32, alpha: f32) -> Rgba {
    let r = ((color >> 16) & 0xFF) as f32 / 255.0;
    let g = ((color >> 8) & 0xFF) as f32 / 255.0;
    let b = (color & 0xFF) as f32 / 255.0;
    Rgba { r, g, b, a: alpha }
}
