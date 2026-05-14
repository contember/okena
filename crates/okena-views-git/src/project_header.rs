//! Git-related rendering for project column headers.
//!
//! Pure render functions extracted from `ProjectColumn` so they can be
//! reused without depending on the full view entity.

use okena_core::theme::ThemeColors;
use okena_git::{
    CiStatus, CommitLogEntry, FileDiffSummary, GitStatus, GraphRow,
    PrState,
};
use okena_files::file_tree::{build_file_tree, expandable_folder_row, expandable_file_row, FileTreeNode};

use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use gpui_component::h_flex;
use okena_ui::tokens::{ui_text_sm, ui_text_ms, ui_text_md};
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
pub fn render_graph_row(
    row: &GraphRow,
    index: usize,
    max_graph_len: usize,
    all_commits: &[CommitLogEntry],
    on_commit_click: Option<Arc<dyn Fn(&str, &str, usize, &mut Window, &mut App)>>,
    t: &ThemeColors,
    cx: &App,
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
    cx: &App,
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
                    .text_size(ui_text_ms(cx))
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
                    .text_size(ui_text_ms(cx))
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
                .map(|(i, row)| render_graph_row(row, i, max_graph_len, &all_commits, on_commit_click.clone(), t, cx)),
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
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child("Loading\u{2026}"),
                    ),
            )
        })
        .into_any_element()
}

/// Render the commit log popover header row (icon + "GRAPH" label).
pub fn render_commit_log_header(t: &ThemeColors, cx: &App) -> Div {
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
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_secondary))
                .child("GRAPH"),
        )
}

// ── Diff popover file list ──────────────────────────────────────────────────

/// Build the diff file tree elements with click handlers attached.
///
/// `on_file_click` is called with the file path when the user clicks a file row.
/// All folders are rendered expanded (no toggle state in popovers).
pub fn render_diff_file_list_interactive(
    summaries: &[FileDiffSummary],
    on_file_click: impl Fn(&str, &mut Window, &mut App) + 'static,
    t: &ThemeColors,
    cx: &App,
) -> Vec<AnyElement> {
    let tree = build_file_tree(summaries.iter().enumerate().map(|(i, f)| (i, &f.path)));
    let on_file_click: Arc<dyn Fn(&str, &mut Window, &mut App)> = Arc::new(on_file_click);
    render_diff_tree_node(&tree, 0, summaries, &on_file_click, t, cx)
}

fn render_diff_tree_node(
    node: &FileTreeNode,
    depth: usize,
    summaries: &[FileDiffSummary],
    on_file_click: &Arc<dyn Fn(&str, &mut Window, &mut App)>,
    t: &ThemeColors,
    cx: &App,
) -> Vec<AnyElement> {
    let mut elements: Vec<AnyElement> = Vec::new();

    for (name, child) in &node.children {
        elements.push(
            expandable_folder_row(name, depth, true, t, cx)
                .into_any_element(),
        );
        elements.extend(render_diff_tree_node(child, depth + 1, summaries, on_file_click, t, cx));
    }

    for &file_index in &node.files {
        if let Some(summary) = summaries.get(file_index) {
            let filename = summary.path.rsplit('/').next().unwrap_or(&summary.path);
            let is_deleted = summary.removed > 0 && summary.added == 0;

            let name_color = if summary.is_new {
                Some(t.diff_added_fg)
            } else if is_deleted {
                Some(t.diff_removed_fg)
            } else {
                None
            };

            let file_path = summary.path.clone();
            let cb = on_file_click.clone();
            elements.push(
                expandable_file_row(filename, depth, name_color, false, t, cx)
                    .id(ElementId::Name(format!("diff-file-{}", file_index).into()))
                    .on_click(move |_, window, cx| {
                        cb(&file_path, window, cx);
                    })
                    // Line counts
                    .when(summary.added > 0 || summary.removed > 0, |d| {
                        d.child(
                            h_flex()
                                .gap(px(4.0))
                                .text_size(ui_text_ms(cx))
                                .flex_shrink_0()
                                .when(summary.added > 0, |d| {
                                    d.child(
                                        div()
                                            .text_color(rgb(t.diff_added_fg))
                                            .child(format!("+{}", summary.added)),
                                    )
                                })
                                .when(summary.removed > 0, |d| {
                                    d.child(
                                        div()
                                            .text_color(rgb(t.diff_removed_fg))
                                            .child(format!("-{}", summary.removed)),
                                    )
                                }),
                        )
                    })
                    .into_any_element(),
            );
        }
    }

    elements
}

// ── Git status bar ──────────────────────────────────────────────────────────

/// Callbacks for the interactive parts of the branch status pill.
///
/// The branch chip (icon + name + chevron) and the PR badge (#num + CI) are
/// rendered as two adjacent clickable regions so they can dispatch different
/// actions: branch chip opens the branch switcher; PR badge opens the PR URL.
pub struct BranchStatusCallbacks {
    /// Called when the user clicks the branch chip. When `None` the chip is
    /// rendered as plain (non-clickable) text — used for read-only providers.
    pub on_branch_click: Option<Arc<dyn Fn(&mut Window, &mut App)>>,
    /// Called when the user clicks the PR badge. When `None` no PR badge is
    /// rendered (in addition to being hidden when the status has no PR info).
    pub on_pr_click: Option<Arc<dyn Fn(&mut Window, &mut App)>>,
    /// Called every layout pass with the on-screen bounds of the branch chip,
    /// so the caller can anchor a popover underneath it.
    pub on_branch_bounds: Option<Arc<dyn Fn(Bounds<Pixels>, &mut App)>>,
}

/// Render the git branch status pill (branch chip + PR badge + CI status).
///
/// The branch chip and PR badge are rendered as two separate clickable
/// regions so the caller can wire different actions to each — typically a
/// branch switcher popover and the PR URL respectively.
pub fn render_branch_status(
    status: &GitStatus,
    callbacks: BranchStatusCallbacks,
    t: &ThemeColors,
) -> AnyElement {
    let pr_info = status.pr_info.clone();
    let pr_number = pr_info.as_ref().map(|p| p.number);
    let pr_state_color = pr_info.as_ref().map(|p| p.state.color(t));
    let ci_checks = pr_info.as_ref().and_then(|p| p.ci_checks.clone());

    let BranchStatusCallbacks {
        on_branch_click,
        on_pr_click,
        on_branch_bounds,
    } = callbacks;

    let branch_clickable = on_branch_click.is_some();

    // Branch chip — icon + name + chevron (when interactive)
    let mut branch_chip = h_flex()
        .id("branch-chip")
        .relative()
        .gap(px(3.0))
        .px(px(2.0))
        .rounded(px(3.0))
        .when(branch_clickable, |d: Stateful<Div>| {
            d.cursor_pointer().hover(|s| s.bg(rgb(t.bg_hover)))
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .child(
            svg()
                .path("icons/git-branch.svg")
                .size(px(10.0))
                .text_color(rgb(t.text_muted)),
        )
        .child(
            div()
                .text_color(rgb(t.text_secondary))
                .max_w(px(100.0))
                .text_ellipsis()
                .overflow_hidden()
                .child(status.branch.clone().unwrap_or_default()),
        )
        .when(branch_clickable, |d| {
            d.child(
                svg()
                    .path("icons/chevron-down.svg")
                    .size(px(8.0))
                    .text_color(rgb(t.text_muted)),
            )
        });

    if let Some(bcb) = on_branch_bounds {
        branch_chip = branch_chip.child(
            canvas(
                move |bounds, _window, app| {
                    bcb(bounds, app);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        );
    }

    if let Some(cb) = on_branch_click {
        branch_chip = branch_chip.on_click(move |_, window, cx| {
            cb(window, cx);
        });
    }

    // PR badge — only when a PR exists; clickable when an on_pr_click is
    // provided (PR URL open).
    let render_pr_badge = pr_number.is_some();
    let pr_badge = if render_pr_badge {
        let pr_clickable = on_pr_click.is_some();
        let badge_color = pr_state_color.unwrap_or(t.text_muted);
        let mut el = h_flex()
            .id("pr-badge")
            .gap(px(3.0))
            .px(px(3.0))
            .rounded(px(3.0))
            .items_center()
            .when(pr_clickable, |d: Stateful<Div>| {
                d.cursor_pointer().hover(|s| s.bg(rgb(t.bg_hover)))
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .child(
                svg()
                    .path("icons/git-pull-request.svg")
                    .size(px(10.0))
                    .text_color(rgb(badge_color)),
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
            el = el.on_click(move |_, window, cx| {
                cb(window, cx);
            });
        }
        Some(el)
    } else {
        None
    };

    h_flex()
        .gap(px(2.0))
        .child(branch_chip)
        .when_some(pr_badge, |d, badge| d.child(badge))
        .into_any_element()
}

/// Render an ahead/behind badge (`↑N ↓M`). Returns `None` when both counts
/// are zero or unavailable — caller can `.when_some(...)` on the result.
pub fn render_ahead_behind_badge(
    counts: (Option<usize>, Option<usize>),
    t: &ThemeColors,
) -> Option<Div> {
    let ahead = counts.0.unwrap_or(0);
    let behind = counts.1.unwrap_or(0);
    if ahead == 0 && behind == 0 {
        return None;
    }
    Some(
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .px(px(4.0))
            .py(px(1.0))
            .rounded(px(3.0))
            .when(ahead > 0, |d| {
                d.child(
                    div()
                        .text_color(rgb(t.term_green))
                        .child(format!("\u{2191}{}", ahead)),
                )
            })
            .when(behind > 0, |d| {
                d.child(
                    div()
                        .text_color(rgb(t.term_yellow))
                        .child(format!("\u{2193}{}", behind)),
                )
            }),
    )
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
