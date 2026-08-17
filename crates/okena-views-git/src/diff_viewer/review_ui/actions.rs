//! Every review interaction goes through one of these; the views only call them.
// Frozen surface: the wave-1 view units call these.
#![allow(dead_code)]

use super::super::DiffViewer;
use super::super::review::{LoadState, ReviewFileKey};
use super::super::review_nav::EvidenceTarget;
use super::model::{AttentionTarget, ReasonKind};
use super::ranking::{ModelInputs, StructureLoad, build_review_model};
use super::state::{
    ContentView, FocusRegion, MarkerSpan, NavRowId, NavigatorMode, RoleFilter, RolePreset,
    SymbolRef,
};
use gpui::{App, ClipboardItem, Context, ScrollStrategy, Window};
use okena_core::review::{ComparisonSide, FileRole};
use std::sync::Arc;

impl DiffViewer {
    /// Rebuild the pure model. Cheap enough to run whenever a dataset lands.
    pub(crate) fn review_rebuild_model(&mut self, cx: &mut Context<Self>) {
        let structure_state = match &self.smart_review.structure {
            LoadState::Idle => StructureLoad::NotStarted,
            LoadState::Loading => StructureLoad::Loading,
            LoadState::Failed(error) => StructureLoad::Failed(error.clone()),
            LoadState::Ready(_) => StructureLoad::Ready,
        };
        let model = Arc::new(build_review_model(ModelInputs {
            inventory: self.smart_review.inventory.ready(),
            inventory_error: self.smart_review.inventory.error(),
            structure: self.smart_review.structure.ready(),
            structure_state,
            diff_mode: &self.diff_mode,
        }));
        self.review_ui.model = Some(Arc::clone(&model));

        // Small comparisons skip the Overview and open the first ranked file.
        if model.small_change
            && !self.review_ui.small_change_applied
            && !self.review_navigation.has_pending()
            && let Some(item) = model.attention.first()
        {
            self.review_ui.small_change_applied = true;
            self.review_open_item(item.target.clone(), cx);
        }
        cx.notify();
    }

    /// Drop everything derived from the previous comparison.
    pub(crate) fn review_reset_for_comparison(&mut self) {
        let state = &mut self.review_ui;
        state.model = None;
        state.content = ContentView::Overview;
        state.selected_symbol = None;
        state.queue_target = None;
        state.marker = None;
        state.nav_cursor = None;
        state.nav_reveal = None;
        state.small_change_applied = false;
        state.expanded_dirs.clear();
        state.expanded_initialized = false;
        state.roles_menu_open = false;
        state.status_popover_open = false;
        state.outline_open = false;
        state.help_open = false;
        state.ledger_open = false;
    }

    /// Indices into `ReviewModel::files` that pass the role filter and the text filter.
    pub(crate) fn review_visible_files(&self) -> Vec<usize> {
        let Some(model) = self.review_ui.model.as_ref() else {
            return Vec::new();
        };
        let needle = self.review_ui.filter_text.to_lowercase();
        model
            .files
            .iter()
            .enumerate()
            .filter(|(_, entry)| self.review_ui.role_filter.allows(entry))
            .filter(|(_, entry)| matches_filter(&entry.display_path, &needle))
            .map(|(index, _)| index)
            .collect()
    }

    /// Indices into `ReviewModel::attention` that pass every navigator filter.
    pub(crate) fn review_visible_attention(&self) -> Vec<usize> {
        let Some(model) = self.review_ui.model.as_ref() else {
            return Vec::new();
        };
        let filter = &self.review_ui.attention_filter;
        let needle = self.review_ui.filter_text.to_lowercase();
        model
            .attention
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                filter.kinds.is_empty()
                    || item
                        .reasons
                        .iter()
                        .any(|reason| filter.kinds.contains(&reason.kind))
            })
            .filter(|(_, item)| filter.include_tests || !item.is_test)
            .filter(|(_, item)| match item.target.file() {
                Some(key) => model
                    .file_index(key)
                    .and_then(|index| model.files.get(index))
                    .is_some_and(|entry| self.review_ui.role_filter.allows(entry)),
                None => true,
            })
            .filter(|(_, item)| {
                matches_filter(&item.path, &needle) || matches_filter(&item.name, &needle)
            })
            .map(|(index, _)| index)
            .collect()
    }

    pub(crate) fn review_set_navigator(&mut self, mode: NavigatorMode, cx: &mut Context<Self>) {
        self.review_ui.navigator = mode;
        match mode {
            NavigatorMode::Attention => self.review_reveal_selected_in_attention(),
            NavigatorMode::Files => self.review_reveal_selected_in_files(),
        }
        cx.notify();
    }

    /// Put the cursor on the open file's first Attention row and scroll to it
    /// once; with nothing open the list stays where it was.
    fn review_reveal_selected_in_attention(&mut self) {
        if self.review_ui.content != ContentView::File {
            return;
        }
        let Some(model) = self.review_ui.model.as_ref() else {
            return;
        };
        let Some(key) = self.smart_review.selected_file.as_ref() else {
            return;
        };
        let Some(index) = model.first_attention_for_file(key) else {
            return;
        };
        let target = model.attention[index].target.clone();
        self.review_ui.nav_cursor = Some(NavRowId::Item(target));
        self.review_ui.nav_reveal = Some(ScrollStrategy::Center);
    }

    /// Same for the tree: expand down to the open file, park the cursor on it.
    fn review_reveal_selected_in_files(&mut self) {
        self.review_expand_to_selected_file();
        if self.review_ui.content != ContentView::File {
            return;
        }
        let Some(key) = self.smart_review.selected_file.clone() else {
            return;
        };
        self.review_ui.nav_cursor = Some(NavRowId::File(key));
        self.review_ui.nav_reveal = Some(ScrollStrategy::Center);
    }

    /// Expand every directory above the open file so its tree row is reachable.
    fn review_expand_to_selected_file(&mut self) {
        let Some(key) = self.smart_review.selected_file.clone() else {
            return;
        };
        let Some(path) = key.new_path.clone().or_else(|| key.old_path.clone()) else {
            return;
        };
        let segments: Vec<&str> = path.split('/').collect();
        for depth in 0..segments.len().saturating_sub(1) {
            self.review_ui
                .expanded_dirs
                .insert(segments[..=depth].join("/"));
        }
    }

    pub(crate) fn review_toggle_dir(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.review_ui.expanded_dirs.remove(path) {
            self.review_ui.expanded_dirs.insert(path.to_string());
        }
        cx.notify();
    }

    pub(crate) fn review_set_flatten(&mut self, flatten: bool, cx: &mut Context<Self>) {
        self.review_ui.flatten = flatten;
        cx.notify();
    }

    pub(crate) fn review_set_role_filter(&mut self, filter: RoleFilter, cx: &mut Context<Self>) {
        self.review_ui.role_filter = filter;
        cx.notify();
    }

    pub(crate) fn review_toggle_role(&mut self, role: FileRole, cx: &mut Context<Self>) {
        self.review_ui.role_filter.toggle(role);
        cx.notify();
    }

    pub(crate) fn review_apply_preset(&mut self, preset: RolePreset, cx: &mut Context<Self>) {
        self.review_ui.role_filter = RoleFilter::preset(preset);
        cx.notify();
    }

    pub(crate) fn review_set_saved_filter(
        &mut self,
        likely_mechanical: Option<bool>,
        not_analyzed: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        if let Some(value) = likely_mechanical {
            self.review_ui.role_filter.likely_mechanical_only = value;
        }
        if let Some(value) = not_analyzed {
            self.review_ui.role_filter.not_analyzed_only = value;
        }
        cx.notify();
    }

    pub(crate) fn review_toggle_reason_filter(&mut self, kind: ReasonKind, cx: &mut Context<Self>) {
        if !self.review_ui.attention_filter.kinds.remove(&kind) {
            self.review_ui.attention_filter.kinds.insert(kind);
        }
        cx.notify();
    }

    pub(crate) fn review_toggle_include_tests(&mut self, cx: &mut Context<Self>) {
        let filter = &mut self.review_ui.attention_filter;
        filter.include_tests = !filter.include_tests;
        cx.notify();
    }

    pub(crate) fn review_toggle_group_by_file(&mut self, cx: &mut Context<Self>) {
        let filter = &mut self.review_ui.attention_filter;
        filter.grouped_by_file = !filter.grouped_by_file;
        cx.notify();
    }

    pub(crate) fn review_open_overview(&mut self, cx: &mut Context<Self>) {
        self.review_ui.outline_open = false;
        self.review_ui.content = ContentView::Overview;
        self.review_ui.selected_symbol = None;
        self.review_ui.marker = None;
        cx.notify();
    }

    pub(crate) fn review_open_file(&mut self, key: ReviewFileKey, cx: &mut Context<Self>) {
        self.review_ui.outline_open = false;
        self.review_ui.content = ContentView::File;
        self.review_ui.selected_symbol = None;
        self.review_ui.marker = None;
        self.review_ui.queue_target = self.review_queue_target_for(&key);
        if self.review_file_is_loaded(&key) {
            // Already on screen: re-selecting must not reload or lose the scroll.
            self.review_navigation.invalidate();
            cx.notify();
            return;
        }
        self.select_smart_file(key, cx);
    }

    /// The open file's source and diff are ready and displayed.
    fn review_file_is_loaded(&self, key: &ReviewFileKey) -> bool {
        self.smart_review.selected_file.as_ref() == Some(key)
            && self.smart_review.file.has_ready_cache(key, true)
            && self.current_file.is_some()
    }

    /// The file's own queue entry when it has one, else its first symbol entry.
    fn review_queue_target_for(&self, key: &ReviewFileKey) -> Option<AttentionTarget> {
        let model = self.review_ui.model.as_ref()?;
        let file_target = AttentionTarget::File(key.clone());
        if model.attention_index(&file_target).is_some() {
            return Some(file_target);
        }
        let index = model.first_attention_for_file(key)?;
        model.attention.get(index).map(|item| item.target.clone())
    }

    pub(crate) fn review_open_symbol(&mut self, symbol: SymbolRef, cx: &mut Context<Self>) {
        let Some(model) = self.review_ui.model.clone() else {
            return;
        };
        let Some(entry) = model
            .file_index(&symbol.file)
            .and_then(|index| model.files.get(index))
        else {
            return;
        };
        let Some(found) = entry
            .symbols
            .iter()
            .find(|candidate| candidate.change_index == symbol.change_index)
        else {
            return;
        };
        let marker = MarkerSpan {
            file: symbol.file.clone(),
            old: found.old_hunks.clone(),
            new: found.new_hunks.clone(),
        };
        let navigation = found.navigation.clone();

        self.review_open_file(symbol.file.clone(), cx);
        self.review_ui.queue_target = Some(AttentionTarget::Symbol {
            file: symbol.file.clone(),
            change_index: symbol.change_index,
        });
        self.review_ui.selected_symbol = Some(symbol.clone());
        self.review_ui.marker = Some(marker);
        self.navigate_to_evidence(
            EvidenceTarget {
                file: symbol.file,
                navigation,
            },
            cx,
        );
    }

    pub(crate) fn review_open_item(&mut self, target: AttentionTarget, cx: &mut Context<Self>) {
        match target {
            AttentionTarget::Symbol { file, change_index } => {
                self.review_open_symbol(SymbolRef { file, change_index }, cx);
            }
            AttentionTarget::File(key) => self.review_open_file(key, cx),
            AttentionTarget::Directory(path) => {
                self.review_ui.expanded_dirs.insert(path.clone());
                let key = self.review_ui.model.as_ref().and_then(|model| {
                    model
                        .first_file_under(&path)
                        .and_then(|index| model.files.get(index))
                        .map(|entry| entry.key.clone())
                });
                if let Some(key) = key {
                    self.review_open_file(key, cx);
                } else {
                    cx.notify();
                }
            }
        }
    }

    /// Move along the visible Attention order; clamps at both ends.
    pub(crate) fn review_step_queue(&mut self, delta: i32, cx: &mut Context<Self>) {
        let visible = self.review_visible_attention();
        let Some(model) = self.review_ui.model.clone() else {
            return;
        };
        let current = self
            .review_ui
            .queue_target
            .as_ref()
            .and_then(|target| model.attention_index(target))
            .and_then(|index| visible.iter().position(|visible| *visible == index));
        let Some(row) = step_index(visible.len(), current, delta) else {
            return;
        };
        let Some(target) = visible
            .get(row)
            .and_then(|index| model.attention.get(*index))
            .map(|item| item.target.clone())
        else {
            return;
        };
        self.review_follow_queue_target(&target);
        self.review_open_item(target, cx);
    }

    /// `]` `[` keep the navigator's cursor on the item they land on, so the
    /// list scrolls along and `↑` `↓` continue from there.
    fn review_follow_queue_target(&mut self, target: &AttentionTarget) {
        let cursor = match self.review_ui.navigator {
            NavigatorMode::Attention => NavRowId::Item(target.clone()),
            NavigatorMode::Files => match target {
                AttentionTarget::Symbol { file, .. } | AttentionTarget::File(file) => {
                    NavRowId::File(file.clone())
                }
                AttentionTarget::Directory(path) => NavRowId::Dir(path.clone()),
            },
        };
        self.review_ui.nav_cursor = Some(cursor);
        self.review_ui.nav_reveal = Some(ScrollStrategy::Nearest);
    }

    /// Move along the open file's changed symbols in source order; clamps.
    pub(crate) fn review_step_symbol(&mut self, delta: i32, cx: &mut Context<Self>) {
        let Some(model) = self.review_ui.model.clone() else {
            return;
        };
        let Some(key) = self.smart_review.selected_file.clone() else {
            return;
        };
        let Some(entry) = model
            .file_index(&key)
            .and_then(|index| model.files.get(index))
        else {
            return;
        };
        // Start from the symbol the bar shows (selected, or the one in view),
        // so `}` and the "k of n" counter agree.
        let current = self.review_current_symbol_index();
        let Some(position) = step_index(entry.symbols.len(), current, delta) else {
            return;
        };
        let Some(change_index) = entry
            .symbols
            .get(position)
            .map(|symbol| symbol.change_index)
        else {
            return;
        };
        self.review_open_symbol(
            SymbolRef {
                file: key,
                change_index,
            },
            cx,
        );
    }

    pub(crate) fn review_set_focus_region(&mut self, region: FocusRegion, cx: &mut Context<Self>) {
        self.review_ui.focus_region = region;
        cx.notify();
    }

    pub(crate) fn review_toggle_details(&mut self, cx: &mut Context<Self>) {
        self.review_ui.details_expanded = !self.review_ui.details_expanded;
        cx.notify();
    }

    pub(crate) fn review_toggle_roles_menu(&mut self, cx: &mut Context<Self>) {
        self.review_ui.roles_menu_open = !self.review_ui.roles_menu_open;
        cx.notify();
    }

    pub(crate) fn review_toggle_status_popover(&mut self, cx: &mut Context<Self>) {
        self.review_ui.status_popover_open = !self.review_ui.status_popover_open;
        cx.notify();
    }

    pub(crate) fn review_toggle_outline(&mut self, cx: &mut Context<Self>) {
        self.review_ui.outline_open = !self.review_ui.outline_open;
        cx.notify();
    }

    pub(crate) fn review_toggle_help(&mut self, cx: &mut Context<Self>) {
        self.review_ui.help_open = !self.review_ui.help_open;
        cx.notify();
    }

    pub(crate) fn review_toggle_commit_ledger(&mut self, cx: &mut Context<Self>) {
        self.review_ui.ledger_open = !self.review_ui.ledger_open;
        cx.notify();
    }

    /// Close any open menu, popover or overlay. True when something closed.
    pub(crate) fn review_dismiss_transient(&mut self, cx: &mut Context<Self>) -> bool {
        let state = &mut self.review_ui;
        let open = state.roles_menu_open
            || state.status_popover_open
            || state.outline_open
            || state.help_open;
        if !open {
            return false;
        }
        state.roles_menu_open = false;
        state.status_popover_open = false;
        state.outline_open = false;
        state.help_open = false;
        cx.notify();
        true
    }

    pub(crate) fn review_set_filter_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.review_ui.filter_text = text.clone();
        self.review_ui
            .filter_input
            .update(cx, |input, cx| input.set_value(text, cx));
        cx.notify();
    }

    pub(crate) fn review_focus_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.review_ui.filter_input.update(cx, |input, cx| {
            input.select_all(cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    pub(crate) fn review_clear_filter(&mut self, cx: &mut Context<Self>) {
        self.review_set_filter_text(String::new(), cx);
    }

    pub(crate) fn review_filter_focused(&self, window: &Window, cx: &App) -> bool {
        self.review_ui
            .filter_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    }

    /// Copy `path:line` of the selected symbol, else the open file's path.
    pub(crate) fn review_copy_path_line(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.review_path_line() else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn review_path_line(&self) -> Option<String> {
        let model = self.review_ui.model.as_ref()?;
        if let Some(symbol) = self.review_ui.selected_symbol.as_ref()
            && let Some(entry) = model
                .file_index(&symbol.file)
                .and_then(|index| model.files.get(index))
            && let Some(found) = entry
                .symbols
                .iter()
                .find(|candidate| candidate.change_index == symbol.change_index)
        {
            let navigation = &found.navigation;
            return Some(format!("{}:{}", navigation.path, navigation.line));
        }
        let key = self.smart_review.selected_file.as_ref()?;
        key.path(ComparisonSide::Head)
            .or_else(|| key.path(ComparisonSide::Base))
            .map(str::to_owned)
    }

    /// Whether the diff line painters should mark this row.
    pub(crate) fn review_marker_matches(&self, side: ComparisonSide, line: usize) -> bool {
        let Some(marker) = self.review_ui.marker.as_ref() else {
            return false;
        };
        self.smart_review.selected_file.as_ref() == Some(&marker.file) && marker.matches(side, line)
    }

    /// The unified/split toggle only applies while a diff is on screen.
    pub(crate) fn review_show_split_toggle(&self) -> bool {
        self.review_ui.content == ContentView::File
    }
}

fn matches_filter(haystack: &str, lowercase_needle: &str) -> bool {
    lowercase_needle.is_empty() || haystack.to_lowercase().contains(lowercase_needle)
}

/// Clamped step within `len` rows; no selection starts at the first row.
fn step_index(len: usize, current: Option<usize>, delta: i32) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let Some(current) = current else {
        return Some(0);
    };
    let current = i64::try_from(current).ok()?;
    let last = i64::try_from(len.saturating_sub(1)).ok()?;
    let next = current.saturating_add(i64::from(delta)).clamp(0, last);
    usize::try_from(next).ok()
}

#[cfg(test)]
mod tests {
    use super::super::fixtures;
    use super::super::ranking::{ModelInputs, StructureLoad, build_review_model};
    use super::super::state::{RoleFilter, RolePreset};
    use super::{matches_filter, step_index};
    use okena_git::DiffMode;

    #[test]
    fn visible_files_intersect_the_role_filter_with_the_text_filter() {
        let inventory = fixtures::inventory();
        let model = build_review_model(ModelInputs {
            inventory: Some(&inventory),
            inventory_error: None,
            structure: None,
            structure_state: StructureLoad::Loading,
            diff_mode: &DiffMode::BranchCompare {
                base: "main".into(),
                head: "feature".into(),
            },
        });
        let visible = |filter: &RoleFilter, needle: &str| -> Vec<String> {
            model
                .files
                .iter()
                .filter(|entry| filter.allows(entry))
                .filter(|entry| matches_filter(&entry.display_path, needle))
                .map(|entry| entry.display_path.clone())
                .collect()
        };

        let everything = RoleFilter::everything();
        assert_eq!(
            visible(&everything, "lib"),
            ["src/lib.rs", "tests/lib.rs"],
            "the text filter alone keeps both roles"
        );
        assert_eq!(
            visible(&RoleFilter::preset(RolePreset::ReviewCode), "lib"),
            ["src/lib.rs"],
            "the role filter drops the test file"
        );
        assert!(
            visible(&everything, "").len() >= 7,
            "an empty filter keeps every inventory file"
        );
    }

    #[test]
    fn steps_clamp_at_both_ends_and_start_at_the_first_row() {
        assert_eq!(step_index(0, None, 1), None);
        assert_eq!(step_index(3, None, 1), Some(0));
        assert_eq!(step_index(3, None, -1), Some(0));
        assert_eq!(step_index(3, Some(0), 1), Some(1));
        assert_eq!(step_index(3, Some(2), 1), Some(2));
        assert_eq!(step_index(3, Some(0), -1), Some(0));
        assert_eq!(step_index(3, Some(1), 5), Some(2));
        assert_eq!(step_index(3, Some(1), -5), Some(0));
    }

    #[test]
    fn the_text_filter_is_case_insensitive_and_empty_means_everything() {
        assert!(matches_filter("src/Lib.rs", ""));
        assert!(matches_filter("src/Lib.rs", "lib"));
        assert!(!matches_filter("src/Lib.rs", "tests"));
    }
}
