//! Client-side review UI state — spec §4. Filters live here, never in the model.
// Frozen surface: the wave-1 view units read these fields.
#![allow(dead_code)]

use super::super::DiffViewer;
use super::super::review::ReviewFileKey;
use super::labels::role_short;
use super::model::{AttentionTarget, FileEntry, ReasonKind, ReviewModel};
use gpui::{AppContext as _, Context, Entity, UniformListScrollHandle};
use okena_core::review::{ComparisonSide, FileRole, ReviewFileStatus};
use okena_ui::simple_input::{InputChangedEvent, SimpleInputState};
use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

/// Residual lines at or below which a rename is treated as a mechanical move.
pub(crate) const MECHANICAL_RESIDUAL_LINES: u64 = 20;

/// All roles in the order they are offered in the Roles menu.
pub(crate) const ALL_ROLES: [FileRole; 11] = [
    FileRole::Implementation,
    FileRole::Test,
    FileRole::Fixture,
    FileRole::Snapshot,
    FileRole::Example,
    FileRole::Documentation,
    FileRole::Configuration,
    FileRole::Lockfile,
    FileRole::Generated,
    FileRole::Vendored,
    FileRole::Unclassified,
];

fn role_bit(role: FileRole) -> u16 {
    let index = ALL_ROLES
        .iter()
        .position(|candidate| *candidate == role)
        .unwrap_or(0);
    1u16 << index
}

/// Set of [`FileRole`]s. A bit set, because `FileRole` is neither `Ord` nor `Hash`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RoleSet(u16);

impl RoleSet {
    pub(crate) fn empty() -> Self {
        Self(0)
    }

    pub(crate) fn all() -> Self {
        Self::from_roles(ALL_ROLES)
    }

    pub(crate) fn from_roles(roles: impl IntoIterator<Item = FileRole>) -> Self {
        roles
            .into_iter()
            .fold(Self::empty(), |set, role| set.with(role))
    }

    pub(crate) fn with(self, role: FileRole) -> Self {
        Self(self.0 | role_bit(role))
    }

    pub(crate) fn without(self, role: FileRole) -> Self {
        Self(self.0 & !role_bit(role))
    }

    pub(crate) fn contains(self, role: FileRole) -> bool {
        self.0 & role_bit(role) != 0
    }

    pub(crate) fn toggled(self, role: FileRole) -> Self {
        if self.contains(role) {
            self.without(role)
        } else {
            self.with(role)
        }
    }

    pub(crate) fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub(crate) fn len(self) -> usize {
        usize::try_from(self.0.count_ones()).unwrap_or(0)
    }

    /// Members in [`ALL_ROLES`] order.
    pub(crate) fn iter(self) -> impl Iterator<Item = FileRole> {
        ALL_ROLES
            .into_iter()
            .filter(move |role| self.contains(*role))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum NavigatorMode {
    #[default]
    Files,
    Attention,
}

/// The open file is `SmartReviewState::selected_file`; this only picks the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ContentView {
    #[default]
    Overview,
    File,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum FocusRegion {
    #[default]
    Navigator,
    Content,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RolePreset {
    #[default]
    Everything,
    ReviewCode,
    Supporting,
    Custom,
}

impl RolePreset {
    /// The role set a preset stands for; `Custom` has none.
    pub(crate) fn roles(self) -> Option<RoleSet> {
        match self {
            Self::Everything => Some(RoleSet::all()),
            Self::ReviewCode => Some(RoleSet::from_roles([
                FileRole::Implementation,
                FileRole::Configuration,
                FileRole::Unclassified,
            ])),
            Self::Supporting => Some(RoleSet::from_roles([
                FileRole::Test,
                FileRole::Fixture,
                FileRole::Snapshot,
                FileRole::Example,
                FileRole::Documentation,
            ])),
            Self::Custom => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Everything => "Everything",
            Self::ReviewCode => "Review code",
            Self::Supporting => "Supporting",
            Self::Custom => "Custom",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RoleFilter {
    pub roles: RoleSet,
    pub preset: RolePreset,
    pub likely_mechanical_only: bool,
    pub not_analyzed_only: bool,
}

impl Default for RoleFilter {
    fn default() -> Self {
        Self::everything()
    }
}

impl RoleFilter {
    pub(crate) fn everything() -> Self {
        Self::preset(RolePreset::Everything)
    }

    pub(crate) fn preset(preset: RolePreset) -> Self {
        Self {
            roles: preset.roles().unwrap_or_else(RoleSet::all),
            preset,
            likely_mechanical_only: false,
            not_analyzed_only: false,
        }
    }

    pub(crate) fn is_everything(&self) -> bool {
        self.preset == RolePreset::Everything
            && !self.likely_mechanical_only
            && !self.not_analyzed_only
    }

    /// Flip one role; the preset follows the resulting set.
    pub(crate) fn toggle(&mut self, role: FileRole) {
        self.roles = self.roles.toggled(role);
        self.preset = preset_for(self.roles);
    }

    pub(crate) fn allows(&self, entry: &FileEntry) -> bool {
        if !self.roles.contains(entry.role) {
            return false;
        }
        if self.likely_mechanical_only && !is_likely_mechanical(entry) {
            return false;
        }
        if self.not_analyzed_only && entry.analysis.is_analyzed() {
            return false;
        }
        true
    }

    /// Roles-button text: the preset name, or up to three short role names.
    pub(crate) fn label(&self) -> String {
        match self.preset {
            RolePreset::Everything => format!("all {}", ALL_ROLES.len()),
            RolePreset::ReviewCode | RolePreset::Supporting => self.preset.label().to_string(),
            RolePreset::Custom => {
                if self.roles.is_empty() {
                    return "none".to_string();
                }
                let shown: Vec<&str> = self.roles.iter().take(3).map(role_short).collect();
                let rest = self.roles.len().saturating_sub(shown.len());
                let joined = shown.join(" + ");
                if rest == 0 {
                    joined
                } else {
                    format!("{joined} +{rest}")
                }
            }
        }
    }
}

fn preset_for(roles: RoleSet) -> RolePreset {
    [
        RolePreset::Everything,
        RolePreset::ReviewCode,
        RolePreset::Supporting,
    ]
    .into_iter()
    .find(|preset| preset.roles() == Some(roles))
    .unwrap_or(RolePreset::Custom)
}

/// A rename small enough that the move itself is the whole change.
pub(crate) fn is_likely_mechanical(entry: &FileEntry) -> bool {
    entry.status == ReviewFileStatus::Renamed && entry.changed_lines() <= MECHANICAL_RESIDUAL_LINES
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AttentionFilter {
    /// Empty means every reason kind.
    pub kinds: BTreeSet<ReasonKind>,
    pub include_tests: bool,
    pub grouped_by_file: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SymbolRef {
    pub file: ReviewFileKey,
    pub change_index: usize,
}

/// Identity of one navigator row, stable across re-renders.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum NavRowId {
    Dir(String),
    File(ReviewFileKey),
    Item(AttentionTarget),
}

/// Lines the selected symbol occupies; 1-based, inclusive on both ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MarkerSpan {
    pub file: ReviewFileKey,
    pub old: Vec<(u32, u32)>,
    pub new: Vec<(u32, u32)>,
}

impl MarkerSpan {
    pub(crate) fn matches(&self, side: ComparisonSide, line: usize) -> bool {
        let Ok(line) = u32::try_from(line) else {
            return false;
        };
        let ranges = match side {
            ComparisonSide::Base => &self.old,
            ComparisonSide::Head => &self.new,
        };
        ranges
            .iter()
            .any(|(start, end)| *start <= line && line <= *end)
    }
}

pub(crate) struct ReviewUiState {
    pub navigator: NavigatorMode,
    pub content: ContentView,
    pub focus_region: FocusRegion,
    pub nav_cursor: Option<NavRowId>,
    pub selected_symbol: Option<SymbolRef>,
    /// Position in the Attention queue, kept by identity rather than index.
    pub queue_target: Option<AttentionTarget>,
    pub role_filter: RoleFilter,
    pub attention_filter: AttentionFilter,
    pub expanded_dirs: HashSet<String>,
    pub expanded_initialized: bool,
    pub flatten: bool,
    pub details_expanded: bool,
    pub filter_input: Entity<SimpleInputState>,
    pub filter_text: String,
    pub roles_menu_open: bool,
    pub status_popover_open: bool,
    pub outline_open: bool,
    pub help_open: bool,
    /// Overview: commit ledger expanded under the Commits fact.
    pub ledger_open: bool,
    pub model: Option<Arc<ReviewModel>>,
    pub small_change_applied: bool,
    pub marker: Option<MarkerSpan>,
    pub content_width: f32,
    pub tree_scroll: UniformListScrollHandle,
    pub attention_scroll: UniformListScrollHandle,
}

impl ReviewUiState {
    pub(crate) fn new(cx: &mut Context<DiffViewer>) -> Self {
        let filter_input =
            cx.new(|cx| SimpleInputState::new(cx).placeholder("Filter files\u{2026}"));
        cx.subscribe(
            &filter_input,
            |this: &mut DiffViewer, input, _: &InputChangedEvent, cx| {
                this.review_ui.filter_text = input.read(cx).value().to_string();
                cx.notify();
            },
        )
        .detach();
        Self {
            navigator: NavigatorMode::default(),
            content: ContentView::default(),
            focus_region: FocusRegion::default(),
            nav_cursor: None,
            selected_symbol: None,
            queue_target: None,
            role_filter: RoleFilter::everything(),
            attention_filter: AttentionFilter::default(),
            expanded_dirs: HashSet::new(),
            expanded_initialized: false,
            flatten: false,
            details_expanded: false,
            filter_input,
            filter_text: String::new(),
            roles_menu_open: false,
            status_popover_open: false,
            outline_open: false,
            help_open: false,
            ledger_open: false,
            model: None,
            small_change_applied: false,
            marker: None,
            content_width: 0.0,
            tree_scroll: UniformListScrollHandle::new(),
            attention_scroll: UniformListScrollHandle::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ALL_ROLES, ComparisonSide, FileRole, MarkerSpan, RoleFilter, RolePreset, RoleSet,
    };
    use super::super::model::{FileAnalysis, FileEntry, Tier};
    use crate::diff_viewer::review::ReviewFileKey;
    use okena_core::review::ReviewFileStatus;

    fn entry(role: FileRole, status: ReviewFileStatus, churn: u64, analyzed: bool) -> FileEntry {
        FileEntry {
            key: ReviewFileKey {
                old_path: Some("a.rs".into()),
                new_path: Some("a.rs".into()),
            },
            display_path: "a.rs".into(),
            old_path: Some("a.rs".into()),
            new_path: Some("a.rs".into()),
            status,
            role,
            rule_id: "builtin.path.implementation.v1".into(),
            similarity: None,
            lines_added: churn,
            lines_deleted: 0,
            binary: false,
            analysis: if analyzed {
                FileAnalysis::Parsed {
                    language: "Rust".into(),
                }
            } else {
                FileAnalysis::NotInStructure
            },
            reasons: Vec::new(),
            tier: Tier::Rest,
            is_test: role == FileRole::Test,
            symbols: Vec::new(),
            structure_index: None,
        }
    }

    #[test]
    fn everything_admits_every_role() {
        let filter = RoleFilter::everything();
        assert!(filter.is_everything());
        for role in ALL_ROLES {
            assert!(filter.allows(&entry(role, ReviewFileStatus::Modified, 1, true)));
        }
    }

    #[test]
    fn presets_split_review_code_from_supporting() {
        let review = RoleFilter::preset(RolePreset::ReviewCode);
        assert!(review.allows(&entry(
            FileRole::Implementation,
            ReviewFileStatus::Modified,
            1,
            true
        )));
        assert!(!review.allows(&entry(FileRole::Test, ReviewFileStatus::Modified, 1, true)));

        let supporting = RoleFilter::preset(RolePreset::Supporting);
        assert!(supporting.allows(&entry(FileRole::Test, ReviewFileStatus::Modified, 1, true)));
        assert!(!supporting.allows(&entry(
            FileRole::Implementation,
            ReviewFileStatus::Modified,
            1,
            true
        )));
    }

    #[test]
    fn saved_filters_narrow_by_move_size_and_analysis() {
        let mut filter = RoleFilter::everything();
        filter.likely_mechanical_only = true;
        assert!(filter.allows(&entry(
            FileRole::Implementation,
            ReviewFileStatus::Renamed,
            20,
            true
        )));
        assert!(!filter.allows(&entry(
            FileRole::Implementation,
            ReviewFileStatus::Renamed,
            21,
            true
        )));
        assert!(!filter.allows(&entry(
            FileRole::Implementation,
            ReviewFileStatus::Modified,
            1,
            true
        )));

        let mut filter = RoleFilter::everything();
        filter.not_analyzed_only = true;
        assert!(filter.allows(&entry(
            FileRole::Implementation,
            ReviewFileStatus::Modified,
            1,
            false
        )));
        assert!(!filter.allows(&entry(
            FileRole::Implementation,
            ReviewFileStatus::Modified,
            1,
            true
        )));
    }

    #[test]
    fn toggling_roles_moves_between_custom_and_the_matching_preset() {
        let mut filter = RoleFilter::everything();
        filter.toggle(FileRole::Test);
        assert_eq!(filter.preset, RolePreset::Custom);
        assert!(!filter.roles.contains(FileRole::Test));

        filter.toggle(FileRole::Test);
        assert_eq!(filter.preset, RolePreset::Everything);

        let mut filter = RoleFilter {
            roles: RoleSet::from_roles([FileRole::Implementation, FileRole::Configuration]),
            preset: RolePreset::Custom,
            likely_mechanical_only: false,
            not_analyzed_only: false,
        };
        filter.toggle(FileRole::Unclassified);
        assert_eq!(filter.preset, RolePreset::ReviewCode);
    }

    #[test]
    fn labels_name_the_preset_or_the_chosen_roles() {
        assert_eq!(RoleFilter::everything().label(), "all 11");
        assert_eq!(
            RoleFilter::preset(RolePreset::ReviewCode).label(),
            "Review code"
        );
        assert_eq!(
            RoleFilter::preset(RolePreset::Supporting).label(),
            "Supporting"
        );

        let mut filter = RoleFilter::everything();
        filter.roles = RoleSet::from_roles([FileRole::Implementation, FileRole::Configuration]);
        filter.preset = RolePreset::Custom;
        assert_eq!(filter.label(), "Impl + Config");

        filter.roles = RoleSet::from_roles([
            FileRole::Implementation,
            FileRole::Test,
            FileRole::Documentation,
            FileRole::Configuration,
            FileRole::Lockfile,
        ]);
        assert_eq!(filter.label(), "Impl + Tests + Docs +2");

        filter.roles = RoleSet::empty();
        assert_eq!(filter.label(), "none");
    }

    #[test]
    fn marker_covers_every_range_on_the_matching_side() {
        let marker = MarkerSpan {
            file: ReviewFileKey {
                old_path: Some("a.rs".into()),
                new_path: Some("a.rs".into()),
            },
            old: vec![(3, 5), (10, 10)],
            new: vec![(4, 6), (20, 22)],
        };
        assert!(marker.matches(ComparisonSide::Base, 3));
        assert!(marker.matches(ComparisonSide::Base, 5));
        assert!(marker.matches(ComparisonSide::Base, 10));
        assert!(!marker.matches(ComparisonSide::Base, 6));
        assert!(marker.matches(ComparisonSide::Head, 6));
        assert!(marker.matches(ComparisonSide::Head, 21));
        assert!(!marker.matches(ComparisonSide::Head, 10));
    }
}
