//! Exact evidence navigation and pure diff-row mapping.

use super::DiffViewer;
use super::review::{FileGeneration, ReviewFileKey, SmartReviewState};
use super::review_ui::ContentView;
use super::types::{DisplayItem, ExpanderRow, SideBySideLine};
use gpui::{Context, ScrollStrategy, UniformListScrollHandle};
use okena_core::review::{ComparisonSide, ReviewNavigationTarget};
use okena_core::types::DiffViewMode;
use okena_git::FileDiff;
use std::fmt;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct NavigationToken(u64);

impl NavigationToken {
    fn next(&mut self) -> Self {
        self.0 = self.0.wrapping_add(1);
        if self.0 == 0 {
            self.0 = 1;
        }
        *self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingNavigation {
    pub(crate) token: NavigationToken,
    pub(crate) generation: FileGeneration,
    pub(crate) target: EvidenceTarget,
    pub(crate) source_started: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ReviewNavigationState {
    next_token: NavigationToken,
    pub(crate) pending: Option<PendingNavigation>,
    pub(crate) unavailable: Option<NavigationUnavailable>,
}

impl ReviewNavigationState {
    pub(crate) fn begin(
        &mut self,
        generation: FileGeneration,
        target: EvidenceTarget,
    ) -> NavigationToken {
        let token = self.next_token.next();
        self.pending = Some(PendingNavigation {
            token,
            generation,
            target,
            source_started: false,
        });
        self.unavailable = None;
        token
    }

    pub(crate) fn invalidate(&mut self) {
        self.next_token.next();
        self.pending = None;
        self.unavailable = None;
    }

    pub(crate) fn accepts(
        &self,
        token: NavigationToken,
        generation: FileGeneration,
        key: &ReviewFileKey,
    ) -> bool {
        self.pending.as_ref().is_some_and(|pending| {
            pending.token == token
                && pending.generation == generation
                && &pending.target.file == key
        })
    }

    pub(crate) fn fail_current(&mut self, error: NavigationUnavailable) {
        self.pending = None;
        self.unavailable = Some(error);
    }

    /// The navigation landed; the row marker now comes from the selected symbol.
    pub(crate) fn finish(&mut self) {
        self.pending = None;
        self.unavailable = None;
    }

    pub(crate) fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
}

impl DiffViewer {
    pub(super) fn navigate_to_evidence(&mut self, target: EvidenceTarget, cx: &mut Context<Self>) {
        // Evidence always lands in the diff, so the content area follows it.
        self.review_ui.content = ContentView::File;
        if preflight_evidence_navigation(
            &mut self.smart_review,
            &mut self.review_navigation,
            &target,
        )
        .is_err()
        {
            cx.notify();
            return;
        }
        // The same file, already loaded and displayed: map straight to the
        // row. Only another file (or a stale load) starts a fresh source load.
        let loaded = self.smart_review.file.has_ready_cache(&target.file, true)
            && self.current_file.is_some();
        let generation = if loaded {
            self.smart_review.file.generation()
        } else {
            self.smart_review.file.begin(target.file.clone())
        };
        self.review_navigation.begin(generation, target);
        if loaded && let Some(pending) = self.review_navigation.pending.as_mut() {
            pending.source_started = true;
        }
        self.resume_review_navigation(cx);
        cx.notify();
    }

    pub(super) fn resume_review_navigation(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.review_navigation.pending.clone() else {
            return;
        };
        if !self
            .review_navigation
            .accepts(pending.token, pending.generation, &pending.target.file)
        {
            return;
        }
        if !self
            .smart_review
            .file
            .accepts(pending.generation, &pending.target.file)
        {
            return;
        }
        let dataset = match &self.smart_review.diff {
            super::review::LoadState::Idle | super::review::LoadState::Loading => return,
            super::review::LoadState::Failed(error) => {
                self.review_navigation
                    .fail_current(NavigationUnavailable::DiffFailed(error.clone()));
                return;
            }
            super::review::LoadState::Ready(dataset) => dataset,
        };
        let dataset_index = match find_exact_file_pair(&dataset.files, &pending.target.file) {
            Ok(index) => index,
            Err(error) => {
                self.review_navigation.fail_current(error);
                return;
            }
        };
        let exact_file = dataset.files[dataset_index].clone();
        let index = match find_exact_file_pair(&self.raw_files, &pending.target.file) {
            Ok(index) => index,
            Err(error) => {
                self.review_navigation.fail_current(error);
                return;
            }
        };
        self.selected_file_index = index;

        if !pending.source_started {
            if let Some(current) = self.review_navigation.pending.as_mut() {
                current.source_started = true;
            }
            self.process_current_file_for_generation(
                Some((pending.generation, pending.target.file.clone())),
                cx,
            );
            return;
        }
        match &self.smart_review.file.source {
            super::review::LoadState::Idle | super::review::LoadState::Loading => return,
            super::review::LoadState::Failed(error) => {
                self.review_navigation
                    .fail_current(NavigationUnavailable::SourceFailed(error.clone()));
                return;
            }
            super::review::LoadState::Ready(_) => {}
        }
        if !self
            .smart_review
            .file
            .has_ready_cache(&pending.target.file, true)
        {
            return;
        }
        let Some(file) = self.current_file.as_ref() else {
            return;
        };
        let requested = match self.view_mode {
            DiffViewMode::Unified => EvidenceView::Unified,
            DiffViewMode::SideBySide => EvidenceView::Split,
        };
        let view = evidence_view(requested, &exact_file);
        let side = pending.target.navigation.side;
        let line = pending.target.navigation.line.get();
        let mapped = match view {
            EvidenceView::Unified => map_unified_row(
                &file.items,
                side,
                line,
                file.old_line_count,
                file.new_line_count,
            ),
            EvidenceView::Split => map_split_row(
                &self.side_by_side_lines,
                side,
                line,
                file.old_line_count,
                file.new_line_count,
            ),
        };
        let mapped = match mapped {
            Ok(EvidenceRow::Hidden {
                old_range,
                new_range,
            }) => {
                if let Err(error) = self.expand_context_by_range_checked(old_range, new_range, cx) {
                    self.review_navigation.fail_current(error);
                    return;
                }
                let Some(file) = self.current_file.as_ref() else {
                    self.review_navigation
                        .fail_current(NavigationUnavailable::MissingCurrentFile);
                    return;
                };
                match view {
                    EvidenceView::Unified => map_unified_row(
                        &file.items,
                        side,
                        line,
                        file.old_line_count,
                        file.new_line_count,
                    ),
                    EvidenceView::Split => map_split_row(
                        &self.side_by_side_lines,
                        side,
                        line,
                        file.old_line_count,
                        file.new_line_count,
                    ),
                }
            }
            other => other,
        };
        let row = match mapped {
            Ok(EvidenceRow::Visible { row, .. }) => row,
            Ok(EvidenceRow::Hidden { .. }) => {
                self.review_navigation
                    .fail_current(NavigationUnavailable::LineUnrepresented { side, line });
                return;
            }
            Err(error) => {
                self.review_navigation.fail_current(error);
                return;
            }
        };
        request_strict_center(&self.scroll_handle, row);
        self.review_navigation.finish();
    }

    /// The selected symbol's marker; it stays until another symbol is selected.
    pub(super) fn semantic_highlight_matches(&self, side: ComparisonSide, line: usize) -> bool {
        self.review_marker_matches(side, line)
    }
}

fn preflight_evidence_navigation(
    smart_review: &mut SmartReviewState,
    navigation: &mut ReviewNavigationState,
    target: &EvidenceTarget,
) -> Result<(), NavigationUnavailable> {
    navigation.invalidate();
    smart_review.set_selected_file(target.file.clone());
    validate_evidence_target(target).inspect_err(|error| {
        navigation.unavailable = Some(error.clone());
    })
}

fn request_strict_center(handle: &UniformListScrollHandle, row: usize) {
    handle.scroll_to_item_strict(row, ScrollStrategy::Center);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EvidenceTarget {
    pub(crate) file: ReviewFileKey,
    pub(crate) navigation: ReviewNavigationTarget,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NavigationUnavailable {
    MissingSide { side: ComparisonSide },
    PathMismatch,
    DiffFailed(String),
    SourceFailed(String),
    MissingFilePair,
    DuplicateFilePair,
    MissingCurrentFile,
    MissingExpander,
    DuplicateExpander,
    NotAnExpander,
    InvalidExpander,
    AsymmetricExpander,
    SourceRangeUnavailable,
    LineOutOfRange { side: ComparisonSide, line: u32 },
    LineUnrepresented { side: ComparisonSide, line: u32 },
    DuplicateLine { side: ComparisonSide, line: u32 },
}

impl fmt::Display for NavigationUnavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSide { side } => write!(formatter, "{side:?} side is absent"),
            Self::PathMismatch => {
                formatter.write_str("evidence path does not match the exact pair")
            }
            Self::DiffFailed(error) => write!(formatter, "exact diff failed: {error}"),
            Self::SourceFailed(error) => write!(formatter, "exact source failed: {error}"),
            Self::MissingFilePair => formatter.write_str("exact file pair is absent from the diff"),
            Self::DuplicateFilePair => {
                formatter.write_str("exact file pair is ambiguous in the diff")
            }
            Self::MissingCurrentFile => formatter.write_str("exact file display is not ready"),
            Self::MissingExpander => formatter.write_str("hidden context expander is absent"),
            Self::DuplicateExpander => formatter.write_str("hidden context expander is ambiguous"),
            Self::NotAnExpander => formatter.write_str("the selected row is not hidden context"),
            Self::InvalidExpander => formatter.write_str("hidden context has an invalid range"),
            Self::AsymmetricExpander => {
                formatter.write_str("hidden context has asymmetric old/new ranges")
            }
            Self::SourceRangeUnavailable => {
                formatter.write_str("hidden context exceeds exact source contents")
            }
            Self::LineOutOfRange { side, line } => {
                write!(formatter, "{side:?} line {line} is outside exact source")
            }
            Self::LineUnrepresented { side, line } => {
                write!(
                    formatter,
                    "{side:?} line {line} is not represented in this diff"
                )
            }
            Self::DuplicateLine { side, line } => {
                write!(formatter, "{side:?} line {line} occurs more than once")
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EvidenceRow {
    Visible {
        row: usize,
        side: ComparisonSide,
    },
    Hidden {
        old_range: (usize, usize),
        new_range: (usize, usize),
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EvidenceView {
    Unified,
    Split,
}

pub(crate) fn validate_evidence_target(
    target: &EvidenceTarget,
) -> Result<(), NavigationUnavailable> {
    let expected = match target.navigation.side {
        ComparisonSide::Base => {
            target
                .file
                .old_path
                .as_deref()
                .ok_or(NavigationUnavailable::MissingSide {
                    side: ComparisonSide::Base,
                })?
        }
        ComparisonSide::Head => {
            target
                .file
                .new_path
                .as_deref()
                .ok_or(NavigationUnavailable::MissingSide {
                    side: ComparisonSide::Head,
                })?
        }
    };
    if expected != target.navigation.path {
        return Err(NavigationUnavailable::PathMismatch);
    }
    Ok(())
}

pub(crate) fn find_exact_file_pair(
    files: &[FileDiff],
    key: &ReviewFileKey,
) -> Result<usize, NavigationUnavailable> {
    let mut matches = files
        .iter()
        .enumerate()
        .filter(|(_, file)| file.old_path == key.old_path && file.new_path == key.new_path)
        .map(|(index, _)| index);
    let first = matches
        .next()
        .ok_or(NavigationUnavailable::MissingFilePair)?;
    if matches.next().is_some() {
        return Err(NavigationUnavailable::DuplicateFilePair);
    }
    Ok(first)
}

pub(crate) fn evidence_view(requested: EvidenceView, file: &FileDiff) -> EvidenceView {
    if file.old_path.is_none() || file.new_path.is_none() {
        EvidenceView::Unified
    } else {
        requested
    }
}

pub(crate) fn map_unified_row(
    items: &[DisplayItem],
    side: ComparisonSide,
    line: u32,
    old_line_count: usize,
    new_line_count: usize,
) -> Result<EvidenceRow, NavigationUnavailable> {
    validate_line_range(side, line, old_line_count, new_line_count)?;
    let line =
        usize::try_from(line).map_err(|_| NavigationUnavailable::LineOutOfRange { side, line })?;
    let mut visible = None;
    let mut hidden = None;
    for (row, item) in items.iter().enumerate() {
        match item {
            DisplayItem::Line(item) if line_on_side(item, side) == Some(line) => {
                if visible.replace(row).is_some() {
                    return Err(NavigationUnavailable::DuplicateLine {
                        side,
                        line: u32::try_from(line).unwrap_or(u32::MAX),
                    });
                }
            }
            DisplayItem::Expander(expander) if expander_contains(expander, side, line) => {
                if hidden.replace(expander.clone()).is_some() {
                    return Err(NavigationUnavailable::DuplicateLine {
                        side,
                        line: u32::try_from(line).unwrap_or(u32::MAX),
                    });
                }
            }
            DisplayItem::Line(_) | DisplayItem::Expander(_) => {}
        }
    }
    match (visible, hidden) {
        (Some(row), None) => Ok(EvidenceRow::Visible { row, side }),
        (None, Some(expander)) => Ok(EvidenceRow::Hidden {
            old_range: expander.old_range,
            new_range: expander.new_range,
        }),
        (Some(_), Some(_)) => Err(NavigationUnavailable::DuplicateLine {
            side,
            line: u32::try_from(line).unwrap_or(u32::MAX),
        }),
        (None, None) => Err(NavigationUnavailable::LineUnrepresented {
            side,
            line: u32::try_from(line).unwrap_or(u32::MAX),
        }),
    }
}

pub(crate) fn map_split_row(
    rows: &[SideBySideLine],
    side: ComparisonSide,
    line: u32,
    old_line_count: usize,
    new_line_count: usize,
) -> Result<EvidenceRow, NavigationUnavailable> {
    validate_line_range(side, line, old_line_count, new_line_count)?;
    let line =
        usize::try_from(line).map_err(|_| NavigationUnavailable::LineOutOfRange { side, line })?;
    let mut visible = None;
    let mut hidden = None;
    for (row, item) in rows.iter().enumerate() {
        let content = match side {
            ComparisonSide::Base => item.left.as_ref(),
            ComparisonSide::Head => item.right.as_ref(),
        };
        if content.is_some_and(|content| content.line_num == line) && visible.replace(row).is_some()
        {
            return Err(NavigationUnavailable::DuplicateLine {
                side,
                line: u32::try_from(line).unwrap_or(u32::MAX),
            });
        }
        if let Some(expander) = item
            .expander
            .as_ref()
            .filter(|expander| expander_contains(expander, side, line))
            && hidden.replace(expander.clone()).is_some()
        {
            return Err(NavigationUnavailable::DuplicateLine {
                side,
                line: u32::try_from(line).unwrap_or(u32::MAX),
            });
        }
    }
    match (visible, hidden) {
        (Some(row), None) => Ok(EvidenceRow::Visible { row, side }),
        (None, Some(expander)) => Ok(EvidenceRow::Hidden {
            old_range: expander.old_range,
            new_range: expander.new_range,
        }),
        (Some(_), Some(_)) => Err(NavigationUnavailable::DuplicateLine {
            side,
            line: u32::try_from(line).unwrap_or(u32::MAX),
        }),
        (None, None) => Err(NavigationUnavailable::LineUnrepresented {
            side,
            line: u32::try_from(line).unwrap_or(u32::MAX),
        }),
    }
}

pub(crate) fn validate_expander(
    expander: &ExpanderRow,
    old_line_count: usize,
    new_line_count: usize,
    old_source_lines: usize,
    new_source_lines: usize,
) -> Result<usize, NavigationUnavailable> {
    let (old_start, old_end) = expander.old_range;
    let (new_start, new_end) = expander.new_range;
    if old_start == 0 || new_start == 0 || old_end < old_start || new_end < new_start {
        return Err(NavigationUnavailable::InvalidExpander);
    }
    let old_len = old_end
        .checked_sub(old_start)
        .and_then(|length| length.checked_add(1))
        .ok_or(NavigationUnavailable::InvalidExpander)?;
    let new_len = new_end
        .checked_sub(new_start)
        .and_then(|length| length.checked_add(1))
        .ok_or(NavigationUnavailable::InvalidExpander)?;
    if old_len != new_len {
        return Err(NavigationUnavailable::AsymmetricExpander);
    }
    if old_end > old_line_count
        || new_end > new_line_count
        || old_end > old_source_lines
        || new_end > new_source_lines
    {
        return Err(NavigationUnavailable::SourceRangeUnavailable);
    }
    Ok(old_len)
}

fn validate_line_range(
    side: ComparisonSide,
    line: u32,
    old_line_count: usize,
    new_line_count: usize,
) -> Result<(), NavigationUnavailable> {
    let count = match side {
        ComparisonSide::Base => old_line_count,
        ComparisonSide::Head => new_line_count,
    };
    if line == 0 || usize::try_from(line).map_or(true, |line| line > count) {
        return Err(NavigationUnavailable::LineOutOfRange { side, line });
    }
    Ok(())
}

fn line_on_side(line: &super::types::DisplayLine, side: ComparisonSide) -> Option<usize> {
    match side {
        ComparisonSide::Base => line.old_line_num,
        ComparisonSide::Head => line.new_line_num,
    }
}

fn expander_contains(expander: &ExpanderRow, side: ComparisonSide, line: usize) -> bool {
    let (start, end) = match side {
        ComparisonSide::Base => expander.old_range,
        ComparisonSide::Head => expander.new_range,
    };
    start <= line && line <= end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_viewer::side_by_side::to_side_by_side;
    use crate::diff_viewer::types::DisplayLine;
    use okena_git::DiffLineType;
    use std::num::NonZeroU32;

    fn line(kind: DiffLineType, old: Option<usize>, new: Option<usize>) -> DisplayItem {
        DisplayItem::Line(DisplayLine {
            line_type: kind,
            old_line_num: old,
            new_line_num: new,
            spans: Vec::new(),
            plain_text: String::new(),
        })
    }

    fn target(key: ReviewFileKey, side: ComparisonSide, path: &str, line: u32) -> EvidenceTarget {
        EvidenceTarget {
            file: key,
            navigation: ReviewNavigationTarget {
                path: path.into(),
                side,
                line: NonZeroU32::new(line).unwrap(),
                byte_offset: None,
                symbol_context: None,
            },
        }
    }

    #[test]
    fn unified_maps_removed_added_and_context_by_exact_side() {
        let items = vec![
            line(DiffLineType::Removed, Some(2), None),
            line(DiffLineType::Added, None, Some(2)),
            line(DiffLineType::Context, Some(3), Some(3)),
        ];
        assert_eq!(
            map_unified_row(&items, ComparisonSide::Base, 2, 3, 3).unwrap(),
            EvidenceRow::Visible {
                row: 0,
                side: ComparisonSide::Base
            }
        );
        assert_eq!(
            map_unified_row(&items, ComparisonSide::Head, 2, 3, 3).unwrap(),
            EvidenceRow::Visible {
                row: 1,
                side: ComparisonSide::Head
            }
        );
        assert_eq!(
            map_unified_row(&items, ComparisonSide::Head, 3, 3, 3).unwrap(),
            EvidenceRow::Visible {
                row: 2,
                side: ComparisonSide::Head
            }
        );
    }

    #[test]
    fn split_scans_left_and_right_rows_directly() {
        let items = vec![
            line(DiffLineType::Removed, Some(2), None),
            line(DiffLineType::Added, None, Some(2)),
        ];
        let rows = to_side_by_side(&items);
        assert_eq!(
            map_split_row(&rows, ComparisonSide::Base, 2, 2, 2).unwrap(),
            EvidenceRow::Visible {
                row: 0,
                side: ComparisonSide::Base
            }
        );
        assert_eq!(
            map_split_row(&rows, ComparisonSide::Head, 2, 2, 2).unwrap(),
            EvidenceRow::Visible {
                row: 0,
                side: ComparisonSide::Head
            }
        );
    }

    #[test]
    fn hidden_rows_and_invalid_ranges_are_explicit() {
        let expander = ExpanderRow {
            old_range: (4, 6),
            new_range: (5, 7),
        };
        let items = vec![DisplayItem::Expander(expander.clone())];
        assert_eq!(
            map_unified_row(&items, ComparisonSide::Head, 6, 10, 10).unwrap(),
            EvidenceRow::Hidden {
                old_range: (4, 6),
                new_range: (5, 7)
            }
        );
        assert_eq!(validate_expander(&expander, 10, 10, 10, 10), Ok(3));
        assert_eq!(
            validate_expander(
                &ExpanderRow {
                    old_range: (1, 2),
                    new_range: (1, 3)
                },
                10,
                10,
                10,
                10
            ),
            Err(NavigationUnavailable::AsymmetricExpander)
        );
        assert_eq!(
            validate_expander(
                &ExpanderRow {
                    old_range: (0, 2),
                    new_range: (1, 3)
                },
                10,
                10,
                10,
                10
            ),
            Err(NavigationUnavailable::InvalidExpander)
        );
    }

    #[test]
    fn add_delete_force_unified_and_exact_pairs_disambiguate() {
        let added = FileDiff {
            old_path: None,
            new_path: Some("new.rs".into()),
            hunks: Vec::new(),
            is_binary: false,
            lines_added: 1,
            lines_removed: 0,
        };
        assert_eq!(
            evidence_view(EvidenceView::Split, &added),
            EvidenceView::Unified
        );
        let files = vec![added.clone(), added];
        let key = ReviewFileKey {
            old_path: None,
            new_path: Some("new.rs".into()),
        };
        assert_eq!(
            find_exact_file_pair(&files, &key),
            Err(NavigationUnavailable::DuplicateFilePair)
        );
        assert_eq!(
            find_exact_file_pair(&[], &key),
            Err(NavigationUnavailable::MissingFilePair)
        );
    }

    #[test]
    fn renamed_targets_validate_the_exact_side_path() {
        let key = ReviewFileKey {
            old_path: Some("old.rs".into()),
            new_path: Some("new.rs".into()),
        };
        assert_eq!(
            validate_evidence_target(&target(key.clone(), ComparisonSide::Base, "old.rs", 2)),
            Ok(())
        );
        assert_eq!(
            validate_evidence_target(&target(key.clone(), ComparisonSide::Head, "new.rs", 3)),
            Ok(())
        );
        assert_eq!(
            validate_evidence_target(&target(key, ComparisonSide::Base, "new.rs", 2)),
            Err(NavigationUnavailable::PathMismatch)
        );
        assert_eq!(
            validate_evidence_target(&target(
                ReviewFileKey {
                    old_path: None,
                    new_path: Some("new.rs".into()),
                },
                ComparisonSide::Base,
                "new.rs",
                1
            )),
            Err(NavigationUnavailable::MissingSide {
                side: ComparisonSide::Base
            })
        );
    }

    #[test]
    fn invalid_preflight_keeps_the_exact_pair_visible() {
        let key = ReviewFileKey {
            old_path: None,
            new_path: Some("new.rs".into()),
        };
        let invalid = target(key.clone(), ComparisonSide::Base, "new.rs", 1);
        let mut smart_review = SmartReviewState::default();
        let mut navigation = ReviewNavigationState::default();

        assert_eq!(
            preflight_evidence_navigation(&mut smart_review, &mut navigation, &invalid),
            Err(NavigationUnavailable::MissingSide {
                side: ComparisonSide::Base,
            })
        );
        assert_eq!(smart_review.selected_file.as_ref(), Some(&key));
        assert_eq!(
            navigation.unavailable,
            Some(NavigationUnavailable::MissingSide {
                side: ComparisonSide::Base,
            })
        );
    }

    #[test]
    fn center_request_is_strict_and_deferred() {
        let handle = UniformListScrollHandle::new();
        request_strict_center(&handle, 17);
        let state = handle.0.borrow();
        let request = state.deferred_scroll_to_item.as_ref().unwrap();
        assert_eq!(request.item_index, 17);
        assert_eq!(request.strategy, ScrollStrategy::Center);
        assert!(request.scroll_strict);
    }

    #[test]
    fn hidden_rows_work_before_between_and_after_hunks() {
        let items = vec![
            DisplayItem::Expander(ExpanderRow {
                old_range: (1, 2),
                new_range: (1, 2),
            }),
            line(DiffLineType::Context, Some(3), Some(3)),
            DisplayItem::Expander(ExpanderRow {
                old_range: (4, 5),
                new_range: (4, 5),
            }),
            line(DiffLineType::Context, Some(6), Some(6)),
            DisplayItem::Expander(ExpanderRow {
                old_range: (7, 8),
                new_range: (7, 8),
            }),
        ];
        for (line, range) in [(1, (1, 2)), (5, (4, 5)), (8, (7, 8))] {
            assert_eq!(
                map_unified_row(&items, ComparisonSide::Head, line, 8, 8),
                Ok(EvidenceRow::Hidden {
                    old_range: range,
                    new_range: range,
                })
            );
        }
        assert_eq!(
            map_unified_row(&items, ComparisonSide::Head, 9, 8, 8),
            Err(NavigationUnavailable::LineOutOfRange {
                side: ComparisonSide::Head,
                line: 9,
            })
        );
    }

    #[test]
    fn unrepresented_and_source_overflow_are_not_guessed() {
        assert_eq!(
            map_unified_row(&[], ComparisonSide::Base, 2, 3, 3),
            Err(NavigationUnavailable::LineUnrepresented {
                side: ComparisonSide::Base,
                line: 2,
            })
        );
        assert_eq!(
            validate_expander(
                &ExpanderRow {
                    old_range: (2, 4),
                    new_range: (2, 4),
                },
                4,
                4,
                3,
                4,
            ),
            Err(NavigationUnavailable::SourceRangeUnavailable)
        );
    }

    #[test]
    fn newer_tokens_and_generations_retire_the_pending_navigation() {
        let key = ReviewFileKey {
            old_path: Some("a.rs".into()),
            new_path: Some("a.rs".into()),
        };
        let mut file = super::super::review::FileViewState::default();
        let generation = file.begin(key.clone());
        let mut state = ReviewNavigationState::default();
        let first = state.begin(
            generation,
            target(key.clone(), ComparisonSide::Head, "a.rs", 2),
        );
        assert!(state.accepts(first, generation, &key));

        let second_generation = file.begin(key.clone());
        let second = state.begin(
            second_generation,
            target(key.clone(), ComparisonSide::Base, "a.rs", 3),
        );
        assert!(!state.accepts(first, generation, &key));
        assert!(state.accepts(second, second_generation, &key));

        state.finish();
        assert!(!state.has_pending());
        assert!(!state.accepts(second, second_generation, &key));
    }
}
