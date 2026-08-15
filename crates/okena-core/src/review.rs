//! Shared review-workspace wire models.
//!
//! These types describe comparison identity and review facts without depending
//! on Git execution, syntax parsers, transport, or UI code.

use std::fmt;
use std::num::NonZeroU32;

use serde::{Deserialize, Deserializer, Serialize};

use crate::types::DiffMode;

/// A complete SHA-1 or SHA-256 Git object ID.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct GitObjectId(String);

impl GitObjectId {
    pub fn new(value: impl Into<String>) -> Result<Self, ReviewModelError> {
        let value = value.into();
        if !matches!(value.len(), 40 | 64) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ReviewModelError::new(
                "Git object ID must be 40 or 64 hexadecimal characters",
            ));
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for GitObjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for GitObjectId {
    type Error = ReviewModelError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for GitObjectId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// Opaque identity of one resolved review comparison.
///
/// Producers derive this from the strategy and resolved snapshots. Mutable
/// snapshots include their fingerprints, so a changed index or working tree
/// produces a different identity without being described as immutable.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReviewComparisonId(pub String);

/// A source snapshot used by a resolved comparison.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewSnapshot {
    Commit {
        oid: GitObjectId,
    },
    EmptyTree {
        oid: GitObjectId,
    },
    /// A mutable index state identified by the observed-state fingerprint.
    Index {
        fingerprint: String,
    },
    /// A mutable worktree state identified by the observed-state fingerprint.
    WorkingTree {
        fingerprint: String,
    },
}

impl ReviewSnapshot {
    pub fn is_immutable(&self) -> bool {
        matches!(self, Self::Commit { .. } | Self::EmptyTree { .. })
    }

    pub fn oid(&self) -> Option<&GitObjectId> {
        match self {
            Self::Commit { oid } | Self::EmptyTree { oid } => Some(oid),
            Self::Index { .. } | Self::WorkingTree { .. } => None,
        }
    }

    fn has_valid_fingerprint(&self) -> bool {
        match self {
            Self::Index { fingerprint } | Self::WorkingTree { fingerprint } => {
                !fingerprint.is_empty()
            }
            Self::Commit { .. } | Self::EmptyTree { .. } => true,
        }
    }
}

/// The exact rule used to select the two effective snapshots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonStrategy {
    IndexToWorkingTree,
    HeadToIndex,
    ParentToCommit,
    EmptyTreeToCommit,
    MergeBaseToHead,
    DirectBaseToHeadWithoutMergeBase,
}

/// One requested target resolved to stale-detection refs and effective inputs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ResolvedComparisonWire")]
pub struct ResolvedComparison {
    /// User-facing refs used to open the comparison.
    requested: DiffMode,
    /// Resolved requested base tip, before merge-base selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    requested_base_oid: Option<GitObjectId>,
    /// Resolved requested head tip used for stale detection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    requested_head_oid: Option<GitObjectId>,
    strategy: ComparisonStrategy,
    /// Effective old snapshot consumed by diff, source, and syntax analysis.
    base: ReviewSnapshot,
    /// Effective new snapshot consumed by diff, source, and syntax analysis.
    head: ReviewSnapshot,
    /// Full merge-base commit OID for a three-dot comparison.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    merge_base_oid: Option<GitObjectId>,
    identity: ReviewComparisonId,
}

#[derive(Deserialize)]
struct ResolvedComparisonWire {
    requested: DiffMode,
    #[serde(default)]
    requested_base_oid: Option<GitObjectId>,
    #[serde(default)]
    requested_head_oid: Option<GitObjectId>,
    strategy: ComparisonStrategy,
    base: ReviewSnapshot,
    head: ReviewSnapshot,
    #[serde(default)]
    merge_base_oid: Option<GitObjectId>,
    identity: ReviewComparisonId,
}

impl TryFrom<ResolvedComparisonWire> for ResolvedComparison {
    type Error = ReviewModelError;

    fn try_from(value: ResolvedComparisonWire) -> Result<Self, Self::Error> {
        let comparison = Self {
            requested: value.requested,
            requested_base_oid: value.requested_base_oid,
            requested_head_oid: value.requested_head_oid,
            strategy: value.strategy,
            base: value.base,
            head: value.head,
            merge_base_oid: value.merge_base_oid,
            identity: value.identity,
        };
        comparison.validate()?;
        Ok(comparison)
    }
}

impl ResolvedComparison {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        requested: DiffMode,
        requested_base_oid: Option<GitObjectId>,
        requested_head_oid: Option<GitObjectId>,
        strategy: ComparisonStrategy,
        base: ReviewSnapshot,
        head: ReviewSnapshot,
        merge_base_oid: Option<GitObjectId>,
        identity: ReviewComparisonId,
    ) -> Result<Self, ReviewModelError> {
        Self::try_from(ResolvedComparisonWire {
            requested,
            requested_base_oid,
            requested_head_oid,
            strategy,
            base,
            head,
            merge_base_oid,
            identity,
        })
    }

    pub fn is_immutable(&self) -> bool {
        self.base.is_immutable() && self.head.is_immutable()
    }

    pub fn requested(&self) -> &DiffMode {
        &self.requested
    }

    pub fn requested_base_oid(&self) -> Option<&GitObjectId> {
        self.requested_base_oid.as_ref()
    }

    pub fn requested_head_oid(&self) -> Option<&GitObjectId> {
        self.requested_head_oid.as_ref()
    }

    pub fn strategy(&self) -> ComparisonStrategy {
        self.strategy
    }

    pub fn base(&self) -> &ReviewSnapshot {
        &self.base
    }

    pub fn head(&self) -> &ReviewSnapshot {
        &self.head
    }

    pub fn merge_base_oid(&self) -> Option<&GitObjectId> {
        self.merge_base_oid.as_ref()
    }

    pub fn identity(&self) -> &ReviewComparisonId {
        &self.identity
    }

    pub fn validate(&self) -> Result<(), ReviewModelError> {
        if self.identity.0.is_empty() {
            return Err(ReviewModelError::new("comparison identity cannot be empty"));
        }
        if !self.base.has_valid_fingerprint() || !self.head.has_valid_fingerprint() {
            return Err(ReviewModelError::new("mutable fingerprint cannot be empty"));
        }

        match (&self.requested, self.strategy, &self.base, &self.head) {
            (
                DiffMode::WorkingTree,
                ComparisonStrategy::IndexToWorkingTree,
                ReviewSnapshot::Index { .. },
                ReviewSnapshot::WorkingTree { .. },
            ) => {
                self.require_no_merge_base()?;
                self.require_requested_oids(None, None)
            }
            (
                DiffMode::Staged,
                ComparisonStrategy::HeadToIndex,
                ReviewSnapshot::Commit { oid },
                ReviewSnapshot::Index { .. },
            ) => {
                self.require_no_merge_base()?;
                self.require_requested_oids(Some(oid), None)
            }
            (
                DiffMode::Commit(_),
                ComparisonStrategy::ParentToCommit,
                ReviewSnapshot::Commit { oid: base_oid },
                ReviewSnapshot::Commit { oid: head_oid },
            ) => {
                self.require_no_merge_base()?;
                self.require_requested_oids(Some(base_oid), Some(head_oid))
            }
            (
                DiffMode::Commit(_),
                ComparisonStrategy::EmptyTreeToCommit,
                ReviewSnapshot::EmptyTree { .. },
                ReviewSnapshot::Commit { oid: head_oid },
            ) => {
                self.require_no_merge_base()?;
                self.require_requested_oids(None, Some(head_oid))
            }
            (
                DiffMode::BranchCompare { .. },
                ComparisonStrategy::MergeBaseToHead,
                ReviewSnapshot::Commit { oid: base_oid },
                ReviewSnapshot::Commit { oid: head_oid },
            ) => {
                if self.merge_base_oid.as_ref() != Some(base_oid) {
                    return Err(ReviewModelError::new(
                        "merge-base strategy must use the merge-base as effective base",
                    ));
                }
                if self.requested_base_oid.is_none() {
                    return Err(ReviewModelError::new(
                        "branch comparison requires a requested base OID",
                    ));
                }
                self.require_requested_head(head_oid)
            }
            (
                DiffMode::BranchCompare { .. },
                ComparisonStrategy::DirectBaseToHeadWithoutMergeBase,
                ReviewSnapshot::Commit { oid: base_oid },
                ReviewSnapshot::Commit { oid: head_oid },
            ) => {
                self.require_no_merge_base()?;
                self.require_requested_oids(Some(base_oid), Some(head_oid))
            }
            _ => Err(ReviewModelError::new(
                "comparison strategy does not match its requested mode and snapshots",
            )),
        }
    }

    fn require_no_merge_base(&self) -> Result<(), ReviewModelError> {
        if self.merge_base_oid.is_some() {
            Err(ReviewModelError::new(
                "comparison strategy cannot carry a merge-base OID",
            ))
        } else {
            Ok(())
        }
    }

    fn require_requested_oids(
        &self,
        base: Option<&GitObjectId>,
        head: Option<&GitObjectId>,
    ) -> Result<(), ReviewModelError> {
        if self.requested_base_oid.as_ref() != base || self.requested_head_oid.as_ref() != head {
            Err(ReviewModelError::new(
                "requested OIDs do not match the comparison snapshots",
            ))
        } else {
            Ok(())
        }
    }

    fn require_requested_head(&self, head: &GitObjectId) -> Result<(), ReviewModelError> {
        if self.requested_head_oid.as_ref() == Some(head) {
            Ok(())
        } else {
            Err(ReviewModelError::new(
                "effective branch head must match the requested head OID",
            ))
        }
    }
}

/// A comparison proven to contain immutable snapshots only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ImmutableResolvedComparison(ResolvedComparison);

impl ImmutableResolvedComparison {
    pub fn as_resolved(&self) -> &ResolvedComparison {
        &self.0
    }

    pub fn into_resolved(self) -> ResolvedComparison {
        self.0
    }

    pub fn requested(&self) -> &DiffMode {
        self.0.requested()
    }

    pub fn requested_base_oid(&self) -> Option<&GitObjectId> {
        self.0.requested_base_oid()
    }

    pub fn requested_head_oid(&self) -> Option<&GitObjectId> {
        self.0.requested_head_oid()
    }

    pub fn strategy(&self) -> ComparisonStrategy {
        self.0.strategy()
    }

    pub fn base(&self) -> &ReviewSnapshot {
        self.0.base()
    }

    pub fn head(&self) -> &ReviewSnapshot {
        self.0.head()
    }

    pub fn merge_base_oid(&self) -> Option<&GitObjectId> {
        self.0.merge_base_oid()
    }

    pub fn identity(&self) -> &ReviewComparisonId {
        self.0.identity()
    }
}

impl TryFrom<ResolvedComparison> for ImmutableResolvedComparison {
    type Error = ReviewModelError;

    fn try_from(value: ResolvedComparison) -> Result<Self, Self::Error> {
        value.validate()?;
        if !value.is_immutable() {
            return Err(ReviewModelError::new(
                "exact diff and source requests require immutable snapshots",
            ));
        }
        Ok(Self(value))
    }
}

impl<'de> Deserialize<'de> for ImmutableResolvedComparison {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let comparison = ResolvedComparison::deserialize(deserializer)?;
        Self::try_from(comparison).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewModelError(String);

impl ReviewModelError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ReviewModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ReviewModelError {}

/// Origin of a deterministic or syntax-derived review fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum FactProvenance {
    Git,
    RuleDerived { rule_id: String },
    SyntaxDerived { language: String, parser: String },
}

/// Why an analysis result stopped before covering every candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TruncationReason {
    ItemLimit,
    ByteLimit,
    TimeLimit,
    CaptureLimit,
    ResponseLimit,
    Cancelled,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewTruncation {
    pub reason: TruncationReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Inspectable coverage shared by deterministic and syntax-derived analysis.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ReviewCoverageWire")]
pub struct ReviewCoverage {
    total_items: u64,
    analyzed_items: u64,
    pending_items: u64,
    skipped_items: u64,
    unsupported_items: u64,
    failed_items: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    truncation: Option<ReviewTruncation>,
}

#[derive(Deserialize)]
struct ReviewCoverageWire {
    total_items: u64,
    analyzed_items: u64,
    pending_items: u64,
    skipped_items: u64,
    unsupported_items: u64,
    failed_items: u64,
    #[serde(default)]
    truncation: Option<ReviewTruncation>,
}

impl TryFrom<ReviewCoverageWire> for ReviewCoverage {
    type Error = ReviewModelError;

    fn try_from(value: ReviewCoverageWire) -> Result<Self, Self::Error> {
        Self::new(
            value.total_items,
            value.analyzed_items,
            value.pending_items,
            value.skipped_items,
            value.unsupported_items,
            value.failed_items,
            value.truncation,
        )
    }
}

impl ReviewCoverage {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        total_items: u64,
        analyzed_items: u64,
        pending_items: u64,
        skipped_items: u64,
        unsupported_items: u64,
        failed_items: u64,
        truncation: Option<ReviewTruncation>,
    ) -> Result<Self, ReviewModelError> {
        let categorized = analyzed_items
            .checked_add(pending_items)
            .and_then(|count| count.checked_add(skipped_items))
            .and_then(|count| count.checked_add(unsupported_items))
            .and_then(|count| count.checked_add(failed_items));
        if categorized != Some(total_items) {
            return Err(ReviewModelError::new(
                "coverage categories must sum to total_items",
            ));
        }
        Ok(Self {
            total_items,
            analyzed_items,
            pending_items,
            skipped_items,
            unsupported_items,
            failed_items,
            truncation,
        })
    }

    pub fn total_items(&self) -> u64 {
        self.total_items
    }

    pub fn analyzed_items(&self) -> u64 {
        self.analyzed_items
    }

    pub fn pending_items(&self) -> u64 {
        self.pending_items
    }

    pub fn skipped_items(&self) -> u64 {
        self.skipped_items
    }

    pub fn unsupported_items(&self) -> u64 {
        self.unsupported_items
    }

    pub fn failed_items(&self) -> u64 {
        self.failed_items
    }

    pub fn truncation(&self) -> Option<&ReviewTruncation> {
        self.truncation.as_ref()
    }

    pub fn is_complete(&self) -> bool {
        self.truncation.is_none()
            && self.analyzed_items == self.total_items
            && self.pending_items == 0
            && self.skipped_items == 0
            && self.unsupported_items == 0
            && self.failed_items == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    Unclassified,
}

/// Stable identity of one deterministic file-classification rule.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ClassificationRuleId(String);

impl ClassificationRuleId {
    pub fn new(value: impl Into<String>) -> Result<Self, ReviewModelError> {
        let value = value.into();
        if value.is_empty() || value.chars().any(char::is_whitespace) {
            return Err(ReviewModelError::new(
                "classification rule ID must be non-empty and contain no whitespace",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ClassificationRuleId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// A file role and the sole rule identity that produced it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileClassification {
    role: FileRole,
    rule_id: ClassificationRuleId,
}

impl FileClassification {
    pub fn from_rule(role: FileRole, rule_id: impl Into<String>) -> Result<Self, ReviewModelError> {
        Ok(Self {
            role,
            rule_id: ClassificationRuleId::new(rule_id)?,
        })
    }

    pub fn role(&self) -> FileRole {
        self.role
    }

    pub fn rule_id(&self) -> &ClassificationRuleId {
        &self.rule_id
    }

    pub fn provenance(&self) -> FactProvenance {
        FactProvenance::RuleDerived {
            rule_id: self.rule_id.0.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewFileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
    TypeChanged,
    ModeChanged,
    SubmoduleChanged,
    Unmerged,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSubmoduleChange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_oid: Option<GitObjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_oid: Option<GitObjectId>,
    #[serde(default)]
    pub worktree_dirty: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewFileFact {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_path: Option<String>,
    pub status: ReviewFileStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub similarity: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_mode: Option<String>,
    /// `None` when Git reports `-`, normally for binary content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines_added: Option<u64>,
    /// `None` when Git reports `-`, normally for binary content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines_deleted: Option<u64>,
    pub binary: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submodule: Option<ReviewSubmoduleChange>,
    pub classification: FileClassification,
    pub provenance: FactProvenance,
}

impl ReviewFileFact {
    pub fn path_on(&self, side: ComparisonSide) -> Option<&str> {
        match side {
            ComparisonSide::Base => self.old_path.as_deref(),
            ComparisonSide::Head => self.new_path.as_deref(),
        }
    }
}

/// One full-OID entry in the comparison's chronological commit ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewCommitFact {
    pub oid: GitObjectId,
    pub parent_oids: Vec<GitObjectId>,
    pub subject: String,
    pub author_name: String,
    pub timestamp: i64,
    pub provenance: FactProvenance,
}

/// Raw totals for an inventory. Binary files contribute to file counts only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewChangeTotals {
    pub commits: u64,
    pub files: u64,
    pub files_added: u64,
    pub files_deleted: u64,
    pub files_modified: u64,
    pub files_renamed: u64,
    pub files_copied: u64,
    pub files_type_changed: u64,
    pub files_mode_changed: u64,
    pub submodule_changes: u64,
    pub binary_files: u64,
    pub lines_added: u64,
    pub lines_deleted: u64,
    pub provenance: FactProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewInventory {
    pub comparison: ResolvedComparison,
    pub totals: ReviewChangeTotals,
    pub commits: Vec<ReviewCommitFact>,
    pub files: Vec<ReviewFileFact>,
    pub coverage: ReviewCoverage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonSide {
    Base,
    Head,
}

/// Descriptive syntax context only; this is not a stable symbol identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolContext {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewNavigationTarget {
    pub path: String,
    pub side: ComparisonSide,
    /// One-based source line.
    pub line: NonZeroU32,
    /// Zero-based UTF-8 byte offset when the producer has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_context: Option<SymbolContext>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewDiffRequest {
    pub comparison: ImmutableResolvedComparison,
    #[serde(default)]
    pub ignore_whitespace: bool,
}

impl ReviewDiffRequest {
    pub fn new(
        comparison: ResolvedComparison,
        ignore_whitespace: bool,
    ) -> Result<Self, ReviewModelError> {
        Ok(Self {
            comparison: comparison.try_into()?,
            ignore_whitespace,
        })
    }
}

/// Exact source request with distinct paths for renames and deletions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ReviewSourceRequestWire")]
pub struct ReviewSourceRequest {
    comparison: ImmutableResolvedComparison,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    old_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    new_path: Option<String>,
}

#[derive(Deserialize)]
struct ReviewSourceRequestWire {
    comparison: ImmutableResolvedComparison,
    #[serde(default)]
    old_path: Option<String>,
    #[serde(default)]
    new_path: Option<String>,
}

impl TryFrom<ReviewSourceRequestWire> for ReviewSourceRequest {
    type Error = ReviewModelError;

    fn try_from(value: ReviewSourceRequestWire) -> Result<Self, Self::Error> {
        Self::from_immutable(value.comparison, value.old_path, value.new_path)
    }
}

impl ReviewSourceRequest {
    pub fn new(
        comparison: ResolvedComparison,
        old_path: Option<String>,
        new_path: Option<String>,
    ) -> Result<Self, ReviewModelError> {
        Self::from_immutable(comparison.try_into()?, old_path, new_path)
    }

    fn from_immutable(
        comparison: ImmutableResolvedComparison,
        old_path: Option<String>,
        new_path: Option<String>,
    ) -> Result<Self, ReviewModelError> {
        if old_path.is_none() && new_path.is_none() {
            return Err(ReviewModelError::new(
                "source request requires an old path, a new path, or both",
            ));
        }
        Ok(Self {
            comparison,
            old_path,
            new_path,
        })
    }

    pub fn comparison(&self) -> &ImmutableResolvedComparison {
        &self.comparison
    }

    pub fn old_path(&self) -> Option<&str> {
        self.old_path.as_deref()
    }

    pub fn new_path(&self) -> Option<&str> {
        self.new_path.as_deref()
    }
}

/// Exact source contents paired with the immutable request that produced them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ExactReviewSourceResponseWire")]
pub struct ExactReviewSourceResponse {
    comparison: ImmutableResolvedComparison,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    old_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    new_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    old_content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    new_content: Option<String>,
}

#[derive(Deserialize)]
struct ExactReviewSourceResponseWire {
    comparison: ImmutableResolvedComparison,
    #[serde(default)]
    old_path: Option<String>,
    #[serde(default)]
    new_path: Option<String>,
    #[serde(default)]
    old_content: Option<String>,
    #[serde(default)]
    new_content: Option<String>,
}

impl TryFrom<ExactReviewSourceResponseWire> for ExactReviewSourceResponse {
    type Error = ReviewModelError;

    fn try_from(value: ExactReviewSourceResponseWire) -> Result<Self, Self::Error> {
        let request =
            ReviewSourceRequest::from_immutable(value.comparison, value.old_path, value.new_path)?;
        Self::new(request, value.old_content, value.new_content)
    }
}

impl ExactReviewSourceResponse {
    pub fn new(
        request: ReviewSourceRequest,
        old_content: Option<String>,
        new_content: Option<String>,
    ) -> Result<Self, ReviewModelError> {
        if request.old_path.is_some() != old_content.is_some() {
            return Err(ReviewModelError::new(
                "exact source response must contain content for exactly the requested old side",
            ));
        }
        if request.new_path.is_some() != new_content.is_some() {
            return Err(ReviewModelError::new(
                "exact source response must contain content for exactly the requested new side",
            ));
        }
        Ok(Self {
            comparison: request.comparison,
            old_path: request.old_path,
            new_path: request.new_path,
            old_content,
            new_content,
        })
    }

    pub fn comparison(&self) -> &ImmutableResolvedComparison {
        &self.comparison
    }

    pub fn old_path(&self) -> Option<&str> {
        self.old_path.as_deref()
    }

    pub fn new_path(&self) -> Option<&str> {
        self.new_path.as_deref()
    }

    pub fn old_content(&self) -> Option<&str> {
        self.old_content.as_deref()
    }

    pub fn new_content(&self) -> Option<&str> {
        self.new_content.as_deref()
    }

    pub fn into_parts(self) -> (ReviewSourceRequest, Option<String>, Option<String>) {
        (
            ReviewSourceRequest {
                comparison: self.comparison,
                old_path: self.old_path,
                new_path: self.new_path,
            },
            self.old_content,
            self.new_content,
        )
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    const REQUESTED_BASE_OID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const MERGE_BASE_OID: &str = "1111111111111111111111111111111111111111";
    const HEAD_OID: &str = "2222222222222222222222222222222222222222";

    fn oid(value: &str) -> GitObjectId {
        GitObjectId::new(value).unwrap()
    }

    fn branch_comparison() -> ResolvedComparison {
        ResolvedComparison::new(
            DiffMode::BranchCompare {
                base: "origin/main".to_string(),
                head: "feature".to_string(),
            },
            Some(oid(REQUESTED_BASE_OID)),
            Some(oid(HEAD_OID)),
            ComparisonStrategy::MergeBaseToHead,
            ReviewSnapshot::Commit {
                oid: oid(MERGE_BASE_OID),
            },
            ReviewSnapshot::Commit { oid: oid(HEAD_OID) },
            Some(oid(MERGE_BASE_OID)),
            ReviewComparisonId(format!("merge-base:{MERGE_BASE_OID}:{HEAD_OID}")),
        )
        .unwrap()
    }

    fn branch_comparison_json() -> Value {
        json!({
            "requested": {
                "branch_compare": {
                    "base": "origin/main",
                    "head": "feature"
                }
            },
            "requested_base_oid": REQUESTED_BASE_OID,
            "requested_head_oid": HEAD_OID,
            "strategy": "merge_base_to_head",
            "base": { "kind": "commit", "oid": MERGE_BASE_OID },
            "head": { "kind": "commit", "oid": HEAD_OID },
            "merge_base_oid": MERGE_BASE_OID,
            "identity": format!("merge-base:{MERGE_BASE_OID}:{HEAD_OID}")
        })
    }

    fn added_file() -> ReviewFileFact {
        ReviewFileFact {
            old_path: None,
            new_path: Some("src/review.rs".to_string()),
            status: ReviewFileStatus::Added,
            similarity: None,
            old_mode: None,
            new_mode: Some("100644".to_string()),
            lines_added: Some(3),
            lines_deleted: Some(0),
            binary: false,
            submodule: None,
            classification: FileClassification::from_rule(
                FileRole::Implementation,
                "builtin.source.rs",
            )
            .unwrap(),
            provenance: FactProvenance::Git,
        }
    }

    fn totals() -> ReviewChangeTotals {
        ReviewChangeTotals {
            commits: 1,
            files: 1,
            files_added: 1,
            files_deleted: 0,
            files_modified: 0,
            files_renamed: 0,
            files_copied: 0,
            files_type_changed: 0,
            files_mode_changed: 0,
            submodule_changes: 0,
            binary_files: 0,
            lines_added: 3,
            lines_deleted: 0,
            provenance: FactProvenance::Git,
        }
    }

    #[test]
    fn comparison_distinguishes_requested_base_from_merge_base() {
        let comparison = branch_comparison();
        assert!(matches!(
            comparison.requested(),
            DiffMode::BranchCompare { base, head }
                if base == "origin/main" && head == "feature"
        ));
        assert_eq!(comparison.strategy(), ComparisonStrategy::MergeBaseToHead);
        assert_eq!(
            comparison.requested_base_oid().unwrap().as_str(),
            REQUESTED_BASE_OID
        );
        assert_eq!(comparison.requested_head_oid().unwrap().as_str(), HEAD_OID);
        assert_eq!(comparison.base().oid().unwrap().as_str(), MERGE_BASE_OID);
        assert_eq!(comparison.head().oid().unwrap().as_str(), HEAD_OID);
        assert_eq!(
            comparison.merge_base_oid().unwrap().as_str(),
            MERGE_BASE_OID
        );
        assert_eq!(
            comparison.identity().0,
            format!("merge-base:{MERGE_BASE_OID}:{HEAD_OID}")
        );
        assert_ne!(comparison.requested_base_oid(), comparison.base().oid());
        assert_eq!(
            serde_json::to_value(comparison).unwrap(),
            branch_comparison_json()
        );
    }

    #[test]
    fn immutable_and_mutable_comparisons_are_distinct() {
        assert!(branch_comparison().is_immutable());

        let staged = ResolvedComparison::new(
            DiffMode::Staged,
            Some(oid(MERGE_BASE_OID)),
            None,
            ComparisonStrategy::HeadToIndex,
            ReviewSnapshot::Commit {
                oid: oid(MERGE_BASE_OID),
            },
            ReviewSnapshot::Index {
                fingerprint: "index-v1".to_string(),
            },
            None,
            ReviewComparisonId("staged:index-v1".to_string()),
        )
        .unwrap();

        assert!(!staged.is_immutable());
        assert!(ReviewDiffRequest::new(staged.clone(), false).is_err());
        assert!(ReviewSourceRequest::new(staged.clone(), None, None).is_err());

        let mutable_json = serde_json::to_value(staged).unwrap();
        let diff_json = json!({ "comparison": mutable_json, "ignore_whitespace": false });
        assert!(serde_json::from_value::<ReviewDiffRequest>(diff_json).is_err());
    }

    #[test]
    fn invalid_strategy_snapshot_combination_is_rejected() {
        let mut value = branch_comparison_json();
        value["strategy"] = json!("head_to_index");
        assert!(serde_json::from_value::<ResolvedComparison>(value).is_err());
    }

    #[test]
    fn malformed_and_abbreviated_object_ids_are_rejected() {
        for value in ["abc1234", "z111111111111111111111111111111111111111"] {
            assert!(serde_json::from_value::<GitObjectId>(json!(value)).is_err());
        }
        assert!(GitObjectId::new("f".repeat(40)).is_ok());
        assert!(GitObjectId::new("f".repeat(64)).is_ok());

        let mut comparison = branch_comparison_json();
        comparison["requested_head_oid"] = json!("2222222");
        assert!(serde_json::from_value::<ResolvedComparison>(comparison).is_err());
    }

    #[test]
    fn renamed_and_deleted_navigation_uses_the_selected_side() {
        let renamed = ReviewFileFact {
            old_path: Some("src/old.rs".to_string()),
            new_path: Some("src/new.rs".to_string()),
            status: ReviewFileStatus::Renamed,
            similarity: Some(94),
            old_mode: Some("100644".to_string()),
            new_mode: Some("100644".to_string()),
            lines_added: Some(2),
            lines_deleted: Some(1),
            binary: false,
            submodule: None,
            classification: FileClassification::from_rule(
                FileRole::Implementation,
                "builtin.source.rs",
            )
            .unwrap(),
            provenance: FactProvenance::Git,
        };
        assert_eq!(renamed.path_on(ComparisonSide::Base), Some("src/old.rs"));
        assert_eq!(renamed.path_on(ComparisonSide::Head), Some("src/new.rs"));

        let deleted = ReviewFileFact {
            old_path: Some("src/deleted.rs".to_string()),
            new_path: None,
            status: ReviewFileStatus::Deleted,
            similarity: None,
            old_mode: Some("100644".to_string()),
            new_mode: None,
            lines_added: Some(0),
            lines_deleted: Some(8),
            binary: false,
            submodule: None,
            classification: FileClassification::from_rule(
                FileRole::Implementation,
                "builtin.source.rs",
            )
            .unwrap(),
            provenance: FactProvenance::Git,
        };
        assert_eq!(
            deleted.path_on(ComparisonSide::Base),
            Some("src/deleted.rs")
        );
        assert_eq!(deleted.path_on(ComparisonSide::Head), None);

        let target = ReviewNavigationTarget {
            path: "src/deleted.rs".to_string(),
            side: ComparisonSide::Base,
            line: NonZeroU32::new(4).unwrap(),
            byte_offset: Some(31),
            symbol_context: Some(SymbolContext {
                name: "removed_function".to_string(),
                kind: Some("function".to_string()),
                signature: None,
            }),
        };
        assert_eq!(
            serde_json::to_value(target).unwrap(),
            json!({
                "path": "src/deleted.rs",
                "side": "base",
                "line": 4,
                "byte_offset": 31,
                "symbol_context": {
                    "name": "removed_function",
                    "kind": "function"
                }
            })
        );
    }

    #[test]
    fn inventory_and_provenance_have_stable_json_shapes() {
        let inventory = ReviewInventory {
            comparison: branch_comparison(),
            totals: totals(),
            commits: vec![ReviewCommitFact {
                oid: oid(HEAD_OID),
                parent_oids: vec![oid(MERGE_BASE_OID)],
                subject: "feat: add review facts".to_string(),
                author_name: "Reviewer".to_string(),
                timestamp: 1_786_742_400,
                provenance: FactProvenance::Git,
            }],
            files: vec![added_file()],
            coverage: ReviewCoverage::new(1, 1, 0, 0, 0, 0, None).unwrap(),
        };
        let value = serde_json::to_value(&inventory).unwrap();
        assert_eq!(
            value,
            json!({
                "comparison": branch_comparison_json(),
                "totals": {
                    "commits": 1,
                    "files": 1,
                    "files_added": 1,
                    "files_deleted": 0,
                    "files_modified": 0,
                    "files_renamed": 0,
                    "files_copied": 0,
                    "files_type_changed": 0,
                    "files_mode_changed": 0,
                    "submodule_changes": 0,
                    "binary_files": 0,
                    "lines_added": 3,
                    "lines_deleted": 0,
                    "provenance": { "source": "git" }
                },
                "commits": [{
                    "oid": HEAD_OID,
                    "parent_oids": [MERGE_BASE_OID],
                    "subject": "feat: add review facts",
                    "author_name": "Reviewer",
                    "timestamp": 1_786_742_400_i64,
                    "provenance": { "source": "git" }
                }],
                "files": [{
                    "new_path": "src/review.rs",
                    "status": "added",
                    "new_mode": "100644",
                    "lines_added": 3,
                    "lines_deleted": 0,
                    "binary": false,
                    "classification": {
                        "role": "implementation",
                        "rule_id": "builtin.source.rs"
                    },
                    "provenance": { "source": "git" }
                }],
                "coverage": {
                    "total_items": 1,
                    "analyzed_items": 1,
                    "pending_items": 0,
                    "skipped_items": 0,
                    "unsupported_items": 0,
                    "failed_items": 0
                }
            })
        );
        assert_eq!(
            serde_json::to_value(FactProvenance::SyntaxDerived {
                language: "rust".to_string(),
                parser: "tree-sitter-rust".to_string(),
            })
            .unwrap(),
            json!({
                "source": "syntax_derived",
                "language": "rust",
                "parser": "tree-sitter-rust"
            })
        );
        assert_eq!(
            serde_json::from_value::<ReviewInventory>(value).unwrap(),
            inventory
        );

        let contradictory = json!({
            "role": "implementation",
            "rule_id": "builtin.source.rs",
            "provenance": { "source": "git" }
        });
        assert!(serde_json::from_value::<FileClassification>(contradictory).is_err());
        assert_eq!(
            inventory.files[0].classification.provenance(),
            FactProvenance::RuleDerived {
                rule_id: "builtin.source.rs".to_string()
            }
        );
    }

    #[test]
    fn exact_request_json_shapes_and_whitespace_default_are_stable() {
        let diff = ReviewDiffRequest::new(branch_comparison(), true).unwrap();
        assert_eq!(
            serde_json::to_value(&diff).unwrap(),
            json!({
                "comparison": branch_comparison_json(),
                "ignore_whitespace": true
            })
        );

        let without_option = json!({ "comparison": branch_comparison_json() });
        let decoded: ReviewDiffRequest = serde_json::from_value(without_option).unwrap();
        assert!(!decoded.ignore_whitespace);

        let source = ReviewSourceRequest::new(
            branch_comparison(),
            Some("src/old.rs".to_string()),
            Some("src/new.rs".to_string()),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(source).unwrap(),
            json!({
                "comparison": branch_comparison_json(),
                "old_path": "src/old.rs",
                "new_path": "src/new.rs"
            })
        );
    }

    #[test]
    fn source_request_requires_the_paths_present_on_each_change_side() {
        let addition =
            ReviewSourceRequest::new(branch_comparison(), None, Some("src/new.rs".to_string()))
                .unwrap();
        assert_eq!(addition.old_path(), None);
        assert_eq!(addition.new_path(), Some("src/new.rs"));

        let deletion = ReviewSourceRequest::new(
            branch_comparison(),
            Some("src/deleted.rs".to_string()),
            None,
        )
        .unwrap();
        assert_eq!(deletion.old_path(), Some("src/deleted.rs"));
        assert_eq!(deletion.new_path(), None);

        let rename = ReviewSourceRequest::new(
            branch_comparison(),
            Some("src/old.rs".to_string()),
            Some("src/new.rs".to_string()),
        )
        .unwrap();
        assert_eq!(rename.old_path(), Some("src/old.rs"));
        assert_eq!(rename.new_path(), Some("src/new.rs"));

        assert!(ReviewSourceRequest::new(branch_comparison(), None, None).is_err());
        assert!(
            serde_json::from_value::<ReviewSourceRequest>(json!({
                "comparison": branch_comparison_json()
            }))
            .is_err()
        );
    }

    #[test]
    fn exact_source_response_json_shapes_are_stable() {
        let rename = ExactReviewSourceResponse::new(
            ReviewSourceRequest::new(
                branch_comparison(),
                Some("src/old.rs".to_string()),
                Some("src/new.rs".to_string()),
            )
            .unwrap(),
            Some("old source\n".to_string()),
            Some("new source\n".to_string()),
        )
        .unwrap();
        let rename_json = json!({
            "comparison": branch_comparison_json(),
            "old_path": "src/old.rs",
            "new_path": "src/new.rs",
            "old_content": "old source\n",
            "new_content": "new source\n"
        });
        assert_eq!(serde_json::to_value(&rename).unwrap(), rename_json);
        assert_eq!(
            serde_json::from_value::<ExactReviewSourceResponse>(rename_json).unwrap(),
            rename
        );

        let addition = ExactReviewSourceResponse::new(
            ReviewSourceRequest::new(branch_comparison(), None, Some("src/added.rs".to_string()))
                .unwrap(),
            None,
            Some(String::new()),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(addition).unwrap(),
            json!({
                "comparison": branch_comparison_json(),
                "new_path": "src/added.rs",
                "new_content": ""
            })
        );

        let deletion = ExactReviewSourceResponse::new(
            ReviewSourceRequest::new(
                branch_comparison(),
                Some("src/deleted.rs".to_string()),
                None,
            )
            .unwrap(),
            Some("deleted source\n".to_string()),
            None,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(deletion).unwrap(),
            json!({
                "comparison": branch_comparison_json(),
                "old_path": "src/deleted.rs",
                "old_content": "deleted source\n"
            })
        );

        let (request, old_content, new_content) = rename.into_parts();
        assert_eq!(request.old_path(), Some("src/old.rs"));
        assert_eq!(request.new_path(), Some("src/new.rs"));
        assert_eq!(old_content.as_deref(), Some("old source\n"));
        assert_eq!(new_content.as_deref(), Some("new source\n"));
    }

    #[test]
    fn exact_source_response_rejects_malformed_or_mutable_sides() {
        let comparison = branch_comparison_json();
        for malformed in [
            json!({
                "comparison": comparison,
                "old_path": "src/old.rs"
            }),
            json!({
                "comparison": branch_comparison_json(),
                "new_path": "src/new.rs",
                "old_content": "unexpected",
                "new_content": "new"
            }),
            json!({
                "comparison": branch_comparison_json()
            }),
        ] {
            assert!(serde_json::from_value::<ExactReviewSourceResponse>(malformed).is_err());
        }

        let staged = ResolvedComparison::new(
            DiffMode::Staged,
            Some(oid(MERGE_BASE_OID)),
            None,
            ComparisonStrategy::HeadToIndex,
            ReviewSnapshot::Commit {
                oid: oid(MERGE_BASE_OID),
            },
            ReviewSnapshot::Index {
                fingerprint: "index-v1".to_string(),
            },
            None,
            ReviewComparisonId("staged:index-v1".to_string()),
        )
        .unwrap();
        assert!(
            serde_json::from_value::<ExactReviewSourceResponse>(json!({
                "comparison": serde_json::to_value(staged).unwrap(),
                "new_path": "src/new.rs",
                "new_content": "new"
            }))
            .is_err()
        );

        let request =
            ReviewSourceRequest::new(branch_comparison(), Some("src/old.rs".to_string()), None)
                .unwrap();
        assert!(ExactReviewSourceResponse::new(request, None, None).is_err());
    }

    #[test]
    fn partial_coverage_has_a_stable_json_shape() {
        let coverage = ReviewCoverage::new(
            12,
            4,
            4,
            1,
            2,
            1,
            Some(ReviewTruncation {
                reason: TruncationReason::TimeLimit,
                limit: Some(500),
                observed: Some(731),
                detail: Some("parser budget exhausted".to_string()),
            }),
        )
        .unwrap();
        assert!(!coverage.is_complete());
        assert_eq!(
            serde_json::to_value(coverage).unwrap(),
            json!({
                "total_items": 12,
                "analyzed_items": 4,
                "pending_items": 4,
                "skipped_items": 1,
                "unsupported_items": 2,
                "failed_items": 1,
                "truncation": {
                    "reason": "time_limit",
                    "limit": 500,
                    "observed": 731,
                    "detail": "parser budget exhausted"
                }
            })
        );

        let invalid = json!({
            "total_items": 12,
            "analyzed_items": 4,
            "pending_items": 2,
            "skipped_items": 1,
            "unsupported_items": 2,
            "failed_items": 1
        });
        assert!(serde_json::from_value::<ReviewCoverage>(invalid).is_err());
    }

    #[test]
    fn navigation_rejects_zero_line_values() {
        let json = r#"{
            "path":"src/main.rs",
            "side":"head",
            "line":0
        }"#;
        assert!(serde_json::from_str::<ReviewNavigationTarget>(json).is_err());
    }
}
