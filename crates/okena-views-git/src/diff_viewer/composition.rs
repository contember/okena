//! What the open comparison is made of — the sidebar's composition panel.
//!
//! Pure state and wording. Roles arrive from the daemon; everything here turns
//! them into rows, a filter, and the sentences that keep partial analysis
//! honest. The rendering lives in `composition_render`.

use std::collections::{BTreeSet, HashMap};

use okena_core::review::{ChangeComposition, FileRole, RoleVolume};

/// Which roles the file tree shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum RoleFilter {
    #[default]
    All,
    Only(BTreeSet<FileRole>),
}

/// A named group of roles, offered next to the legend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RolePreset {
    Everything,
    /// What a reviewer reads for correctness.
    ReviewCode,
    /// What supports it.
    Supporting,
}

impl RolePreset {
    pub(super) const ALL: [Self; 3] = [Self::Everything, Self::ReviewCode, Self::Supporting];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Everything => "Everything",
            Self::ReviewCode => "Review code",
            Self::Supporting => "Supporting",
        }
    }

    fn filter(self) -> RoleFilter {
        match self {
            Self::Everything => RoleFilter::All,
            Self::ReviewCode => RoleFilter::Only(
                [
                    FileRole::Implementation,
                    FileRole::Unclassified,
                    FileRole::Configuration,
                ]
                .into(),
            ),
            Self::Supporting => RoleFilter::Only(
                [
                    FileRole::Test,
                    FileRole::Fixture,
                    FileRole::Snapshot,
                    FileRole::Example,
                    FileRole::Documentation,
                ]
                .into(),
            ),
        }
    }
}

/// One legend row. The sidebar is narrow, so the role name owns the first line
/// and everything countable goes on the second.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct LegendRow {
    pub role: FileRole,
    pub label: &'static str,
    pub percent: String,
    /// `22 files, 2 221 lines`, with the borrowed share named when there is one.
    pub detail: String,
    pub selected: bool,
}

/// The composition panel's whole state.
#[derive(Default)]
pub(crate) struct CompositionState {
    pub(super) data: Option<ChangeComposition>,
    pub(super) loading: bool,
    pub(super) error: Option<String>,
    pub(super) filter: RoleFilter,
    /// Display path to role, so filtering never rescans the file list.
    roles: HashMap<String, FileRole>,
}

impl CompositionState {
    /// Replace the composition and reset the filter — a new comparison's roles
    /// are not the old one's.
    pub(super) fn set(&mut self, composition: ChangeComposition) {
        self.roles = composition
            .files
            .iter()
            .map(|file| (file.path.clone(), file.role))
            .collect();
        self.data = Some(composition);
        self.loading = false;
        self.error = None;
        self.filter = RoleFilter::All;
    }

    pub(super) fn fail(&mut self, error: String) {
        self.data = None;
        self.roles.clear();
        self.loading = false;
        self.filter = RoleFilter::All;
        self.error = Some(error);
    }

    pub(super) fn clear(&mut self) {
        *self = Self {
            loading: true,
            ..Self::default()
        };
    }

    /// Whether the file at `path` passes the role filter. A file the daemon
    /// never classified is always shown: hiding it would lose it silently.
    pub(super) fn accepts(&self, path: &str) -> bool {
        match &self.filter {
            RoleFilter::All => true,
            RoleFilter::Only(roles) => self.roles.get(path).is_none_or(|role| roles.contains(role)),
        }
    }

    pub(super) fn is_filtered(&self) -> bool {
        self.filter != RoleFilter::All
    }

    /// Click a legend row: isolate that role, or clear when it already is.
    pub(super) fn toggle(&mut self, role: FileRole) {
        let isolated = BTreeSet::from([role]);
        self.filter = match &self.filter {
            RoleFilter::Only(roles) if *roles == isolated => RoleFilter::All,
            _ => RoleFilter::Only(isolated),
        };
    }

    pub(super) fn apply(&mut self, preset: RolePreset) {
        self.filter = preset.filter();
    }

    pub(super) fn is_active(&self, preset: RolePreset) -> bool {
        self.filter == preset.filter()
    }

    /// Legend rows, largest role first.
    pub(super) fn rows(&self) -> Vec<LegendRow> {
        let Some(data) = &self.data else {
            return Vec::new();
        };
        data.roles.iter().map(|volume| self.row(volume)).collect()
    }

    fn row(&self, volume: &RoleVolume) -> LegendRow {
        LegendRow {
            role: volume.role,
            label: volume.role.label(),
            percent: percent(volume.percent),
            detail: detail(volume, self.is_lower_bound()),
            selected: matches!(&self.filter, RoleFilter::Only(roles) if roles.contains(&volume.role)),
        }
    }

    /// Headline: `385 files · +12 040 −3 118`.
    pub(super) fn headline(&self) -> Option<String> {
        let totals = self.data.as_ref()?.totals;
        Some(format!(
            "{} \u{00B7} +{} \u{2212}{}",
            count(totals.files as u64, "file"),
            group(totals.added),
            group(totals.deleted),
        ))
    }

    /// The caveat under the legend, when analysis did not reach everything.
    ///
    /// Only the inline-test split depends on it — roles and line counts are
    /// exact for every file, always.
    pub(super) fn caveat(&self) -> Option<String> {
        let coverage = &self.data.as_ref()?.coverage;
        if coverage.candidates == 0 || coverage.is_complete() {
            return None;
        }
        let languages = if coverage.languages.is_empty() {
            String::new()
        } else {
            format!(" {}", coverage.languages.join(", "))
        };
        Some(format!(
            "inline tests read in {} of {}{languages} files \u{2014} the split is a lower bound",
            group(coverage.analyzed as u64),
            group(coverage.candidates as u64),
        ))
    }

    /// Whether counts derived from structure are lower bounds.
    pub(super) fn is_lower_bound(&self) -> bool {
        self.data
            .as_ref()
            .is_some_and(|data| data.coverage.candidates > 0 && !data.coverage.is_complete())
    }

    /// Sidebar footer while a filter is on: `113 of 385 files`.
    pub(super) fn filter_summary(&self, visible: usize) -> Option<String> {
        let data = self.data.as_ref()?;
        self.is_filtered().then(|| {
            format!(
                "{} of {}",
                group(visible as u64),
                count(data.totals.files as u64, "file")
            )
        })
    }

    /// Bar segments, in legend order, as (role, share of width).
    pub(super) fn segments(&self) -> Vec<(FileRole, f32)> {
        let Some(data) = &self.data else {
            return Vec::new();
        };
        if data.totals.changed_lines == 0 {
            return Vec::new();
        }
        data.roles
            .iter()
            .filter(|volume| volume.changed_lines > 0)
            .map(|volume| (volume.role, volume.percent / 100.0))
            .collect()
    }
}

/// `22 files, 2 221 lines`. A role holding only borrowed lines says so rather
/// than printing a file count it does not have.
fn detail(volume: &RoleVolume, lower_bound: bool) -> String {
    let mut parts = Vec::with_capacity(3);
    if volume.files > 0 {
        parts.push(count(volume.files as u64, "file"));
    }
    let lines = count(volume.changed_lines, "line");
    parts.push(if lower_bound && volume.role == FileRole::Test {
        format!("\u{2265} {lines}")
    } else {
        lines
    });
    if volume.borrowed_lines > 0 {
        parts.push(if volume.borrowed_lines == volume.changed_lines {
            "in implementation files".to_string()
        } else {
            format!("{} in implementation files", group(volume.borrowed_lines))
        });
    }
    parts.join(" \u{00B7} ")
}

/// `45 %`, or `< 1 %` for a share too small to round up to one.
fn percent(value: f32) -> String {
    if value <= 0.0 {
        return String::new();
    }
    if value < 0.5 {
        return "< 1 %".to_string();
    }
    format!("{value:.0} %")
}

/// `1 file` / `385 files`.
fn count(value: u64, noun: &str) -> String {
    if value == 1 {
        format!("1 {noun}")
    } else {
        format!("{} {noun}s", group(value))
    }
}

/// `15 692` — grouped in threes with a thin space, so long numbers stay readable.
fn group(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push('\u{202F}');
        }
        out.push(character);
    }
    out
}

#[cfg(test)]
mod tests {
    use okena_core::review::{AnalysisCoverage, CompositionFile, CompositionTotals, FileAnalysis};

    use super::*;

    fn file(path: &str, role: FileRole, added: u64, inline: u64) -> CompositionFile {
        CompositionFile {
            path: path.to_string(),
            old_path: Some(path.to_string()),
            new_path: Some(path.to_string()),
            role,
            rule_id: "builtin.path.implementation.v1".into(),
            added,
            deleted: 0,
            is_binary: false,
            inline_test_lines: inline,
            analysis: FileAnalysis::Analyzed,
        }
    }

    fn volume(role: FileRole, files: usize, lines: u64, percent: f32, borrowed: u64) -> RoleVolume {
        RoleVolume {
            role,
            files,
            changed_lines: lines,
            percent,
            borrowed_lines: borrowed,
        }
    }

    fn state() -> CompositionState {
        let mut state = CompositionState::default();
        state.set(ChangeComposition {
            files: vec![
                file("src/engine.rs", FileRole::Implementation, 60, 20),
                file("docs/guide.md", FileRole::Documentation, 40, 0),
            ],
            roles: vec![
                volume(FileRole::Implementation, 1, 40, 40.0, 0),
                volume(FileRole::Documentation, 1, 40, 40.0, 0),
                volume(FileRole::Test, 0, 20, 20.0, 20),
            ],
            totals: CompositionTotals {
                files: 2,
                added: 100,
                deleted: 0,
                changed_lines: 100,
            },
            coverage: AnalysisCoverage {
                analyzed: 1,
                candidates: 1,
                languages: vec!["Rust".into()],
                ..AnalysisCoverage::default()
            },
        });
        state
    }

    #[test]
    fn nothing_is_filtered_until_a_role_is_picked() {
        let state = state();
        assert!(!state.is_filtered());
        assert!(state.accepts("src/engine.rs"));
        assert!(state.accepts("docs/guide.md"));
    }

    #[test]
    fn clicking_a_role_isolates_it_and_clicking_again_clears() {
        let mut state = state();
        state.toggle(FileRole::Implementation);
        assert!(state.is_filtered());
        assert!(state.accepts("src/engine.rs"));
        assert!(!state.accepts("docs/guide.md"));

        state.toggle(FileRole::Implementation);
        assert!(!state.is_filtered());
        assert!(state.accepts("docs/guide.md"));
    }

    #[test]
    fn a_file_with_no_known_role_survives_every_filter() {
        let mut state = state();
        state.toggle(FileRole::Test);
        assert!(state.accepts("some/file/added/after.rs"));
    }

    #[test]
    fn presets_select_groups_of_roles_and_report_themselves_active() {
        let mut state = state();
        state.apply(RolePreset::Supporting);
        assert!(state.is_active(RolePreset::Supporting));
        assert!(!state.is_active(RolePreset::ReviewCode));
        assert!(state.accepts("docs/guide.md"));
        assert!(!state.accepts("src/engine.rs"));

        state.apply(RolePreset::Everything);
        assert!(!state.is_filtered());
        assert!(state.is_active(RolePreset::Everything));
    }

    #[test]
    fn a_row_holding_only_borrowed_lines_names_where_they_came_from() {
        let rows = state().rows();
        let tests = rows
            .iter()
            .find(|row| row.role == FileRole::Test)
            .expect("tests row");
        assert_eq!(tests.detail, "20 lines \u{00B7} in implementation files");
        assert_eq!(tests.percent, "20 %");
    }

    #[test]
    fn a_row_with_files_of_its_own_counts_them_before_its_lines() {
        let rows = state().rows();
        let implementation = rows
            .iter()
            .find(|row| row.role == FileRole::Implementation)
            .expect("implementation row");
        assert_eq!(implementation.detail, "1 file \u{00B7} 40 lines");
    }

    #[test]
    fn complete_coverage_prints_no_caveat() {
        let state = state();
        assert_eq!(state.caveat(), None);
        assert!(!state.is_lower_bound());
    }

    #[test]
    fn partial_coverage_says_the_split_is_a_lower_bound() {
        let mut state = state();
        if let Some(data) = state.data.as_mut() {
            data.coverage.analyzed = 63;
            data.coverage.candidates = 97;
        }
        assert!(state.is_lower_bound());
        assert_eq!(
            state.caveat().as_deref(),
            Some("inline tests read in 63 of 97 Rust files \u{2014} the split is a lower bound")
        );
        // Only the Tests row is derived from structure, so only it is bounded.
        let rows = state.rows();
        let tests = rows
            .iter()
            .find(|row| row.role == FileRole::Test)
            .expect("tests row");
        assert!(tests.detail.starts_with("\u{2265} 20 lines"));
        let implementation = rows
            .iter()
            .find(|row| row.role == FileRole::Implementation)
            .expect("implementation row");
        assert!(!implementation.detail.contains('\u{2265}'));
    }

    #[test]
    fn a_comparison_with_no_analysable_file_has_no_caveat() {
        let mut state = state();
        if let Some(data) = state.data.as_mut() {
            data.coverage = AnalysisCoverage::default();
        }
        assert_eq!(state.caveat(), None);
        assert!(!state.is_lower_bound());
    }

    #[test]
    fn loading_a_new_comparison_drops_the_previous_filter() {
        let mut state = state();
        state.toggle(FileRole::Test);
        state.clear();
        assert!(!state.is_filtered());
        assert!(state.loading);
        assert!(state.data.is_none());
    }

    #[test]
    fn a_failed_load_keeps_the_tree_unfiltered() {
        let mut state = state();
        state.toggle(FileRole::Test);
        state.fail("no such branch".into());
        assert!(!state.is_filtered());
        assert!(state.accepts("anything.rs"));
        assert_eq!(state.error.as_deref(), Some("no such branch"));
    }

    #[test]
    fn the_filter_summary_only_appears_while_filtering() {
        let mut state = state();
        assert_eq!(state.filter_summary(2), None);
        state.toggle(FileRole::Implementation);
        assert_eq!(state.filter_summary(1).as_deref(), Some("1 of 2 files"));
    }

    #[test]
    fn segments_follow_the_legend_and_skip_empty_roles() {
        let segments = state().segments();
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].0, FileRole::Implementation);
        assert!((segments.iter().map(|(_, share)| share).sum::<f32>() - 1.0).abs() < 0.01);
    }

    #[test]
    fn numbers_are_grouped_and_small_shares_never_read_as_zero() {
        assert_eq!(group(15_692), "15\u{202F}692");
        assert_eq!(group(1_234_567), "1\u{202F}234\u{202F}567");
        assert_eq!(group(42), "42");
        assert_eq!(percent(0.2), "< 1 %");
        assert_eq!(percent(0.0), "");
        assert_eq!(percent(45.3), "45 %");
        assert_eq!(count(1, "file"), "1 file");
        assert_eq!(count(0, "file"), "0 files");
    }

    #[test]
    fn the_headline_states_both_sides_of_the_change() {
        assert_eq!(
            state().headline().as_deref(),
            Some("2 files \u{00B7} +100 \u{2212}0")
        );
    }
}
