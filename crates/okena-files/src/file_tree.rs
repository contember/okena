//! Shared file tree data structure, builder, and rendering helpers.
//!
//! Used by both the diff viewer and file viewer for sidebar navigation,
//! and by the git status popover.

use std::collections::BTreeMap;

use okena_core::theme::ThemeColors;
use okena_ui::file_icon::file_icon;
use okena_ui::tokens::ui_text;
use gpui::prelude::*;
use gpui::*;

/// A node in the file tree.
#[derive(Default, Clone)]
pub struct FileTreeNode {
    /// Files at this level (index into files vec).
    pub files: Vec<usize>,
    /// Subdirectories.
    pub children: BTreeMap<String, FileTreeNode>,
}

/// Build a file tree from an iterator of (index, relative_path) pairs.
pub fn build_file_tree(paths: impl Iterator<Item = (usize, impl AsRef<str>)>) -> FileTreeNode {
    let mut root = FileTreeNode::default();
    for (index, path) in paths {
        let parts: Vec<&str> = path.as_ref().split('/').collect();
        let mut node = &mut root;
        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                node.files.push(index);
            } else {
                node = node.children.entry(part.to_string()).or_default();
            }
        }
    }
    root
}

/// Base div for an expandable folder row: chevron + folder icon + name.
///
/// Caller chains `.id(...)`, `.on_click(...)`, `.when(...)` for selection,
/// and `.child(...)` for extras (e.g. scope button).
pub fn expandable_folder_row(
    name: &str,
    depth: usize,
    is_expanded: bool,
    t: &ThemeColors,
    cx: &App,
) -> Div {
    let indent = depth as f32 * 14.0;
    div()
        .flex()
        .items_center()
        .h(px(26.0))
        .pl(px(indent + 8.0))
        .pr(px(12.0))
        .mx(px(4.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(t.bg_hover)))
        // Chevron
        .child(
            svg()
                .path(if is_expanded { "icons/chevron-down.svg" } else { "icons/chevron-right.svg" })
                .size(px(14.0))
                .text_color(rgb(t.text_muted))
                .mr(px(4.0))
                .flex_shrink_0(),
        )
        // Folder icon
        .child(
            svg()
                .path("icons/folder.svg")
                .size(px(14.0))
                .text_color(rgb(t.text_secondary))
                .mr(px(4.0))
                .flex_shrink_0(),
        )
        // Folder name
        .child(
            div()
                .flex_1()
                .text_size(ui_text(13.0, cx))
                .text_color(rgb(t.text_primary))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(format!("{name}/")),
        )
}

/// Base div for an expandable file row: file icon + filename.
///
/// Caller chains `.id(...)`, `.on_click(...)`, `.when(...)` for selection,
/// and `.child(...)` for extras (e.g. match count badge, diff stats).
///
/// Use `name_color` to override the filename color (e.g. for diff status).
/// Pass `None` to use the default `text_primary`.
pub fn expandable_file_row(
    filename: &str,
    depth: usize,
    name_color: Option<u32>,
    t: &ThemeColors,
    cx: &App,
) -> Div {
    let indent = depth as f32 * 14.0;
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .h(px(26.0))
        .pl(px(indent + 8.0 + 18.0)) // extra 18px to align past chevron
        .pr(px(12.0))
        .mx(px(4.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(t.bg_hover)))
        // File icon
        .child(
            file_icon(filename, t, cx)
                .mr(px(4.0)),
        )
        // Filename
        .child(
            div()
                .flex_1()
                .text_size(ui_text(13.0, cx))
                .text_color(rgb(name_color.unwrap_or(t.text_primary)))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(filename.to_string()),
        )
}

#[cfg(test)]
mod tests {
    use super::build_file_tree;

    #[test]
    fn test_build_file_tree_empty() {
        let tree = build_file_tree(std::iter::empty::<(usize, &str)>());
        assert!(tree.files.is_empty());
        assert!(tree.children.is_empty());
    }

    #[test]
    fn test_build_file_tree_flat_files() {
        let files = vec!["a.rs", "b.rs", "c.rs"];
        let tree = build_file_tree(files.iter().enumerate().map(|(i, f)| (i, *f)));
        assert_eq!(tree.files, vec![0, 1, 2]);
        assert!(tree.children.is_empty());
    }

    #[test]
    fn test_build_file_tree_nested_dirs() {
        let files = vec!["src/main.rs", "src/lib.rs", "README.md"];
        let tree = build_file_tree(files.iter().enumerate().map(|(i, f)| (i, *f)));
        assert_eq!(tree.files, vec![2]); // README.md at root
        assert!(tree.children.contains_key("src"));
        let src = &tree.children["src"];
        assert_eq!(src.files, vec![0, 1]);
    }

    #[test]
    fn test_build_file_tree_multi_level() {
        let files = vec![
            "src/views/mod.rs",
            "src/views/render.rs",
            "src/main.rs",
            "Cargo.toml",
        ];
        let tree = build_file_tree(files.iter().enumerate().map(|(i, f)| (i, *f)));
        assert_eq!(tree.files, vec![3]); // Cargo.toml
        let src = &tree.children["src"];
        assert_eq!(src.files, vec![2]); // main.rs
        let views = &src.children["views"];
        assert_eq!(views.files, vec![0, 1]); // mod.rs, render.rs
    }
}
