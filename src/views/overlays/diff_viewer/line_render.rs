//! Line rendering for the diff viewer.
//!
//! Also contains shared rendering helpers, constants, and methods used by
//! both the unified and side-by-side diff views.

use super::types::{DisplayLine, HighlightedSpan};
use super::DiffViewer;
use crate::git::DiffLineType;
use crate::theme::ThemeColors;
use crate::views::components::{build_styled_text_with_backgrounds, find_word_boundaries, selection_bg_ranges};
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

// ── Shared DiffViewer methods ───────────────────────────────────────────

impl DiffViewer {
    /// Line height in pixels for the current font size.
    pub(super) fn line_height(&self) -> f32 {
        self.file_font_size * LINE_HEIGHT_FACTOR
    }

    /// Measured character width for monospace font.
    pub(super) fn char_width(&self) -> f32 {
        self.measured_char_width
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
    /// Returns `(Div, TextLayout)` so callers can use the layout for position mapping.
    pub(super) fn render_scrollable_content(
        &self,
        spans: &[HighlightedSpan],
        bg_ranges: &[(std::ops::Range<usize>, Hsla)],
        line_height: f32,
    ) -> (Div, TextLayout) {
        let styled_text = build_styled_text_with_backgrounds(spans, bg_ranges);
        let text_layout = styled_text.layout().clone();
        let content = div()
            .flex_1()
            .overflow_hidden()
            .whitespace_nowrap()
            .line_height(px(line_height))
            .child(
                div()
                    .pl(px(CONTENT_PADDING))
                    .ml(px(-self.scroll_x))
                    .child(styled_text),
            );
        (content, text_layout)
    }

    /// Render scrollable content div with a pre-built StyledText.
    fn render_scrollable_content_with_text(
        &self,
        styled_text: StyledText,
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
                    .child(styled_text),
            )
    }

    /// Render a single diff line with syntax highlighting.
    pub(super) fn render_line(
        &self,
        line_index: usize,
        line: &DisplayLine,
        t: &ThemeColors,
        _gutter_width: f32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
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

        let bg_ranges = selection_bg_ranges(&self.selection, line_index, line.plain_text.len());

        let plain_text = line.plain_text.clone();
        let line_len = line.plain_text.len();
        let char_width = self.char_width();
        let num_col_width = (self.line_num_width as f32) * char_width + 12.0;

        // Build styled text and capture layout for position-to-index mapping
        let styled_text = build_styled_text_with_backgrounds(&line.spans, &bg_ranges);
        let text_layout = styled_text.layout().clone();

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
                {
                    let text_layout = text_layout.clone();
                    let plain_text = plain_text.clone();
                    cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                        let col = text_layout.index_for_position(event.position)
                            .unwrap_or_else(|ix| ix)
                            .min(line_len);
                        if event.click_count >= 3 {
                            this.selection.start = Some((line_index, 0));
                            this.selection.end = Some((line_index, line_len));
                            this.selection.finish();
                        } else if event.click_count == 2 {
                            let (start, end) = find_word_boundaries(&plain_text, col);
                            this.selection.start = Some((line_index, start));
                            this.selection.end = Some((line_index, end));
                            this.selection.finish();
                        } else {
                            this.selection.start = Some((line_index, col));
                            this.selection.end = Some((line_index, col));
                            this.selection.is_selecting = true;
                        }
                        this.selection_side = None;
                        cx.notify();
                    })
                },
            )
            .on_mouse_move({
                let text_layout = text_layout.clone();
                cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                    if this.selection.is_selecting {
                        let col = text_layout.index_for_position(event.position)
                            .unwrap_or_else(|ix| ix)
                            .min(line_len);
                        this.selection.end = Some((line_index, col));
                        cx.notify();
                    }
                })
            })
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
            .child(self.render_scrollable_content_with_text(styled_text, line_height))
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
