//! Side-by-side diff view transformation and rendering.

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
                        header_spans: Vec::new(),
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
        let line_height = font_size * 1.6;

        if line.is_header {
            // Header spans both columns
            div()
                .flex()
                .h(px(line_height))
                .text_size(px(font_size * 0.9))
                .font_family("monospace")
                .bg(rgb(t.diff_hunk_header_bg))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .pl(px(12.0))
                        .text_color(rgb(t.diff_hunk_header_fg))
                        .children(line.header_spans.iter().map(|span| {
                            div().text_color(span.color).child(span.text.clone())
                        })),
                )
        } else {
            // Two-column layout
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
        _is_left: bool,
        line_num_width: usize,
        line_height: f32,
    ) -> Div {
        match content {
            Some(c) => {
                // Two-level background: light tint for the line, stronger for changed words
                let (indicator, line_bg, word_bg, indicator_color) = match c.line_type {
                    DiffLineType::Added => (
                        "+",
                        Some(rgba(t.diff_added_bg, 0.35)),
                        Some(rgba(t.diff_added_bg, 0.8)),
                        t.diff_added_fg,
                    ),
                    DiffLineType::Removed => (
                        "-",
                        Some(rgba(t.diff_removed_bg, 0.35)),
                        Some(rgba(t.diff_removed_bg, 0.8)),
                        t.diff_removed_fg,
                    ),
                    DiffLineType::Context => (" ", None, None, t.text_muted),
                    DiffLineType::Header => ("", None, None, t.text_secondary),
                };

                // Format line number - show empty for 0
                let line_num = if c.line_num > 0 {
                    format!("{:>width$}", c.line_num, width = line_num_width)
                } else {
                    " ".repeat(line_num_width)
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

                // Gutter with line number and indicator
                let gutter = div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .h_full()
                    .pl(px(8.0))
                    .child(
                        div()
                            .w(px((line_num_width * 8) as f32))
                            .text_color(rgb(t.text_muted))
                            .text_right()
                            .child(line_num),
                    )
                    .child(
                        div()
                            .w(px(24.0))
                            .text_center()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(indicator_color))
                            .child(indicator),
                    );

                // Content with word-level highlighting
                let content_div = self.render_spans_with_word_highlight(
                    &c.spans,
                    &c.changed_ranges,
                    word_bg,
                );

                column.child(gutter).child(content_div)
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
    fn render_spans_with_word_highlight(
        &self,
        spans: &[super::types::HighlightedSpan],
        changed_ranges: &[ChangedRange],
        word_bg: Option<Rgba>,
    ) -> Div {
        let mut content_div = div().flex_1().flex().pl(px(4.0)).overflow_hidden();

        if changed_ranges.is_empty() || word_bg.is_none() {
            // No word-level highlighting, just render spans normally
            for span in spans {
                content_div = content_div.child(div().text_color(span.color).child(span.text.clone()));
            }
            return content_div;
        }

        let word_bg = word_bg.unwrap();
        let mut current_col = 0;

        for span in spans {
            let span_chars: Vec<char> = span.text.chars().collect();
            let span_len = span_chars.len();
            let span_end = current_col + span_len;

            // Check if any changed range overlaps this span
            let mut char_idx = 0;
            while char_idx < span_len {
                let global_idx = current_col + char_idx;

                // Find if this character is in a changed range
                let in_changed = changed_ranges
                    .iter()
                    .any(|r| global_idx >= r.start && global_idx < r.end);

                // Find the extent of this segment (changed or unchanged)
                let segment_start = char_idx;
                while char_idx < span_len {
                    let g_idx = current_col + char_idx;
                    let is_changed = changed_ranges
                        .iter()
                        .any(|r| g_idx >= r.start && g_idx < r.end);
                    if is_changed != in_changed {
                        break;
                    }
                    char_idx += 1;
                }

                // Render this segment
                let segment: String = span_chars[segment_start..char_idx].iter().collect();
                if !segment.is_empty() {
                    let mut seg_div = div().text_color(span.color);
                    if in_changed {
                        seg_div = seg_div.bg(word_bg).rounded(px(2.0));
                    }
                    content_div = content_div.child(seg_div.child(segment));
                }
            }

            current_col = span_end;
        }

        content_div
    }
}

/// Helper to create rgba from u32 color and alpha.
fn rgba(color: u32, alpha: f32) -> Rgba {
    let r = ((color >> 16) & 0xFF) as f32 / 255.0;
    let g = ((color >> 8) & 0xFF) as f32 / 255.0;
    let b = (color & 0xFF) as f32 / 255.0;
    Rgba { r, g, b, a: alpha }
}
