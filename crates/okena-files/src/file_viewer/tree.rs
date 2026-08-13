//! Shared-row adaptation and keyboard navigation for the lazy file tree.

use super::FileViewer;
use crate::file_tree::{FileTreeNavigationDirection, FileTreeRow, adjacent_file_tree_item};
use crate::list_directory::DirEntry;
use gpui::Context;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

fn collect_file_tree_rows(
    parent_path: &str,
    depth: usize,
    loaded_dirs: &HashMap<String, Vec<DirEntry>>,
    loading_dirs: &HashSet<String>,
    expanded_folders: &HashSet<String>,
    include_collapsed: bool,
    rows: &mut Vec<FileTreeRow<String>>,
) {
    let Some(entries) = loaded_dirs.get(parent_path) else {
        if loading_dirs.contains(parent_path) {
            rows.push(FileTreeRow::Loading { depth });
        }
        return;
    };

    for entry in entries {
        let path = if parent_path.is_empty() {
            entry.name.clone()
        } else {
            format!("{parent_path}/{}", entry.name)
        };

        if entry.is_dir {
            let is_expanded = expanded_folders.contains(&path);
            rows.push(FileTreeRow::Folder {
                path: path.clone(),
                name: entry.name.clone(),
                depth,
                is_expanded,
            });
            if include_collapsed || is_expanded {
                collect_file_tree_rows(
                    &path,
                    depth + 1,
                    loaded_dirs,
                    loading_dirs,
                    expanded_folders,
                    include_collapsed,
                    rows,
                );
            }
        } else {
            rows.push(FileTreeRow::File { item: path, depth });
        }
    }
}

impl FileViewer {
    fn collect_file_tree_rows(&self, include_collapsed: bool) -> Vec<FileTreeRow<String>> {
        let mut rows = Vec::new();
        collect_file_tree_rows(
            "",
            0,
            &self.loaded_dirs,
            &self.loading_dirs,
            &self.expanded_folders,
            include_collapsed,
            &mut rows,
        );
        rows
    }

    pub(super) fn invalidate_visible_tree_rows(&mut self) {
        *self.visible_tree_rows.get_mut() = None;
        self.tree_scroll_handle.0.borrow_mut().last_item_size = None;
    }

    pub(super) fn visible_file_tree_rows(&self) -> Arc<Vec<FileTreeRow<String>>> {
        if let Some(rows) = self.visible_tree_rows.borrow().as_ref() {
            return rows.clone();
        }

        let rows = Arc::new(self.collect_file_tree_rows(false));
        *self.visible_tree_rows.borrow_mut() = Some(rows.clone());
        rows
    }

    fn navigate_file_tree(
        &mut self,
        direction: FileTreeNavigationDirection,
        cx: &mut Context<Self>,
    ) {
        let visible = self.visible_file_tree_rows();
        let all = self.collect_file_tree_rows(true);
        let active_path = &self.active_tab().relative_path;
        let selected = (!active_path.is_empty()).then_some(active_path);

        if let Some(path) = adjacent_file_tree_item(&visible, &all, selected, direction) {
            if let Some(index) = visible
                .iter()
                .position(|row| matches!(row, FileTreeRow::File { item, .. } if item == &path))
            {
                self.tree_scroll_handle
                    .scroll_to_item(index, gpui::ScrollStrategy::Nearest);
            }
            self.navigate_to_file_no_history(path, cx);
        }
    }

    pub(super) fn previous_file_tree_item(&mut self, cx: &mut Context<Self>) {
        self.navigate_file_tree(FileTreeNavigationDirection::Previous, cx);
    }

    pub(super) fn next_file_tree_item(&mut self, cx: &mut Context<Self>) {
        self.navigate_file_tree(FileTreeNavigationDirection::Next, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::collect_file_tree_rows;
    use crate::file_tree::FileTreeRow;
    use crate::list_directory::DirEntry;
    use std::collections::{HashMap, HashSet};

    fn entry(name: &str, is_dir: bool) -> DirEntry {
        DirEntry {
            name: name.to_string(),
            is_dir,
        }
    }

    fn loaded_dirs() -> HashMap<String, Vec<DirEntry>> {
        HashMap::from([
            (
                String::new(),
                vec![entry("src", true), entry("README.md", false)],
            ),
            (
                "src".to_string(),
                vec![entry("views", true), entry("lib.rs", false)],
            ),
            ("src/views".to_string(), vec![entry("render.rs", false)]),
        ])
    }

    fn file_paths(rows: &[FileTreeRow<String>]) -> Vec<&str> {
        rows.iter()
            .filter_map(|row| match row {
                FileTreeRow::File { item, .. } => Some(item.as_str()),
                FileTreeRow::Folder { .. } | FileTreeRow::Loading { .. } => None,
            })
            .collect()
    }

    #[test]
    fn lazy_rows_match_visible_render_order() {
        let mut rows = Vec::new();
        collect_file_tree_rows(
            "",
            0,
            &loaded_dirs(),
            &HashSet::new(),
            &HashSet::from(["src".to_string()]),
            false,
            &mut rows,
        );

        assert_eq!(file_paths(&rows), vec!["src/lib.rs", "README.md"]);
    }

    #[test]
    fn all_rows_include_loaded_collapsed_contents() {
        let mut rows = Vec::new();
        collect_file_tree_rows(
            "",
            0,
            &loaded_dirs(),
            &HashSet::new(),
            &HashSet::from(["src".to_string()]),
            true,
            &mut rows,
        );

        assert_eq!(
            file_paths(&rows),
            vec!["src/views/render.rs", "src/lib.rs", "README.md"]
        );
    }

    #[test]
    fn loading_row_uses_child_depth() {
        let mut rows = Vec::new();
        collect_file_tree_rows(
            "",
            0,
            &HashMap::from([(
                String::new(),
                vec![entry("src", true), entry("README.md", false)],
            )]),
            &HashSet::from(["src".to_string()]),
            &HashSet::from(["src".to_string()]),
            false,
            &mut rows,
        );

        assert!(matches!(
            rows.as_slice(),
            [
                FileTreeRow::Folder { depth: 0, .. },
                FileTreeRow::Loading { depth: 1 },
                FileTreeRow::File { depth: 0, .. }
            ]
        ));
    }
}
