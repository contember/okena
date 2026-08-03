//! Shared file tree data structure, builder, and rendering helpers.
//!
//! Used by both the diff viewer and file viewer for sidebar navigation,
//! and by the git status popover.

use std::collections::{BTreeMap, HashSet};

use gpui::prelude::*;
use gpui::*;
use okena_core::theme::ThemeColors;
use okena_ui::file_icon::file_icon;
use okena_ui::tokens::ui_text;

/// A node in the file tree.
#[derive(Default, Clone)]
pub struct FileTreeNode {
    /// Files at this level (index into files vec).
    pub files: Vec<usize>,
    /// Subdirectories.
    pub children: BTreeMap<String, FileTreeNode>,
}

/// A flattened row in the order shown by a file tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileTreeRow<T> {
    Folder {
        path: String,
        name: String,
        depth: usize,
        is_expanded: bool,
    },
    File {
        item: T,
        depth: usize,
    },
    Loading {
        depth: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileTreeNavigationDirection {
    Previous,
    Next,
}

/// Build a file tree from an iterator of (index, relative_path) pairs.
pub fn build_file_tree(paths: impl Iterator<Item = (usize, impl AsRef<str>)>) -> FileTreeNode {
    let mut root = FileTreeNode::default();
    for (index, path) in paths {
        let parts: Vec<&str> = path.as_ref().split(['/', '\\']).collect();
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

/// Flatten an indexed tree in the same order used by the tree renderer.
pub fn indexed_file_tree_rows(
    node: &FileTreeNode,
    expanded_folders: &HashSet<String>,
    include_collapsed: bool,
) -> Vec<FileTreeRow<usize>> {
    fn collect(
        node: &FileTreeNode,
        parent_path: &str,
        depth: usize,
        expanded_folders: &HashSet<String>,
        include_collapsed: bool,
        rows: &mut Vec<FileTreeRow<usize>>,
    ) {
        for (name, child) in &node.children {
            let path = if parent_path.is_empty() {
                name.clone()
            } else {
                format!("{parent_path}/{name}")
            };
            let is_expanded = expanded_folders.contains(&path);
            rows.push(FileTreeRow::Folder {
                path: path.clone(),
                name: name.clone(),
                depth,
                is_expanded,
            });
            if include_collapsed || is_expanded {
                collect(
                    child,
                    &path,
                    depth + 1,
                    expanded_folders,
                    include_collapsed,
                    rows,
                );
            }
        }

        rows.extend(
            node.files
                .iter()
                .copied()
                .map(|item| FileTreeRow::File { item, depth }),
        );
    }

    let mut rows = Vec::new();
    collect(node, "", 0, expanded_folders, include_collapsed, &mut rows);
    rows
}

/// Find the adjacent visible file, falling back from a selection hidden by collapse.
pub fn adjacent_file_tree_item<T: Clone + Eq>(
    visible_rows: &[FileTreeRow<T>],
    all_rows: &[FileTreeRow<T>],
    selected: Option<&T>,
    direction: FileTreeNavigationDirection,
) -> Option<T> {
    let visible: Vec<&T> = visible_rows
        .iter()
        .filter_map(|row| match row {
            FileTreeRow::File { item, .. } => Some(item),
            FileTreeRow::Folder { .. } | FileTreeRow::Loading { .. } => None,
        })
        .collect();

    let Some(selected) = selected else {
        return match direction {
            FileTreeNavigationDirection::Previous => visible.last().map(|item| (**item).clone()),
            FileTreeNavigationDirection::Next => visible.first().map(|item| (**item).clone()),
        };
    };

    if let Some(position) = visible.iter().position(|item| *item == selected) {
        return match direction {
            FileTreeNavigationDirection::Previous => position
                .checked_sub(1)
                .map(|position| (*visible[position]).clone()),
            FileTreeNavigationDirection::Next => {
                visible.get(position + 1).map(|item| (**item).clone())
            }
        };
    }

    let all: Vec<&T> = all_rows
        .iter()
        .filter_map(|row| match row {
            FileTreeRow::File { item, .. } => Some(item),
            FileTreeRow::Folder { .. } | FileTreeRow::Loading { .. } => None,
        })
        .collect();
    let position = all.iter().position(|item| *item == selected)?;

    match direction {
        FileTreeNavigationDirection::Previous => all[..position]
            .iter()
            .rev()
            .find(|item| visible.iter().any(|visible_item| *visible_item == **item))
            .map(|item| (**item).clone()),
        FileTreeNavigationDirection::Next => all[position + 1..]
            .iter()
            .find(|item| visible.iter().any(|visible_item| *visible_item == **item))
            .map(|item| (**item).clone()),
    }
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
        .cursor_pointer()
        .hover(|s| s.bg(rgb(t.bg_hover)))
        // Chevron
        .child(
            svg()
                .path(if is_expanded {
                    "icons/chevron-down.svg"
                } else {
                    "icons/chevron-right.svg"
                })
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
                .child(name.to_string()),
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
    is_open: bool,
    t: &ThemeColors,
    cx: &App,
) -> Div {
    let indent = depth as f32 * 14.0;
    div()
        .relative()
        .flex()
        .items_center()
        .gap(px(6.0))
        .h(px(26.0))
        .pl(px(indent + 8.0 + 18.0)) // extra 18px to align past chevron
        .pr(px(12.0))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(t.bg_hover)))
        // Left-edge accent bar for open files
        .when(is_open, |d| {
            d.child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(5.0))
                    .bottom(px(5.0))
                    .w(px(2.0))
                    .bg(rgb(t.border_active))
                    .rounded_r(px(1.0)),
            )
        })
        // File icon
        .child(file_icon(filename, t, cx).mr(px(4.0)).flex_shrink_0())
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
    use super::{
        FileTreeNavigationDirection, FileTreeRow, adjacent_file_tree_item, build_file_tree,
        indexed_file_tree_rows,
    };
    use std::collections::HashSet;

    #[test]
    fn test_build_file_tree_empty() {
        let tree = build_file_tree(std::iter::empty::<(usize, &str)>());
        assert!(tree.files.is_empty());
        assert!(tree.children.is_empty());
    }

    #[test]
    fn test_build_file_tree_flat_files() {
        let files = ["a.rs", "b.rs", "c.rs"];
        let tree = build_file_tree(files.iter().enumerate().map(|(i, f)| (i, *f)));
        assert_eq!(tree.files, vec![0, 1, 2]);
        assert!(tree.children.is_empty());
    }

    #[test]
    fn test_build_file_tree_nested_dirs() {
        let files = ["src/main.rs", "src/lib.rs", "README.md"];
        let tree = build_file_tree(files.iter().enumerate().map(|(i, f)| (i, *f)));
        assert_eq!(tree.files, vec![2]); // README.md at root
        assert!(tree.children.contains_key("src"));
        let src = &tree.children["src"];
        assert_eq!(src.files, vec![0, 1]);
    }

    #[test]
    fn test_build_file_tree_multi_level() {
        let files = [
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

    fn navigation_tree() -> super::FileTreeNode {
        let paths = ["README.md", "src/lib.rs", "src/views/mod.rs", "src/main.rs"];
        build_file_tree(paths.iter().enumerate())
    }

    fn expanded() -> HashSet<String> {
        HashSet::from(["src".to_string(), "src/views".to_string()])
    }

    fn file_items(rows: &[FileTreeRow<usize>]) -> Vec<usize> {
        rows.iter()
            .filter_map(|row| match row {
                FileTreeRow::File { item, .. } => Some(*item),
                FileTreeRow::Folder { .. } | FileTreeRow::Loading { .. } => None,
            })
            .collect()
    }

    #[test]
    fn indexed_rows_match_rendered_tree_order() {
        let rows = indexed_file_tree_rows(&navigation_tree(), &expanded(), false);

        assert_eq!(file_items(&rows), vec![2, 1, 3, 0]);
    }

    #[test]
    fn adjacent_item_follows_visible_tree_order() {
        let tree = navigation_tree();
        let visible = indexed_file_tree_rows(&tree, &expanded(), false);
        let all = indexed_file_tree_rows(&tree, &expanded(), true);

        assert_eq!(
            adjacent_file_tree_item(&visible, &all, Some(&2), FileTreeNavigationDirection::Next),
            Some(1)
        );
        assert_eq!(
            adjacent_file_tree_item(
                &visible,
                &all,
                Some(&0),
                FileTreeNavigationDirection::Previous
            ),
            Some(3)
        );
        assert_eq!(
            adjacent_file_tree_item(&visible, &all, Some(&0), FileTreeNavigationDirection::Next),
            None
        );
    }

    #[test]
    fn adjacent_item_skips_collapsed_folder_contents() {
        let tree = navigation_tree();
        let expanded = HashSet::from(["src".to_string()]);
        let visible = indexed_file_tree_rows(&tree, &expanded, false);
        let all = indexed_file_tree_rows(&tree, &expanded, true);

        assert_eq!(file_items(&visible), vec![1, 3, 0]);
        assert_eq!(
            adjacent_file_tree_item(&visible, &all, Some(&2), FileTreeNavigationDirection::Next),
            Some(1)
        );
    }

    #[test]
    fn adjacent_item_starts_at_visible_edge_without_selection() {
        let tree = navigation_tree();
        let visible = indexed_file_tree_rows(&tree, &expanded(), false);

        assert_eq!(
            adjacent_file_tree_item(&visible, &visible, None, FileTreeNavigationDirection::Next),
            Some(2)
        );
        assert_eq!(
            adjacent_file_tree_item(
                &visible,
                &visible,
                None,
                FileTreeNavigationDirection::Previous
            ),
            Some(0)
        );
    }
}
