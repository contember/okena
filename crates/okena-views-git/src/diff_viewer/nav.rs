//! Navigation actions for the diff viewer: file/commit/folder selection,
//! view-mode toggles, detach handling, close.

use super::side_by_side;
use super::DiffViewer;
use super::DiffViewerEvent;
use crate::settings::{git_settings, set_git_settings};

use okena_core::types::DiffViewMode;
use okena_git::DiffMode;

use gpui::*;

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
        if self.selected_file_index > 0 {
            self.select_file(self.selected_file_index - 1, cx);
        }
    }

    pub(super) fn next_file(&mut self, cx: &mut Context<Self>) {
        if self.selected_file_index + 1 < self.file_stats.len() {
            self.select_file(self.selected_file_index + 1, cx);
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
        if !self.can_prev_commit() { return; }
        self.commit_index -= 1;
        self.navigate_to_current_commit(cx);
    }

    pub(super) fn next_commit(&mut self, cx: &mut Context<Self>) {
        if !self.can_next_commit() { return; }
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
