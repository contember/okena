//! Per-row renderer for the railway commit graph.
//!
//! Layout responsibility lives in [`super::lane_layout`]; this file just
//! paints what the layout computed. Each `LaneRow` is rendered in its own
//! container with its own rails canvas. Rails are split into an upper half
//! (top edge → vertical middle) and a lower half (middle → bottom edge), so
//! merges fan in above the dot and forks fan out below it, all inside the
//! single row.
//!
//! Hover is a plain `.hover()` background on the row: GPUI paints a
//! container's quad *before* its children, so the tint sits behind the rails
//! canvas (rails stay visible on top, which is the intended look) — no
//! `deferred`/`paint_layer` tricks needed.

use super::lane_layout::{Half, LaneId, LaneRow, Rail};

use okena_core::theme::ThemeColors;
use okena_ui::tokens::{ui_text_md, ui_text_ms, ui_text_sm};

use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use std::collections::BTreeMap;

/// Width of each lane in pixels (centre-to-centre distance between two
/// adjacent display columns).
const GRAPH_CELL_W: f32 = 12.0;
/// Thickness of railway lines.
const RAIL_W: f32 = 2.0;
/// Diameter of commit dots.
const DOT_SIZE: f32 = 8.0;
/// Commit row height.
const COMMIT_ROW_H: f32 = 28.0;

/// Horizontal padding before the graph column inside each row.
pub(super) const GRAPH_PAD_LEFT: f32 = 4.0;

/// Lane color palette. We pick by `palette_idx % len`, where `palette_idx`
/// is the lane's assigned color (stable for the lane's lifetime).
const LANE_COLORS: &[fn(&ThemeColors) -> u32] = &[
    |t| t.term_cyan,
    |t| t.term_green,
    |t| t.term_yellow,
    |t| t.term_magenta,
    |t| t.term_blue,
    |t| t.term_red,
];

fn lane_color(lane_id: LaneId, palette: &BTreeMap<LaneId, usize>, t: &ThemeColors) -> u32 {
    let idx = palette.get(&lane_id).copied().unwrap_or(lane_id as usize);
    LANE_COLORS[idx % LANE_COLORS.len()](t)
}

/// Build a stable color palette: assign each `lane_id` an index in order of
/// first appearance so adjacent lanes don't clash too often.
pub(super) fn build_palette(rows: &[LaneRow]) -> BTreeMap<LaneId, usize> {
    let mut palette: BTreeMap<LaneId, usize> = BTreeMap::new();
    for row in rows {
        let next = palette.len();
        palette.entry(row.dot.1).or_insert(next);
        for rail in &row.rails {
            let next = palette.len();
            palette.entry(rail.lane_id).or_insert(next);
        }
    }
    palette
}

fn col_x(col: usize) -> f32 {
    col as f32 * GRAPH_CELL_W + GRAPH_CELL_W / 2.0
}

fn stroke(window: &mut Window, color: u32, build: impl FnOnce(&mut PathBuilder)) {
    let options = StrokeOptions::default()
        .with_line_width(RAIL_W)
        .with_line_join(lyon::path::LineJoin::Round)
        .with_line_cap(lyon::path::LineCap::Round);
    let mut b = PathBuilder::stroke(px(RAIL_W)).with_style(PathStyle::Stroke(options));
    build(&mut b);
    if let Ok(path) = b.build() {
        window.paint_path(path, rgb(color));
    }
}

/// Paint one row's rails into the given bounds origin. Upper rails span
/// `[oy, oy+H/2]`, lower rails span `[oy+H/2, oy+H]`.
fn paint_rails(
    window: &mut Window,
    ox: f32,
    oy: f32,
    row_h: f32,
    rails: &[Rail],
    palette: &BTreeMap<LaneId, usize>,
    t: &ThemeColors,
) {
    let mid_y = oy + row_h / 2.0;

    for rail in rails {
        let (y_start, y_end) = match rail.half {
            Half::Upper => (oy, mid_y),
            Half::Lower => (mid_y, oy + row_h),
        };
        let x_start = ox + col_x(rail.from_col);
        let x_end = ox + col_x(rail.to_col);
        let color = lane_color(rail.lane_id, palette, t);

        if (x_start - x_end).abs() < 0.01 {
            stroke(window, color, |b| {
                b.move_to(point(px(x_start), px(y_start)));
                b.line_to(point(px(x_start), px(y_end)));
            });
        } else {
            // Cubic bezier with vertical tangents at both ends so the curve
            // chains smoothly into the vertical rails above / below.
            let y_ctrl = (y_start + y_end) / 2.0;
            stroke(window, color, |b| {
                b.move_to(point(px(x_start), px(y_start)));
                b.cubic_bezier_to(
                    point(px(x_end), px(y_end)),
                    point(px(x_start), px(y_ctrl)),
                    point(px(x_end), px(y_ctrl)),
                );
            });
        }
    }
}

/// Render a ref label pill (e.g. "HEAD -> main", "origin/main", "tag: v1.0").
fn render_ref_label(ref_name: &str, t: &ThemeColors, cx: &App) -> AnyElement {
    let color = if ref_name.contains("HEAD") {
        t.term_cyan
    } else if ref_name.starts_with("tag:") {
        t.term_yellow
    } else if ref_name.starts_with("origin/") || ref_name.contains('/') {
        t.term_green
    } else {
        t.term_magenta
    };
    let bg = {
        let c: Hsla = rgb(color).into();
        hsla(c.h, c.s, c.l, 0.15)
    };
    div()
        .px(px(4.0))
        .py(px(1.0))
        .rounded(px(3.0))
        .bg(bg)
        .text_size(ui_text_sm(cx))
        .text_color(rgb(color))
        .flex_shrink_0()
        .max_w(px(140.0))
        .text_ellipsis()
        .overflow_hidden()
        .child(ref_name.to_string())
        .into_any_element()
}

/// Render a single commit row. The rails canvas lives *inside* the row
/// container; the row's `.hover()` background tint paints behind it.
///
/// `on_commit_click` is called with `(commit_hash, commit_message, commit_index)`
/// when the user clicks a commit row.
///
/// `on_commit_right_click` is called with `(commit_hash, mouse_position)` when
/// the user right-clicks a commit row — used to open a context menu (e.g.
/// "Send to Terminal", "Copy Hash").
pub(super) fn render_lane_row(
    row: &LaneRow,
    index: usize,
    max_col: usize,
    palette: &BTreeMap<LaneId, usize>,
    on_commit_click: super::CommitClickCallback,
    on_commit_right_click: super::CommitRightClickCallback,
    t: &ThemeColors,
    cx: &App,
) -> AnyElement {
    let row_h = COMMIT_ROW_H;
    let graph_width = (max_col + 1) as f32 * GRAPH_CELL_W;

    let rails = row.rails.clone();
    let palette_for_canvas = palette.clone();
    let theme = *t;

    let rails_canvas = canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let ox = f32::from(bounds.origin.x);
            let oy = f32::from(bounds.origin.y);
            paint_rails(window, ox, oy, row_h, &rails, &palette_for_canvas, &theme);
        },
    )
    .absolute()
    .left(px(GRAPH_PAD_LEFT))
    .top(px(0.0))
    .w(px(graph_width))
    .h(px(row_h));

    let entry = &row.entry;
    let (dot_col, dot_lane) = row.dot;
    let dot_color = lane_color(dot_lane, palette, t);
    let dot_x = GRAPH_PAD_LEFT + dot_col as f32 * GRAPH_CELL_W + (GRAPH_CELL_W - DOT_SIZE) / 2.0;
    let dot_y = (row_h - DOT_SIZE) / 2.0;

    let mut row_el = h_flex()
        .id(ElementId::Name(format!("graph-row-{}", index).into()))
        .relative()
        .pr(px(12.0))
        .h(px(row_h))
        .cursor_pointer()
        // Hover tint — painted behind children (rails/dot/text stay visible).
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .child(rails_canvas)
        .child(
            div()
                .absolute()
                .left(px(dot_x))
                .top(px(dot_y))
                .w(px(DOT_SIZE))
                .h(px(DOT_SIZE))
                .rounded(px(DOT_SIZE / 2.0))
                .bg(rgb(dot_color)),
        )
        // Spacer reserving the graph column so text starts past the rails.
        .child(
            div()
                .flex_shrink_0()
                .w(px(GRAPH_PAD_LEFT + graph_width))
                .h(px(row_h)),
        )
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .h(px(row_h))
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(ui_text_md(cx))
                        .text_color(rgb(t.text_primary))
                        .text_ellipsis()
                        .overflow_hidden()
                        .flex_shrink()
                        .min_w_0()
                        .child(entry.message.clone()),
                )
                .children(entry.refs.iter().map(|r| render_ref_label(r, t, cx)))
                .child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .flex_shrink_0()
                        .child(entry.author.clone()),
                ),
        );

    if let Some(cb) = on_commit_right_click {
        let hash = entry.hash.clone();
        row_el = row_el.on_mouse_down(
            MouseButton::Right,
            move |event: &MouseDownEvent, window, cx| {
                cb(&hash, event.position, window, cx);
            },
        );
    }

    if let Some(cb) = on_commit_click {
        let hash = entry.hash.clone();
        let msg = entry.message.clone();
        // Layout rows are 1:1 with the commit list, so the row index is the
        // commit index.
        let commit_idx = index;
        row_el
            .on_click(move |_, window, cx| {
                cb(&hash, &msg, commit_idx, window, cx);
            })
            .into_any_element()
    } else {
        row_el.cursor_default().into_any_element()
    }
}
