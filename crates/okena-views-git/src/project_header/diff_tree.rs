//! Click-through file list rendered inside the diff summary popover.

use okena_core::theme::ThemeColors;
use okena_files::file_tree::{build_file_tree, expandable_file_row, expandable_folder_row, FileTreeNode};
use okena_git::FileDiffSummary;
use okena_ui::tokens::ui_text_ms;

use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use std::sync::Arc;

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
