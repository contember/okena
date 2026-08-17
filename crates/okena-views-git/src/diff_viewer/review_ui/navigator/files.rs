//! Files mode: the directory tree — spec §7.

use super::super::super::DiffViewer;
use super::super::labels::nav as words;
use super::super::labels::{self as labels, glyph};
use super::super::model::AttentionTarget;
use super::super::state::SymbolRef;
use super::rows::{self, DetailKind, DetailRow, DirRow, FileRow, NavRow, NavRowKind, SymbolRow};
use super::{TREE_ROW_HEIGHT, chip, chip_tone, churn_cell, selection_bar};
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use gpui_component::tooltip::Tooltip;
use okena_core::theme::ThemeColors;
use okena_review::CallChangeKind;
use okena_ui::file_icon::file_icon;
use okena_ui::tokens::{ICON_SM, RADIUS_MD, ui_text_ms, ui_text_sm};
use std::sync::Arc;

/// How far one tree level indents.
const INDENT: f32 = 12.0;
/// The kind glyph column of a symbol row; its detail lines start after it.
const GLYPH_WIDTH: f32 = 13.0;
/// The marker column of a detail line — `sig` is the widest thing in it, and
/// every line's text has to start at the same x.
const MARKER_WIDTH: f32 = 20.0;

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
        let tree = Arc::new(rows::nav_rows(&model, &rows::TreeArgs::of(state)));
        if tree.is_empty() {
            return super::empty_state(words::NO_FILE_MATCH, t, cx).into_any_element();
        }
        let ids: Vec<Option<super::NavRowId>> = tree.iter().map(|row| row.id.clone()).collect();
        let scroll = self.review_ui.tree_scroll.clone();
        self.review_reveal_cursor(&ids, &scroll);

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
            NavRowKind::Symbol(symbol) => self.render_symbol_row(row, symbol, t, cx),
            NavRowKind::Detail(detail) => self.render_detail_row(row, detail, t, cx),
        }
    }

    fn render_dir_row(
        &self,
        row: &NavRow,
        dir: &DirRow,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(id @ super::NavRowId::Dir(path)) = &row.id else {
            return div().into_any_element();
        };
        let cursor = self.review_ui.nav_cursor.as_ref() == Some(id);
        let for_click = path.clone();
        tree_row(
            super::nav_element_id("review-row", id),
            row.depth,
            cursor,
            false,
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
                .truncate()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_primary))
                .child(dir.name.clone()),
        )
        .when(dir.no_tests, |d| {
            d.child(chip(words::NO_TESTS_MARKER, super::ChipTone::Warn, t, cx))
        })
        .when_some(dir.role_badge, |d, badge| d.child(role_badge(badge, t, cx)))
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
        let Some(id @ super::NavRowId::File(key)) = &row.id else {
            return div().into_any_element();
        };
        let cursor = self.review_ui.nav_cursor.as_ref() == Some(id);
        let open = self.smart_review.selected_file.as_ref() == Some(key);
        let name_color = if file.dimmed {
            t.text_muted
        } else {
            t.text_primary
        };
        let for_click = key.clone();
        let tooltip = file.tooltip.clone();
        tree_row(
            super::nav_element_id("review-row", id),
            row.depth,
            cursor,
            open,
            t,
        )
        // The block below belongs to this row, so the row reads as its header.
        // A fill and not a rule: a border would make this row taller than the
        // rest, and the virtualized list measures one height for all of them.
        .when(file.outlined && !open && !cursor, |d| {
            d.bg(rgb(t.bg_secondary))
        })
        .child(div().w(ICON_SM).flex_shrink_0())
        .child(file_icon(&file.icon_name, t, cx).flex_shrink_0())
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(name_color))
                .child(file.name_display.clone()),
        )
        .children(file.markers.iter().map(|marker| {
            chip(marker.label.clone(), chip_tone(marker.kind), t, cx).into_any_element()
        }))
        .when_some(file.role_badge, |d, badge| {
            d.child(role_badge(badge, t, cx))
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

    /// One changed symbol under its file — spec §7. The fill marks the symbol
    /// the content area currently shows.
    fn render_symbol_row(
        &self,
        row: &NavRow,
        symbol: &SymbolRow,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(id) = row.id.as_ref() else {
            return div().into_any_element();
        };
        let cursor = self.review_ui.nav_cursor.as_ref() == Some(id);
        let open = matches!(&symbol.target, AttentionTarget::Symbol { file, change_index }
            if self.review_ui.selected_symbol
                == Some(SymbolRef { file: file.clone(), change_index: *change_index }));
        let tooltip = SharedString::from(symbol.tooltip.clone());
        let target = symbol.target.clone();
        tree_row(
            super::nav_element_id("review-symbol", id),
            row.depth,
            cursor,
            open,
            t,
        )
        .child(
            div()
                .w(px(GLYPH_WIDTH))
                .flex_shrink_0()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(glyph(symbol.glyph)),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_primary))
                .child(symbol.name.clone()),
        )
        .children(symbol.markers.iter().map(|marker| {
            chip(marker.label.clone(), chip_tone(marker.kind), t, cx).into_any_element()
        }))
        .when_some(symbol.role_badge, |d, badge| {
            d.child(role_badge(badge, t, cx))
        })
        .child(div().flex_1())
        .child(churn_cell(symbol.added, symbol.deleted, t, cx))
        .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
        .on_click(cx.listener(move |this, _, _window, cx| {
            this.review_ui.nav_cursor = Some(super::NavRowId::Item(target.clone()));
            this.review_open_item(target.clone(), cx);
        }))
        .into_any_element()
    }

    /// One line of what changed inside a symbol: the signature pair, or a call.
    /// It opens the symbol like the row above it, but `↑` `↓` step over it.
    fn render_detail_row(
        &self,
        row: &NavRow,
        detail: &DetailRow,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tooltip = SharedString::from(detail.text.clone());
        let target = detail.target.clone();
        tree_row(
            super::detail_element_id(&detail.target, detail.position),
            row.depth,
            false,
            false,
            t,
        )
        .child(detail_marker(detail.kind, t, cx))
        .child(
            div()
                .min_w_0()
                .truncate()
                .when(detail.kind != DetailKind::More, |d| {
                    d.font_family("monospace")
                })
                .text_size(ui_text_sm(cx))
                .text_color(rgb(if detail.kind == DetailKind::More {
                    t.text_muted
                } else {
                    t.text_secondary
                }))
                .child(detail.text.clone()),
        )
        .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
        .on_click(cx.listener(move |this, _, _window, cx| {
            this.review_ui.nav_cursor = Some(super::NavRowId::Item(target.clone()));
            this.review_open_item(target.clone(), cx);
        }))
        .into_any_element()
    }
}

/// `sig` for a signature change, `+` `−` `~` for a call; nothing for the
/// `… 4 more` line, whose own words say what it is.
fn detail_marker(kind: DetailKind, t: &ThemeColors, cx: &App) -> Div {
    let (text, color) = match kind {
        DetailKind::Signature => (words::SIGNATURE_LINE, t.warning),
        DetailKind::Call(CallChangeKind::Added) => (
            labels::calls::call_marker(CallChangeKind::Added),
            t.diff_added_fg,
        ),
        DetailKind::Call(CallChangeKind::Removed) => (
            labels::calls::call_marker(CallChangeKind::Removed),
            t.diff_removed_fg,
        ),
        DetailKind::Call(CallChangeKind::Modified) => (
            labels::calls::call_marker(CallChangeKind::Modified),
            t.term_blue,
        ),
        DetailKind::More => ("", t.text_muted),
    };
    div()
        .w(px(MARKER_WIDTH))
        .flex_shrink_0()
        .font_family("monospace")
        .text_size(ui_text_sm(cx))
        .text_color(rgb(color))
        .child(text)
}

/// `Tests` / `Docs` … — outlined, so it reads as a label and not a reason.
fn role_badge(badge: &'static str, t: &ThemeColors, cx: &App) -> Div {
    div()
        .flex_shrink_0()
        .px(px(4.0))
        .rounded(RADIUS_MD)
        .border_1()
        .border_color(rgb(t.border))
        .text_size(ui_text_sm(cx))
        .text_color(rgb(t.text_muted))
        .child(badge)
}

/// One row of the tree: the accent stripe, the indent rail, then the content.
/// The stripe marks the keyboard cursor, the fill the file that is open.
fn tree_row(
    id: ElementId,
    depth: usize,
    cursor: bool,
    open: bool,
    t: &ThemeColors,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .h(px(TREE_ROW_HEIGHT))
        .w_full()
        .items_center()
        .gap(px(4.0))
        .cursor_pointer()
        .when(open, |d| d.bg(rgb(t.bg_selection)))
        .when(cursor && !open, |d| d.bg(rgb(t.bg_hover)))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .child(selection_bar(cursor, t))
        .child(indent_rail(depth, t))
}

/// The indent, drawn rather than left blank: one hairline per level the row
/// hangs under, so five levels of tree read as five levels and not as text at
/// five x positions.
fn indent_rail(depth: usize, t: &ThemeColors) -> Div {
    h_flex()
        .flex_shrink_0()
        .h_full()
        .children((0..depth).map(|_| {
            div()
                .w(px(INDENT))
                .h_full()
                .border_l_1()
                .border_color(rgb(t.border))
        }))
}
