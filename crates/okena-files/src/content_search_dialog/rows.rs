//! Result-list row rendering (file headers, match rows, styled code lines).

use super::{ContentSearchDialog, search_match_bg};
use crate::code_view::build_styled_text_with_backgrounds;
use crate::theme::theme;
use gpui::prelude::FluentBuilder;
use gpui::*;
use okena_ui::file_icon::file_icon;
use okena_ui::selectable_list::selectable_list_item;
use okena_ui::tokens::{ui_text, ui_text_ms, ui_text_sm};
use std::path::Path;

impl ContentSearchDialog {
    /// Render a file header row.
    pub(super) fn render_file_header(
        &self,
        idx: usize,
        relative_path: &str,
        match_count: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx);
        let is_selected = idx == self.selected_index;
        let filename = Path::new(relative_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| relative_path.to_string());

        selectable_list_item(
            ElementId::Name(format!("file-header-{}", idx).into()),
            is_selected,
            &t,
        )
        .w_full()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _window, cx| {
                this.selected_index = idx;
                this.open_selected(cx);
            }),
        )
        .gap(px(8.0))
        .child(file_icon(&filename, &t, cx))
        .child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .gap(px(8.0))
                .overflow_hidden()
                .child(
                    div()
                        .text_size(ui_text(13.0, cx))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(t.text_primary))
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(relative_path.to_string()),
                )
                .child(
                    div()
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.text_muted))
                        .child(format!("{} match{}", match_count, if match_count == 1 { "" } else { "es" })),
                ),
        )
    }

    /// Render a single styled code line (used for both match and context lines).
    pub(super) fn render_code_line(
        &mut self,
        file_path: &Path,
        line_number: usize,
        line_content: &str,
        match_ranges: Option<&[std::ops::Range<usize>]>,
        t: &okena_core::theme::ThemeColors,
        cx: &App,
    ) -> Div {
        let styled_text = if let Some(highlighted) = self.get_highlighted_line(file_path, line_number) {
            if let Some(ranges) = match_ranges {
                let match_bg = search_match_bg(t.search_match_bg);
                let bg_ranges: Vec<(std::ops::Range<usize>, Hsla)> = ranges
                    .iter()
                    .filter(|r| r.end <= highlighted.plain_text.len())
                    .map(|r| (r.clone(), match_bg))
                    .collect();
                build_styled_text_with_backgrounds(&highlighted.spans, &bg_ranges)
            } else {
                build_styled_text_with_backgrounds(&highlighted.spans, &[])
            }
        } else if let Some(ranges) = match_ranges {
            let match_bg = search_match_bg(t.search_match_bg);
            let highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = ranges
                .iter()
                .filter(|r| r.end <= line_content.len())
                .map(|r| (r.clone(), HighlightStyle {
                    background_color: Some(match_bg),
                    ..Default::default()
                }))
                .collect();
            StyledText::new(line_content.to_string()).with_highlights(highlights)
        } else {
            StyledText::new(line_content.to_string())
        };

        let is_context = match_ranges.is_none();

        div()
            .flex()
            .gap(px(8.0))
            .when(is_context, |d| d.opacity(0.5))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .min_w(px(40.0))
                    .flex_shrink_0()
                    .child(format!("{:>4}", line_number)),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_size(ui_text_ms(cx))
                    .font_family("monospace")
                    .text_color(rgb(if is_context { t.text_muted } else { t.text_primary }))
                    .child(styled_text),
            )
    }

    /// Render a match result row with optional context lines as one selectable block.
    pub(super) fn render_match_row(
        &mut self,
        idx: usize,
        file_path: &Path,
        line_number: usize,
        line_content: &str,
        match_ranges: &[std::ops::Range<usize>],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let is_selected = idx == self.selected_index;

        selectable_list_item(
            ElementId::Name(format!("match-{}", idx).into()),
            is_selected,
            &t,
        )
        .w_full()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                this.selected_index = idx;
                if event.click_count >= 2 {
                    this.open_selected(cx);
                }
                cx.notify();
            }),
        )
        .gap(px(8.0))
        .pl(px(28.0))
        .child(self.render_code_line(file_path, line_number, line_content, Some(match_ranges), &t, cx))
        .into_any_element()
    }
}
