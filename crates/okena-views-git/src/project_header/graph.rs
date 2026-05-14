//! Git graph rendering: railway-style commit graph used by the commit log
//! popover. Each character of the textual `graph` string is drawn as one or
//! more absolutely-positioned rails/dots in a lane-coloured palette.

use okena_core::theme::ThemeColors;
use okena_git::{CommitLogEntry, GraphRow};
use okena_ui::tokens::{ui_text_md, ui_text_ms, ui_text_sm};

use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use std::sync::Arc;

/// Width of each graph character column in pixels.
pub const GRAPH_CELL_W: f32 = 10.0;
/// Thickness of railway lines.
pub const RAIL_W: f32 = 2.0;
/// Diameter of commit dots.
pub const DOT_SIZE: f32 = 8.0;
/// Commit row height.
pub const COMMIT_ROW_H: f32 = 24.0;
/// Connector row height.
pub const CONNECTOR_ROW_H: f32 = 10.0;

/// Lane color palette for graph railways.
const LANE_COLORS: &[fn(&ThemeColors) -> u32] = &[
    |t| t.term_cyan,
    |t| t.term_green,
    |t| t.term_yellow,
    |t| t.term_magenta,
    |t| t.term_blue,
    |t| t.term_red,
];

fn lane_color(lane_idx: usize, t: &ThemeColors) -> u32 {
    LANE_COLORS[lane_idx % LANE_COLORS.len()](t)
}

/// Render graph prefix as a single relatively-positioned container with
/// absolutely-positioned railway elements. This ensures lines connect
/// across lane centers regardless of character cell boundaries.
pub fn render_graph_column(graph: &str, max_len: usize, row_h: f32, t: &ThemeColors) -> Div {
    let padded: String = if graph.len() < max_len {
        format!("{:<width$}", graph, width = max_len)
    } else {
        graph.to_string()
    };

    // X coordinate of the rail's left edge for a given column position
    let rail_x = |pos: usize| -> f32 {
        pos as f32 * GRAPH_CELL_W + (GRAPH_CELL_W - RAIL_W) / 2.0
    };

    let mid_y = (row_h - RAIL_W) / 2.0;

    let mut elements: Vec<AnyElement> = Vec::new();

    for (pos, ch) in padded.chars().enumerate() {
        let lane_idx = pos / 2;
        let color = lane_color(lane_idx, t);

        match ch {
            '|' => {
                // Vertical rail -- full height at lane center
                elements.push(
                    div()
                        .absolute()
                        .left(px(rail_x(pos)))
                        .top(px(0.0))
                        .w(px(RAIL_W))
                        .h(px(row_h))
                        .bg(rgb(color))
                        .into_any_element(),
                );
            }
            '*' => {
                // Vertical rail through entire row
                elements.push(
                    div()
                        .absolute()
                        .left(px(rail_x(pos)))
                        .top(px(0.0))
                        .w(px(RAIL_W))
                        .h(px(row_h))
                        .bg(rgb(color))
                        .into_any_element(),
                );
                // Dot on top, centered
                let dot_x = pos as f32 * GRAPH_CELL_W + (GRAPH_CELL_W - DOT_SIZE) / 2.0;
                let dot_y = (row_h - DOT_SIZE) / 2.0;
                elements.push(
                    div()
                        .absolute()
                        .left(px(dot_x))
                        .top(px(dot_y))
                        .w(px(DOT_SIZE))
                        .h(px(DOT_SIZE))
                        .rounded(px(DOT_SIZE / 2.0))
                        .bg(rgb(color))
                        .into_any_element(),
                );
            }
            '\\' => {
                // Fork: S-curve from left lane (top) to right lane (bottom)
                let diag_color = lane_color((pos + 1) / 2, t);
                let lx = rail_x(pos.saturating_sub(1));
                let rx = rail_x(pos + 1);

                // Top vertical: left lane center -> middle
                elements.push(
                    div()
                        .absolute()
                        .left(px(lx))
                        .top(px(0.0))
                        .w(px(RAIL_W))
                        .h(px(mid_y + RAIL_W))
                        .bg(rgb(diag_color))
                        .into_any_element(),
                );
                // Horizontal bridge: left lane center -> right lane center
                elements.push(
                    div()
                        .absolute()
                        .left(px(lx))
                        .top(px(mid_y))
                        .w(px(rx + RAIL_W - lx))
                        .h(px(RAIL_W))
                        .bg(rgb(diag_color))
                        .into_any_element(),
                );
                // Bottom vertical: right lane center -> bottom
                elements.push(
                    div()
                        .absolute()
                        .left(px(rx))
                        .top(px(mid_y))
                        .w(px(RAIL_W))
                        .h(px(row_h - mid_y))
                        .bg(rgb(diag_color))
                        .into_any_element(),
                );
            }
            '/' => {
                // Merge: S-curve from right lane (top) to left lane (bottom)
                let diag_color = lane_color((pos + 1) / 2, t);
                let lx = rail_x(pos.saturating_sub(1));
                let rx = rail_x(pos + 1);

                // Top vertical: right lane center -> middle
                elements.push(
                    div()
                        .absolute()
                        .left(px(rx))
                        .top(px(0.0))
                        .w(px(RAIL_W))
                        .h(px(mid_y + RAIL_W))
                        .bg(rgb(diag_color))
                        .into_any_element(),
                );
                // Horizontal bridge
                elements.push(
                    div()
                        .absolute()
                        .left(px(lx))
                        .top(px(mid_y))
                        .w(px(rx + RAIL_W - lx))
                        .h(px(RAIL_W))
                        .bg(rgb(diag_color))
                        .into_any_element(),
                );
                // Bottom vertical: left lane center -> bottom
                elements.push(
                    div()
                        .absolute()
                        .left(px(lx))
                        .top(px(mid_y))
                        .w(px(RAIL_W))
                        .h(px(row_h - mid_y))
                        .bg(rgb(diag_color))
                        .into_any_element(),
                );
            }
            '_' => {
                // Horizontal connector
                elements.push(
                    div()
                        .absolute()
                        .left(px(pos as f32 * GRAPH_CELL_W))
                        .top(px(mid_y))
                        .w(px(GRAPH_CELL_W))
                        .h(px(RAIL_W))
                        .bg(rgb(color))
                        .into_any_element(),
                );
            }
            _ => {} // space -- nothing
        }
    }

    div().relative().flex_shrink_0().children(elements)
}

/// Render a ref label pill (e.g. "HEAD -> main", "origin/main", "tag: v1.0").
pub fn render_ref_label(ref_name: &str, t: &ThemeColors, cx: &App) -> AnyElement {
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

/// Render a single commit graph row (either a commit entry or a connector line).
///
/// `on_commit_click` is called with `(commit_hash, commit_message, commit_index)`
/// when the user clicks a commit row.
///
/// `on_commit_right_click` is called with `(commit_hash, mouse_position)` when
/// the user right-clicks a commit row — used to open a context menu (e.g.
/// "Send to Terminal", "Copy Hash").
pub fn render_graph_row(
    row: &GraphRow,
    index: usize,
    max_graph_len: usize,
    all_commits: &[CommitLogEntry],
    on_commit_click: Option<Arc<dyn Fn(&str, &str, usize, &mut Window, &mut App)>>,
    on_commit_right_click: Option<Arc<dyn Fn(&str, gpui::Point<gpui::Pixels>, &mut Window, &mut App)>>,
    t: &ThemeColors,
    cx: &App,
) -> AnyElement {
    let graph_width = max_graph_len as f32 * GRAPH_CELL_W;

    match row {
        GraphRow::Commit(entry) => {
            let mut row_el = h_flex()
                .id(ElementId::Name(format!("graph-row-{}", index).into()))
                .pl(px(4.0))
                .pr(px(12.0))
                .h(px(COMMIT_ROW_H))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .child(
                    render_graph_column(&entry.graph, max_graph_len, COMMIT_ROW_H, t)
                        .w(px(graph_width))
                        .h(px(COMMIT_ROW_H)),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .h(px(COMMIT_ROW_H))
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
                row_el = row_el.on_mouse_down(MouseButton::Right, move |event: &MouseDownEvent, window, cx| {
                    cb(&hash, event.position, window, cx);
                });
            }

            if let Some(cb) = on_commit_click {
                let hash = entry.hash.clone();
                let msg = entry.message.clone();
                let commit_idx = all_commits
                    .iter()
                    .position(|c| c.hash == entry.hash)
                    .unwrap_or(0);
                row_el
                    .on_click(move |_, window, cx| {
                        cb(&hash, &msg, commit_idx, window, cx);
                    })
                    .into_any_element()
            } else {
                row_el.cursor_default().into_any_element()
            }
        }
        GraphRow::Connector(graph) => div()
            .pl(px(4.0))
            .h(px(CONNECTOR_ROW_H))
            .child(
                render_graph_column(graph, max_graph_len, CONNECTOR_ROW_H, t)
                    .w(px(graph_width))
                    .h(px(CONNECTOR_ROW_H)),
            )
            .into_any_element(),
    }
}
