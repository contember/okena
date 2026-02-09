//! Line rendering for the diff viewer.
//!
//! Also contains shared rendering helpers, constants, and methods used by
//! both the unified and side-by-side diff views.

use super::types::{DisplayLine, HighlightedSpan};
use super::{DiffViewer, SIDEBAR_WIDTH};
use crate::git::DiffLineType;
use crate::theme::ThemeColors;
use crate::ui::Selection2DExtension;
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;

// ── Shared constants ────────────────────────────────────────────────────

/// Width of the left accent indicator bar (always reserved for alignment).
pub(super) const ACCENT_WIDTH: f32 = 3.0;
/// Background alpha for changed lines (subtle tint).
pub(super) const LINE_BG_ALPHA: f32 = 0.06;
/// Background alpha for word-level diff highlights.
pub(super) const WORD_BG_ALPHA: f32 = 0.18;
/// Alpha for the left accent bar.
pub(super) const ACCENT_ALPHA: f32 = 0.7;
/// Line height as a multiple of font size.
pub(super) const LINE_HEIGHT_FACTOR: f32 = 1.8;
/// Character width as a fraction of font size (monospace approximation).
pub(super) const CHAR_WIDTH_FACTOR: f32 = 0.6;
/// Padding before text content.
pub(super) const CONTENT_PADDING: f32 = 10.0;

// ── Shared helper functions ─────────────────────────────────────────────

/// Helper to create rgba from u32 color and alpha.
pub(super) fn rgba(color: u32, alpha: f32) -> Rgba {
    let r = ((color >> 16) & 0xFF) as f32 / 255.0;
    let g = ((color >> 8) & 0xFF) as f32 / 255.0;
    let b = (color & 0xFF) as f32 / 255.0;
    Rgba { r, g, b, a: alpha }
}

/// Extract the function/context name from a hunk header.
/// Input:  "@@ -19,6 +19,7 @@ fn some_function"
/// Output: "fn some_function" (or empty if no context)
pub(super) fn extract_hunk_context(header: &str) -> &str {
    if let Some(pos) = header.find("@@") {
        let rest = &header[pos + 2..];
        if let Some(pos2) = rest.find("@@") {
            let context = rest[pos2 + 2..].trim();
            if !context.is_empty() {
                return context;
            }
        }
    }
    ""
}

/// Build a StyledText with optional background highlights (e.g. selection or word-level diff).
/// Splits syntax color highlights at background range boundaries to produce
/// non-overlapping highlights (required by `StyledText::compute_runs`).
pub(super) fn build_styled_text_with_backgrounds(
    spans: &[HighlightedSpan],
    bg_ranges: &[(std::ops::Range<usize>, Hsla)],
) -> StyledText {
    let mut text = String::new();
    let mut highlights = Vec::new();

    for span in spans {
        text.push_str(&span.text);
    }

    if bg_ranges.is_empty() {
        // Fast path: no background highlights, just syntax colors
        let mut offset = 0;
        for span in spans {
            let start = offset;
            offset += span.text.len();
            if start < offset {
                highlights.push((
                    start..offset,
                    HighlightStyle {
                        color: Some(span.color.into()),
                        ..Default::default()
                    },
                ));
            }
        }
    } else {
        // Split syntax spans at background range boundaries so no highlights overlap
        let mut offset = 0;
        for span in spans {
            let span_start = offset;
            let span_end = offset + span.text.len();
            offset = span_end;

            if span_start >= span_end {
                continue;
            }

            // Collect boundary points from bg_ranges that fall within this span
            let mut boundaries = vec![span_start];
            for (br, _) in bg_ranges {
                if br.start > span_start && br.start < span_end {
                    boundaries.push(br.start);
                }
                if br.end > span_start && br.end < span_end {
                    boundaries.push(br.end);
                }
            }
            boundaries.push(span_end);
            boundaries.sort();
            boundaries.dedup();

            for window in boundaries.windows(2) {
                let sub_start = window[0];
                let sub_end = window[1];
                if sub_start >= sub_end {
                    continue;
                }

                let mut style = HighlightStyle {
                    color: Some(span.color.into()),
                    ..Default::default()
                };

                // Apply background if this sub-range falls within any background range
                for (br, bg_color) in bg_ranges {
                    if sub_start >= br.start && sub_end <= br.end {
                        style.background_color = Some(*bg_color);
                        break;
                    }
                }

                highlights.push((sub_start..sub_end, style));
            }
        }
    }

    StyledText::new(text).with_highlights(highlights)
}

// ── Shared DiffViewer methods ───────────────────────────────────────────

impl DiffViewer {
    /// Line height in pixels for the current font size.
    pub(super) fn line_height(&self) -> f32 {
        self.file_font_size * LINE_HEIGHT_FACTOR
    }

    /// Approximate character width for monospace font.
    pub(super) fn char_width(&self) -> f32 {
        self.file_font_size * CHAR_WIDTH_FACTOR
    }

    /// Get background, word-highlight, and accent colors for a given line type.
    /// Returns `(line_bg, word_bg, accent_color)`.
    pub(super) fn line_colors(
        &self,
        line_type: DiffLineType,
        t: &ThemeColors,
    ) -> (Option<Rgba>, Option<Rgba>, Option<Rgba>) {
        match line_type {
            DiffLineType::Added => (
                Some(rgba(t.diff_added_bg, LINE_BG_ALPHA)),
                Some(rgba(t.diff_added_bg, WORD_BG_ALPHA)),
                Some(rgba(t.diff_added_fg, ACCENT_ALPHA)),
            ),
            DiffLineType::Removed => (
                Some(rgba(t.diff_removed_bg, LINE_BG_ALPHA)),
                Some(rgba(t.diff_removed_bg, WORD_BG_ALPHA)),
                Some(rgba(t.diff_removed_fg, ACCENT_ALPHA)),
            ),
            DiffLineType::Context | DiffLineType::Header => (None, None, None),
        }
    }

    /// Render a chunk/hunk header (@@ ... @@) as a clean separator.
    pub(super) fn render_hunk_header(
        &self,
        text: &str,
        idx: usize,
        prefix: &str,
        t: &ThemeColors,
    ) -> Stateful<Div> {
        let context = extract_hunk_context(text);
        let font_size = self.file_font_size;
        let line_height = self.line_height();

        div()
            .id(ElementId::Name(format!("{}-{}", prefix, idx).into()))
            .w_full()
            .h(px(line_height))
            .flex()
            .items_center()
            .font_family("monospace")
            .bg(rgba(t.diff_hunk_header_bg, 0.3))
            .border_t_1()
            .border_color(rgba(t.border, 0.5))
            .px(px(16.0))
            .gap(px(8.0))
            .child(
                div()
                    .w(px(32.0))
                    .h(px(1.0))
                    .bg(rgba(t.diff_hunk_header_fg, 0.3))
                    .flex_shrink_0(),
            )
            .when(!context.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(px(font_size * 0.85))
                        .text_color(rgba(t.diff_hunk_header_fg, 0.7))
                        .font_family("monospace")
                        .child(context.to_string()),
                )
            })
            .child(
                div()
                    .flex_1()
                    .h(px(1.0))
                    .bg(rgba(t.diff_hunk_header_fg, 0.15))
                    .flex_shrink_0(),
            )
    }

    /// Render scrollable content div with syntax-highlighted text.
    /// Used by both unified and side-by-side views.
    pub(super) fn render_scrollable_content(
        &self,
        spans: &[HighlightedSpan],
        bg_ranges: &[(std::ops::Range<usize>, Hsla)],
        line_height: f32,
    ) -> Div {
        div()
            .flex_1()
            .overflow_hidden()
            .whitespace_nowrap()
            .line_height(px(line_height))
            .child(
                div()
                    .pl(px(CONTENT_PADDING))
                    .ml(px(-self.scroll_x))
                    .child(build_styled_text_with_backgrounds(spans, bg_ranges)),
            )
    }

    /// Calculate column position from x coordinate.
    pub(super) fn x_to_column(&self, x: f32, gutter_width: f32) -> usize {
        let char_width = self.char_width();
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
        let line_height = self.line_height();

        if line.line_type == DiffLineType::Header {
            return self.render_hunk_header(&line.plain_text, line_index, "diff-header", t);
        }

        let old_num = line
            .old_line_num
            .map(|n| format!("{:>width$}", n, width = self.line_num_width))
            .unwrap_or_else(|| " ".repeat(self.line_num_width));
        let new_num = line
            .new_line_num
            .map(|n| format!("{:>width$}", n, width = self.line_num_width))
            .unwrap_or_else(|| " ".repeat(self.line_num_width));

        let (line_bg, _, accent_color) = self.line_colors(line.line_type, t);

        let spans = line.spans.clone();
        let plain_text = line.plain_text.clone();

        let char_width = self.char_width();
        let num_col_width = (self.line_num_width as f32) * char_width + 12.0;

        div()
            .id(ElementId::Name(format!("diff-line-{}", line_index).into()))
            .w_full()
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
            // Left accent bar (fixed width, always present for alignment)
            .child(
                div()
                    .w(px(ACCENT_WIDTH))
                    .h_full()
                    .flex_shrink_0()
                    .when_some(accent_color, |d, color| d.bg(color)),
            )
            // Gutter with line numbers
            .child(
                h_flex()
                    .flex_shrink_0()
                    .h_full()
                    .items_center()
                    .child(
                        div()
                            .w(px(num_col_width))
                            .pr(px(8.0))
                            .text_color(rgba(t.text_muted, 0.6))
                            .text_right()
                            .child(old_num),
                    )
                    .child(
                        div()
                            .w(px(num_col_width))
                            .pr(px(8.0))
                            .text_color(rgba(t.text_muted, 0.6))
                            .text_right()
                            .child(new_num),
                    )
                    // Subtle separator between gutter and content
                    .child(
                        div()
                            .w(px(1.0))
                            .h(px(line_height * 0.6))
                            .bg(rgba(t.border, 0.3))
                            .flex_shrink_0(),
                    ),
            )
            // Content — use StyledText for gap-free rendering
            .child(if has_selection {
                self.render_line_with_selection(line_index, &plain_text, &spans, false)
            } else {
                self.render_scrollable_content(&spans, &[], line_height)
            })
    }

    /// Render a line with selection highlighting (uses individual divs for selection ranges).
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
                return self.render_scrollable_content(spans, &[], self.line_height());
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

        // Build background ranges for selection
        let bg_ranges: Vec<(std::ops::Range<usize>, Hsla)> = if sel_start < sel_end {
            vec![(sel_start..sel_end, selection_bg.into())]
        } else {
            vec![]
        };

        self.render_scrollable_content(spans, &bg_ranges, self.line_height())
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
