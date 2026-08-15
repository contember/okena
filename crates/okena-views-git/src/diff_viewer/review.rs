//! Pure state for coordinating immutable smart-review datasets.

use okena_core::review::{
    ExactReviewSourceResponse, ImmutableResolvedComparison, ReviewDiffRequest, ReviewInventory,
};
use okena_git::{DiffMode, ExactReviewDiffResponse, FileDiff};
use okena_review::ReviewStructure;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReviewLens {
    Inventory,
    Structure,
    CallDiff,
    Diff,
}

impl ReviewLens {
    pub(crate) const ALL: [Self; 4] =
        [Self::Inventory, Self::Structure, Self::CallDiff, Self::Diff];
}

impl Default for ReviewLens {
    fn default() -> Self {
        Self::ALL[0]
    }
}

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
    pub(crate) mode: Option<DiffMode>,
    pub(crate) lens: ReviewLens,
    pub(crate) inventory: LoadState<ReviewInventory>,
    pub(crate) diff: LoadState<DiffDataset>,
    pub(crate) structure: LoadState<ReviewStructure>,
    pub(crate) file: FileViewState,
}

impl SmartReviewState {
    pub(crate) fn begin(&mut self, mode: DiffMode) -> ReviewEpoch {
        let epoch = self.epoch.next();
        self.mode = Some(mode);
        self.lens = ReviewLens::Inventory;
        self.inventory = LoadState::Loading;
        self.diff = LoadState::Idle;
        self.structure = LoadState::Idle;
        self.file.clear();
        epoch
    }

    pub(crate) fn disable(&mut self) -> ReviewEpoch {
        let epoch = self.epoch.next();
        self.mode = None;
        self.inventory = LoadState::Idle;
        self.diff = LoadState::Idle;
        self.structure = LoadState::Idle;
        self.file.clear();
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
        Some((epoch, comparison))
    }

    pub(crate) fn accepts(&self, epoch: ReviewEpoch, mode: &DiffMode) -> bool {
        self.epoch == epoch && self.mode.as_ref() == Some(mode)
    }

    pub(crate) fn is_current(&self, epoch: ReviewEpoch) -> bool {
        self.epoch == epoch
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
                return Some(Err(error));
            }
        };
        if response.comparison() != expected {
            let error = "Exact review diff returned a different comparison".to_string();
            self.diff = LoadState::Failed(error.clone());
            return Some(Err(error));
        }
        let (comparison, diff) = response.into_parts();
        let files = diff.files;
        self.diff = LoadState::Ready(DiffDataset {
            comparison,
            files: files.clone(),
        });
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
                return Some(Err(error));
            }
        };
        if structure.comparison() != expected {
            let error = "Structured review returned a different comparison".to_string();
            self.structure = LoadState::Failed(error.clone());
            return Some(Err(error));
        }
        self.structure = LoadState::Ready(structure);
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn comparison_json() -> Value {
        let base = "1".repeat(40);
        let merge_base = "2".repeat(40);
        let head = "3".repeat(40);
        json!({
            "requested": {
                "branch_compare": { "base": "main", "head": "feature" }
            },
            "requested_base_oid": base,
            "requested_head_oid": head,
            "strategy": "merge_base_to_head",
            "base": { "kind": "commit", "oid": merge_base },
            "head": { "kind": "commit", "oid": head },
            "merge_base_oid": merge_base,
            "identity": format!("branch:merge-base:{base}:{head}:{merge_base}")
        })
    }

    fn coverage_json() -> Value {
        json!({
            "total_items": 0,
            "analyzed_items": 0,
            "pending_items": 0,
            "skipped_items": 0,
            "unsupported_items": 0,
            "failed_items": 0
        })
    }

    fn inventory() -> ReviewInventory {
        serde_json::from_value(json!({
            "comparison": comparison_json(),
            "totals": {
                "commits": 0,
                "files": 0,
                "files_added": 0,
                "files_deleted": 0,
                "files_modified": 0,
                "files_renamed": 0,
                "files_copied": 0,
                "files_type_changed": 0,
                "files_mode_changed": 0,
                "submodule_changes": 0,
                "binary_files": 0,
                "lines_added": 0,
                "lines_deleted": 0,
                "provenance": { "source": "git" }
            },
            "commits": [],
            "files": [],
            "coverage": coverage_json()
        }))
        .unwrap()
    }

    fn exact_diff() -> ExactReviewDiffResponse {
        serde_json::from_value(json!({
            "comparison": comparison_json(),
            "diff": { "files": [] }
        }))
        .unwrap()
    }

    fn structure() -> ReviewStructure {
        serde_json::from_value(json!({
            "comparison": comparison_json(),
            "files": [],
            "coverage": coverage_json(),
            "language_coverage": [],
            "errors": []
        }))
        .unwrap()
    }

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
        let expected = ImmutableResolvedComparison::try_from(inventory().comparison).unwrap();
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
        let comparison = ImmutableResolvedComparison::try_from(inventory().comparison).unwrap();
        let (diff, structure) = derived_requests(&comparison, true);
        assert_eq!(diff, structure);
        assert_eq!(diff.comparison, comparison);
        assert!(diff.ignore_whitespace);
    }

    #[test]
    fn whitespace_reload_retains_inventory_and_selected_lens() {
        let mode = DiffMode::BranchCompare {
            base: "main".into(),
            head: "feature".into(),
        };
        let inventory = inventory();
        let mut state = SmartReviewState::default();
        state.begin(mode.clone());
        state.inventory = LoadState::Ready(inventory.clone());
        state.lens = ReviewLens::Structure;

        let (epoch, comparison) = state.begin_derived_reload(&mode).unwrap();
        assert!(state.accepts(epoch, &mode));
        assert_eq!(state.lens, ReviewLens::Structure);
        assert_eq!(state.inventory.ready().unwrap(), &inventory);
        assert_eq!(comparison.as_resolved(), &inventory.comparison);
        assert!(matches!(state.diff, LoadState::Loading));
        assert!(matches!(state.structure, LoadState::Loading));
    }

    #[test]
    fn smart_mode_defaults_to_inventory_lens() {
        let mut state = SmartReviewState::default();
        assert_eq!(state.lens, ReviewLens::Inventory);
        state.lens = ReviewLens::CallDiff;
        state.begin(DiffMode::Commit("abc".into()));
        assert_eq!(state.lens, ReviewLens::Inventory);
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
        let expected = ImmutableResolvedComparison::try_from(inventory().comparison).unwrap();
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
