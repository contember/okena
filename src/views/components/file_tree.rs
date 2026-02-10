//! Shared file tree data structure and builder.
//!
//! Used by both the diff viewer and file viewer for sidebar navigation.

use std::collections::BTreeMap;

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

#[cfg(test)]
mod tests {
    use super::*;

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
