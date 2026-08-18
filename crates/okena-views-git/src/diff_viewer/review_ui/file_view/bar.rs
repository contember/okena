//! The 32 px symbol bar and the details block under it — spec §9.

use super::super::super::DiffViewer;
use super::super::super::line_render::{WORD_BG_ALPHA, rgba as tint};
use super::super::labels;
use super::super::labels::reasons as words;
use super::super::model::{CallRow, ReasonKind, SymbolEntry};
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
const DETAILS_WORD: &str = " details";
const COLLAPSED_ARROW: &str = "\u{25B8}";
const EXPANDED_ARROW: &str = "\u{25BE}";
const REASONS_TITLE: &str = "Reasons";
const LINES_TITLE: &str = "Lines";
const SIGNATURE_TITLE: &str = "Signature";
const CALLS_TITLE: &str = "Calls";
const CALLS_CAVEAT: &str = "same file, syntactic \u{00B7} callers are not tracked";
const COMPLEXITY_TITLE: &str = "Complexity";
/// The details label column.
const LABEL_WIDTH: Pixels = px(72.0);
/// Calls listed before `… n more`; the diff underneath has the rest.
const MAX_CALL_ROWS: usize = 8;

pub(super) fn render(
    view: &DiffViewer,
    t: &ThemeColors,
    cx: &mut Context<DiffViewer>,
) -> Option<AnyElement> {
    let entry = view.review_open_entry()?;
    let index = view.review_current_symbol_index()?;
    let symbol = entry.symbols.get(index)?;
    let narrow = view.review_content_is_narrow();
    let expanded = view.review_ui.details_expanded;
    // A narrow column keeps the name and two chips; the counter and the churn
    // repeat what the header and the diff already say — spec §12.
    let chip_limit = if narrow { NARROW_CHIPS } else { BAR_CHIPS };
    let counter = (!narrow).then(|| {
        format!(
            "{}{}{NEXT_HINT}",
            text::symbol_counter(index, entry.symbols.len()),
            text::DOT
        )
    });

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
        .when(!narrow, |d| {
            d.children(churn(
                u64::from(symbol.lines_added),
                u64::from(symbol.lines_deleted),
                t,
                cx,
            ))
        })
        .child(div().flex_1())
        .when_some(counter, |d, counter| {
            d.child(word(counter, t.text_muted, cx))
        })
        .child(details_toggle(expanded, narrow, t, cx));

    if !expanded {
        return Some(bar.into_any_element());
    }
    Some(
        v_flex()
            .flex_none()
            .child(bar)
            .child(details(symbol, chip_limit, t, cx))
            .into_any_element(),
    )
}

/// The arrow alone in a narrow column; the word is the first thing to go — §12.
fn details_toggle(
    expanded: bool,
    narrow: bool,
    t: &ThemeColors,
    cx: &mut Context<DiffViewer>,
) -> AnyElement {
    let arrow = if expanded {
        EXPANDED_ARROW
    } else {
        COLLAPSED_ARROW
    };
    let label = if narrow {
        arrow.to_string()
    } else {
        format!("{arrow}{DETAILS_WORD}")
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

/// The details block: a label column and one row per fact the symbol has.
/// It always has at least the line span, so opening it never shows nothing.
fn details(symbol: &SymbolEntry, shown_chips: usize, t: &ThemeColors, cx: &App) -> AnyElement {
    let complex = symbol
        .reasons
        .iter()
        .any(|reason| reason.kind == ReasonKind::Complex);
    v_flex()
        .flex_none()
        .min_w_0()
        .px(px(16.0))
        .py(px(8.0))
        .gap(px(6.0))
        .overflow_hidden()
        .bg(rgb(t.bg_secondary))
        .border_b_1()
        .border_color(rgb(t.border))
        // Every reason, when the bar could not fit them all.
        .when(symbol.reasons.len() > shown_chips, |d| {
            d.child(detail_row(
                REASONS_TITLE,
                h_flex()
                    .flex_wrap()
                    .gap(px(4.0))
                    .children(symbol.reasons.iter().map(|reason| chip(reason, t, cx)))
                    .into_any_element(),
                t,
                cx,
            ))
        })
        .child(detail_row(
            LINES_TITLE,
            plain(
                text::line_span(&symbol.old_hunks, &symbol.new_hunks),
                t.text_secondary,
                cx,
            ),
            t,
            cx,
        ))
        .when_some(symbol.signature.as_ref(), |d, (old, new)| {
            d.child(detail_row(
                SIGNATURE_TITLE,
                signature_block(old, new, t, cx),
                t,
                cx,
            ))
        })
        .when(!symbol.calls.is_empty(), |d| {
            d.child(detail_row(
                CALLS_TITLE,
                calls_block(&symbol.calls, t, cx),
                t,
                cx,
            ))
        })
        .when(complex, |d| {
            d.children(
                metrics_text(symbol).map(|line| {
                    detail_row(COMPLEXITY_TITLE, plain(line, t.text_secondary, cx), t, cx)
                }),
            )
        })
        .into_any_element()
}

/// `label   content` — the label column keeps every row aligned.
fn detail_row(title: &'static str, content: AnyElement, t: &ThemeColors, cx: &App) -> AnyElement {
    h_flex()
        .items_start()
        .gap(px(12.0))
        .min_w_0()
        .child(
            div()
                .flex_none()
                .w(LABEL_WIDTH)
                .pt(px(1.0))
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(title),
        )
        .child(div().flex_1().min_w_0().child(content))
        .into_any_element()
}

fn plain(text: String, color: u32, cx: &App) -> AnyElement {
    div()
        .min_w_0()
        .text_size(ui_text_ms(cx))
        .text_color(rgb(color))
        .child(text)
        .into_any_element()
}

fn signature_block(old: &str, new: &str, t: &ThemeColors, cx: &App) -> AnyElement {
    let segments = token_diff(old, new);
    v_flex()
        .min_w_0()
        .gap(px(2.0))
        .overflow_hidden()
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
        .min_w_0()
        .gap(px(8.0))
        .items_start()
        .overflow_hidden()
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
    let rows = text::call_lines(calls, MAX_CALL_ROWS);
    v_flex()
        .min_w_0()
        .gap(px(2.0))
        .overflow_hidden()
        .children(rows.shown.iter().map(|row| call_line(row, t, cx)))
        .when_some(rows.hidden_note(), |d, note| {
            d.child(
                div()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(note),
            )
        })
        .child(
            div()
                .pt(px(2.0))
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(CALLS_CAVEAT),
        )
        .into_any_element()
}

/// `− parse(x)          in loop` — marker, one-line call text, context.
fn call_line(row: &text::CallLine, t: &ThemeColors, cx: &App) -> AnyElement {
    let tone = match row.change {
        CallChangeKind::Added => t.diff_added_fg,
        CallChangeKind::Removed => t.diff_removed_fg,
        CallChangeKind::Modified => t.warning,
    };
    h_flex()
        .min_w_0()
        .gap(px(8.0))
        .items_center()
        .child(
            div()
                .flex_none()
                .w(px(8.0))
                .font_family("monospace")
                .text_size(ui_text_ms(cx))
                .text_color(rgb(tone))
                .child(text::call_marker(row.change)),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_family("monospace")
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_secondary))
                .child(row.text_with_count()),
        )
        .when_some(row.context.clone(), |d, context| {
            d.child(
                div()
                    .flex_none()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(context),
            )
        })
        .into_any_element()
}

/// What made the symbol complex; only shown when complexity is a reason — §9.
fn metrics_text(symbol: &SymbolEntry) -> Option<String> {
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
    (!parts.is_empty()).then(|| parts.join(text::DOT))
}
