//! Analysis status pill and its details popover — spec §10.

mod pill;
mod popover;

use super::super::DiffViewer;
use super::labels::status as words;
use super::model::AnalysisStatus;
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use gpui_component::tooltip::Tooltip;
use okena_core::theme::ThemeColors;
use okena_ui::popover::popover_panel;
use okena_ui::tokens::{ui_text_ms, ui_text_sm};
use pill::{PillTone, pill_view};
use popover::{PopoverRow, popover_rows};

/// One spinner frame; the app draws no animations.
const SPINNER: &str = "\u{25D0}";

const PANEL_WIDTH: Pixels = px(360.0);

impl DiffViewer {
    pub(crate) fn render_status_pill(&self, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        let status = self.review_status();
        let view = pill_view(&status);
        let message = match &status {
            AnalysisStatus::Unavailable { message } => Some(message.clone()),
            _ => None,
        };
        h_flex()
            .id("review-status-pill")
            .h(px(22.0))
            .px(px(8.0))
            .gap(px(6.0))
            .items_center()
            .rounded_full()
            .bg(rgb(t.bg_secondary))
            .border_1()
            .border_color(rgb(t.border))
            .child(tone_marker(view.tone, t, cx))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(view.text),
            )
            .when(view.has_details, |d| {
                d.child(
                    div()
                        .id("review-status-details")
                        .cursor_pointer()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.term_blue))
                        .hover(|s| s.text_color(rgb(t.text_primary)))
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.review_toggle_status_popover(cx);
                        }))
                        .child(words::DETAILS_LINK),
                )
            })
            .when_some(message, |d, message| {
                d.tooltip(move |window, cx| Tooltip::new(message.clone()).build(window, cx))
            })
            .into_any_element()
    }

    pub(crate) fn render_status_popover(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.review_ui.status_popover_open {
            return None;
        }
        // Only the `details` link opens this; states without one have nothing to add.
        if !pill_view(&self.review_status()).has_details {
            return None;
        }
        let model = self.review_ui.model.as_ref()?;
        let rows: Vec<AnyElement> = popover_rows(model)
            .into_iter()
            .map(|row| render_row(&row, t, cx))
            .collect();
        let oids = words::oid_line(&model.coverage);

        let panel = popover_panel("review-status-popover", t)
            .absolute()
            .top(px(6.0))
            .right(px(16.0))
            .w(PANEL_WIDTH)
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(
                div()
                    .pb(px(4.0))
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(words::POPOVER_TITLE),
            )
            .children(rows)
            .child(
                div()
                    .pt(px(6.0))
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(words::FOOTER)
                    .when(!oids.is_empty(), |d| {
                        d.child(div().pt(px(4.0)).font_family("monospace").child(oids))
                    }),
            );

        // Occludes, so the dismissing click never also lands on the row underneath.
        Some(
            div()
                .id("review-status-backdrop")
                .occlude()
                .absolute()
                .inset_0()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _window, cx| this.review_toggle_status_popover(cx)),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _window, cx| this.review_toggle_status_popover(cx)),
                )
                .child(panel)
                .into_any_element(),
        )
    }

    /// The pill renders before the first dataset lands, so default to loading.
    fn review_status(&self) -> AnalysisStatus {
        self.review_ui
            .model
            .as_ref()
            .map_or(AnalysisStatus::LoadingInventory, |model| {
                model.status.clone()
            })
    }
}

fn tone_marker(tone: PillTone, t: &ThemeColors, cx: &App) -> AnyElement {
    let color = match tone {
        PillTone::Green => t.success,
        PillTone::Amber => t.warning,
        PillTone::Red => t.error,
        PillTone::Busy => {
            return div()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child(SPINNER)
                .into_any_element();
        }
    };
    div()
        .w(px(6.0))
        .h(px(6.0))
        .rounded_full()
        .bg(rgb(color))
        .into_any_element()
}

fn render_row(row: &PopoverRow, t: &ThemeColors, cx: &App) -> AnyElement {
    let color = if row.warn {
        t.warning
    } else {
        t.text_secondary
    };
    let detail_color = if row.warn { t.warning } else { t.text_muted };
    h_flex()
        .py(px(4.0))
        .gap(px(12.0))
        .items_start()
        .justify_between()
        .border_b_1()
        .border_color(rgb(t.border))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(color))
                .child(row.sentence.clone()),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(detail_color))
                .child(row.detail.clone()),
        )
        .into_any_element()
}
