//! Status groups inside an expanded project's Terminals list.
//!
//! When at least one of a project's terminals carries a status that resolves
//! to a board column, its terminal rows are split under small non-cursor
//! headers (one per non-empty column, then "No status"). A project without
//! statuses renders the flat list exactly as before. The grouping itself is
//! `okena_workspace::terminal_board`; the keyboard cursor walks the same
//! grouped order (`cursor.rs`), and the headers are folded into the cursor's
//! scroll-index map with [`fold_extra_rows`].

use gpui::{App, Context, Div, FontWeight, ParentElement, Styled, div, px, rgb};
use okena_ui::theme::theme;
use okena_ui::tokens::ui_text_xs;
use okena_workspace::terminal_board::{StatusGroup, group_terminals_by_status, has_status_groups};
use std::collections::HashMap;

use super::Sidebar;

/// The status groups to draw for a project's terminals, or `None` when the
/// project has no resolvable status and renders flat.
pub(crate) fn terminal_status_groups(
    terminal_ids: &[String],
    statuses: &HashMap<String, String>,
    cx: &App,
) -> Option<Vec<StatusGroup>> {
    let columns = okena_ui::board::board_columns(cx);
    has_status_groups(terminal_ids, statuses, &columns)
        .then(|| group_terminals_by_status(terminal_ids, statuses, &columns))
}

/// Terminal ids in the order the sidebar draws them: grouped when the project
/// has statuses, as given otherwise.
pub(crate) fn ordered_terminal_ids(
    terminal_ids: &[String],
    statuses: &HashMap<String, String>,
    cx: &App,
) -> Vec<String> {
    match terminal_status_groups(terminal_ids, statuses, cx) {
        Some(groups) => groups.into_iter().flat_map(|g| g.terminal_ids).collect(),
        None => terminal_ids.to_vec(),
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

impl Sidebar {
    /// Small muted caption between terminal rows: `TO DO`, `REVIEW`, …
    pub(crate) fn render_status_group_header(
        &self,
        label: &str,
        color: okena_core::theme::FolderColor,
        left_padding: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = theme(cx);
        let text_color = match color {
            okena_core::theme::FolderColor::Default => t.text_muted,
            color => t.get_folder_color(color),
        };
        div()
            .h(px(18.0))
            .pl(px(left_padding))
            .pr(px(8.0))
            .flex()
            .items_center()
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
