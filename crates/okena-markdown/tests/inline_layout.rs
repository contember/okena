//! Regression test for the markdown inline-wrap layout bug.
//!
//! A block that mixes inline code chips with a long trailing plain-text run used
//! to render with a huge vertical gap when it was a flex child (a list item next
//! to its bullet): `min-width: auto` pinned the inline-flow container to its
//! widest word, so its wrapping height was measured at that narrow width while it
//! was painted full-width. We render block 0 at a fixed width and assert the
//! measured height stays at the natural multi-line text height.

use gpui::prelude::*;
use gpui::{div, px, AvailableSpace, Point, Size, TestAppContext};
use okena_core::theme::DARK_THEME;
use okena_markdown::{MarkdownDocument, RenderedNode};

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
    // measured 220px (10 lines); the same text as a paragraph is ~66px.
    let height = measure_block_height(cx, &format!("- {BODY}"), 600.0);
    assert!(
        height < 132.0,
        "list block height {height}px is inflated (expected a handful of text lines)"
    );
}

#[gpui::test]
fn paragraph_with_long_trailing_text_is_not_inflated(cx: &mut TestAppContext) {
    // The same content as a paragraph always wrapped correctly; guards the
    // baseline so the list assertion stays meaningful.
    let height = measure_block_height(cx, BODY, 600.0);
    assert!(
        height < 132.0,
        "paragraph block height {height}px is inflated (expected a handful of text lines)"
    );
}
