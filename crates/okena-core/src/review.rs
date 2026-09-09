//! Wire types for the review composition: what a comparison is made of.
//!
//! The daemon classifies every changed file and measures how much of the change
//! is implementation as opposed to what supports it. Clients only render this.

use serde::{Deserialize, Serialize};

/// What a changed file is for. Decided from its path alone, by one rule.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum FileRole {
    Implementation,
    Test,
    Fixture,
    Snapshot,
    Example,
    Documentation,
    Generated,
    Vendored,
    Lockfile,
    Configuration,
    #[default]
    Unclassified,
}

impl FileRole {
    /// Every role, in the order the legend lists them.
    pub const ALL: [Self; 11] = [
        Self::Implementation,
        Self::Test,
        Self::Fixture,
        Self::Snapshot,
        Self::Example,
        Self::Documentation,
        Self::Generated,
        Self::Vendored,
        Self::Lockfile,
        Self::Configuration,
        Self::Unclassified,
    ];

    /// Display name, as it reaches the screen.
    pub fn label(self) -> &'static str {
        match self {
            Self::Implementation => "Implementation",
            Self::Test => "Tests",
            Self::Fixture => "Fixtures",
            Self::Snapshot => "Snapshots",
            Self::Example => "Examples",
            Self::Documentation => "Docs",
            Self::Generated => "Generated",
            Self::Vendored => "Vendored",
            Self::Lockfile => "Lockfiles",
            Self::Configuration => "Config",
            Self::Unclassified => "Unclassified",
        }
    }

    /// Whether the role carries the change itself. Unclassified counts as
    /// implementation: an unrecognized path is more likely code than support.
    pub fn is_implementation(self) -> bool {
        matches!(self, Self::Implementation | Self::Unclassified)
    }
}

/// The role a file was given, and the rule that gave it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileClassification {
    pub role: FileRole,
    /// Stable id of the matching rule, for "why this role".
    pub rule_id: String,
}

/// How far structural analysis got with one file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAnalysis {
    /// Nothing to analyze: binary, or no supported language.
    #[default]
    Skipped,
    /// The whole file was read.
    Analyzed,
    /// A limit stopped the read; counts from this file are lower bounds.
    Partial,
    /// Worth analyzing, but the run's file budget ended before it.
    NotReached,
    /// The file's content could not be loaded.
    Unavailable,
}

/// One changed file, with its role and what analysis found inside it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompositionFile {
    /// Head path when there is one, else the base path.
    pub path: String,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub role: FileRole,
    pub rule_id: String,
    pub added: u64,
    pub deleted: u64,
    pub is_binary: bool,
    /// Changed lines that sit inside a test scope of this file. Only ever
    /// non-zero for a file whose own role is not Test.
    pub inline_test_lines: u64,
    pub analysis: FileAnalysis,
}

impl CompositionFile {
    /// Added plus deleted — the size of the change, not its net effect.
    pub fn changed_lines(&self) -> u64 {
        self.added.saturating_add(self.deleted)
    }
}

/// One row of the composition: how much of the change this role accounts for.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoleVolume {
    pub role: FileRole,
    /// Files classified into this role.
    pub files: usize,
    /// Changed lines attributed here, after inline test scopes move to Tests.
    pub changed_lines: u64,
    /// Share of the comparison's changed lines, 0.0 to 100.0.
    pub percent: f32,
    /// Of `changed_lines`, how many came from files of another role. Only
    /// Tests can borrow, and only from inline test scopes.
    pub borrowed_lines: u64,
}

/// Comparison-wide totals, before any role split.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompositionTotals {
    pub files: usize,
    pub added: u64,
    pub deleted: u64,
    pub changed_lines: u64,
}

/// What structural analysis actually reached. Counts derived from it are lower
/// bounds unless this says the run was complete.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisCoverage {
    /// Files read in full.
    pub analyzed: usize,
    /// Files with a supported language — the denominator.
    pub candidates: usize,
    /// Files a limit cut short.
    pub partial: usize,
    /// Files the run's file budget never got to.
    pub not_reached: usize,
    /// Files whose content could not be loaded.
    pub unavailable: usize,
    /// Languages seen, in the order they were first met.
    pub languages: Vec<String>,
}

impl AnalysisCoverage {
    /// Whether every candidate file was read in full.
    pub fn is_complete(&self) -> bool {
        self.analyzed == self.candidates
    }
}

/// What a comparison is made of.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChangeComposition {
    pub files: Vec<CompositionFile>,
    /// Roles with at least one file or line, largest first.
    pub roles: Vec<RoleVolume>,
    pub totals: CompositionTotals,
    pub coverage: AnalysisCoverage,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_role_has_a_label_and_appears_in_all() {
        assert_eq!(FileRole::ALL.len(), 11);
        for role in FileRole::ALL {
            assert!(!role.label().is_empty());
        }
    }

    #[test]
    fn unclassified_ranks_with_implementation() {
        assert!(FileRole::Implementation.is_implementation());
        assert!(FileRole::Unclassified.is_implementation());
        assert!(!FileRole::Test.is_implementation());
        assert!(!FileRole::Documentation.is_implementation());
    }

    #[test]
    fn changed_lines_add_both_sides() {
        let file = CompositionFile {
            path: "src/main.rs".into(),
            old_path: None,
            new_path: Some("src/main.rs".into()),
            role: FileRole::Implementation,
            rule_id: "builtin.path.implementation.v1".into(),
            added: 10,
            deleted: 4,
            is_binary: false,
            inline_test_lines: 0,
            analysis: FileAnalysis::Analyzed,
        };
        assert_eq!(file.changed_lines(), 14);
    }

    #[test]
    fn coverage_is_complete_only_when_every_candidate_was_read_in_full() {
        let full = AnalysisCoverage {
            analyzed: 3,
            candidates: 3,
            ..AnalysisCoverage::default()
        };
        assert!(full.is_complete());
        // The states are exclusive, so anything but Analyzed leaves a shortfall.
        assert!(
            !AnalysisCoverage {
                analyzed: 2,
                partial: 1,
                ..full.clone()
            }
            .is_complete()
        );
        assert!(
            !AnalysisCoverage {
                analyzed: 2,
                not_reached: 1,
                ..full
            }
            .is_complete()
        );
    }

    #[test]
    fn roles_survive_a_serde_round_trip_by_name() {
        let json = serde_json::to_string(&FileRole::Documentation).expect("role serializes");
        assert_eq!(json, "\"documentation\"");
        let back: FileRole = serde_json::from_str(&json).expect("role deserializes");
        assert_eq!(back, FileRole::Documentation);
    }
}
