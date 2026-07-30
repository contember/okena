//! Navigation actions for the diff viewer: file/commit/folder selection,
//! view-mode toggles, detach handling, close.

use super::DiffViewer;
use super::DiffViewerEvent;
use super::side_by_side;
use crate::settings::{git_settings, set_git_settings};

use okena_core::types::DiffViewMode;
use okena_files::file_tree::FileTreeNode;
use okena_git::DiffMode;

use gpui::*;
use std::collections::HashSet;

#[derive(Clone, Copy)]
enum FileNavigationDirection {
    Previous,
    Next,
}

fn collect_file_indices(
    node: &FileTreeNode,
    parent_path: &str,
    expanded_folders: &HashSet<String>,
    include_collapsed: bool,
    out: &mut Vec<usize>,
) {
    for (name, child) in &node.children {
        let folder_path = if parent_path.is_empty() {
            name.clone()
        } else {
            format!("{parent_path}/{name}")
        };
        if include_collapsed || expanded_folders.contains(&folder_path) {
            collect_file_indices(
                child,
                &folder_path,
                expanded_folders,
                include_collapsed,
                out,
            );
        }
    }
    out.extend(node.files.iter().copied());
}

fn adjacent_visible_file_index(
    tree: &FileTreeNode,
    expanded_folders: &HashSet<String>,
    selected_file_index: usize,
    direction: FileNavigationDirection,
) -> Option<usize> {
    let mut visible = Vec::new();
    collect_file_indices(tree, "", expanded_folders, false, &mut visible);

    if let Some(position) = visible
        .iter()
        .position(|&index| index == selected_file_index)
    {
        return match direction {
            FileNavigationDirection::Previous => {
                position.checked_sub(1).map(|position| visible[position])
            }
            FileNavigationDirection::Next => visible.get(position + 1).copied(),
        };
    }

    let visible: HashSet<usize> = visible.into_iter().collect();
    let mut all = Vec::new();
    collect_file_indices(tree, "", expanded_folders, true, &mut all);
    let position = all.iter().position(|&index| index == selected_file_index)?;

    match direction {
        FileNavigationDirection::Previous => all[..position]
            .iter()
            .rev()
            .find(|index| visible.contains(index))
            .copied(),
        FileNavigationDirection::Next => all[position + 1..]
            .iter()
            .find(|index| visible.contains(index))
            .copied(),
    }
}

impl DiffViewer {
    pub(super) fn toggle_folder(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.expanded_folders.contains(path) {
            self.expanded_folders.remove(path);
        } else {
            self.expanded_folders.insert(path.to_string());
        }
        cx.notify();
    }

    pub(super) fn toggle_mode(&mut self, cx: &mut Context<Self>) {
        let new_mode = self.diff_mode.toggle();
        self.load_diff_async(new_mode, None, cx);
    }

    pub(super) fn toggle_view_mode(&mut self, cx: &mut Context<Self>) {
        self.view_mode = self.view_mode.toggle();
        self.selection.clear();
        self.selection_side = None;
        self.update_side_by_side_cache();
        // Persist through ExtensionSettingsStore
        let mut gs = git_settings(cx);
        gs.diff_view_mode = self.view_mode;
        set_git_settings(&gs, cx);
        cx.notify();
    }

    pub(super) fn toggle_ignore_whitespace(&mut self, cx: &mut Context<Self>) {
        self.ignore_whitespace = !self.ignore_whitespace;
        let mode = self.diff_mode.clone();
        self.load_diff_async(mode, None, cx);
        // Persist through ExtensionSettingsStore
        let mut gs = git_settings(cx);
        gs.diff_ignore_whitespace = self.ignore_whitespace;
        set_git_settings(&gs, cx);
    }

    pub(super) fn update_side_by_side_cache(&mut self) {
        if self.view_mode == DiffViewMode::SideBySide {
            if let Some(file) = &self.current_file {
                self.side_by_side_lines = side_by_side::to_side_by_side(&file.items);
            } else {
                self.side_by_side_lines.clear();
            }
        } else {
            self.side_by_side_lines.clear();
        }
    }

    pub(super) fn select_file(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.file_stats.len() {
            return;
        }
        if index == self.selected_file_index && self.current_file.is_some() {
            return;
        }
        self.selected_file_index = index;
        self.selection.clear();
        self.selection_side = None;
        self.scroll_x = 0.0;
        self.current_file = None;
        self.side_by_side_lines.clear();

        self.process_current_file_async(cx);
        cx.notify();
    }

    pub(super) fn prev_file(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = adjacent_visible_file_index(
            &self.file_tree,
            &self.expanded_folders,
            self.selected_file_index,
            FileNavigationDirection::Previous,
        ) {
            self.select_file(index, cx);
        }
    }

    pub(super) fn next_file(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = adjacent_visible_file_index(
            &self.file_tree,
            &self.expanded_folders,
            self.selected_file_index,
            FileNavigationDirection::Next,
        ) {
            self.select_file(index, cx);
        }
    }

    pub(super) fn close(&self, cx: &mut Context<Self>) {
        cx.emit(DiffViewerEvent::Close);
    }

    /// Mark the viewer as hosted inside a detached window.
    pub fn set_detached(&mut self, detached: bool, cx: &mut Context<Self>) {
        if self.is_detached != detached {
            self.is_detached = detached;
            cx.notify();
        }
    }

    /// Whether this viewer is hosted in a detached window.
    pub fn is_detached(&self) -> bool {
        self.is_detached
    }

    /// Request to detach the viewer into a separate OS window.
    pub(super) fn request_detach(&self, cx: &mut Context<Self>) {
        cx.emit(DiffViewerEvent::Detach);
    }

    pub(super) fn has_commits(&self) -> bool {
        !self.commits.is_empty()
    }

    pub(super) fn can_prev_commit(&self) -> bool {
        self.has_commits() && self.commit_index > 0
    }

    pub(super) fn can_next_commit(&self) -> bool {
        self.has_commits() && self.commit_index + 1 < self.commits.len()
    }

    pub(super) fn prev_commit(&mut self, cx: &mut Context<Self>) {
        if !self.can_prev_commit() {
            return;
        }
        self.commit_index -= 1;
        self.navigate_to_current_commit(cx);
    }

    pub(super) fn next_commit(&mut self, cx: &mut Context<Self>) {
        if !self.can_next_commit() {
            return;
        }
        self.commit_index += 1;
        self.navigate_to_current_commit(cx);
    }

    fn navigate_to_current_commit(&mut self, cx: &mut Context<Self>) {
        let commit = &self.commits[self.commit_index];
        self.commit_message = Some(commit.message.clone());
        let mode = DiffMode::Commit(commit.hash.clone());
        self.load_diff_async(mode, None, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{FileNavigationDirection, adjacent_visible_file_index, collect_file_indices};
    use okena_files::file_tree::build_file_tree;
    use std::collections::HashSet;

    fn tree() -> okena_files::file_tree::FileTreeNode {
        let paths = ["README.md", "src/lib.rs", "src/views/mod.rs", "src/main.rs"];
        build_file_tree(paths.iter().enumerate().map(|(index, path)| (index, path)))
    }

    fn expanded() -> HashSet<String> {
        HashSet::from(["src".to_string(), "src/views".to_string()])
    }

    #[test]
    fn file_order_matches_rendered_tree() {
        let mut indices = Vec::new();
        collect_file_indices(&tree(), "", &expanded(), false, &mut indices);

        assert_eq!(indices, vec![2, 1, 3, 0]);
    }

    #[test]
    fn navigation_follows_rendered_tree_order() {
        let tree = tree();
        let expanded = expanded();

        assert_eq!(
            adjacent_visible_file_index(&tree, &expanded, 2, FileNavigationDirection::Next),
            Some(1)
        );
        assert_eq!(
            adjacent_visible_file_index(&tree, &expanded, 0, FileNavigationDirection::Previous),
            Some(3)
        );
        assert_eq!(
            adjacent_visible_file_index(&tree, &expanded, 0, FileNavigationDirection::Next),
            None
        );
    }

    #[test]
    fn navigation_skips_files_in_collapsed_folders() {
        let tree = tree();
        let expanded = HashSet::from(["src".to_string()]);

        assert_eq!(
            adjacent_visible_file_index(&tree, &expanded, 3, FileNavigationDirection::Previous),
            Some(1)
        );
        assert_eq!(
            adjacent_visible_file_index(&tree, &expanded, 2, FileNavigationDirection::Next),
            Some(1)
        );
    }
}
