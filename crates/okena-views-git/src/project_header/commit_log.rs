//! Commit log content: list of graph rows (loading / empty fallbacks).

use super::graph::{build_palette, render_lane_row};
use super::lane_layout;

use okena_core::theme::ThemeColors;
use okena_git::CommitLogEntry;
use okena_ui::tokens::ui_text_ms;

use gpui::prelude::*;
use gpui::*;
use std::sync::Arc;

/// Render the "loading..." or "no commits" content, or the list of commit
/// graph rows.
///
/// `on_commit_click` is called with `(commit_hash, commit_message, commit_index)`
/// when the user clicks on a commit row.
///
/// `on_commit_right_click` is called with `(commit_hash, mouse_position)` when
/// the user right-clicks a commit row — used to open a context menu (e.g.
/// "Send to Terminal", "Copy Hash").
pub fn render_commit_log_content(
    entries: &[CommitLogEntry],
    loading: bool,
    on_commit_click: Option<Arc<dyn Fn(&str, &str, usize, &mut Window, &mut App)>>,
    on_commit_right_click: Option<Arc<dyn Fn(&str, gpui::Point<gpui::Pixels>, &mut Window, &mut App)>>,
    t: &ThemeColors,
    cx: &App,
) -> AnyElement {
    if loading && entries.is_empty() {
        return placeholder(cx, t, "Loading\u{2026}");
    }
    if entries.is_empty() {
        return placeholder(cx, t, "No commits");
    }

    let layout = lane_layout::compute(entries);
    let palette = build_palette(&layout.rows);
    let max_col = layout.max_col;

    div()
        .children(layout.rows.iter().enumerate().map(|(i, r)| {
            render_lane_row(
                r,
                i,
                max_col,
                &palette,
                on_commit_click.clone(),
                on_commit_right_click.clone(),
                t,
                cx,
            )
        }))
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

fn placeholder(cx: &App, t: &ThemeColors, label: &str) -> AnyElement {
    div()
        .px(px(14.0))
        .py(px(16.0))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_muted))
                .child(label.to_string()),
        )
        .into_any_element()
}
