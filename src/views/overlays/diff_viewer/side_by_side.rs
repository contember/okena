//! Side-by-side diff view transformation and rendering.

use super::line_render::{rgba, ACCENT_WIDTH};
use super::types::{ChangedRange, DisplayLine, SideBySideLine, SideContent};
use super::DiffViewer;
use crate::git::DiffLineType;
use crate::theme::ThemeColors;
use gpui::prelude::*;
use gpui::*;

/// Compute the changed character ranges between two strings.
/// Returns (old_ranges, new_ranges) - the ranges in each string that differ.
fn compute_changed_ranges(old: &str, new: &str) -> (Vec<ChangedRange>, Vec<ChangedRange>) {
    let old_chars: Vec<char> = old.chars().collect();
    let new_chars: Vec<char> = new.chars().collect();

    // Find common prefix length
    let prefix_len = old_chars
        .iter()
        .zip(new_chars.iter())
        .take_while(|(a, b)| a == b)
        .count();

    // Find common suffix length (but don't overlap with prefix)
    let old_remaining = old_chars.len() - prefix_len;
    let new_remaining = new_chars.len() - prefix_len;
    let suffix_len = old_chars
        .iter()
        .rev()
        .zip(new_chars.iter().rev())
        .take(old_remaining.min(new_remaining))
        .take_while(|(a, b)| a == b)
        .count();

    let old_change_end = old_chars.len() - suffix_len;
    let new_change_end = new_chars.len() - suffix_len;

    // If there's an actual change in the middle
    let old_ranges = if prefix_len < old_change_end {
        vec![ChangedRange {
            start: prefix_len,
            end: old_change_end,
        }]
    } else {
        vec![]
    };

    let new_ranges = if prefix_len < new_change_end {
        vec![ChangedRange {
            start: prefix_len,
            end: new_change_end,
        }]
    } else {
        vec![]
    };

    (old_ranges, new_ranges)
}

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
                });
                i += 1;
            }
            DiffLineType::Context => {
                let content = SideContent {
                    line_num: line.old_line_num.unwrap_or(0),
                    line_type: DiffLineType::Context,
                    spans: line.spans.clone(),
                    plain_text: line.plain_text.clone(),
                    changed_ranges: vec![],
                };
                result.push(SideBySideLine {
                    left: Some(content.clone()),
                    right: Some(SideContent {
                        line_num: line.new_line_num.unwrap_or(0),
                        ..content
                    }),
                    is_header: false,
                    header_text: String::new(),
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

                // Pair them up with word-level diff
                let max_len = removed_lines.len().max(added_lines.len());
                for j in 0..max_len {
                    // Compute changed ranges if both sides exist
                    let (old_ranges, new_ranges) =
                        if let (Some(old_line), Some(new_line)) =
                            (removed_lines.get(j), added_lines.get(j))
                        {
                            compute_changed_ranges(&old_line.plain_text, &new_line.plain_text)
                        } else {
                            (vec![], vec![])
                        };

                    let left = removed_lines.get(j).map(|l| SideContent {
                        line_num: l.old_line_num.unwrap_or(0),
                        line_type: DiffLineType::Removed,
                        spans: l.spans.clone(),
                        plain_text: l.plain_text.clone(),
                        changed_ranges: old_ranges,
                    });
                    let right = added_lines.get(j).map(|l| SideContent {
                        line_num: l.new_line_num.unwrap_or(0),
                        line_type: DiffLineType::Added,
                        spans: l.spans.clone(),
                        plain_text: l.plain_text.clone(),
                        changed_ranges: new_ranges,
                    });
                    result.push(SideBySideLine {
                        left,
                        right,
                        is_header: false,
                        header_text: String::new(),
                    });
                }
            }
            DiffLineType::Added => {
                // Pure addition without preceding removal - highlight entire line
                let full_range = if !line.plain_text.is_empty() {
                    vec![ChangedRange {
                        start: 0,
                        end: line.plain_text.chars().count(),
                    }]
                } else {
                    vec![]
                };
                result.push(SideBySideLine {
                    left: None,
                    right: Some(SideContent {
                        line_num: line.new_line_num.unwrap_or(0),
                        line_type: DiffLineType::Added,
                        spans: line.spans.clone(),
                        plain_text: line.plain_text.clone(),
                        changed_ranges: full_range,
                    }),
                    is_header: false,
                    header_text: String::new(),
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
        idx: usize,
        line: &SideBySideLine,
        t: &ThemeColors,
        _cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let font_size = self.file_font_size;
        let line_height = self.line_height();

        if line.is_header {
            return self.render_hunk_header(&line.header_text, idx, "sbs-header", t);
        }

        // Two-column layout
        let left = line.left.clone();
        let right = line.right.clone();
        let border_color = t.border;

        div()
            .id(ElementId::Name(format!("sbs-line-{}", idx).into()))
            .w_full()
            .h(px(line_height))
            .text_size(px(font_size))
            .font_family("monospace")
            .flex()
            .child(self.render_side_column_content(&left, t, line_height))
            .child(
                div()
                    .w(px(1.0))
                    .h(px(line_height))
                    .bg(rgb(border_color))
                    .flex_shrink_0(),
            )
            .child(self.render_side_column_content(&right, t, line_height))
    }

    /// Render one column (left or right) of a side-by-side line.
    fn render_side_column_content(
        &self,
        content: &Option<SideContent>,
        t: &ThemeColors,
        line_height: f32,
    ) -> Div {
        let char_width = self.char_width();
        let num_col_width = (self.line_num_width as f32) * char_width + 8.0;

        match content {
            Some(c) => {
                let (line_bg, word_bg, accent_color) = self.line_colors(c.line_type, t);

                // Format line number - show empty for 0
                let line_num = if c.line_num > 0 {
                    format!("{:>width$}", c.line_num, width = self.line_num_width)
                } else {
                    " ".repeat(self.line_num_width)
                };

                let mut column = div()
                    .flex_1()
                    .h(px(line_height))
                    .flex()
                    .items_center()
                    .overflow_hidden();

                if let Some(bg) = line_bg {
                    column = column.bg(bg);
                }

                // Left accent bar (fixed width child, always present for alignment)
                let accent = div()
                    .w(px(ACCENT_WIDTH))
                    .h_full()
                    .flex_shrink_0()
                    .when_some(accent_color, |d, color| d.bg(color));

                // Gutter with line number
                let gutter = div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .h_full()
                    .child(
                        div()
                            .w(px(num_col_width))
                            .pr(px(8.0))
                            .text_color(rgba(t.text_muted, 0.6))
                            .text_right()
                            .child(line_num),
                    )
                    // Subtle separator
                    .child(
                        div()
                            .w(px(1.0))
                            .h(px(line_height * 0.6))
                            .bg(rgba(t.border, 0.3))
                            .flex_shrink_0(),
                    );

                // Content with word-level highlighting
                let content_div = self.render_spans_with_word_highlight(
                    &c.spans,
                    &c.changed_ranges,
                    word_bg,
                    line_height,
                );

                column.child(accent).child(gutter).child(content_div)
            }
            None => {
                // Empty side - very subtle background
                div()
                    .flex_1()
                    .h(px(line_height))
                    .bg(rgba(t.bg_secondary, 0.5))
            }
        }
    }

    /// Render spans with word-level highlighting for changed ranges.
    /// Uses StyledText for gap-free rendering.
    fn render_spans_with_word_highlight(
        &self,
        spans: &[super::types::HighlightedSpan],
        changed_ranges: &[ChangedRange],
        word_bg: Option<Rgba>,
        line_height: f32,
    ) -> Div {
        // Convert char-based changed_ranges to byte-based background ranges
        let bg_ranges: Vec<(std::ops::Range<usize>, Hsla)> =
            if let Some(word_bg) = word_bg {
                if !changed_ranges.is_empty() {
                    // Build text to get char-to-byte mapping
                    let text: String = spans.iter().map(|s| s.text.as_str()).collect();
                    let chars: Vec<char> = text.chars().collect();

                    changed_ranges
                        .iter()
                        .filter_map(|range| {
                            let byte_start: usize = chars[..range.start.min(chars.len())]
                                .iter()
                                .map(|c| c.len_utf8())
                                .sum();
                            let byte_end: usize = chars[..range.end.min(chars.len())]
                                .iter()
                                .map(|c| c.len_utf8())
                                .sum();
                            if byte_start < byte_end {
                                Some((byte_start..byte_end, Hsla::from(word_bg)))
                            } else {
                                None
                            }
                        })
                        .collect()
                } else {
                    vec![]
                }
            } else {
                vec![]
            };

        self.render_scrollable_content(spans, &bg_ranges, line_height)
    }
}
