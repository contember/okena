//! Pure state for coordinating immutable smart-review datasets.

use okena_core::review::{
    ComparisonSide, ExactReviewSourceResponse, ImmutableResolvedComparison, ReviewDiffRequest,
    ReviewInventory,
};
use okena_git::{DiffMode, ExactReviewDiffResponse, FileDiff};
use okena_review::ReviewStructure;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ReviewFileKey {
    pub(crate) old_path: Option<String>,
    pub(crate) new_path: Option<String>,
}

impl ReviewFileKey {
    pub(crate) fn from_diff(file: &FileDiff) -> Self {
        Self {
            old_path: file.old_path.clone(),
            new_path: file.new_path.clone(),
        }
    }

    pub(crate) fn from_inventory(file: &okena_core::review::ReviewFileFact) -> Self {
        Self {
            old_path: file.old_path.clone(),
            new_path: file.new_path.clone(),
        }
    }

    pub(crate) fn display(&self) -> String {
        match (&self.old_path, &self.new_path) {
            (Some(old), Some(new)) if old != new => format!("{old} → {new}"),
            (_, Some(new)) => new.clone(),
            (Some(old), None) => old.clone(),
            (None, None) => "(no file)".into(),
        }
    }

    pub(crate) fn path(&self, side: ComparisonSide) -> Option<&str> {
        match side {
            ComparisonSide::Base => self.old_path.as_deref(),
            ComparisonSide::Head => self.new_path.as_deref(),
        }
    }

    pub(crate) fn matches_inventory(&self, file: &okena_core::review::ReviewFileFact) -> bool {
        self.old_path == file.old_path && self.new_path == file.new_path
    }

    pub(crate) fn matches_source(&self, source: &ExactReviewSourceResponse) -> bool {
        self.old_path.as_deref() == source.old_path()
            && self.new_path.as_deref() == source.new_path()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ReviewEpoch(u64);

impl ReviewEpoch {
    fn next(&mut self) -> Self {
        self.0 = self.0.wrapping_add(1);
        if self.0 == 0 {
            self.0 = 1;
        }
        *self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FileGeneration(u64);

impl FileGeneration {
    fn next(&mut self) -> Self {
        self.0 = self.0.wrapping_add(1);
        if self.0 == 0 {
            self.0 = 1;
        }
        *self
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) enum LoadState<T> {
    #[default]
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> LoadState<T> {
    pub(crate) fn ready(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            Self::Idle | Self::Loading | Self::Failed(_) => None,
        }
    }

    pub(crate) fn error(&self) -> Option<&str> {
        match self {
            Self::Failed(error) => Some(error),
            Self::Idle | Self::Loading | Self::Ready(_) => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DiffDataset {
    pub(crate) comparison: ImmutableResolvedComparison,
    pub(crate) files: Vec<FileDiff>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FileViewState {
    generation: FileGeneration,
    pub(crate) key: Option<ReviewFileKey>,
    pub(crate) source: LoadState<ExactReviewSourceResponse>,
    cache_ready: bool,
}

impl FileViewState {
    pub(crate) fn begin(&mut self, key: ReviewFileKey) -> FileGeneration {
        let generation = self.generation.next();
        self.key = Some(key);
        self.source = LoadState::Loading;
        self.cache_ready = false;
        generation
    }

    pub(crate) fn clear(&mut self) {
        self.generation.next();
        self.key = None;
        self.source = LoadState::Idle;
        self.cache_ready = false;
    }

    pub(crate) fn accepts(&self, generation: FileGeneration, key: &ReviewFileKey) -> bool {
        self.generation == generation && self.key.as_ref() == Some(key)
    }

    pub(crate) fn mark_cache_ready(
        &mut self,
        generation: FileGeneration,
        key: &ReviewFileKey,
    ) -> bool {
        if !self.accepts(generation, key) {
            return false;
        }
        self.cache_ready = true;
        true
    }

    pub(crate) fn has_ready_cache(&self, key: &ReviewFileKey, smart: bool) -> bool {
        if !self.cache_ready || self.key.as_ref() != Some(key) {
            return false;
        }
        !smart
            || matches!(
                &self.source,
                LoadState::Ready(source) if key.matches_source(source)
            )
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SmartReviewState {
    epoch: ReviewEpoch,
    content_revision: u64,
    selection_revision: u64,
    pub(crate) mode: Option<DiffMode>,
    pub(crate) selected_file: Option<ReviewFileKey>,
    pub(crate) inventory: LoadState<ReviewInventory>,
    pub(crate) diff: LoadState<DiffDataset>,
    pub(crate) structure: LoadState<ReviewStructure>,
    pub(crate) file: FileViewState,
}

impl SmartReviewState {
    fn bump_content_revision(&mut self) {
        self.content_revision = self.content_revision.wrapping_add(1);
        if self.content_revision == 0 {
            self.content_revision = 1;
        }
    }

    fn bump_selection_revision(&mut self) {
        self.selection_revision = self.selection_revision.wrapping_add(1);
        if self.selection_revision == 0 {
            self.selection_revision = 1;
        }
    }

    /// Cache scope for everything derived from the datasets.
    #[allow(dead_code)]
    pub(crate) fn content_revision(&self) -> u64 {
        self.content_revision
    }

    /// Cache scope for everything derived from the selected file.
    #[allow(dead_code)]
    pub(crate) fn selection_revision(&self) -> u64 {
        self.selection_revision
    }

    pub(crate) fn changed(&mut self) {
        self.bump_content_revision();
    }

    pub(crate) fn begin(&mut self, mode: DiffMode) -> ReviewEpoch {
        let epoch = self.epoch.next();
        self.mode = Some(mode);
        self.selected_file = None;
        self.inventory = LoadState::Loading;
        self.diff = LoadState::Idle;
        self.structure = LoadState::Idle;
        self.file.clear();
        self.bump_content_revision();
        self.bump_selection_revision();
        epoch
    }

    pub(crate) fn disable(&mut self) -> ReviewEpoch {
        let epoch = self.epoch.next();
        self.mode = None;
        self.selected_file = None;
        self.inventory = LoadState::Idle;
        self.diff = LoadState::Idle;
        self.structure = LoadState::Idle;
        self.file.clear();
        self.bump_content_revision();
        self.bump_selection_revision();
        epoch
    }

    pub(crate) fn begin_derived_reload(
        &mut self,
        mode: &DiffMode,
    ) -> Option<(ReviewEpoch, ImmutableResolvedComparison)> {
        if self.mode.as_ref() != Some(mode) {
            return None;
        }
        let inventory = self.inventory.ready()?.clone();
        let comparison =
            ImmutableResolvedComparison::try_from(inventory.comparison.clone()).ok()?;
        let epoch = self.epoch.next();
        self.inventory = LoadState::Ready(inventory);
        self.diff = LoadState::Loading;
        self.structure = LoadState::Loading;
        self.file.clear();
        self.bump_content_revision();
        Some((epoch, comparison))
    }

    pub(crate) fn accepts(&self, epoch: ReviewEpoch, mode: &DiffMode) -> bool {
        self.epoch == epoch && self.mode.as_ref() == Some(mode)
    }

    pub(crate) fn is_current(&self, epoch: ReviewEpoch) -> bool {
        self.epoch == epoch
    }

    pub(crate) fn set_inventory(
        &mut self,
        inventory: ReviewInventory,
        preferred_path: Option<&str>,
    ) {
        let retained = self.selected_file.as_ref().filter(|selected| {
            inventory
                .files
                .iter()
                .any(|file| selected.matches_inventory(file))
        });
        let selected = retained.cloned().or_else(|| {
            preferred_path
                .and_then(|path| {
                    inventory.files.iter().find(|file| {
                        file.old_path.as_deref() == Some(path)
                            || file.new_path.as_deref() == Some(path)
                    })
                })
                .or_else(|| inventory.files.first())
                .map(ReviewFileKey::from_inventory)
        });
        self.selected_file = selected;
        self.inventory = LoadState::Ready(inventory);
        self.bump_content_revision();
        self.bump_selection_revision();
    }

    pub(crate) fn set_selected_file(&mut self, key: ReviewFileKey) {
        if self.selected_file.as_ref() != Some(&key) {
            self.selected_file = Some(key);
            self.file.clear();
            self.bump_selection_revision();
        }
    }

    pub(crate) fn comparison(&self) -> Option<ImmutableResolvedComparison> {
        if let Some(dataset) = self.diff.ready() {
            return Some(dataset.comparison.clone());
        }
        self.inventory.ready().and_then(|inventory| {
            ImmutableResolvedComparison::try_from(inventory.comparison.clone()).ok()
        })
    }

    pub(crate) fn accept_diff(
        &mut self,
        epoch: ReviewEpoch,
        mode: &DiffMode,
        expected: &ImmutableResolvedComparison,
        result: Result<ExactReviewDiffResponse, String>,
    ) -> Option<Result<Vec<FileDiff>, String>> {
        if !self.accepts(epoch, mode) {
            return None;
        }
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                self.diff = LoadState::Failed(error.clone());
                self.bump_content_revision();
                return Some(Err(error));
            }
        };
        if response.comparison() != expected {
            let error = "Exact review diff returned a different comparison".to_string();
            self.diff = LoadState::Failed(error.clone());
            self.bump_content_revision();
            return Some(Err(error));
        }
        let (comparison, diff) = response.into_parts();
        let files = diff.files;
        self.diff = LoadState::Ready(DiffDataset {
            comparison,
            files: files.clone(),
        });
        self.bump_content_revision();
        Some(Ok(self
            .diff
            .ready()
            .map_or(files, |dataset| dataset.files.clone())))
    }

    pub(crate) fn accept_structure(
        &mut self,
        epoch: ReviewEpoch,
        mode: &DiffMode,
        expected: &ImmutableResolvedComparison,
        result: Result<ReviewStructure, String>,
    ) -> Option<Result<(), String>> {
        if !self.accepts(epoch, mode) {
            return None;
        }
        let structure = match result {
            Ok(structure) => structure,
            Err(error) => {
                self.structure = LoadState::Failed(error.clone());
                self.bump_content_revision();
                return Some(Err(error));
            }
        };
        if structure.comparison() != expected {
            let error = "Structured review returned a different comparison".to_string();
            self.structure = LoadState::Failed(error.clone());
            self.bump_content_revision();
            return Some(Err(error));
        }
        self.structure = LoadState::Ready(structure);
        self.bump_content_revision();
        Some(Ok(()))
    }
}

pub(crate) fn is_smart_mode(mode: &DiffMode) -> bool {
    matches!(mode, DiffMode::BranchCompare { .. } | DiffMode::Commit(_))
}

pub(crate) fn derived_requests(
    comparison: &ImmutableResolvedComparison,
    ignore_whitespace: bool,
) -> (ReviewDiffRequest, ReviewDiffRequest) {
    let request = ReviewDiffRequest {
        comparison: comparison.clone(),
        ignore_whitespace,
    };
    (request.clone(), request)
}

pub(crate) fn theme_requires_rehighlight(requested_dark: bool, current_dark: bool) -> bool {
    requested_dark != current_dark
}

pub(crate) fn adjacent_inventory_file(
    inventory: &ReviewInventory,
    selected: Option<&ReviewFileKey>,
    forward: bool,
) -> Option<ReviewFileKey> {
    let index = selected
        .and_then(|selected| {
            inventory
                .files
                .iter()
                .position(|file| selected.matches_inventory(file))
        })
        .unwrap_or(0);
    let next = if forward {
        index
            .saturating_add(1)
            .min(inventory.files.len().saturating_sub(1))
    } else {
        index.saturating_sub(1)
    };
    inventory.files.get(next).map(ReviewFileKey::from_inventory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_viewer::review_ui::fixtures::{
        empty_inventory, exact_diff, inventory, structure,
    };

    #[test]
    fn stale_mode_and_file_generations_are_rejected() {
        let mut state = SmartReviewState::default();
        let old_epoch = state.begin(DiffMode::Commit("old".into()));
        let new_mode = DiffMode::Commit("new".into());
        let new_epoch = state.begin(new_mode.clone());
        assert!(!state.accepts(old_epoch, &DiffMode::Commit("old".into())));
        assert!(state.accepts(new_epoch, &new_mode));

        let a = ReviewFileKey {
            old_path: Some("a.rs".into()),
            new_path: Some("a.rs".into()),
        };
        let b = ReviewFileKey {
            old_path: Some("b.rs".into()),
            new_path: Some("b.rs".into()),
        };
        let generation_a = state.file.begin(a.clone());
        let generation_b = state.file.begin(b.clone());
        assert!(!state.file.accepts(generation_a, &a));
        assert!(state.file.accepts(generation_b, &b));
    }

    #[test]
    fn ordinary_modes_are_not_smart_review_modes() {
        assert!(!is_smart_mode(&DiffMode::WorkingTree));
        assert!(!is_smart_mode(&DiffMode::Staged));
        assert!(is_smart_mode(&DiffMode::Commit("abc".into())));
        assert!(is_smart_mode(&DiffMode::BranchCompare {
            base: "main".into(),
            head: "feature".into(),
        }));
    }

    #[test]
    fn diff_and_structure_accept_in_either_order_for_one_full_comparison() {
        let mode = DiffMode::BranchCompare {
            base: "main".into(),
            head: "feature".into(),
        };
        let expected = ImmutableResolvedComparison::try_from(empty_inventory().comparison).unwrap();
        for structure_first in [false, true] {
            let mut state = SmartReviewState::default();
            let epoch = state.begin(mode.clone());
            if structure_first {
                assert_eq!(
                    state.accept_structure(epoch, &mode, &expected, Ok(structure())),
                    Some(Ok(()))
                );
                assert!(
                    state
                        .accept_diff(epoch, &mode, &expected, Ok(exact_diff()))
                        .unwrap()
                        .is_ok()
                );
            } else {
                assert!(
                    state
                        .accept_diff(epoch, &mode, &expected, Ok(exact_diff()))
                        .unwrap()
                        .is_ok()
                );
                assert_eq!(
                    state.accept_structure(epoch, &mode, &expected, Ok(structure())),
                    Some(Ok(()))
                );
            }
            assert!(matches!(state.diff, LoadState::Ready(_)));
            assert!(matches!(state.structure, LoadState::Ready(_)));
        }
    }

    #[test]
    fn derived_requests_share_the_full_comparison_and_whitespace_flag() {
        let comparison =
            ImmutableResolvedComparison::try_from(empty_inventory().comparison).unwrap();
        let (diff, structure) = derived_requests(&comparison, true);
        assert_eq!(diff, structure);
        assert_eq!(diff.comparison, comparison);
        assert!(diff.ignore_whitespace);
    }

    #[test]
    fn whitespace_reload_retains_the_inventory_and_reloads_the_derived_datasets() {
        let mode = DiffMode::BranchCompare {
            base: "main".into(),
            head: "feature".into(),
        };
        let inventory = empty_inventory();
        let mut state = SmartReviewState::default();
        state.begin(mode.clone());
        state.inventory = LoadState::Ready(inventory.clone());

        let (epoch, comparison) = state.begin_derived_reload(&mode).unwrap();
        assert!(state.accepts(epoch, &mode));
        assert_eq!(state.inventory.ready().unwrap(), &inventory);
        assert_eq!(comparison.as_resolved(), &inventory.comparison);
        assert!(matches!(state.diff, LoadState::Loading));
        assert!(matches!(state.structure, LoadState::Loading));
    }

    #[test]
    fn inventory_selection_survives_diff_failure_and_derived_reload() {
        let mode = DiffMode::Commit("abc".into());
        let inventory = inventory();
        let comparison =
            ImmutableResolvedComparison::try_from(inventory.comparison.clone()).unwrap();
        let mut state = SmartReviewState::default();
        let epoch = state.begin(mode.clone());
        state.set_inventory(inventory, None);
        assert_eq!(
            state.selected_file.as_ref().unwrap().new_path.as_deref(),
            Some("src/lib.rs")
        );

        state.set_selected_file(ReviewFileKey {
            old_path: Some("tests/lib.rs".into()),
            new_path: Some("tests/lib.rs".into()),
        });
        assert!(
            state
                .accept_diff(epoch, &mode, &comparison, Err("diff failed".into()))
                .unwrap()
                .is_err()
        );
        assert_eq!(
            state.selected_file.as_ref().unwrap().new_path.as_deref(),
            Some("tests/lib.rs")
        );

        state.begin_derived_reload(&mode).unwrap();
        assert_eq!(
            state.selected_file.as_ref().unwrap().new_path.as_deref(),
            Some("tests/lib.rs")
        );
    }

    #[test]
    fn content_and_selection_revisions_invalidate_independent_cache_scopes() {
        let mut state = SmartReviewState::default();
        state.begin(DiffMode::Commit("abc".into()));
        let initial_content = state.content_revision();
        let initial_selection = state.selection_revision();
        state.set_inventory(inventory(), None);
        let after_data_content = state.content_revision();
        let after_data_selection = state.selection_revision();
        assert!(after_data_content > initial_content);
        assert!(after_data_selection > initial_selection);
        state.set_selected_file(ReviewFileKey {
            old_path: Some("tests/lib.rs".into()),
            new_path: Some("tests/lib.rs".into()),
        });
        assert_eq!(state.content_revision(), after_data_content);
        assert!(state.selection_revision() > after_data_selection);
    }

    #[test]
    fn keyboard_adjacency_returns_canonical_inventory_pairs() {
        let inventory = inventory();
        let first = adjacent_inventory_file(&inventory, None, false).unwrap();
        assert_eq!(first.new_path.as_deref(), Some("src/lib.rs"));
        let second = adjacent_inventory_file(&inventory, Some(&first), true).unwrap();
        assert_eq!(second.old_path.as_deref(), Some("tests/lib.rs"));
        assert_eq!(second.new_path.as_deref(), Some("tests/lib.rs"));
        assert_eq!(
            adjacent_inventory_file(&inventory, Some(&second), false),
            Some(first)
        );
    }

    #[test]
    fn file_cache_and_theme_guards_reject_stale_or_failed_source() {
        let mut file = FileViewState::default();
        let a = ReviewFileKey {
            old_path: Some("a.rs".into()),
            new_path: Some("a.rs".into()),
        };
        let b = ReviewFileKey {
            old_path: Some("b.rs".into()),
            new_path: Some("b.rs".into()),
        };
        let generation_a = file.begin(a.clone());
        let generation_b = file.begin(b.clone());
        assert!(!file.mark_cache_ready(generation_a, &a));
        assert!(file.mark_cache_ready(generation_b, &b));
        assert!(file.has_ready_cache(&b, false));
        assert!(!file.has_ready_cache(&a, false));

        file.source = LoadState::Failed("source failed".into());
        assert!(!file.has_ready_cache(&b, true));
        assert_eq!(file.source.error(), Some("source failed"));
        assert!(theme_requires_rehighlight(false, true));
        assert!(!theme_requires_rehighlight(true, true));
    }

    #[test]
    fn smart_dataset_failure_is_local_and_can_recover() {
        let mode = DiffMode::BranchCompare {
            base: "main".into(),
            head: "feature".into(),
        };
        let expected = ImmutableResolvedComparison::try_from(empty_inventory().comparison).unwrap();
        let mut state = SmartReviewState::default();
        let epoch = state.begin(mode.clone());

        assert!(
            state
                .accept_diff(epoch, &mode, &expected, Err("diff failed".into()))
                .unwrap()
                .is_err()
        );
        assert_eq!(state.diff.error(), Some("diff failed"));
        assert_eq!(
            state.accept_structure(epoch, &mode, &expected, Ok(structure())),
            Some(Ok(()))
        );
        assert!(matches!(state.structure, LoadState::Ready(_)));

        assert!(
            state
                .accept_diff(epoch, &mode, &expected, Ok(exact_diff()))
                .unwrap()
                .is_ok()
        );
        assert!(matches!(state.diff, LoadState::Ready(_)));
        assert!(matches!(state.structure, LoadState::Ready(_)));
    }
}
