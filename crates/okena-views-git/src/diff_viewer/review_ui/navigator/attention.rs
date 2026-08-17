//! Attention mode: the ordered list — spec §7.

use super::super::super::DiffViewer;
use super::super::labels::{glyph, nav as words};
use super::items::{self, AttentionRow, AttentionRowKind, ChipView, GroupRow, ItemRow};
use super::rows::basename;
use super::{ITEM_ROW_HEIGHT, TREE_ROW_HEIGHT, chip, chip_tone, churn_cell, selection_bar};
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::theme::ThemeColors;
use okena_ui::file_icon::file_icon;
use okena_ui::tokens::{RADIUS_STD, ui_text_ms, ui_text_sm};
use std::sync::Arc;

/// How far an item indents under its file header in the grouped variant.
const GROUP_INDENT: f32 = 12.0;
const DOT: &str = "\u{00B7}";

impl DiffViewer {
    pub(crate) fn render_attention_list(
        &mut self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(model) = self.review_ui.model.clone() else {
            return div().flex_1().into_any_element();
        };
        let state = &self.review_ui;
        let chips = items::reason_chips(&model, &state.attention_filter);
        let rows = Arc::new(items::attention_rows(
            &model,
            &state.attention_filter,
            &state.role_filter,
            &state.filter_text,
        ));
        let ids: Vec<Option<super::NavRowId>> = rows.iter().map(|row| row.id.clone()).collect();
        self.review_keep_cursor_visible(&ids, &self.review_ui.attention_scroll);

        let colors = *t;
        let view = cx.entity().clone();
        let count = rows.len();
        let body = if count == 0 {
            self.render_attention_empty(t, cx)
        } else {
            uniform_list("review-attention-list", count, move |range, _window, cx| {
                view.update(cx, |this, cx| {
                    range
                        .filter_map(|index| rows.get(index))
                        .map(|row| this.render_attention_row(row, &colors, cx))
                        .collect::<Vec<AnyElement>>()
                })
            })
            .flex_1()
            .min_h_0()
            .track_scroll(&self.review_ui.attention_scroll)
            .into_any_element()
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            // The chips stay on screen; one of them is usually what emptied the list.
            .child(self.render_reason_chips(&chips, t, cx))
            .child(body)
            .into_any_element()
    }

    fn render_attention_empty(&self, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        super::empty_state(words::NO_ITEM_MATCH, t, cx)
            .child(DOT)
            .child(
                div()
                    .id("review-attention-clear")
                    .cursor_pointer()
                    .text_color(rgb(t.term_blue))
                    .hover(|s| s.text_color(rgb(t.text_primary)))
                    .child(words::CLEAR)
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.review_clear_attention_filters(cx);
                    })),
            )
            .into_any_element()
    }

    /// The OR filters over the ranked list, plus the tests toggle — spec §7.
    fn render_reason_chips(
        &self,
        chips: &[ChipView],
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let include_tests = self.review_ui.attention_filter.include_tests;
        h_flex()
            .flex_wrap()
            .flex_shrink_0()
            .px(px(super::COLUMN_PADDING))
            .pb(px(6.0))
            .gap(px(4.0))
            .children(chips.iter().map(|view| {
                let kinds = items::chip_toggle_kinds(view, &self.review_ui.attention_filter);
                filter_chip(
                    ElementId::Name(format!("review-chip-{}", view.word).into()),
                    view.label.clone(),
                    view.active,
                    t,
                    cx,
                )
                .on_click(cx.listener(move |this, _, _window, cx| {
                    for kind in &kinds {
                        this.review_toggle_reason_filter(*kind, cx);
                    }
                }))
                .into_any_element()
            }))
            .child(
                filter_chip(
                    ElementId::Name("review-chip-tests".into()),
                    words::TESTS_CHIP.to_string(),
                    include_tests,
                    t,
                    cx,
                )
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.review_toggle_include_tests(cx);
                })),
            )
            .into_any_element()
    }

    fn render_attention_row(
        &self,
        row: &AttentionRow,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match &row.kind {
            AttentionRowKind::Tier(label) => tier_separator(label, t, cx),
            AttentionRowKind::Group(group) => self.render_group_row(group, t, cx),
            AttentionRowKind::Item(item) => self.render_item_row(row, item, t, cx),
        }
    }

    /// A header *is* its file: it paints and opens like a tree file row.
    fn render_group_row(
        &self,
        group: &GroupRow,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = super::NavRowId::File(group.key.clone());
        let selected = self.review_ui.nav_cursor.as_ref() == Some(&id)
            || self.smart_review.selected_file.as_ref() == Some(&group.key);
        let for_click = group.key.clone();
        h_flex()
            .id(super::nav_element_id("review-group", &id))
            .h(px(ITEM_ROW_HEIGHT))
            .w_full()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .when(selected, |d| d.bg(rgb(t.bg_selection)))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .child(selection_bar(selected, t))
            .child(div().w(px(4.0)).flex_shrink_0())
            .child(file_icon(basename(&group.path), t, cx).flex_shrink_0())
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_secondary))
                    .child(group.path.clone()),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex_shrink_0()
                    .pr(px(super::COLUMN_PADDING))
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(group.count.to_string()),
            )
            .on_click(cx.listener(move |this, _, _window, cx| {
                // The pointer moves the cursor too, so `↑` `↓` carry on from here.
                this.review_ui.nav_cursor = Some(super::NavRowId::File(for_click.clone()));
                this.review_open_file(for_click.clone(), cx);
            }))
            .into_any_element()
    }

    fn render_item_row(
        &self,
        row: &AttentionRow,
        item: &ItemRow,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = row
            .id
            .as_ref()
            .is_some_and(|id| self.review_ui.nav_cursor.as_ref() == Some(id))
            || self.review_ui.queue_target.as_ref() == Some(&item.target);
        let name_color = if item.dimmed {
            t.text_muted
        } else {
            t.text_primary
        };
        let indent = if item.nested { GROUP_INDENT } else { 0.0 };
        let target = item.target.clone();
        h_flex()
            .id(super::nav_element_id(
                "review-item",
                &super::NavRowId::Item(item.target.clone()),
            ))
            .h(px(ITEM_ROW_HEIGHT))
            .w_full()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .when(selected, |d| d.bg(rgb(t.bg_selection)))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .child(selection_bar(selected, t))
            .child(div().w(px(indent)).flex_shrink_0())
            .child(
                div()
                    .w(px(14.0))
                    .flex_shrink_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(glyph(item.glyph)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.0))
                    .pr(px(super::COLUMN_PADDING))
                    .child(
                        h_flex()
                            .gap(px(6.0))
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(name_color))
                                    .child(item.name.clone()),
                            )
                            .child(churn_cell(item.added, item.deleted, t, cx)),
                    )
                    .child(
                        h_flex()
                            .gap(px(4.0))
                            .items_center()
                            .children(item.chips.iter().map(|reason| {
                                chip(reason.label.clone(), chip_tone(reason.kind), t, cx)
                                    .into_any_element()
                            }))
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(item.path.clone()),
                            ),
                    ),
            )
            .on_click(cx.listener(move |this, _, _window, cx| {
                // The pointer moves the cursor too, so `↑` `↓` carry on from here.
                this.review_ui.nav_cursor = Some(super::NavRowId::Item(target.clone()));
                this.review_open_item(target.clone(), cx);
            }))
            .into_any_element()
    }
}

fn tier_separator(label: &'static str, t: &ThemeColors, cx: &App) -> AnyElement {
    h_flex()
        .h(px(ITEM_ROW_HEIGHT))
        .w_full()
        .items_end()
        .px(px(super::COLUMN_PADDING))
        .pb(px(4.0))
        .text_size(ui_text_sm(cx))
        .text_color(rgb(t.text_muted))
        .child(label)
        .into_any_element()
}

fn filter_chip(
    id: ElementId,
    label: String,
    active: bool,
    t: &ThemeColors,
    cx: &App,
) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .h(px(TREE_ROW_HEIGHT))
        .px(px(6.0))
        .flex()
        .items_center()
        .rounded(RADIUS_STD)
        .bg(rgb(if active {
            t.bg_selection
        } else {
            t.bg_secondary
        }))
        .border_1()
        .border_color(rgb(if active { t.border_active } else { t.border }))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .text_size(ui_text_sm(cx))
        .text_color(rgb(if active {
            t.text_primary
        } else {
            t.text_secondary
        }))
        .child(label)
}
