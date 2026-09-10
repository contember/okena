//! The composition panel: a stacked bar and a clickable legend.

use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use gpui_component::{h_flex, v_flex};
use okena_core::review::FileRole;
use okena_core::theme::ThemeColors;
use okena_ui::tokens::{ui_text_ms, ui_text_sm};

use super::DiffViewer;
use super::composition::LegendRow;

/// Swatch colour per role, from the theme's folder palette so it follows the
/// theme instead of hard-coding hues.
fn swatch(role: FileRole, t: &ThemeColors) -> u32 {
    match role {
        FileRole::Implementation => t.folder_blue,
        FileRole::Test => t.folder_green,
        FileRole::Fixture => t.folder_teal,
        FileRole::Snapshot => t.folder_cyan,
        FileRole::Example => t.folder_lime,
        FileRole::Documentation => t.folder_purple,
        FileRole::Configuration => t.folder_yellow,
        FileRole::Lockfile => t.folder_orange,
        FileRole::Vendored => t.folder_indigo,
        FileRole::Generated => t.folder_default,
        FileRole::Unclassified => t.folder_pink,
    }
}

impl DiffViewer {
    /// Reserve the same space across loading, empty, error, and ready states.
    pub(super) fn render_composition(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let panel = v_flex()
            .flex_shrink_0()
            .h(ui_text_ms(cx) * 8.5 + px(16.0))
            .px(px(16.0))
            .py(px(8.0))
            .border_b_1()
            .border_color(rgb(t.border));
        if let Some(message) = self.composition_status() {
            return Some(
                panel
                    .child(
                        v_flex()
                            .id("composition-status")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .gap(px(8.0))
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child(message)
                            .when(self.composition.loading, |element| {
                                element
                                    .child(div().h(px(3.0)).w_full().bg(rgb(t.bg_secondary)))
                                    .children([0.65, 0.85, 0.45].map(|width| {
                                        div()
                                            .h(ui_text_ms(cx))
                                            .w(relative(width))
                                            .rounded(px(2.0))
                                            .bg(rgb(t.bg_secondary))
                                    }))
                            }),
                    )
                    .into_any_element(),
            );
        }
        let rows = self.composition.rows();
        if rows.is_empty() {
            return Some(
                panel
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child("No composition to show")
                    .into_any_element(),
            );
        }
        // Built eagerly: each row installs a listener, so it cannot be produced
        // from a closure that would have to hold `cx`.
        let mut legend = Vec::with_capacity(rows.len());
        for row in rows {
            legend.push(self.render_legend_row(row, t, cx).into_any_element());
        }

        Some(
            panel
                .child(
                    v_flex()
                        .id("composition-content")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .gap(px(4.0))
                        .child(self.render_composition_headline(t, cx))
                        .child(self.render_composition_bar(t))
                        .child(v_flex().flex_shrink_0().children(legend))
                        .children(self.composition.caveat().map(|caveat| {
                            div()
                                .flex_shrink_0()
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.text_muted))
                                .child(caveat)
                        })),
                )
                .into_any_element(),
        )
    }

    /// The one-line state while the panel has no rows to show.
    fn composition_status(&self) -> Option<String> {
        if self.composition.loading {
            return Some("Reading composition\u{2026}".to_string());
        }
        self.composition
            .error
            .as_ref()
            .map(|error| format!("Composition unavailable \u{2014} {error}"))
    }

    fn render_composition_headline(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .flex_shrink_0()
            .justify_between()
            .items_center()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .children(self.composition.headline())
    }

    fn render_composition_bar(&self, t: &ThemeColors) -> impl IntoElement {
        let segments = self.composition.segments();
        h_flex()
            .flex_shrink_0()
            .h(px(3.0))
            .w_full()
            .rounded(px(3.0))
            .overflow_hidden()
            .bg(rgb(t.bg_secondary))
            .children(segments.into_iter().map(|(role, share)| {
                div()
                    .h_full()
                    .w(relative(share.max(0.0)))
                    .bg(rgb(swatch(role, t)))
            }))
    }

    fn render_legend_row(
        &self,
        row: LegendRow,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let role = row.role;
        let text_color = if row.selected {
            t.text_primary
        } else {
            t.text_secondary
        };

        h_flex()
            .id(SharedString::from(format!("role-{}", row.label)))
            .px(px(4.0))
            .py(px(2.0))
            .rounded(px(4.0))
            .gap(px(6.0))
            .items_center()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(text_color))
            .cursor_pointer()
            .when(row.selected, |element| element.bg(rgb(t.bg_secondary)))
            .hover(|element| element.bg(rgb(t.bg_hover)))
            .tooltip(move |window, cx| Tooltip::new(row.detail.clone()).build(window, cx))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.composition.toggle(role);
                this.apply_role_filter(cx);
            }))
            .child(
                div()
                    .flex_shrink_0()
                    .size(px(6.0))
                    .rounded(px(2.0))
                    .bg(rgb(swatch(role, t))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(row.label),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(rgb(t.text_muted))
                    .child(row.percent),
            )
    }

    /// The sidebar footer, shown only while a role filter hides something.
    pub(super) fn render_composition_footer(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let summary = self.composition.filter_summary(self.visible_file_count())?;
        Some(
            h_flex()
                .id("composition-filter-footer")
                .flex_shrink_0()
                .px(px(16.0))
                .py(px(6.0))
                .gap(px(6.0))
                .justify_between()
                .border_t_1()
                .border_color(rgb(t.border))
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(summary)
                .child(
                    div()
                        .id("composition-filter-clear")
                        .cursor_pointer()
                        .text_color(rgb(t.text_secondary))
                        .hover(|element| element.text_color(rgb(t.text_primary)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.composition.clear_filter();
                            this.apply_role_filter(cx);
                        }))
                        .child("show all"),
                )
                .into_any_element(),
        )
    }
}
