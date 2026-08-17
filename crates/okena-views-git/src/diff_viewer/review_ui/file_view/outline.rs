//! The outline popover: base and head outlines side by side — spec §9.

use super::super::super::DiffViewer;
use super::super::super::review::ReviewFileKey;
use super::super::labels;
use super::super::state::SymbolRef;
use super::structure::{OutlineRow, outline_rows};
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use gpui_component::v_flex;
use okena_core::theme::ThemeColors;
use okena_ui::popover::popover_panel;
use okena_ui::tokens::{ui_text_ms, ui_text_sm};
use std::collections::HashMap;

const PANEL_WIDTH: Pixels = px(520.0);
const PANEL_HEIGHT: Pixels = px(420.0);
/// One nesting step, in pixels; deeper nesting stops moving right.
const INDENT: f32 = 12.0;
const MAX_INDENT_DEPTH: usize = 8;
const BASE_TITLE: &str = "Base";
const HEAD_TITLE: &str = "Head";
const CHANGED_MARK: &str = "\u{25CF}";

pub(super) fn render(
    view: &DiffViewer,
    t: &ThemeColors,
    cx: &mut Context<DiffViewer>,
) -> Option<AnyElement> {
    if !view.review_ui.outline_open {
        return None;
    }
    let entry = view.review_open_entry()?;
    let (old, new) = view.review_open_outline()?;
    let changed: HashMap<String, usize> = entry
        .symbols
        .iter()
        .map(|symbol| (symbol.qualified.clone(), symbol.change_index))
        .collect();
    let base = outline_rows(old, &changed);
    let head = outline_rows(new, &changed);
    let key = entry.key.clone();

    let panel = popover_panel("review-outline-popover", t)
        .absolute()
        .top(px(46.0))
        .right(px(16.0))
        .w(PANEL_WIDTH)
        .max_h(PANEL_HEIGHT)
        .overflow_hidden()
        .flex()
        .child(
            h_flex()
                .w_full()
                .gap(px(12.0))
                .items_start()
                .child(column("review-outline-base", BASE_TITLE, base, None, t, cx))
                .child(column(
                    "review-outline-head",
                    HEAD_TITLE,
                    head,
                    Some(key),
                    t,
                    cx,
                )),
        );

    // Occludes, so the dismissing click never also lands on the diff underneath.
    Some(
        div()
            .id("review-outline-backdrop")
            .occlude()
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| this.review_toggle_outline(cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _window, cx| this.review_toggle_outline(cx)),
            )
            .child(panel)
            .into_any_element(),
    )
}

/// One snapshot's outline. `file` is set only for the head column, whose changed
/// symbols open in the diff.
fn column(
    id: &'static str,
    title: &'static str,
    rows: Vec<OutlineRow>,
    file: Option<ReviewFileKey>,
    t: &ThemeColors,
    cx: &mut Context<DiffViewer>,
) -> AnyElement {
    v_flex()
        .flex_1()
        .min_w_0()
        .gap(px(1.0))
        .child(
            div()
                .pb(px(4.0))
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(title),
        )
        .children(
            rows.into_iter()
                .enumerate()
                .map(|(index, row)| outline_row(id, index, &row, file.clone(), t, cx)),
        )
        .into_any_element()
}

fn outline_row(
    id: &'static str,
    index: usize,
    row: &OutlineRow,
    file: Option<ReviewFileKey>,
    t: &ThemeColors,
    cx: &mut Context<DiffViewer>,
) -> AnyElement {
    let changed = row.change_index.is_some();
    let name_color = if changed {
        t.text_primary
    } else {
        t.text_secondary
    };
    let content = h_flex()
        .gap(px(6.0))
        .items_center()
        .pl(indent(row.depth))
        .pr(px(4.0))
        .text_size(ui_text_ms(cx))
        .child(
            div()
                .flex_none()
                .text_color(rgb(t.text_muted))
                .child(labels::glyph(row.glyph)),
        )
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .text_color(rgb(name_color))
                .child(row.name.clone()),
        )
        .when(changed, |d| {
            d.child(
                div()
                    .flex_none()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.warning))
                    .child(CHANGED_MARK),
            )
        });

    match file.zip(row.change_index) {
        Some((file, change_index)) => content
            .id((id, index))
            .cursor_pointer()
            .rounded(px(3.0))
            .hover(|style| style.bg(rgb(t.bg_hover)))
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.review_open_symbol(
                    SymbolRef {
                        file: file.clone(),
                        change_index,
                    },
                    cx,
                );
                this.review_toggle_outline(cx);
            }))
            .into_any_element(),
        None => content.into_any_element(),
    }
}

fn indent(depth: usize) -> Pixels {
    let steps = u16::try_from(depth.min(MAX_INDENT_DEPTH)).unwrap_or(0);
    px(f32::from(steps) * INDENT)
}

#[cfg(test)]
mod tests {
    use super::{INDENT, MAX_INDENT_DEPTH, indent};
    use gpui::px;

    #[test]
    fn indentation_grows_per_level_and_stops_at_the_cap() {
        assert_eq!(indent(0), px(0.0));
        assert_eq!(indent(2), px(2.0 * INDENT));
        assert_eq!(indent(MAX_INDENT_DEPTH), px(8.0 * INDENT));
        assert_eq!(indent(MAX_INDENT_DEPTH + 5), indent(MAX_INDENT_DEPTH));
    }
}
