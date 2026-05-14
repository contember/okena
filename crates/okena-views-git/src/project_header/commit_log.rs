//! Commit log content: list of graph rows (loading / empty fallbacks).

use super::graph::render_graph_row;

use okena_core::theme::ThemeColors;
use okena_git::{CommitLogEntry, GraphRow};
use okena_ui::tokens::ui_text_ms;

use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use std::sync::Arc;

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
