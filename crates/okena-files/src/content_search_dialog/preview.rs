//! Preview panel rendering for the expanded content-search dialog.

use super::{ContentSearchDialog, ResultRow, search_match_bg};
use crate::code_view::{build_styled_text_with_backgrounds, selection_bg_ranges};
use crate::theme::theme;
use gpui::prelude::FluentBuilder;
use gpui::*;
use okena_ui::text_utils::find_word_boundaries;
use okena_ui::tokens::{ui_text, ui_text_ms, ui_text_sm};

impl ContentSearchDialog {
    /// Render the file preview panel showing the selected match's file.
    pub(super) fn render_preview_panel(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);

        // Get the currently selected match info
        let selected_match = self.rows.get(self.selected_index).and_then(|row| match row {
            ResultRow::Match {
                file_path,
                line_number,
                match_ranges,
                ..
            } => Some((file_path.clone(), *line_number, match_ranges.clone())),
            ResultRow::FileHeader { file_path, .. } => Some((file_path.clone(), 1, vec![])),
        });

        let Some((file_path, match_line, _match_ranges)) = selected_match else {
            return div()
                .flex_1()
                .h_full()
                .bg(rgb(t.bg_primary))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.text_muted))
                        .child("Select a match to preview"),
                );
        };

        // Reset selection when preview file changes
        if self.preview_file.as_ref() != Some(&file_path) {
            self.preview_selection = crate::code_view::CodeSelection::default();
            self.preview_file = Some(file_path.clone());
        }

        // Ensure file is in highlight cache — load asynchronously if needed
        if !self.highlight_cache.contains_key(&file_path) {
            self.ensure_file_in_cache(&file_path.clone(), cx);
            // Show loading state while file is being fetched
            return div()
                .flex_1()
                .h_full()
                .bg(rgb(t.bg_primary))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.text_muted))
                        .child("Loading…"),
                );
        }

        let lines = self.highlight_cache.get(&file_path).cloned().unwrap_or_default();
        let line_count = lines.len();
        let match_bg = search_match_bg(t.search_match_bg);
        let current_match_bg = Hsla::from(Rgba {
            r: ((t.search_current_bg >> 16) & 0xFF) as f32 / 255.0,
            g: ((t.search_current_bg >> 8) & 0xFF) as f32 / 255.0,
            b: (t.search_current_bg & 0xFF) as f32 / 255.0,
            a: 0.4,
        });

        // Find all matches in this file to highlight them all (current brighter)
        let all_matches_in_file: Vec<(usize, Vec<std::ops::Range<usize>>)> = self
            .rows
            .iter()
            .filter_map(|row| match row {
                ResultRow::Match {
                    file_path: fp,
                    line_number,
                    match_ranges,
                    ..
                } if *fp == file_path => Some((*line_number, match_ranges.clone())),
                _ => None,
            })
            .collect();

        let relative_path = file_path.to_string_lossy().to_string();

        // Scroll to the match line — only when the selection changed.
        // Doing this on every render would re-anchor the scroll position
        // and prevent the user from scrolling past the highlighted row.
        let scroll_target = (file_path.clone(), match_line);
        if self.last_scrolled_match.as_ref() != Some(&scroll_target) {
            let scroll_to = match_line.saturating_sub(5); // 5 lines above for context
            self.preview_scroll_handle
                .scroll_to_item(scroll_to, ScrollStrategy::Top);
            self.last_scrolled_match = Some(scroll_target);
        }

        let view = cx.entity().clone();

        div()
            .flex_1()
            .h_full()
            .bg(rgb(t.bg_primary))
            .border_l_1()
            .border_color(rgb(t.border))
            .flex()
            .flex_col()
            // File path header
            .child(
                div()
                    .px(px(12.0))
                    .py(px(8.0))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(relative_path),
            )
            // File content
            .child(
                uniform_list(
                    "preview-lines",
                    line_count,
                    move |range, _window, cx| {
                        view.update(cx, |this, cx| {
                            let t = theme(cx);
                            range
                                .map(|line_idx| {
                                    let line_number = line_idx + 1;
                                    let line_num_str = format!("{:>4}", line_number);

                                    // Check if this line has matches
                                    let line_match = all_matches_in_file
                                        .iter()
                                        .find(|(ln, _)| *ln == line_number);

                                    let is_current_match = line_number == match_line;

                                    // Combine match highlights with selection highlights
                                    let line_len = lines.get(line_idx).map_or(0, |hl| hl.plain_text.len());
                                    let sel_bg_ranges = selection_bg_ranges(&this.preview_selection, line_idx, line_len);

                                    let styled_text = if let Some(hl) =
                                        lines.get(line_idx)
                                    {
                                        let mut bg_ranges: Vec<(std::ops::Range<usize>, Hsla)> = Vec::new();
                                        if let Some((_, ranges)) = line_match {
                                            let bg = if is_current_match {
                                                current_match_bg
                                            } else {
                                                match_bg
                                            };
                                            bg_ranges.extend(
                                                ranges
                                                    .iter()
                                                    .filter(|r| r.end <= hl.plain_text.len())
                                                    .map(|r| (r.clone(), bg)),
                                            );
                                        }
                                        bg_ranges.extend(sel_bg_ranges);
                                        build_styled_text_with_backgrounds(
                                            &hl.spans, &bg_ranges,
                                        )
                                    } else {
                                        StyledText::new(String::new())
                                    };

                                    let text_layout = styled_text.layout().clone();
                                    let plain_text = lines.get(line_idx).map(|hl| hl.plain_text.clone()).unwrap_or_default();

                                    let row_bg = if is_current_match {
                                        Some(current_match_bg)
                                    } else if line_match.is_some() {
                                        Some(match_bg)
                                    } else {
                                        None
                                    };

                                    div()
                                        .id(ElementId::Name(format!("preview-line-{}", line_idx).into()))
                                        .flex()
                                        .items_center()
                                        .px(px(8.0))
                                        .h(px(24.0))
                                        .text_size(ui_text(13.0, cx))
                                        .font_family("monospace")
                                        .when_some(row_bg, |d, bg| d.bg(bg))
                                        .on_mouse_down(MouseButton::Left, {
                                            let text_layout = text_layout.clone();
                                            let plain_text = plain_text.clone();
                                            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                                let col = text_layout
                                                    .index_for_position(event.position)
                                                    .unwrap_or_else(|ix| ix)
                                                    .min(line_len);
                                                if event.click_count >= 3 {
                                                    this.preview_selection.start = Some((line_idx, 0));
                                                    this.preview_selection.end = Some((line_idx, line_len));
                                                    this.preview_selection.finish();
                                                } else if event.click_count == 2 {
                                                    let (start, end) = find_word_boundaries(&plain_text, col);
                                                    this.preview_selection.start = Some((line_idx, start));
                                                    this.preview_selection.end = Some((line_idx, end));
                                                    this.preview_selection.finish();
                                                } else {
                                                    this.preview_selection.start = Some((line_idx, col));
                                                    this.preview_selection.end = Some((line_idx, col));
                                                    this.preview_selection.is_selecting = true;
                                                }
                                                cx.notify();
                                            })
                                        })
                                        .on_mouse_move({
                                            let text_layout = text_layout.clone();
                                            cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                                                if this.preview_selection.is_selecting {
                                                    let col = text_layout
                                                        .index_for_position(event.position)
                                                        .unwrap_or_else(|ix| ix)
                                                        .min(line_len);
                                                    this.preview_selection.end = Some((line_idx, col));
                                                    cx.notify();
                                                }
                                            })
                                        })
                                        .on_mouse_up(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _window, cx| {
                                                this.preview_selection.finish();
                                                cx.notify();
                                            }),
                                        )
                                        .child(
                                            div()
                                                .text_color(rgb(t.text_muted))
                                                .min_w(px(44.0))
                                                .flex_shrink_0()
                                                .text_size(ui_text_ms(cx))
                                                .child(line_num_str),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .overflow_hidden()
                                                .text_color(rgb(t.text_primary))
                                                .child(styled_text),
                                        )
                                        .into_any_element()
                                })
                                .collect()
                        })
                    },
                )
                .flex_1()
                .track_scroll(&self.preview_scroll_handle),
            )
    }
}
