//! Files mode: the directory tree — spec §7.

use super::super::super::DiffViewer;
use super::super::labels::nav as words;
use super::rows::{self, DirRow, FileRow, NavRow, NavRowKind};
use super::{TREE_ROW_HEIGHT, chip, chip_tone, churn_cell, selection_bar};
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use gpui_component::tooltip::Tooltip;
use okena_core::theme::ThemeColors;
use okena_ui::file_icon::file_icon;
use okena_ui::tokens::{ICON_SM, RADIUS_MD, ui_text_ms, ui_text_sm};
use std::sync::Arc;

/// How far one tree level indents.
const INDENT: f32 = 12.0;

impl DiffViewer {
    pub(crate) fn render_files_tree(
        &mut self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(model) = self.review_ui.model.clone() else {
            return div().flex_1().into_any_element();
        };
        let state = &self.review_ui;
        let tree = Arc::new(rows::nav_rows(
            &model,
            &state.role_filter,
            &state.filter_text,
            &state.expanded_dirs,
            state.flatten,
            state.expanded_initialized,
        ));
        if tree.is_empty() {
            return super::empty_state(words::NO_FILE_MATCH, t, cx).into_any_element();
        }
        let ids: Vec<Option<super::NavRowId>> =
            tree.iter().map(|row| Some(row.id.clone())).collect();
        self.review_keep_cursor_visible(&ids, &self.review_ui.tree_scroll);

        let colors = *t;
        let view = cx.entity().clone();
        let count = tree.len();
        div()
            .flex_1()
            .min_h_0()
            .child(
                uniform_list("review-files-tree", count, move |range, _window, cx| {
                    view.update(cx, |this, cx| {
                        range
                            .filter_map(|index| tree.get(index))
                            .map(|row| this.render_tree_row(row, &colors, cx))
                            .collect::<Vec<AnyElement>>()
                    })
                })
                .size_full()
                .track_scroll(&self.review_ui.tree_scroll),
            )
            .into_any_element()
    }

    fn render_tree_row(&self, row: &NavRow, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        match &row.kind {
            NavRowKind::Dir(dir) => self.render_dir_row(row, dir, t, cx),
            NavRowKind::File(file) => self.render_file_row(row, file, t, cx),
        }
    }

    fn render_dir_row(
        &self,
        row: &NavRow,
        dir: &DirRow,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let super::NavRowId::Dir(path) = &row.id else {
            return div().into_any_element();
        };
        let selected = self.review_ui.nav_cursor.as_ref() == Some(&row.id);
        let for_click = path.clone();
        tree_row(
            super::nav_element_id("review-row", &row.id),
            row.depth,
            selected,
            t,
        )
        .child(
            svg()
                .path(if dir.expanded {
                    "icons/chevron-down.svg"
                } else {
                    "icons/chevron-right.svg"
                })
                .size(ICON_SM)
                .flex_shrink_0()
                .text_color(rgb(t.text_muted)),
        )
        .child(
            svg()
                .path("icons/folder.svg")
                .size(px(14.0))
                .flex_shrink_0()
                .text_color(rgb(t.text_secondary)),
        )
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_primary))
                .child(dir.name.clone()),
        )
        .when(dir.no_tests, |d| {
            d.child(chip(words::NO_TESTS_MARKER, super::ChipTone::Warn, t, cx))
        })
        .child(div().flex_1())
        .child(
            div()
                .flex_shrink_0()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(dir.file_count.to_string()),
        )
        .child(churn_cell(dir.added, dir.deleted, t, cx))
        .on_click(cx.listener(move |this, _, _window, cx| {
            // The pointer moves the cursor too, so `↑` `↓` carry on from here.
            this.review_ui.nav_cursor = Some(super::NavRowId::Dir(for_click.clone()));
            this.review_toggle_dir(&for_click, cx);
        }))
        .into_any_element()
    }

    fn render_file_row(
        &self,
        row: &NavRow,
        file: &FileRow,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let super::NavRowId::File(key) = &row.id else {
            return div().into_any_element();
        };
        let selected = self.review_ui.nav_cursor.as_ref() == Some(&row.id)
            || self.smart_review.selected_file.as_ref() == Some(key);
        let name_color = if file.dimmed {
            t.text_muted
        } else {
            t.text_primary
        };
        let for_click = key.clone();
        let tooltip = file.tooltip.clone();
        tree_row(
            super::nav_element_id("review-row", &row.id),
            row.depth,
            selected,
            t,
        )
        .child(div().w(ICON_SM).flex_shrink_0())
        .child(file_icon(&file.icon_name, t, cx).flex_shrink_0())
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(name_color))
                .child(file.name_display.clone()),
        )
        .children(file.markers.iter().map(|marker| {
            chip(marker.label.clone(), chip_tone(marker.kind), t, cx).into_any_element()
        }))
        .when_some(file.role_badge, |d, badge| {
            d.child(
                div()
                    .flex_shrink_0()
                    .px(px(4.0))
                    .rounded(RADIUS_MD)
                    .border_1()
                    .border_color(rgb(t.border))
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(badge),
            )
        })
        .child(div().flex_1())
        .child(churn_cell(file.added, file.deleted, t, cx))
        .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
        .on_click(cx.listener(move |this, _, _window, cx| {
            this.review_ui.nav_cursor = Some(super::NavRowId::File(for_click.clone()));
            this.review_open_file(for_click.clone(), cx);
        }))
        .into_any_element()
    }
}

/// One row of the tree: the accent stripe, the indent, then the content.
fn tree_row(id: ElementId, depth: usize, selected: bool, t: &ThemeColors) -> Stateful<Div> {
    let indent = f32::from(u16::try_from(depth).unwrap_or(u16::MAX)) * INDENT;
    h_flex()
        .id(id)
        .h(px(TREE_ROW_HEIGHT))
        .w_full()
        .items_center()
        .gap(px(4.0))
        .cursor_pointer()
        .when(selected, |d| d.bg(rgb(t.bg_selection)))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .child(selection_bar(selected, t))
        .child(div().w(px(indent)).flex_shrink_0())
}
