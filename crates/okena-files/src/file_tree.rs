//! Shared file tree data structure, builder, and rendering helpers.
//!
//! Used by both the diff viewer and file viewer for sidebar navigation,
//! and by the git status popover.

use std::collections::BTreeMap;

use okena_core::theme::ThemeColors;
use okena_ui::file_icon::file_icon;
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;

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

/// A flattened item from a file tree, ready for rendering.
pub enum FileTreeItem<'a> {
    Folder { name: &'a str, depth: usize },
    File { index: usize, depth: usize },
}

/// Flatten a file tree into an ordered list of items for rendering.
pub fn flatten_file_tree(node: &FileTreeNode, depth: usize) -> Vec<FileTreeItem<'_>> {
    let mut items = Vec::new();
    for (name, child) in &node.children {
        let has_content = !child.files.is_empty() || !child.children.is_empty();
        if has_content {
            items.push(FileTreeItem::Folder { name, depth });
            items.extend(flatten_file_tree(child, depth + 1));
        }
    }
    for &file_index in &node.files {
        items.push(FileTreeItem::File { index: file_index, depth });
    }
    items
}

/// Render a folder row in a file tree.
pub fn render_folder_row(name: &str, depth: usize, t: &ThemeColors) -> AnyElement {
    let indent = depth * 14;
    h_flex()
        .h(px(26.0))
        .pl(px(indent as f32 + 12.0))
        .child(
            div()
                .text_size(px(11.0))
                .text_color(rgb(t.text_secondary))
                .child(format!("{}/", name)),
        )
        .into_any_element()
}

/// Render a file row in a file tree (without id or click handler).
///
/// The caller should chain `.id(...)` and `.on_click(...)` on the returned `Div`.
pub fn render_file_row(
    depth: usize,
    filename: &str,
    added: usize,
    removed: usize,
    is_new: bool,
    is_deleted: bool,
    selected: bool,
    t: &ThemeColors,
) -> Div {
    let indent = depth * 14;

    let (status_char, status_color) = if is_new {
        ("A", t.diff_added_fg)
    } else if is_deleted {
        ("D", t.diff_removed_fg)
    } else {
        ("M", t.text_muted)
    };

    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .h(px(26.0))
        .pl(px(indent as f32 + 12.0))
        .pr(px(12.0))
        .mx(px(4.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .when(selected, |d| d.bg(rgb(t.bg_selection)))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        // Status badge
        .child(
            div()
                .text_size(px(10.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(status_color))
                .child(status_char),
        )
        // File type icon
        .child(file_icon(filename, t))
        // Filename
        .child(
            div()
                .flex_1()
                .text_size(px(12.0))
                .text_color(rgb(t.text_primary))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(filename.to_string()),
        )
        // Line counts
        .when(added > 0 || removed > 0, |d| {
            d.child(
                h_flex()
                    .gap(px(4.0))
                    .text_size(px(11.0))
                    .when(added > 0, |d| {
                        d.child(
                            div()
                                .text_color(rgb(t.diff_added_fg))
                                .child(format!("+{}", added)),
                        )
                    })
                    .when(removed > 0, |d| {
                        d.child(
                            div()
                                .text_color(rgb(t.diff_removed_fg))
                                .child(format!("-{}", removed)),
                        )
                    }),
            )
        })
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
