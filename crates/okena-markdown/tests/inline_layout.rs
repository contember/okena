//! Regression test for the markdown inline-wrap layout bug.
//!
//! A block that mixes inline code chips with a long trailing plain-text run used
//! to render with a huge vertical gap when it was a flex child (a list item next
//! to its bullet): `min-width: auto` pinned the inline-flow container to its
//! widest word, so its wrapping height was measured at that narrow width while it
//! was painted full-width. We render block 0 at a fixed width and assert the
//! measured height stays at the natural multi-line text height.

use gpui::prelude::*;
use gpui::{AvailableSpace, Point, Size, TestAppContext, div, point, px};
use okena_core::theme::DARK_THEME;
use okena_markdown::{MarkdownDocument, MarkdownTextRun, RenderedNode};
use std::cell::RefCell;
use std::rc::Rc;

/// The shared body of both contember-oss "Package Groups" shapes: inline code
/// chips up front, then a long plain-text run.
const BODY: &str = "**Engine** (`engine-server`, `engine-http`): The backend server. \
    `engine-server` bootstraps and clusters, `engine-http` provides Koa-based \
    HTTP/WebSocket routing and multi-tenant project resolution, the three API \
    packages implement GraphQL resolvers for content CRUD, schema/migration \
    management, and identity/project/membership management.";

/// Render block 0 of `md` inside a fixed-width container and return its height.
fn measure_block_height(cx: &mut TestAppContext, md: &str, width: f32) -> f32 {
    let doc = MarkdownDocument::parse(md);
    let vcx = cx.add_empty_window();

    vcx.draw(
        Point::default(),
        Size {
            width: AvailableSpace::Definite(px(width)),
            height: AvailableSpace::MinContent,
        },
        |_window, cx| {
            let node = match doc.render_node(0, &DARK_THEME, cx, None) {
                Some(RenderedNode::Simple { div, .. }) => div,
                _ => div(),
            };
            div()
                .w(px(width))
                .debug_selector(|| "block".to_string())
                .child(node)
        },
    );

    f32::from(
        vcx.debug_bounds("block")
            .expect("block bounds should be recorded")
            .size
            .height,
    )
}

#[gpui::test]
fn list_item_with_long_trailing_text_is_not_inflated(cx: &mut TestAppContext) {
    // A bullet (flex child) — the shape that triggered the bug. Pre-fix this
    // measured 220px (10 lines); wrapped correctly it is ~138px (6 lines at the
    // body leading), so 160px sits clear of both.
    let height = measure_block_height(cx, &format!("- {BODY}"), 600.0);
    assert!(
        height < 160.0,
        "list block height {height}px is inflated (expected a handful of text lines)"
    );
}

#[gpui::test]
fn paragraph_with_long_trailing_text_is_not_inflated(cx: &mut TestAppContext) {
    // The same content as a paragraph always wrapped correctly; guards the
    // baseline so the list assertion stays meaningful.
    let height = measure_block_height(cx, BODY, 600.0);
    assert!(
        height < 160.0,
        "paragraph block height {height}px is inflated (expected a handful of text lines)"
    );
}

/// Long text runs separated by inline code — the shape where a run that wraps
/// used to occupy a full-width box and push the chip after it onto a new line.
const MIXED: &str = "If it is a one-way door with a wide blast radius, data migration, \
    public API or security model, say so and offer `comprehensive-plan` instead. \
    Anything that is hard to unship deserves the slower path, so reach for \
    `comprehensive-plan` again. When the change is cheap to revert and the blast \
    radius is small, `weigh-options` is enough on its own. Deliberation that costs \
    more than the mistake is waste, and `weigh-smallest` cuts it down.";

/// The same prose without the code spans, as the baseline line count.
const MIXED_PLAIN: &str = "If it is a one-way door with a wide blast radius, data migration, \
    public API or security model, say so and offer comprehensive-plan instead. \
    Anything that is hard to unship deserves the slower path, so reach for \
    comprehensive-plan again. When the change is cheap to revert and the blast \
    radius is small, weigh-options is enough on its own. Deliberation that costs \
    more than the mistake is waste, and weigh-smallest cuts it down.";

#[gpui::test]
fn inline_code_does_not_force_a_line_break(cx: &mut TestAppContext) {
    // Pre-fix this was 276px against a 161px baseline — five lines lost to the
    // break each chip forced after the run before it.
    let mixed = measure_block_height(cx, MIXED, 600.0);
    let plain = measure_block_height(cx, MIXED_PLAIN, 600.0);
    assert!(
        mixed <= plain,
        "inline code added lines: {mixed}px against a {plain}px plain-text baseline"
    );
}

#[gpui::test]
fn bold_does_not_force_a_line_break(cx: &mut TestAppContext) {
    // Bold leads, plain text follows: the text after it used to start on its own
    // line (184px against the same 161px baseline).
    let bold = measure_block_height(
        cx,
        &format!("**The right one is X** — {MIXED_PLAIN}"),
        600.0,
    );
    let plain = measure_block_height(cx, &format!("The right one is X — {MIXED_PLAIN}"), 600.0);
    assert!(
        bold <= plain,
        "bold added lines: {bold}px against a {plain}px plain-text baseline"
    );
}

#[gpui::test]
fn text_run_hit_testing_returns_global_character_offsets(cx: &mut TestAppContext) {
    let doc = MarkdownDocument::parse("# Prefix\n\nAé🙂Z");
    let captured: Rc<RefCell<Option<(MarkdownTextRun, usize, usize)>>> = Default::default();
    let captured_for_draw = captured.clone();
    let vcx = cx.add_empty_window();

    vcx.draw(
        Point::default(),
        Size {
            width: AvailableSpace::Definite(px(400.0)),
            height: AvailableSpace::MinContent,
        },
        |_window, cx| {
            let node = match doc.render_node(1, &DARK_THEME, cx, None) {
                Some(RenderedNode::Simple {
                    div,
                    start_offset,
                    end_offset,
                    text_runs,
                }) => {
                    let run = text_runs
                        .first()
                        .expect("paragraph should expose a text run")
                        .clone();
                    *captured_for_draw.borrow_mut() = Some((run, start_offset, end_offset));
                    div
                }
                _ => div(),
            };
            div().w(px(400.0)).child(node)
        },
    );

    let (run, start_offset, end_offset) = captured
        .borrow()
        .clone()
        .expect("render should capture the paragraph run");
    let bounds = run.bounds();
    let middle_y = bounds.origin.y + bounds.size.height / 2.0;
    let at_start = run
        .index_for_position(point(bounds.origin.x, middle_y))
        .unwrap_or_else(|offset| offset);
    let past_end = run
        .index_for_position(point(
            bounds.origin.x + bounds.size.width + px(20.0),
            middle_y,
        ))
        .unwrap_or_else(|offset| offset);

    assert_eq!(at_start, start_offset);
    assert_eq!(past_end, end_offset - 1);
    assert_eq!(past_end - at_start, 4, "Unicode must count as characters");
}

fn run_start_offset(run: &MarkdownTextRun) -> usize {
    let bounds = run.bounds();
    run.index_for_position(bounds.origin)
        .unwrap_or_else(|offset| offset)
}

#[gpui::test]
fn table_text_runs_follow_flat_text_offsets(cx: &mut TestAppContext) {
    let doc = MarkdownDocument::parse("| Hé | B🙂 |\n| --- | --- |\n| one | two |\n");
    let captured: Rc<RefCell<Vec<MarkdownTextRun>>> = Default::default();
    let captured_for_draw = captured.clone();
    let vcx = cx.add_empty_window();

    vcx.draw(
        Point::default(),
        Size {
            width: AvailableSpace::Definite(px(500.0)),
            height: AvailableSpace::MinContent,
        },
        |_window, cx| {
            let Some(RenderedNode::Table { header, rows }) =
                doc.render_node(0, &DARK_THEME, cx, None)
            else {
                return div();
            };
            let units = header.into_iter().chain(rows);
            let mut children = Vec::new();
            for unit in units {
                captured_for_draw
                    .borrow_mut()
                    .extend(unit.text_runs.iter().cloned());
                children.push(unit.div.into_any_element());
            }
            div().flex().flex_col().children(children)
        },
    );

    let starts = captured
        .borrow()
        .iter()
        .map(run_start_offset)
        .collect::<Vec<_>>();
    assert_eq!(starts, [0, 3, 6, 10]);
    assert_eq!(doc.plain_text, "Hé\tB🙂\none\ttwo\n");
}

#[gpui::test]
fn frontmatter_text_runs_follow_flat_text_offsets(cx: &mut TestAppContext) {
    let doc = MarkdownDocument::parse("---\ntitle: Žluť\nitems:\n  - one\n---\n");
    let captured: Rc<RefCell<Vec<MarkdownTextRun>>> = Default::default();
    let captured_for_draw = captured.clone();
    let vcx = cx.add_empty_window();

    vcx.draw(
        Point::default(),
        Size {
            width: AvailableSpace::Definite(px(500.0)),
            height: AvailableSpace::MinContent,
        },
        |_window, cx| {
            let Some(RenderedNode::Simple { div, text_runs, .. }) =
                doc.render_node(0, &DARK_THEME, cx, None)
            else {
                return div();
            };
            captured_for_draw.borrow_mut().extend(text_runs);
            div
        },
    );

    let starts = captured
        .borrow()
        .iter()
        .map(run_start_offset)
        .collect::<Vec<_>>();
    assert_eq!(starts, [0, 7, 12, 21, 23]);
    assert_eq!(doc.plain_text, "title: Žluť\nitems:\n  • one\n");
}
