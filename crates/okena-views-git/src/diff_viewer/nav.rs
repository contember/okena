//! Navigation actions for the diff viewer: file/commit/folder selection,
//! view-mode toggles, detach handling, close.

use super::DiffViewer;
use super::DiffViewerEvent;
use super::side_by_side;
use crate::settings::{git_settings, set_git_settings};

use okena_core::types::DiffViewMode;
use okena_files::file_tree::{
    FileTreeNavigationDirection, FileTreeRow, adjacent_file_tree_item, indexed_file_tree_rows,
};
use okena_git::DiffMode;

use gpui::*;

const REVISION_PAGE_SIZE: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RevisionTarget {
    Uncommitted,
    Commit(usize),
}

pub(super) fn history_starts_at_head(commits: &[okena_git::CommitLogEntry]) -> bool {
    commits.first().is_some_and(|commit| {
        commit
            .refs
            .iter()
            .any(|reference| reference == "HEAD" || reference.starts_with("HEAD -> "))
    })
}

fn newer_revision_target(
    is_uncommitted: bool,
    commit_index: usize,
    history_includes_uncommitted: bool,
) -> Option<RevisionTarget> {
    if is_uncommitted {
        None
    } else if commit_index > 0 {
        Some(RevisionTarget::Commit(commit_index - 1))
    } else if history_includes_uncommitted {
        Some(RevisionTarget::Uncommitted)
    } else {
        None
    }
}

fn older_revision_target(
    is_uncommitted: bool,
    commit_index: usize,
    commit_count: usize,
    history_includes_uncommitted: bool,
) -> Option<RevisionTarget> {
    if is_uncommitted {
        (history_includes_uncommitted && commit_count > 0).then_some(RevisionTarget::Commit(0))
    } else if commit_index + 1 < commit_count {
        Some(RevisionTarget::Commit(commit_index + 1))
    } else {
        None
    }
}

impl DiffViewer {
    pub(super) fn file_tree_rows(&self, include_collapsed: bool) -> Vec<FileTreeRow<usize>> {
        indexed_file_tree_rows(&self.file_tree, &self.expanded_folders, include_collapsed)
    }

    pub(super) fn toggle_folder(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.expanded_folders.contains(path) {
            self.expanded_folders.remove(path);
        } else {
            self.expanded_folders.insert(path.to_string());
        }
        cx.notify();
    }

    pub(super) fn toggle_mode(&mut self, cx: &mut Context<Self>) {
        if !self.is_uncommitted() {
            return;
        }
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

    fn navigate_file_tree(
        &mut self,
        direction: FileTreeNavigationDirection,
        cx: &mut Context<Self>,
    ) {
        let visible = self.file_tree_rows(false);
        let all = self.file_tree_rows(true);
        if let Some(index) =
            adjacent_file_tree_item(&visible, &all, Some(&self.selected_file_index), direction)
        {
            self.select_file(index, cx);
        }
    }

    pub(super) fn prev_file(&mut self, cx: &mut Context<Self>) {
        self.navigate_file_tree(FileTreeNavigationDirection::Previous, cx);
    }

    pub(super) fn next_file(&mut self, cx: &mut Context<Self>) {
        self.navigate_file_tree(FileTreeNavigationDirection::Next, cx);
    }

    pub(super) fn close(&self, cx: &mut Context<Self>) {
        cx.emit(DiffViewerEvent::Close);
    }

    pub(super) fn back(&self, cx: &mut Context<Self>) {
        cx.emit(DiffViewerEvent::Back);
    }

    pub(super) fn back_or_close(&self, cx: &mut Context<Self>) {
        if self.can_go_back {
            self.back(cx);
        } else {
            self.close(cx);
        }
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

    pub(super) fn is_uncommitted(&self) -> bool {
        matches!(self.diff_mode, DiffMode::WorkingTree | DiffMode::Staged)
    }

    pub(super) fn shows_revision_bar(&self) -> bool {
        self.is_uncommitted() || matches!(self.diff_mode, DiffMode::Commit(_))
    }

    pub(super) fn can_navigate_newer(&self) -> bool {
        newer_revision_target(
            self.is_uncommitted(),
            self.commit_index,
            self.history_includes_uncommitted,
        )
        .is_some()
    }

    pub(super) fn can_navigate_older(&self) -> bool {
        older_revision_target(
            self.is_uncommitted(),
            self.commit_index,
            self.commits.len(),
            self.history_includes_uncommitted,
        )
        .is_some()
    }

    pub(super) fn navigate_newer(&mut self, cx: &mut Context<Self>) {
        let Some(target) = newer_revision_target(
            self.is_uncommitted(),
            self.commit_index,
            self.history_includes_uncommitted,
        ) else {
            return;
        };
        self.navigate_to_revision(target, cx);
    }

    pub(super) fn navigate_older(&mut self, cx: &mut Context<Self>) {
        let Some(target) = older_revision_target(
            self.is_uncommitted(),
            self.commit_index,
            self.commits.len(),
            self.history_includes_uncommitted,
        ) else {
            return;
        };
        self.navigate_to_revision(target, cx);
    }

    fn navigate_to_revision(&mut self, target: RevisionTarget, cx: &mut Context<Self>) {
        match target {
            RevisionTarget::Uncommitted => {
                self.commit_message = None;
                self.load_diff_async(self.uncommitted_mode.clone(), None, cx);
            }
            RevisionTarget::Commit(index) => {
                if self.is_uncommitted() {
                    self.uncommitted_mode = self.diff_mode.clone();
                }
                self.commit_index = index;
                self.navigate_to_current_commit(cx);
            }
        }
    }

    fn navigate_to_current_commit(&mut self, cx: &mut Context<Self>) {
        let commit = &self.commits[self.commit_index];
        self.commit_message = Some(commit.message.clone());
        let mode = DiffMode::Commit(commit.hash.clone());
        self.load_diff_async(mode, None, cx);
    }

    pub(super) fn load_commit_history_async(&mut self, cx: &mut Context<Self>) {
        if self.commit_history_loading || !self.commits.is_empty() {
            return;
        }

        self.commit_history_loading = true;
        cx.notify();

        let provider = self.provider.clone();
        cx.spawn(async move |this, cx| {
            let result =
                smol::unblock(move || provider.get_commit_graph(REVISION_PAGE_SIZE, None)).await;

            let _ = this.update(cx, |this, cx| {
                this.commit_history_loading = false;
                if let Ok(commits) = result {
                    this.commits = commits;
                    this.commit_index = 0;
                    this.history_includes_uncommitted = true;
                }
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RevisionTarget, history_starts_at_head, newer_revision_target, older_revision_target,
    };
    use okena_git::CommitLogEntry;

    fn commit(reference: &str) -> CommitLogEntry {
        CommitLogEntry {
            hash: "abcdef0".to_string(),
            parents: Vec::new(),
            message: "Message".to_string(),
            author: "Author".to_string(),
            timestamp: 0,
            refs: vec![reference.to_string()],
        }
    }

    #[test]
    fn detects_only_histories_rooted_at_head() {
        assert!(history_starts_at_head(&[commit("HEAD -> main")]));
        assert!(history_starts_at_head(&[commit("HEAD")]));
        assert!(!history_starts_at_head(&[commit("feature")]));
        assert!(!history_starts_at_head(&[]));
    }

    #[test]
    fn uncommitted_and_head_are_adjacent_revisions() {
        assert_eq!(
            older_revision_target(true, 0, 3, true),
            Some(RevisionTarget::Commit(0))
        );
        assert_eq!(
            newer_revision_target(false, 0, true),
            Some(RevisionTarget::Uncommitted)
        );
    }

    #[test]
    fn alternate_branch_history_does_not_cross_into_uncommitted() {
        assert_eq!(older_revision_target(true, 0, 3, false), None);
        assert_eq!(newer_revision_target(false, 0, false), None);
    }

    #[test]
    fn commit_navigation_stays_within_loaded_history() {
        assert_eq!(
            newer_revision_target(false, 2, false),
            Some(RevisionTarget::Commit(1))
        );
        assert_eq!(
            older_revision_target(false, 1, 3, false),
            Some(RevisionTarget::Commit(2))
        );
        assert_eq!(older_revision_target(false, 2, 3, false), None);
    }
}
