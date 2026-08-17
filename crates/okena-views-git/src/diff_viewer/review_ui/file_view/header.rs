//! The 40 px file header — spec §9: what the open file is, how big it is, why
//! it is ranked where it is, and where it sits in the queue.

use super::super::super::DiffViewer;
use super::super::super::review::ReviewFileKey;
use super::super::labels;
use super::super::labels::reasons as words;
use super::super::model::FileEntry;
use super::text;
use super::{chip, churn, word};
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use gpui_component::tooltip::Tooltip;
use gpui_component::v_flex;
use okena_core::theme::ThemeColors;
use okena_ui::tokens::{ui_text_md, ui_text_ms, ui_text_sm};

/// The header states the file's strongest reasons; the navigator has the rest.
const HEADER_CHIPS: usize = 3;
const ROW_HEIGHT: Pixels = px(40.0);
const SUMMARY_HEIGHT: Pixels = px(22.0);
const OUTLINE_LINK: &str = "outline";
const PREVIOUS: &str = "\u{2039}";
const NEXT: &str = "\u{203A}";
const NO_FILE: &str = "No file selected";

pub(super) fn render(
    view: &DiffViewer,
    t: &ThemeColors,
    cx: &mut Context<DiffViewer>,
) -> AnyElement {
    let Some(entry) = view.review_open_entry() else {
        return placeholder(view.smart_review.selected_file.as_ref(), t, cx);
    };
    let narrow = view.review_content_is_narrow();
    let visible = view.review_visible_attention();
    let summary = view
        .review_ui
        .model
        .as_ref()
        .and_then(|model| text::header_summary(model));
    let queue =
        view.review_ui.model.as_ref().and_then(|model| {
            text::queue_label(&visible, model, view.review_ui.queue_target.as_ref())
        });
    let has_outline = view.review_open_outline().is_some();

    let row = h_flex()
        .h(ROW_HEIGHT)
        .px(px(16.0))
        .gap(px(8.0))
        .flex_none()
        .items_center()
        .border_b_1()
        .border_color(rgb(t.border))
        .child(path_element(entry, t, cx))
        .child(role_badge(entry, narrow, t, cx))
        .child(word(labels::status_label(entry.status), t.text_muted, cx))
        .children(churn(entry.lines_added, entry.lines_deleted, t, cx))
        .children(
            entry
                .reasons
                .iter()
                .take(HEADER_CHIPS)
                .map(|reason| chip(reason, t, cx)),
        )
        .child(div().flex_1())
        .when(!narrow, |d| {
            d.child(word(text::analysis_label(entry), t.text_muted, cx))
        })
        .when(has_outline, |d| d.child(outline_link(t, cx)))
        .when_some(queue, |d, queue| d.child(queue_group(queue, t, cx)));

    match summary {
        Some(summary) => v_flex()
            .flex_none()
            .child(summary_line(summary, t, cx))
            .child(row)
            .into_any_element(),
        None => row.into_any_element(),
    }
}

/// `src/build/compile.ts`, or `old → new · moved 98 %` for a rename.
fn path_element(entry: &FileEntry, t: &ThemeColors, cx: &App) -> AnyElement {
    let renamed = match (entry.old_path.as_deref(), entry.new_path.as_deref()) {
        (Some(old), Some(new)) if old != new => Some((old, new)),
        _ => None,
    };
    let mut row = h_flex()
        .min_w_0()
        .gap(px(6.0))
        .items_center()
        .overflow_hidden();
    match renamed {
        Some((old, new)) => {
            row = row
                .child(path_text(old, t, cx))
                .child(word(text::ARROW, t.text_muted, cx))
                .child(path_text(new, t, cx));
            if let Some(similarity) = entry.similarity {
                let moved = format!("\u{00B7} {}", words::moved_label(similarity));
                row = row.child(word(moved, t.text_muted, cx));
            }
        }
        None => row = row.child(path_text(&entry.display_path, t, cx)),
    }
    row.into_any_element()
}

fn path_text(path: &str, t: &ThemeColors, cx: &App) -> AnyElement {
    let (directory, base) = text::split_path(path);
    h_flex()
        .min_w_0()
        .overflow_hidden()
        .text_size(ui_text_md(cx))
        .when(!directory.is_empty(), |d| {
            d.child(
                div()
                    .flex_none()
                    .text_color(rgb(t.text_muted))
                    .child(directory.to_string()),
            )
        })
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(rgb(t.text_primary))
                .child(base.to_string()),
        )
        .into_any_element()
}

/// The role, with the rule that classified it on hover — spec §9.
fn role_badge(entry: &FileEntry, narrow: bool, t: &ThemeColors, cx: &App) -> AnyElement {
    let label = if narrow {
        labels::role_short(entry.role)
    } else {
        labels::role_label(entry.role)
    };
    // Narrow columns drop the language line, so the badge carries it instead.
    let tooltip = if narrow {
        format!(
            "{}{}{}",
            labels::rule_sentence(&entry.rule_id),
            text::DOT,
            text::analysis_label(entry)
        )
    } else {
        labels::rule_sentence(&entry.rule_id)
    };
    div()
        .id("review-file-role")
        .flex_none()
        .px(px(5.0))
        .rounded(px(3.0))
        .bg(rgb(t.bg_secondary))
        .text_size(ui_text_sm(cx))
        .text_color(rgb(t.text_secondary))
        .child(label)
        .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
        .into_any_element()
}

fn outline_link(t: &ThemeColors, cx: &mut Context<DiffViewer>) -> AnyElement {
    div()
        .id("review-file-outline")
        .flex_none()
        .cursor_pointer()
        .text_size(ui_text_ms(cx))
        .text_color(rgb(t.term_blue))
        .hover(|style| style.text_color(rgb(t.text_primary)))
        .on_click(cx.listener(|this, _, _window, cx| this.review_toggle_outline(cx)))
        .child(OUTLINE_LINK)
        .into_any_element()
}

/// `3 of 236` with the two steps through the Attention order.
fn queue_group(label: String, t: &ThemeColors, cx: &mut Context<DiffViewer>) -> AnyElement {
    h_flex()
        .flex_none()
        .gap(px(4.0))
        .items_center()
        .child(word(label, t.text_muted, cx))
        .child(step_button("review-queue-prev", PREVIOUS, -1, t, cx))
        .child(step_button("review-queue-next", NEXT, 1, t, cx))
        .into_any_element()
}

fn step_button(
    id: &'static str,
    glyph: &'static str,
    delta: i32,
    t: &ThemeColors,
    cx: &mut Context<DiffViewer>,
) -> AnyElement {
    div()
        .id(id)
        .flex_none()
        .w(px(18.0))
        .h(px(18.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.0))
        .bg(rgb(t.bg_secondary))
        .border_1()
        .border_color(rgb(t.border))
        .cursor_pointer()
        .text_size(ui_text_ms(cx))
        .text_color(rgb(t.text_secondary))
        .hover(|style| style.bg(rgb(t.bg_hover)))
        .on_click(cx.listener(move |this, _, _window, cx| this.review_step_queue(delta, cx)))
        .child(glyph)
        .into_any_element()
}

/// How tall the header is, so overlays anchored under it know where it ends.
pub(super) fn height(view: &DiffViewer) -> Pixels {
    let summary = view
        .review_ui
        .model
        .as_ref()
        .is_some_and(|model| text::header_summary(model).is_some());
    if summary && view.review_open_entry().is_some() {
        ROW_HEIGHT + SUMMARY_HEIGHT
    } else {
        ROW_HEIGHT
    }
}

/// The line a small comparison gets instead of the Overview — spec §12.
fn summary_line(summary: String, t: &ThemeColors, cx: &App) -> AnyElement {
    div()
        .flex_none()
        .h(SUMMARY_HEIGHT)
        .px(px(16.0))
        .flex()
        .items_center()
        .border_b_1()
        .border_color(rgb(t.border))
        .text_size(ui_text_ms(cx))
        .text_color(rgb(t.text_muted))
        .child(summary)
        .into_any_element()
}

fn placeholder(key: Option<&ReviewFileKey>, t: &ThemeColors, cx: &App) -> AnyElement {
    let path = key.map_or_else(|| NO_FILE.to_string(), ReviewFileKey::display);
    div()
        .h(ROW_HEIGHT)
        .px(px(16.0))
        .flex_none()
        .flex()
        .items_center()
        .border_b_1()
        .border_color(rgb(t.border))
        .text_size(ui_text_ms(cx))
        .text_color(rgb(t.text_secondary))
        .child(path)
        .into_any_element()
}
