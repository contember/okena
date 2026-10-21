//! Status groups among a parent project's worktrees in the sidebar.
//!
//! When at least one worktree of a parent carries a status that resolves to
//! a board column, the worktree rows are split under small non-cursor
//! captions (one per non-empty column, then "No status"). A parent without
//! statuses renders its worktrees exactly as before. The grouping itself is
//! `okena_workspace::status_board`; the keyboard cursor walks the same
//! grouped order (`cursor.rs`), and the captions are folded into the cursor's
//! scroll-index map with [`fold_extra_rows`].

use gpui::{
    AnyElement, App, Context, Div, FontWeight, IntoElement, ParentElement, Styled, div, px, rgb,
};
use okena_core::theme::FolderColor;
use okena_ui::theme::theme;
use okena_ui::tokens::ui_text_xs;
use okena_workspace::status_board::{GroupedRow, caption_label, grouped_rows};

use super::Sidebar;

/// One worktree row to draw: its index into the children list and, on the
/// first row of a group, the caption (label, colour) to draw above it.
pub(crate) struct WorktreeRow {
    pub index: usize,
    pub caption: Option<(String, FolderColor)>,
}

/// The order to draw `statuses.len()` worktree rows in: grouped by status
/// when any resolves to a column, as given otherwise.
pub(crate) fn worktree_rows(statuses: &[Option<&str>], cx: &App) -> Vec<WorktreeRow> {
    let grouped = if okena_ui::board::status_board_enabled(cx) {
        grouped_rows(statuses, &okena_ui::board::board_columns(cx))
    } else {
        None
    };
    match grouped {
        Some(rows) => rows
            .into_iter()
            .map(|GroupedRow { index, caption }| WorktreeRow {
                index,
                caption: caption.map(|column| {
                    (
                        caption_label(&column).to_string(),
                        column.map(|c| c.color).unwrap_or_default(),
                    )
                }),
            })
            .collect(),
        None => (0..statuses.len())
            .map(|index| WorktreeRow {
                index,
                caption: None,
            })
            .collect(),
    }
}

/// Shift each cursor item's scroll-child index by the number of non-cursor
/// rows (`extra_rows`, each recorded as the `flat_idx` it was inserted at)
/// that precede it. A row inserted at position `h` sits before cursor item
/// `h` and every later one.
pub(crate) fn fold_extra_rows(scroll_indices: &mut [usize], extra_rows: &[usize]) {
    if extra_rows.is_empty() {
        return;
    }
    for (i, target) in scroll_indices.iter_mut().enumerate() {
        *target += extra_rows.iter().filter(|&&h| h <= i).count();
    }
}

/// Draw a thin tree guide in the parent's colour down the left of every
/// row of an expanded family (main checkout, captions, worktrees and their
/// expanded children). Each row is wrapped so consecutive rows join into one
/// continuous line at `x`; the row itself is untouched.
pub(crate) fn add_tree_guide(rows: &mut [AnyElement], x: f32, color: u32) {
    let mut line = rgb(color);
    line.a = 0.55;
    for row in rows.iter_mut() {
        let inner = std::mem::replace(row, div().into_any_element());
        *row = div()
            .relative()
            .w_full()
            .child(inner)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(x))
                    .w(px(1.5))
                    .bg(line),
            )
            .into_any_element();
    }
}

impl Sidebar {
    /// Small caption between worktree rows: `TO DO`, `REVIEW`, … in the
    /// column's colour (muted when it has none).
    pub(crate) fn render_status_group_header(
        &self,
        label: &str,
        color: FolderColor,
        left_padding: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = theme(cx);
        let text_color = match color {
            FolderColor::Default => t.text_muted,
            color => t.get_folder_color(color),
        };
        div()
            .h(px(18.0))
            .pl(px(left_padding))
            .pr(px(8.0))
            .flex()
            .items_center()
            .gap(px(5.0))
            .child(
                div()
                    .size(px(6.0))
                    .rounded_full()
                    .bg(rgb(text_color))
                    .flex_shrink_0(),
            )
            .child(
                div()
                    .text_size(ui_text_xs(cx))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(text_color))
                    .child(label.to_uppercase()),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::fold_extra_rows;

    #[test]
    fn fold_extra_rows_counts_headers_at_or_before_each_item() {
        // Cursor items 0..5 map 1:1 to scroll children starting at 1; headers
        // were inserted before items 1 and 3.
        let mut indices: Vec<usize> = (1..=5).collect();
        fold_extra_rows(&mut indices, &[1, 3]);
        assert_eq!(indices, vec![1, 3, 4, 6, 7]);
    }

    #[test]
    fn fold_extra_rows_is_identity_without_headers() {
        let mut indices = vec![2, 3, 4];
        fold_extra_rows(&mut indices, &[]);
        assert_eq!(indices, vec![2, 3, 4]);
    }
}
