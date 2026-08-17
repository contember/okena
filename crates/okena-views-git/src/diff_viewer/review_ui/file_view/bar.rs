//! The 32 px symbol bar and the details block under it — spec §9.

use super::super::super::DiffViewer;
use super::super::super::line_render::{WORD_BG_ALPHA, rgba as tint};
use super::super::labels;
use super::super::labels::reasons as words;
use super::super::model::{CallRow, FileEntry, ReasonKind, SymbolEntry};
use super::structure::viewport_symbol;
use super::text;
use super::token_diff::{Segment, SegmentKind, token_diff};
use super::{chip, churn, word};
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use gpui_component::v_flex;
use okena_core::theme::ThemeColors;
use okena_review::CallChangeKind;
use okena_ui::tokens::{ui_text_ms, ui_text_sm};

const BAR_CHIPS: usize = 3;
/// A narrow column keeps the name and the first two chips — spec §12.
const NARROW_CHIPS: usize = 2;
const NEXT_HINT: &str = "} next";
const DETAILS_COLLAPSED: &str = "\u{25B8} details";
const DETAILS_EXPANDED: &str = "\u{25BE} details";
const SIGNATURE_TITLE: &str = "Signature (normalized)";
const CALLS_TITLE: &str = "Calls changed in this function";
const CALLS_CAVEAT: &str = "\u{2014} same file, syntactic; callers are not tracked";
const COMPLEXITY_TITLE: &str = "Complexity";

pub(super) fn render(
    view: &DiffViewer,
    t: &ThemeColors,
    cx: &mut Context<DiffViewer>,
) -> Option<AnyElement> {
    let entry = view.review_open_entry()?;
    if entry.symbols.is_empty() {
        return None;
    }
    let index = current_symbol(view, entry);
    let symbol = entry.symbols.get(index)?;
    let narrow = view.review_content_is_narrow();
    let expanded = view.review_ui.details_expanded;
    let chip_limit = if narrow { NARROW_CHIPS } else { BAR_CHIPS };
    let counter = format!(
        "{}{}{NEXT_HINT}",
        text::symbol_counter(index, entry.symbols.len()),
        text::DOT
    );

    let bar = h_flex()
        .h(px(32.0))
        .px(px(16.0))
        .gap(px(8.0))
        .flex_none()
        .items_center()
        .bg(rgb(t.bg_secondary))
        .border_b_1()
        .border_color(rgb(t.border))
        .child(word(labels::glyph(symbol.glyph), t.text_muted, cx))
        .child(
            div()
                .flex_none()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_primary))
                .child(symbol.name.clone()),
        )
        .children(
            symbol
                .reasons
                .iter()
                .take(chip_limit)
                .map(|reason| chip(reason, t, cx)),
        )
        .children(churn(
            u64::from(symbol.lines_added),
            u64::from(symbol.lines_deleted),
            t,
            cx,
        ))
        .child(div().flex_1())
        .child(word(counter, t.text_muted, cx))
        .child(details_toggle(expanded, t, cx));

    if !expanded {
        return Some(bar.into_any_element());
    }
    Some(
        v_flex()
            .flex_none()
            .child(bar)
            .child(details(symbol, t, cx))
            .into_any_element(),
    )
}

/// The selected symbol wins; otherwise the bar follows the viewport — spec §9.
fn current_symbol(view: &DiffViewer, entry: &FileEntry) -> usize {
    if let Some(selected) = view.review_ui.selected_symbol.as_ref()
        && selected.file == entry.key
        && let Some(index) = entry
            .symbols
            .iter()
            .position(|symbol| symbol.change_index == selected.change_index)
    {
        return index;
    }
    let (old, new) = view.review_viewport_lines();
    viewport_symbol(&entry.symbols, old, new).unwrap_or(0)
}

fn details_toggle(expanded: bool, t: &ThemeColors, cx: &mut Context<DiffViewer>) -> AnyElement {
    let label = if expanded {
        DETAILS_EXPANDED
    } else {
        DETAILS_COLLAPSED
    };
    div()
        .id("review-symbol-details")
        .flex_none()
        .cursor_pointer()
        .text_size(ui_text_ms(cx))
        .text_color(rgb(t.term_blue))
        .hover(|style| style.text_color(rgb(t.text_primary)))
        .on_click(cx.listener(|this, _, _window, cx| this.review_toggle_details(cx)))
        .child(label)
        .into_any_element()
}

fn details(symbol: &SymbolEntry, t: &ThemeColors, cx: &App) -> AnyElement {
    let complex = symbol
        .reasons
        .iter()
        .any(|reason| reason.kind == ReasonKind::Complex);
    v_flex()
        .flex_none()
        .px(px(16.0))
        .py(px(8.0))
        .gap(px(10.0))
        .bg(rgb(t.bg_secondary))
        .border_b_1()
        .border_color(rgb(t.border))
        .when_some(symbol.signature.as_ref(), |d, (old, new)| {
            d.child(signature_block(old, new, t, cx))
        })
        .when(!symbol.calls.is_empty(), |d| {
            d.child(calls_block(&symbol.calls, t, cx))
        })
        .when(complex, |d| d.children(metrics_block(symbol, t, cx)))
        .into_any_element()
}

fn signature_block(old: &str, new: &str, t: &ThemeColors, cx: &App) -> AnyElement {
    let segments = token_diff(old, new);
    v_flex()
        .gap(px(2.0))
        .child(caption(SIGNATURE_TITLE, t, cx))
        .child(signature_line(
            "\u{2212}",
            t.diff_removed_fg,
            &segments,
            SegmentKind::Removed,
            t,
            cx,
        ))
        .child(signature_line(
            "+",
            t.diff_added_fg,
            &segments,
            SegmentKind::Added,
            t,
            cx,
        ))
        .into_any_element()
}

fn signature_line(
    marker: &'static str,
    tone: u32,
    segments: &[Segment],
    changed: SegmentKind,
    t: &ThemeColors,
    cx: &App,
) -> AnyElement {
    h_flex()
        .gap(px(8.0))
        .items_start()
        .font_family("monospace")
        .text_size(ui_text_ms(cx))
        .child(
            div()
                .flex_none()
                .w(px(8.0))
                .text_color(rgb(tone))
                .child(marker),
        )
        .child(
            h_flex()
                .min_w_0()
                .flex_wrap()
                .text_color(rgb(t.text_secondary))
                .children(
                    segments
                        .iter()
                        .filter(|segment| segment.on_side(changed))
                        .map(|segment| {
                            div()
                                .flex_none()
                                .when(segment.kind == changed, |d| {
                                    d.bg(tint(tone, WORD_BG_ALPHA)).text_color(rgb(tone))
                                })
                                .child(segment.text.clone())
                                .into_any_element()
                        }),
                ),
        )
        .into_any_element()
}

fn calls_block(calls: &[CallRow], t: &ThemeColors, cx: &App) -> AnyElement {
    v_flex()
        .gap(px(4.0))
        .child(
            h_flex()
                .gap(px(6.0))
                .items_center()
                .child(caption(CALLS_TITLE, t, cx))
                .child(
                    div()
                        .flex_none()
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.text_muted))
                        .child(CALLS_CAVEAT),
                ),
        )
        .child(
            h_flex()
                .flex_wrap()
                .gap(px(14.0))
                .children(calls.iter().map(|row| call_element(row, t, cx))),
        )
        .into_any_element()
}

fn call_element(row: &CallRow, t: &ThemeColors, cx: &App) -> AnyElement {
    let tone = match row.change {
        CallChangeKind::Added => t.diff_added_fg,
        CallChangeKind::Removed => t.diff_removed_fg,
        CallChangeKind::Modified => t.warning,
    };
    h_flex()
        .flex_none()
        .gap(px(4.0))
        .items_center()
        .text_size(ui_text_ms(cx))
        .child(div().flex_none().text_color(rgb(tone)).child(format!(
            "{} {}",
            text::call_marker(row.change),
            text::call_text(row)
        )))
        .when_some(text::call_context(row), |d, context| {
            d.child(
                div()
                    .flex_none()
                    .text_color(rgb(t.text_muted))
                    .child(context),
            )
        })
        .into_any_element()
}

/// What made the symbol complex; only shown when complexity is a reason — §9.
fn metrics_block(symbol: &SymbolEntry, t: &ThemeColors, cx: &App) -> Option<AnyElement> {
    let mut parts = Vec::new();
    if let Some(depth) = symbol.metrics.depth {
        parts.push(words::nesting_label(depth));
    }
    if let Some(params) = symbol.metrics.params {
        parts.push(words::params_label(params));
    }
    if let Some(lines) = symbol.metrics.lines {
        parts.push(words::lines_label(lines));
    }
    if let Some(members) = symbol.metrics.members {
        parts.push(words::members_label(members));
    }
    if parts.is_empty() {
        return None;
    }
    Some(
        v_flex()
            .gap(px(2.0))
            .child(caption(COMPLEXITY_TITLE, t, cx))
            .child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(parts.join(text::DOT)),
            )
            .into_any_element(),
    )
}

fn caption(title: &'static str, t: &ThemeColors, cx: &App) -> AnyElement {
    div()
        .flex_none()
        .text_size(ui_text_sm(cx))
        .text_color(rgb(t.text_muted))
        .child(title)
        .into_any_element()
}
