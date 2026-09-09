//! The composition panel: a stacked bar, a clickable legend, and role presets.

use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::review::FileRole;
use okena_core::theme::ThemeColors;
use okena_ui::tokens::{ui_text_ms, ui_text_sm};

use super::DiffViewer;
use super::composition::{LegendRow, RolePreset};

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
    /// The whole panel, or nothing while there is no composition to show.
    pub(super) fn render_composition(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if let Some(message) = self.composition_status() {
            return Some(
                div()
                    .flex_shrink_0()
                    .px(px(16.0))
                    .py(px(8.0))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(message)
                    .into_any_element(),
            );
        }
        let rows = self.composition.rows();
        if rows.is_empty() {
            return None;
        }
        // Built eagerly: each row installs a listener, so it cannot be produced
        // from a closure that would have to hold `cx`.
        let mut legend = Vec::with_capacity(rows.len());
        for row in rows {
            legend.push(self.render_legend_row(row, t, cx).into_any_element());
        }
        let presets = self.render_role_presets(t, cx);

        Some(
            v_flex()
                .flex_shrink_0()
                .px(px(16.0))
                .py(px(10.0))
                .gap(px(8.0))
                .border_b_1()
                .border_color(rgb(t.border))
                .child(self.render_composition_headline(t, cx))
                .child(self.render_composition_bar(t))
                .children(legend)
                .child(presets)
                .children(self.composition.caveat().map(|caveat| {
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .child(caveat)
                }))
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
            .justify_between()
            .items_center()
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            .font_weight(FontWeight::MEDIUM)
            .child("COMPOSITION")
            .children(self.composition.headline())
    }

    fn render_composition_bar(&self, t: &ThemeColors) -> impl IntoElement {
        let segments = self.composition.segments();
        h_flex()
            .h(px(6.0))
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

        v_flex()
            .id(SharedString::from(format!("role-{}", row.label)))
            .px(px(4.0))
            .py(px(2.0))
            .rounded(px(4.0))
            .cursor_pointer()
            .when(row.selected, |element| element.bg(rgb(t.bg_selection)))
            .hover(|element| element.bg(rgb(t.bg_hover)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.composition.toggle(role);
                this.apply_role_filter(cx);
            }))
            .child(
                h_flex()
                    .gap(px(6.0))
                    .items_center()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(text_color))
                    .child(
                        div()
                            .flex_shrink_0()
                            .size(px(8.0))
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
                    ),
            )
            .child(
                div()
                    .pl(px(14.0))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(row.detail),
            )
    }

    fn render_role_presets(&self, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        let mut chips = Vec::with_capacity(RolePreset::ALL.len());
        for preset in RolePreset::ALL {
            let active = self.composition.is_active(preset);
            chips.push(
                div()
                    .id(SharedString::from(format!("preset-{}", preset.label())))
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(if active { t.text_primary } else { t.text_muted }))
                    .when(active, |element| element.bg(rgb(t.bg_selection)))
                    .hover(|element| element.bg(rgb(t.bg_hover)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.composition.apply(preset);
                        this.apply_role_filter(cx);
                    }))
                    .child(preset.label())
                    .into_any_element(),
            );
        }
        h_flex().gap(px(4.0)).children(chips).into_any_element()
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
                            this.composition.apply(RolePreset::Everything);
                            this.apply_role_filter(cx);
                        }))
                        .child("show all"),
                )
                .into_any_element(),
        )
    }
}
