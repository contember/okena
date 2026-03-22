//! Git-related rendering for project column headers.
//!
//! Pure render functions extracted from `ProjectColumn` so they can be
//! reused without depending on the full view entity.

use okena_core::theme::ThemeColors;
use okena_git::{
    CiStatus, CommitLogEntry, FileDiffSummary, GitStatus, GraphRow,
    PrState,
};
use okena_files::file_tree::{build_file_tree, flatten_file_tree, render_file_row, render_folder_row, FileTreeItem};

use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use gpui_component::h_flex;
use std::sync::Arc;

// ── Theme-dependent color traits ────────────────────────────────────────────

/// Extension trait: map `PrState` to a theme color.
pub trait PrStateColor {
    fn color(&self, t: &ThemeColors) -> u32;
}

impl PrStateColor for PrState {
    fn color(&self, t: &ThemeColors) -> u32 {
        match self {
            PrState::Open => t.term_green,
            PrState::Draft => t.text_muted,
            PrState::Merged => t.term_magenta,
            PrState::Closed => t.term_red,
        }
    }
}

/// Extension trait: map `CiStatus` to a theme color.
pub trait CiStatusColor {
    fn color(&self, t: &ThemeColors) -> u32;
}

impl CiStatusColor for CiStatus {
    fn color(&self, t: &ThemeColors) -> u32 {
        match self {
            CiStatus::Success => t.term_green,
            CiStatus::Failure => t.term_red,
            CiStatus::Pending => t.term_yellow,
        }
    }
}

// ── Graph rendering constants ───────────────────────────────────────────────

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

// ── Graph rendering ─────────────────────────────────────────────────────────

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
pub fn render_ref_label(ref_name: &str, t: &ThemeColors) -> AnyElement {
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
        .text_size(px(10.0))
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
pub fn render_graph_row(
    row: &GraphRow,
    index: usize,
    max_graph_len: usize,
    all_commits: &[CommitLogEntry],
    on_commit_click: Option<Arc<dyn Fn(&str, &str, usize, &mut Window, &mut App)>>,
    t: &ThemeColors,
) -> AnyElement {
    let graph_width = max_graph_len as f32 * GRAPH_CELL_W;

    match row {
        GraphRow::Commit(entry) => {
            let row_el = h_flex()
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
                                .text_size(px(12.0))
                                .text_color(rgb(t.text_primary))
                                .text_ellipsis()
                                .overflow_hidden()
                                .flex_shrink()
                                .min_w_0()
                                .child(entry.message.clone()),
                        )
                        .children(entry.refs.iter().map(|r| render_ref_label(r, t)))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(t.text_muted))
                                .flex_shrink_0()
                                .child(entry.author.clone()),
                        ),
                );

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

/// Render the "loading..." or "no commits" content, or the list of commit graph rows.
///
/// `on_commit_click` is called with `(commit_hash, commit_message, commit_index)`
/// when the user clicks on a commit row.
pub fn render_commit_log_content(
    entries: &[GraphRow],
    loading: bool,
    on_commit_click: Option<Arc<dyn Fn(&str, &str, usize, &mut Window, &mut App)>>,
    t: &ThemeColors,
) -> AnyElement {
    if loading && entries.is_empty() {
        return div()
            .px(px(14.0))
            .py(px(16.0))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(t.text_muted))
                    .child("Loading\u{2026}"),
            )
            .into_any_element();
    }

    if entries.is_empty() {
        return div()
            .px(px(14.0))
            .py(px(16.0))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(t.text_muted))
                    .child("No commits"),
            )
            .into_any_element();
    }

    let max_graph_len = entries
        .iter()
        .map(|row| match row {
            GraphRow::Commit(e) => e.graph.len(),
            GraphRow::Connector(g) => g.len(),
        })
        .max()
        .unwrap_or(0);

    let all_commits: Vec<CommitLogEntry> = entries
        .iter()
        .filter_map(|r| match r {
            GraphRow::Commit(e) => Some(e.clone()),
            _ => None,
        })
        .collect();

    div()
        .children(
            entries
                .iter()
                .enumerate()
                .map(|(i, row)| render_graph_row(row, i, max_graph_len, &all_commits, on_commit_click.clone(), t)),
        )
        .when(loading, |d| {
            d.child(
                div()
                    .w_full()
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(t.text_muted))
                            .child("Loading\u{2026}"),
                    ),
            )
        })
        .into_any_element()
}

/// Render the commit log popover header row (icon + "GRAPH" label).
pub fn render_commit_log_header(t: &ThemeColors) -> Div {
    h_flex()
        .px(px(10.0))
        .py(px(6.0))
        .gap(px(6.0))
        .items_center()
        .border_b_1()
        .border_color(rgb(t.border))
        .child(
            svg()
                .path("icons/git-commit.svg")
                .size(px(11.0))
                .text_color(rgb(t.text_muted)),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(rgb(t.text_secondary))
                .child("GRAPH"),
        )
}

// ── Diff popover file list ──────────────────────────────────────────────────

/// Build the file tree elements for a diff file summary popover.
///
/// Each file element is a `Div` with an id like `"diff-file-{index}"`.
/// The caller should attach `.on_click(...)` handlers to each file element.
pub fn render_diff_file_list(
    summaries: &[FileDiffSummary],
    t: &ThemeColors,
) -> Vec<AnyElement> {
    let tree = build_file_tree(summaries.iter().enumerate().map(|(i, f)| (i, &f.path)));

    let mut tree_elements: Vec<AnyElement> = Vec::new();
    for item in flatten_file_tree(&tree, 0) {
        match item {
            FileTreeItem::Folder { name, depth } => {
                tree_elements.push(render_folder_row(name, depth, t));
            }
            FileTreeItem::File { index, depth } => {
                if let Some(summary) = summaries.get(index) {
                    let filename = summary
                        .path
                        .rsplit('/')
                        .next()
                        .unwrap_or(&summary.path);
                    let is_deleted = summary.removed > 0 && summary.added == 0;
                    tree_elements.push(
                        render_file_row(
                            depth,
                            filename,
                            summary.added,
                            summary.removed,
                            summary.is_new,
                            is_deleted,
                            false,
                            t,
                        )
                        .id(ElementId::Name(
                            format!("diff-file-{}", index).into(),
                        ))
                        .into_any_element(),
                    );
                }
            }
        }
    }
    tree_elements
}

/// Build the diff file tree elements with click handlers attached.
///
/// `on_file_click` is called with the file path when the user clicks a file row.
pub fn render_diff_file_list_interactive(
    summaries: &[FileDiffSummary],
    on_file_click: impl Fn(&str, &mut Window, &mut App) + 'static,
    t: &ThemeColors,
) -> Vec<AnyElement> {
    let tree = build_file_tree(summaries.iter().enumerate().map(|(i, f)| (i, &f.path)));
    let on_file_click = std::sync::Arc::new(on_file_click);

    let mut tree_elements: Vec<AnyElement> = Vec::new();
    for item in flatten_file_tree(&tree, 0) {
        match item {
            FileTreeItem::Folder { name, depth } => {
                tree_elements.push(render_folder_row(name, depth, t));
            }
            FileTreeItem::File { index, depth } => {
                if let Some(summary) = summaries.get(index) {
                    let filename = summary
                        .path
                        .rsplit('/')
                        .next()
                        .unwrap_or(&summary.path);
                    let is_deleted = summary.removed > 0 && summary.added == 0;
                    let file_path = summary.path.clone();
                    let cb = on_file_click.clone();
                    tree_elements.push(
                        render_file_row(
                            depth,
                            filename,
                            summary.added,
                            summary.removed,
                            summary.is_new,
                            is_deleted,
                            false,
                            t,
                        )
                        .id(ElementId::Name(
                            format!("diff-file-{}", index).into(),
                        ))
                        .on_click(move |_, window, cx| {
                            cb(&file_path, window, cx);
                        })
                        .into_any_element(),
                    );
                }
            }
        }
    }
    tree_elements
}

// ── Git status bar ──────────────────────────────────────────────────────────

/// Render the git branch status pill (branch name + PR badge + CI status).
///
/// `on_pr_click` is called when the user clicks the PR link (if any).
pub fn render_branch_status(
    status: &GitStatus,
    on_pr_click: Option<impl Fn(&mut Window, &mut App) + 'static>,
    t: &ThemeColors,
) -> AnyElement {
    let pr_info = status.pr_info.clone();
    let (icon_path, icon_color) = if let Some(ref pr) = pr_info {
        ("icons/git-pull-request.svg", pr.state.color(t))
    } else {
        ("icons/git-branch.svg", t.text_muted)
    };
    let pr_number = pr_info.as_ref().map(|p| p.number);
    let ci_checks = pr_info.as_ref().and_then(|p| p.ci_checks.clone());
    let has_pr = pr_info.is_some();

    let el = h_flex()
        .id("branch-status")
        .gap(px(3.0))
        .when(has_pr, |d: Stateful<Div>| {
            d.cursor_pointer()
                .rounded(px(3.0))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
        })
        .child(
            svg()
                .path(icon_path)
                .size(px(10.0))
                .text_color(rgb(icon_color)),
        )
        .child(
            div()
                .text_color(rgb(t.text_secondary))
                .max_w(px(100.0))
                .text_ellipsis()
                .overflow_hidden()
                .child(status.branch.clone().unwrap_or_default()),
        )
        .when_some(pr_number, |d, num| {
            d.child(div().text_color(rgb(t.text_muted)).child(format!("#{num}")))
        })
        .when_some(ci_checks, |d, checks| {
            let tooltip = checks.tooltip_text();
            d.child(
                div()
                    .id("ci-status")
                    .child(
                        svg()
                            .path(checks.status.icon())
                            .size(px(8.0))
                            .text_color(rgb(checks.status.color(t))),
                    )
                    .tooltip(move |_window, cx| Tooltip::new(tooltip.clone()).build(_window, cx)),
            )
        });

    if let Some(cb) = on_pr_click {
        el.on_click(move |_, window, cx| {
            cb(window, cx);
        })
        .into_any_element()
    } else {
        el.into_any_element()
    }
}

/// Render the diff stats badge (`+N / -M`).
///
/// Returns a `Div` (not yet stateful). The caller should:
/// - Assign an `id(...)` and attach hover/click handlers
/// - Attach a canvas to capture bounds for popover positioning
pub fn render_diff_stats_badge(lines_added: usize, lines_removed: usize, t: &ThemeColors) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(3.0))
        .px(px(4.0))
        .py(px(1.0))
        .rounded(px(3.0))
        .child(
            div()
                .text_color(rgb(t.term_green))
                .child(format!("+{}", lines_added)),
        )
        .child(div().text_color(rgb(t.text_muted)).child("/"))
        .child(
            div()
                .text_color(rgb(t.term_red))
                .child(format!("-{}", lines_removed)),
        )
}
