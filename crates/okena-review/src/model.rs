use okena_core::review::{
    ComparisonSide, ImmutableResolvedComparison, ReviewCoverage, ReviewNavigationTarget,
    ReviewTruncation, TruncationReason,
};
use okena_syntax::{
    CallFact, SourceRange, SymbolFact, SymbolKey, SyntaxLanguage, SyntaxProvenance,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::num::NonZeroU32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelError(String);

impl ModelError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}
impl fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for ModelError {}

/// One-based inclusive changed-line range, distinct from a UTF-8 source range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct ChangedLineRange {
    start: NonZeroU32,
    end: NonZeroU32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangedLineRangeWire {
    start: NonZeroU32,
    end: NonZeroU32,
}
impl TryFrom<ChangedLineRangeWire> for ChangedLineRange {
    type Error = ModelError;
    fn try_from(value: ChangedLineRangeWire) -> Result<Self, Self::Error> {
        Self::new(value.start, value.end)
    }
}
impl<'de> Deserialize<'de> for ChangedLineRange {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        ChangedLineRangeWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}
impl ChangedLineRange {
    pub fn new(start: NonZeroU32, end: NonZeroU32) -> Result<Self, ModelError> {
        if start > end {
            return Err(ModelError::new("changed-line range starts after it ends"));
        }
        Ok(Self { start, end })
    }
    pub fn start(self) -> NonZeroU32 {
        self.start
    }
    pub fn end(self) -> NonZeroU32 {
        self.end
    }
    pub fn line_count(self) -> u32 {
        self.end.get() - self.start.get() + 1
    }
    fn intersects_source(self, source: SourceRange) -> bool {
        self.start.get() <= source.end_line().get() && source.start_line().get() <= self.end.get()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct ChangedHunk {
    old: Option<ChangedLineRange>,
    new: Option<ChangedLineRange>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangedHunkWire {
    old: Option<ChangedLineRange>,
    new: Option<ChangedLineRange>,
}
impl TryFrom<ChangedHunkWire> for ChangedHunk {
    type Error = ModelError;
    fn try_from(value: ChangedHunkWire) -> Result<Self, Self::Error> {
        Self::new(value.old, value.new)
    }
}
impl<'de> Deserialize<'de> for ChangedHunk {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        ChangedHunkWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}
impl ChangedHunk {
    pub fn new(
        old: Option<ChangedLineRange>,
        new: Option<ChangedLineRange>,
    ) -> Result<Self, ModelError> {
        if old.is_none() && new.is_none() {
            return Err(ModelError::new("changed hunk requires at least one side"));
        }
        Ok(Self { old, new })
    }
    pub fn old(&self) -> Option<ChangedLineRange> {
        self.old
    }
    pub fn new_range(&self) -> Option<ChangedLineRange> {
        self.new
    }
}

/// A descriptive symbol occurrence. It is a location, not an identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SymbolReference {
    side: ComparisonSide,
    range: SourceRange,
    key: SymbolKey,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SymbolReferenceWire {
    side: ComparisonSide,
    range: SourceRange,
    key: SymbolKey,
}
impl TryFrom<SymbolReferenceWire> for SymbolReference {
    type Error = ModelError;
    fn try_from(value: SymbolReferenceWire) -> Result<Self, Self::Error> {
        Ok(Self::new(value.side, value.range, value.key))
    }
}
impl<'de> Deserialize<'de> for SymbolReference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        SymbolReferenceWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}
impl SymbolReference {
    pub fn new(side: ComparisonSide, range: SourceRange, key: SymbolKey) -> Self {
        Self { side, range, key }
    }
    pub fn side(&self) -> ComparisonSide {
        self.side
    }
    pub fn range(&self) -> SourceRange {
        self.range
    }
    pub fn key(&self) -> &SymbolKey {
        &self.key
    }
}

/// Compact hierarchy for rendering surrounding structure on either snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OutlineFact {
    provenance: SyntaxProvenance,
    symbol: SymbolReference,
    children: Vec<OutlineFact>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OutlineFactWire {
    provenance: SyntaxProvenance,
    symbol: SymbolReference,
    children: Vec<OutlineFact>,
}
impl TryFrom<OutlineFactWire> for OutlineFact {
    type Error = ModelError;
    fn try_from(value: OutlineFactWire) -> Result<Self, Self::Error> {
        Self::new(value.provenance, value.symbol, value.children)
    }
}
impl<'de> Deserialize<'de> for OutlineFact {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        OutlineFactWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}
impl OutlineFact {
    pub fn new(
        provenance: SyntaxProvenance,
        symbol: SymbolReference,
        children: Vec<Self>,
    ) -> Result<Self, ModelError> {
        if children.iter().any(|child| {
            child.provenance != provenance
                || child.symbol.side() != symbol.side()
                || !symbol.range().contains(child.symbol.range())
        }) {
            return Err(ModelError::new(
                "outline children must share provenance and be on the same side inside their parent",
            ));
        }
        Ok(Self {
            provenance,
            symbol,
            children,
        })
    }
    pub fn provenance(&self) -> &SyntaxProvenance {
        &self.provenance
    }
    pub fn symbol(&self) -> &SymbolReference {
        &self.symbol
    }
    pub fn children(&self) -> &[Self] {
        &self.children
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolChangeKind {
    Added,
    Removed,
    Modified,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SignatureChange {
    old_signature: String,
    new_signature: String,
    old_range: SourceRange,
    new_range: SourceRange,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignatureChangeWire {
    old_signature: String,
    new_signature: String,
    old_range: SourceRange,
    new_range: SourceRange,
}
impl TryFrom<SignatureChangeWire> for SignatureChange {
    type Error = ModelError;
    fn try_from(value: SignatureChangeWire) -> Result<Self, Self::Error> {
        Self::new(
            value.old_signature,
            value.new_signature,
            value.old_range,
            value.new_range,
        )
    }
}
impl<'de> Deserialize<'de> for SignatureChange {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        SignatureChangeWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}
impl SignatureChange {
    pub fn new(
        old_signature: impl Into<String>,
        new_signature: impl Into<String>,
        old_range: SourceRange,
        new_range: SourceRange,
    ) -> Result<Self, ModelError> {
        let old_signature = old_signature.into();
        let new_signature = new_signature.into();
        if old_signature.trim().is_empty()
            || new_signature.trim().is_empty()
            || old_signature == new_signature
        {
            return Err(ModelError::new(
                "signature change requires two distinct non-empty signatures",
            ));
        }
        Ok(Self {
            old_signature,
            new_signature,
            old_range,
            new_range,
        })
    }
    fn validate_facts(&self, old: &SymbolFact, new: &SymbolFact) -> Result<(), ModelError> {
        if self.old_signature != old.normalized_signature()
            || self.new_signature != new.normalized_signature()
            || self.old_range != old.signature_range()
            || self.new_range != new.signature_range()
        {
            return Err(ModelError::new(
                "signature change must exactly match its paired symbol facts",
            ));
        }
        Ok(())
    }
    pub fn old_signature(&self) -> &str {
        &self.old_signature
    }
    pub fn new_signature(&self) -> &str {
        &self.new_signature
    }
    pub fn old_range(&self) -> SourceRange {
        self.old_range
    }
    pub fn new_range(&self) -> SourceRange {
        self.new_range
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SymbolChange {
    kind: SymbolChangeKind,
    old: Option<SymbolFact>,
    new: Option<SymbolFact>,
    signature_change: Option<SignatureChange>,
    body_changed: bool,
    changed_old_lines: u32,
    changed_new_lines: u32,
    hunks: Vec<ChangedHunk>,
    navigation: ReviewNavigationTarget,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SymbolChangeWire {
    kind: SymbolChangeKind,
    old: Option<SymbolFact>,
    new: Option<SymbolFact>,
    signature_change: Option<SignatureChange>,
    body_changed: bool,
    changed_old_lines: u32,
    changed_new_lines: u32,
    hunks: Vec<ChangedHunk>,
    navigation: ReviewNavigationTarget,
}
impl TryFrom<SymbolChangeWire> for SymbolChange {
    type Error = ModelError;
    fn try_from(value: SymbolChangeWire) -> Result<Self, Self::Error> {
        Self::new_validated(
            value.kind,
            value.old,
            value.new,
            value.signature_change,
            value.body_changed,
            value.hunks,
            value.navigation,
            Some((value.changed_old_lines, value.changed_new_lines)),
        )
    }
}
impl<'de> Deserialize<'de> for SymbolChange {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        SymbolChangeWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}
impl SymbolChange {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        kind: SymbolChangeKind,
        old: Option<SymbolFact>,
        new: Option<SymbolFact>,
        signature_change: Option<SignatureChange>,
        body_changed: bool,
        hunks: Vec<ChangedHunk>,
        navigation: ReviewNavigationTarget,
    ) -> Result<Self, ModelError> {
        Self::new_validated(
            kind,
            old,
            new,
            signature_change,
            body_changed,
            hunks,
            navigation,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_validated(
        kind: SymbolChangeKind,
        old: Option<SymbolFact>,
        new: Option<SymbolFact>,
        signature_change: Option<SignatureChange>,
        body_changed: bool,
        hunks: Vec<ChangedHunk>,
        navigation: ReviewNavigationTarget,
        reported_counts: Option<(u32, u32)>,
    ) -> Result<Self, ModelError> {
        let valid = match kind {
            SymbolChangeKind::Added => {
                old.is_none()
                    && new.is_some()
                    && signature_change.is_none()
                    && !body_changed
                    && navigation.side == ComparisonSide::Head
            }
            SymbolChangeKind::Removed => {
                old.is_some()
                    && new.is_none()
                    && signature_change.is_none()
                    && !body_changed
                    && navigation.side == ComparisonSide::Base
            }
            SymbolChangeKind::Modified => {
                old.is_some() && new.is_some() && (signature_change.is_some() || body_changed)
            }
        };
        if !valid {
            return Err(ModelError::new(
                "symbol change shape does not match its kind",
            ));
        }
        if let (Some(old), Some(new)) = (&old, &new) {
            if old.key() != new.key() {
                return Err(ModelError::new(
                    "matched symbols must have the same qualified key",
                ));
            }
            if old.provenance().language() != new.provenance().language() {
                return Err(ModelError::new(
                    "matched symbols must use the same syntax language",
                ));
            }
            if signature_change.is_none()
                && old.normalized_signature() != new.normalized_signature()
            {
                return Err(ModelError::new(
                    "body-only changes require an unchanged normalized signature",
                ));
            }
            if let Some(signature) = &signature_change {
                signature.validate_facts(old, new)?;
            }
        }
        if hunks.is_empty() {
            return Err(ModelError::new("symbol changes require changed hunks"));
        }
        let unique_hunks: HashSet<_> = hunks.iter().collect();
        if unique_hunks.len() != hunks.len() {
            return Err(ModelError::new(
                "symbol changes cannot cite duplicate hunks",
            ));
        }
        for hunk in &hunks {
            let intersects_old = old.as_ref().is_some_and(|fact| {
                hunk.old()
                    .is_some_and(|lines| lines.intersects_source(fact.full_range()))
            });
            let intersects_new = new.as_ref().is_some_and(|fact| {
                hunk.new_range()
                    .is_some_and(|lines| lines.intersects_source(fact.full_range()))
            });
            let old_outside = old.as_ref().is_some_and(|fact| {
                hunk.old()
                    .is_some_and(|lines| !lines.intersects_source(fact.full_range()))
            });
            let new_outside = new.as_ref().is_some_and(|fact| {
                hunk.new_range()
                    .is_some_and(|lines| !lines.intersects_source(fact.full_range()))
            });
            if old_outside || new_outside || !intersects_old && !intersects_new {
                return Err(ModelError::new(
                    "symbol change hunks must intersect a paired symbol occurrence",
                ));
            }
        }
        if let (Some(signature), Some(old), Some(new)) = (&signature_change, &old, &new) {
            if !side_has_intersection(&hunks, ComparisonSide::Base, signature.old_range())
                && !side_has_intersection(&hunks, ComparisonSide::Head, signature.new_range())
            {
                return Err(ModelError::new(
                    "signature changes require hunk evidence on at least one exact signature range",
                ));
            }
            signature.validate_facts(old, new)?;
        }
        if body_changed {
            let body_has_evidence = [
                (ComparisonSide::Base, old.as_ref()),
                (ComparisonSide::Head, new.as_ref()),
            ]
            .into_iter()
            .any(|(side, fact)| {
                fact.and_then(SymbolFact::body_range)
                    .is_some_and(|body| side_has_intersection(&hunks, side, body))
            });
            if !body_has_evidence {
                return Err(ModelError::new(
                    "body changes require hunk evidence on at least one body range",
                ));
            }
        }
        if kind == SymbolChangeKind::Modified {
            for hunk in &hunks {
                for (side, fact) in [
                    (ComparisonSide::Base, old.as_ref()),
                    (ComparisonSide::Head, new.as_ref()),
                ] {
                    let Some(lines) = hunk_range(hunk, side) else {
                        continue;
                    };
                    let signature_relevant = signature_change.as_ref().is_some_and(|signature| {
                        let range = match side {
                            ComparisonSide::Base => signature.old_range(),
                            ComparisonSide::Head => signature.new_range(),
                        };
                        lines.intersects_source(range)
                    });
                    let body_relevant = body_changed
                        && fact
                            .and_then(SymbolFact::body_range)
                            .is_some_and(|body| lines.intersects_source(body));
                    if !signature_relevant && !body_relevant {
                        return Err(ModelError::new(
                            "every present modified-hunk side must intersect a changed dimension",
                        ));
                    }
                }
            }
        }
        let changed_old_lines = old.as_ref().map_or(Ok(0), |fact| {
            changed_line_count(&hunks, ComparisonSide::Base, fact.full_range())
        })?;
        let changed_new_lines = new.as_ref().map_or(Ok(0), |fact| {
            changed_line_count(&hunks, ComparisonSide::Head, fact.full_range())
        })?;
        if changed_old_lines == 0 && changed_new_lines == 0 {
            return Err(ModelError::new(
                "symbol changes require changed-line evidence",
            ));
        }
        if reported_counts.is_some_and(|counts| counts != (changed_old_lines, changed_new_lines)) {
            return Err(ModelError::new(
                "serialized changed-line counts must equal derived hunk intersections",
            ));
        }
        validate_navigation(&navigation)?;
        Ok(Self {
            kind,
            old,
            new,
            signature_change,
            body_changed,
            changed_old_lines,
            changed_new_lines,
            hunks,
            navigation,
        })
    }
    pub fn kind(&self) -> SymbolChangeKind {
        self.kind
    }
    pub fn old(&self) -> Option<&SymbolFact> {
        self.old.as_ref()
    }
    pub fn new_fact(&self) -> Option<&SymbolFact> {
        self.new.as_ref()
    }
    pub fn signature_change(&self) -> Option<&SignatureChange> {
        self.signature_change.as_ref()
    }
    pub fn body_changed(&self) -> bool {
        self.body_changed
    }
    pub fn changed_old_lines(&self) -> u32 {
        self.changed_old_lines
    }
    pub fn changed_new_lines(&self) -> u32 {
        self.changed_new_lines
    }
    pub fn hunks(&self) -> &[ChangedHunk] {
        &self.hunks
    }
    pub fn navigation(&self) -> &ReviewNavigationTarget {
        &self.navigation
    }
}

fn side_has_intersection(hunks: &[ChangedHunk], side: ComparisonSide, source: SourceRange) -> bool {
    hunks
        .iter()
        .any(|hunk| hunk_range(hunk, side).is_some_and(|range| range.intersects_source(source)))
}

fn hunk_range(hunk: &ChangedHunk, side: ComparisonSide) -> Option<ChangedLineRange> {
    match side {
        ComparisonSide::Base => hunk.old(),
        ComparisonSide::Head => hunk.new_range(),
    }
}

fn changed_line_count(
    hunks: &[ChangedHunk],
    side: ComparisonSide,
    source: SourceRange,
) -> Result<u32, ModelError> {
    let mut intersections: Vec<(u32, u32)> = hunks
        .iter()
        .filter_map(|hunk| hunk_range(hunk, side))
        .filter_map(|range| {
            let start = range.start().get().max(source.start_line().get());
            let end = range.end().get().min(source.end_line().get());
            (start <= end).then_some((start, end))
        })
        .collect();
    intersections.sort_unstable();
    let mut total = 0_u64;
    let mut current: Option<(u32, u32)> = None;
    for (start, end) in intersections {
        match current {
            Some((current_start, current_end)) if start <= current_end.saturating_add(1) => {
                current = Some((current_start, current_end.max(end)));
            }
            Some((current_start, current_end)) => {
                total += u64::from(current_end - current_start + 1);
                current = Some((start, end));
            }
            None => current = Some((start, end)),
        }
    }
    if let Some((start, end)) = current {
        total += u64::from(end - start + 1);
    }
    u32::try_from(total).map_err(|_| ModelError::new("derived changed-line count overflowed"))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "metric", rename_all = "snake_case")]
pub enum StructuralMetric {
    FunctionLineCount { lines: u32 },
    ChangedLines { old: u32, new: u32 },
    ParameterCount { parameters: u32 },
    SyntacticNestingDepth { depth: u32 },
    TypeMemberCount { members: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StructuralHotspot {
    symbol: SymbolReference,
    metric: StructuralMetric,
    provenance: SyntaxProvenance,
    navigation: ReviewNavigationTarget,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StructuralHotspotWire {
    symbol: SymbolReference,
    metric: StructuralMetric,
    provenance: SyntaxProvenance,
    navigation: ReviewNavigationTarget,
}
impl TryFrom<StructuralHotspotWire> for StructuralHotspot {
    type Error = ModelError;
    fn try_from(value: StructuralHotspotWire) -> Result<Self, Self::Error> {
        Self::new(
            value.symbol,
            value.metric,
            value.provenance,
            value.navigation,
        )
    }
}
impl<'de> Deserialize<'de> for StructuralHotspot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        StructuralHotspotWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}
impl StructuralHotspot {
    pub fn new(
        symbol: SymbolReference,
        metric: StructuralMetric,
        provenance: SyntaxProvenance,
        navigation: ReviewNavigationTarget,
    ) -> Result<Self, ModelError> {
        validate_navigation(&navigation)?;
        if symbol.side() != navigation.side {
            return Err(ModelError::new(
                "hotspot location and navigation must use the same side",
            ));
        }
        Ok(Self {
            symbol,
            metric,
            provenance,
            navigation,
        })
    }
    pub fn symbol(&self) -> &SymbolReference {
        &self.symbol
    }
    pub fn metric(&self) -> &StructuralMetric {
        &self.metric
    }
    pub fn provenance(&self) -> &SyntaxProvenance {
        &self.provenance
    }
    pub fn navigation(&self) -> &ReviewNavigationTarget {
        &self.navigation
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallChangeKind {
    Added,
    Removed,
    Modified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallPairingStrategy {
    /// The comparator found exactly one matching occurrence inside each enclosing range.
    UniqueOccurrenceWithinEnclosingRange,
}

/// Evidence for pairing two call occurrences across snapshots.
///
/// Candidate counts make the comparator's collection-level uniqueness claim explicit. The model
/// requires 1:1, but cannot independently recount the comparator's candidate collection. Repeated
/// or otherwise ambiguous calls deliberately degrade to separate added and removed changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CallPairingEvidence {
    strategy: CallPairingStrategy,
    old_call_range: SourceRange,
    new_call_range: SourceRange,
    old_enclosing_range: SourceRange,
    new_enclosing_range: SourceRange,
    old_candidate_count: u32,
    new_candidate_count: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallPairingEvidenceWire {
    strategy: CallPairingStrategy,
    old_call_range: SourceRange,
    new_call_range: SourceRange,
    old_enclosing_range: SourceRange,
    new_enclosing_range: SourceRange,
    old_candidate_count: u32,
    new_candidate_count: u32,
}

impl TryFrom<CallPairingEvidenceWire> for CallPairingEvidence {
    type Error = ModelError;
    fn try_from(value: CallPairingEvidenceWire) -> Result<Self, Self::Error> {
        Self::new(
            value.strategy,
            value.old_call_range,
            value.new_call_range,
            value.old_enclosing_range,
            value.new_enclosing_range,
            value.old_candidate_count,
            value.new_candidate_count,
        )
    }
}

impl<'de> Deserialize<'de> for CallPairingEvidence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        CallPairingEvidenceWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}

impl CallPairingEvidence {
    pub fn new(
        strategy: CallPairingStrategy,
        old_call_range: SourceRange,
        new_call_range: SourceRange,
        old_enclosing_range: SourceRange,
        new_enclosing_range: SourceRange,
        old_candidate_count: u32,
        new_candidate_count: u32,
    ) -> Result<Self, ModelError> {
        if !old_enclosing_range.contains(old_call_range)
            || !new_enclosing_range.contains(new_call_range)
        {
            return Err(ModelError::new(
                "paired call locations must be inside their enclosing ranges",
            ));
        }
        if old_candidate_count != 1 || new_candidate_count != 1 {
            return Err(ModelError::new(
                "modified call pairing requires exactly one candidate on each side",
            ));
        }
        Ok(Self {
            strategy,
            old_call_range,
            new_call_range,
            old_enclosing_range,
            new_enclosing_range,
            old_candidate_count,
            new_candidate_count,
        })
    }
    pub fn strategy(&self) -> CallPairingStrategy {
        self.strategy
    }
    pub fn old_call_range(&self) -> SourceRange {
        self.old_call_range
    }
    pub fn new_call_range(&self) -> SourceRange {
        self.new_call_range
    }
    pub fn old_enclosing_range(&self) -> SourceRange {
        self.old_enclosing_range
    }
    pub fn new_enclosing_range(&self) -> SourceRange {
        self.new_enclosing_range
    }
    pub fn old_candidate_count(&self) -> u32 {
        self.old_candidate_count
    }
    pub fn new_candidate_count(&self) -> u32 {
        self.new_candidate_count
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CallDiffChange {
    kind: CallChangeKind,
    old: Option<CallFact>,
    new: Option<CallFact>,
    arguments_changed: bool,
    control_context_changed: bool,
    pairing: Option<CallPairingEvidence>,
    navigation: ReviewNavigationTarget,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallDiffChangeWire {
    kind: CallChangeKind,
    old: Option<CallFact>,
    new: Option<CallFact>,
    arguments_changed: bool,
    control_context_changed: bool,
    pairing: Option<CallPairingEvidence>,
    navigation: ReviewNavigationTarget,
}
impl TryFrom<CallDiffChangeWire> for CallDiffChange {
    type Error = ModelError;
    fn try_from(value: CallDiffChangeWire) -> Result<Self, Self::Error> {
        Self::new(
            value.kind,
            value.old,
            value.new,
            value.arguments_changed,
            value.control_context_changed,
            value.pairing,
            value.navigation,
        )
    }
}
impl<'de> Deserialize<'de> for CallDiffChange {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        CallDiffChangeWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}
impl CallDiffChange {
    pub fn new(
        kind: CallChangeKind,
        old: Option<CallFact>,
        new: Option<CallFact>,
        arguments_changed: bool,
        control_context_changed: bool,
        pairing: Option<CallPairingEvidence>,
        navigation: ReviewNavigationTarget,
    ) -> Result<Self, ModelError> {
        let valid = match kind {
            CallChangeKind::Added => {
                old.is_none()
                    && new.is_some()
                    && !arguments_changed
                    && !control_context_changed
                    && pairing.is_none()
                    && navigation.side == ComparisonSide::Head
            }
            CallChangeKind::Removed => {
                old.is_some()
                    && new.is_none()
                    && !arguments_changed
                    && !control_context_changed
                    && pairing.is_none()
                    && navigation.side == ComparisonSide::Base
            }
            CallChangeKind::Modified => {
                old.is_some()
                    && new.is_some()
                    && pairing.is_some()
                    && (arguments_changed || control_context_changed)
            }
        };
        if !valid {
            return Err(ModelError::new(
                "call diff shape does not match its kind or changed dimensions",
            ));
        }
        if let (Some(old), Some(new)) = (&old, &new)
            && (old.callee_text() != new.callee_text()
                || old.enclosing_symbol() != new.enclosing_symbol())
        {
            return Err(ModelError::new(
                "paired call modifications require the same callee and enclosing symbol",
            ));
        }
        if let (Some(old), Some(new)) = (&old, &new) {
            let actual_arguments_changed = old.argument_text() != new.argument_text();
            let actual_control_context_changed = old.control_context() != new.control_context();
            if arguments_changed != actual_arguments_changed
                || control_context_changed != actual_control_context_changed
            {
                return Err(ModelError::new(
                    "call modification flags must match the paired syntactic facts",
                ));
            }
            let evidence = pairing.as_ref().ok_or_else(|| {
                ModelError::new("modified calls require explicit pairing evidence")
            })?;
            if evidence.old_call_range() != old.call_site_range()
                || evidence.new_call_range() != new.call_site_range()
            {
                return Err(ModelError::new(
                    "call pairing evidence must name the paired call-site locations",
                ));
            }
        }
        validate_navigation(&navigation)?;
        Ok(Self {
            kind,
            old,
            new,
            arguments_changed,
            control_context_changed,
            pairing,
            navigation,
        })
    }
    pub fn kind(&self) -> CallChangeKind {
        self.kind
    }
    pub fn old(&self) -> Option<&CallFact> {
        self.old.as_ref()
    }
    pub fn new_fact(&self) -> Option<&CallFact> {
        self.new.as_ref()
    }
    pub fn arguments_changed(&self) -> bool {
        self.arguments_changed
    }
    pub fn control_context_changed(&self) -> bool {
        self.control_context_changed
    }
    pub fn pairing(&self) -> Option<&CallPairingEvidence> {
        self.pairing.as_ref()
    }
    pub fn navigation(&self) -> &ReviewNavigationTarget {
        &self.navigation
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAnalysisStatus {
    Parsed,
    Partial,
    Pending,
    Unsupported,
    Failed,
    Skipped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisStage {
    Detection,
    Parsing,
    Comparison,
    Budget,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AnalysisError {
    path: Option<String>,
    stage: AnalysisStage,
    message: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnalysisErrorWire {
    path: Option<String>,
    stage: AnalysisStage,
    message: String,
}
impl TryFrom<AnalysisErrorWire> for AnalysisError {
    type Error = ModelError;
    fn try_from(value: AnalysisErrorWire) -> Result<Self, Self::Error> {
        Self::new(value.path, value.stage, value.message)
    }
}
impl<'de> Deserialize<'de> for AnalysisError {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        AnalysisErrorWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}
impl AnalysisError {
    pub fn new(
        path: Option<String>,
        stage: AnalysisStage,
        message: impl Into<String>,
    ) -> Result<Self, ModelError> {
        let message = message.into();
        if message.trim().is_empty() || path.as_ref().is_some_and(|path| path.trim().is_empty()) {
            return Err(ModelError::new(
                "analysis error requires a message and a valid optional path",
            ));
        }
        Ok(Self {
            path,
            stage,
            message,
        })
    }
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }
    pub fn stage(&self) -> AnalysisStage {
        self.stage
    }
    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StructuredFile {
    old_path: Option<String>,
    new_path: Option<String>,
    language: Option<SyntaxLanguage>,
    old_provenance: Option<SyntaxProvenance>,
    new_provenance: Option<SyntaxProvenance>,
    status: FileAnalysisStatus,
    old_outline: Vec<OutlineFact>,
    new_outline: Vec<OutlineFact>,
    symbol_changes: Vec<SymbolChange>,
    hotspots: Vec<StructuralHotspot>,
    call_diff: Vec<CallDiffChange>,
    changed_hunks: Vec<ChangedHunk>,
    errors: Vec<AnalysisError>,
    truncation: Option<ReviewTruncation>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StructuredFileWire {
    old_path: Option<String>,
    new_path: Option<String>,
    language: Option<SyntaxLanguage>,
    old_provenance: Option<SyntaxProvenance>,
    new_provenance: Option<SyntaxProvenance>,
    status: FileAnalysisStatus,
    old_outline: Vec<OutlineFact>,
    new_outline: Vec<OutlineFact>,
    symbol_changes: Vec<SymbolChange>,
    hotspots: Vec<StructuralHotspot>,
    call_diff: Vec<CallDiffChange>,
    changed_hunks: Vec<ChangedHunk>,
    errors: Vec<AnalysisError>,
    truncation: Option<ReviewTruncation>,
}
impl TryFrom<StructuredFileWire> for StructuredFile {
    type Error = ModelError;
    fn try_from(value: StructuredFileWire) -> Result<Self, Self::Error> {
        Self::new(
            value.old_path,
            value.new_path,
            value.language,
            value.old_provenance,
            value.new_provenance,
            value.status,
            value.old_outline,
            value.new_outline,
            value.symbol_changes,
            value.hotspots,
            value.call_diff,
            value.changed_hunks,
            value.errors,
            value.truncation,
        )
    }
}
impl<'de> Deserialize<'de> for StructuredFile {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        StructuredFileWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}
impl StructuredFile {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        old_path: Option<String>,
        new_path: Option<String>,
        language: Option<SyntaxLanguage>,
        old_provenance: Option<SyntaxProvenance>,
        new_provenance: Option<SyntaxProvenance>,
        status: FileAnalysisStatus,
        old_outline: Vec<OutlineFact>,
        new_outline: Vec<OutlineFact>,
        symbol_changes: Vec<SymbolChange>,
        hotspots: Vec<StructuralHotspot>,
        call_diff: Vec<CallDiffChange>,
        changed_hunks: Vec<ChangedHunk>,
        errors: Vec<AnalysisError>,
        truncation: Option<ReviewTruncation>,
    ) -> Result<Self, ModelError> {
        if old_path.is_none() && new_path.is_none() {
            return Err(ModelError::new(
                "structured file requires at least one path",
            ));
        }
        if old_path.as_ref().is_some_and(|path| path.trim().is_empty())
            || new_path.as_ref().is_some_and(|path| path.trim().is_empty())
        {
            return Err(ModelError::new("structured file paths must not be empty"));
        }
        let has_facts = !old_outline.is_empty()
            || !new_outline.is_empty()
            || !symbol_changes.is_empty()
            || !hotspots.is_empty()
            || !call_diff.is_empty();
        if matches!(
            status,
            FileAnalysisStatus::Pending
                | FileAnalysisStatus::Unsupported
                | FileAnalysisStatus::Failed
                | FileAnalysisStatus::Skipped
        ) && has_facts
        {
            return Err(ModelError::new(
                "unsuccessful files cannot contain structured facts",
            ));
        }
        if status == FileAnalysisStatus::Unsupported && language.is_some() {
            return Err(ModelError::new(
                "unsupported files cannot claim a syntax language",
            ));
        }
        if status == FileAnalysisStatus::Unsupported
            && (old_provenance.is_some() || new_provenance.is_some())
        {
            return Err(ModelError::new(
                "unsupported files cannot claim syntax provenance",
            ));
        }
        if status == FileAnalysisStatus::Pending
            && (old_provenance.is_some() || new_provenance.is_some())
        {
            return Err(ModelError::new(
                "pending files cannot claim syntax provenance",
            ));
        }
        if matches!(
            status,
            FileAnalysisStatus::Parsed | FileAnalysisStatus::Partial
        ) && language.is_none()
        {
            return Err(ModelError::new(
                "successful structured files require a syntax language",
            ));
        }
        if matches!(
            status,
            FileAnalysisStatus::Parsed | FileAnalysisStatus::Partial
        ) && (old_path.is_some() != old_provenance.is_some()
            || new_path.is_some() != new_provenance.is_some())
        {
            return Err(ModelError::new(
                "analyzed snapshot paths require matching syntax provenance",
            ));
        }
        if status == FileAnalysisStatus::Parsed && (truncation.is_some() || !errors.is_empty()) {
            return Err(ModelError::new(
                "parsed files cannot carry errors or truncation",
            ));
        }
        if let Some(truncation) = &truncation {
            validate_review_truncation(truncation)?;
        }
        if status == FileAnalysisStatus::Partial && truncation.is_none() && errors.is_empty() {
            return Err(ModelError::new(
                "partial files require errors or truncation",
            ));
        }
        if status == FileAnalysisStatus::Pending {
            if truncation.is_none() && errors.is_empty() {
                return Err(ModelError::new(
                    "pending files require budget, cancellation, time, or analysis error evidence",
                ));
            }
            if errors
                .iter()
                .any(|error| error.stage() != AnalysisStage::Budget)
            {
                return Err(ModelError::new(
                    "pending files can only carry budget-stage analysis errors",
                ));
            }
            if truncation
                .as_ref()
                .is_some_and(|truncation| !is_pending_truncation(truncation.reason))
            {
                return Err(ModelError::new(
                    "pending file truncation must describe a budget, cancellation, or time limit",
                ));
            }
        }
        if status == FileAnalysisStatus::Failed && errors.is_empty() {
            return Err(ModelError::new(
                "failed files require analysis error evidence",
            ));
        }
        let unique_file_hunks: HashSet<_> = changed_hunks.iter().collect();
        if unique_file_hunks.len() != changed_hunks.len() {
            return Err(ModelError::new(
                "structured files cannot contain duplicate changed hunks",
            ));
        }
        if symbol_changes.iter().any(|change| {
            change
                .hunks()
                .iter()
                .any(|hunk| !unique_file_hunks.contains(hunk))
        }) {
            return Err(ModelError::new(
                "symbol changes can only cite hunks from their structured file",
            ));
        }
        for fact in old_outline.iter() {
            if fact.symbol().side() != ComparisonSide::Base
                || Some(fact.provenance()) != old_provenance.as_ref()
            {
                return Err(ModelError::new(
                    "old outline must use the base side and old document provenance",
                ));
            }
        }
        for fact in new_outline.iter() {
            if fact.symbol().side() != ComparisonSide::Head
                || Some(fact.provenance()) != new_provenance.as_ref()
            {
                return Err(ModelError::new(
                    "new outline must use the head side and new document provenance",
                ));
            }
        }
        let file = Self {
            old_path,
            new_path,
            language,
            old_provenance,
            new_provenance,
            status,
            old_outline,
            new_outline,
            symbol_changes,
            hotspots,
            call_diff,
            changed_hunks,
            errors,
            truncation,
        };
        if let Some(language) = file.language {
            if file
                .old_provenance
                .iter()
                .chain(file.new_provenance.iter())
                .any(|provenance| provenance.language() != language)
            {
                return Err(ModelError::new(
                    "document provenance must use the file syntax language",
                ));
            }
            let symbols_match = file.symbol_changes.iter().all(|change| {
                change
                    .old()
                    .into_iter()
                    .chain(change.new_fact())
                    .all(|fact| fact.provenance().language() == language)
            });
            let hotspots_match = file
                .hotspots
                .iter()
                .all(|hotspot| hotspot.provenance().language() == language);
            let calls_match = file.call_diff.iter().all(|change| {
                change
                    .old()
                    .into_iter()
                    .chain(change.new_fact())
                    .all(|fact| fact.provenance().language() == language)
            });
            if !symbols_match || !hotspots_match || !calls_match {
                return Err(ModelError::new(
                    "structured facts must use the file syntax language",
                ));
            }
        }
        for navigation in file
            .symbol_changes
            .iter()
            .map(SymbolChange::navigation)
            .chain(file.hotspots.iter().map(StructuralHotspot::navigation))
            .chain(file.call_diff.iter().map(CallDiffChange::navigation))
        {
            let expected = file
                .path_on(navigation.side)
                .ok_or_else(|| ModelError::new("navigation targets a missing comparison side"))?;
            if navigation.path != expected {
                return Err(ModelError::new(
                    "navigation path does not match its structured file side",
                ));
            }
        }
        Ok(file)
    }
    pub fn path_on(&self, side: ComparisonSide) -> Option<&str> {
        match side {
            ComparisonSide::Base => self.old_path.as_deref(),
            ComparisonSide::Head => self.new_path.as_deref(),
        }
    }
    pub fn old_path(&self) -> Option<&str> {
        self.old_path.as_deref()
    }
    pub fn new_path(&self) -> Option<&str> {
        self.new_path.as_deref()
    }
    pub fn language(&self) -> Option<SyntaxLanguage> {
        self.language
    }
    pub fn status(&self) -> FileAnalysisStatus {
        self.status
    }
    pub fn old_provenance(&self) -> Option<&SyntaxProvenance> {
        self.old_provenance.as_ref()
    }
    pub fn new_provenance(&self) -> Option<&SyntaxProvenance> {
        self.new_provenance.as_ref()
    }
    pub fn old_outline(&self) -> &[OutlineFact] {
        &self.old_outline
    }
    pub fn new_outline(&self) -> &[OutlineFact] {
        &self.new_outline
    }
    pub fn symbol_changes(&self) -> &[SymbolChange] {
        &self.symbol_changes
    }
    pub fn hotspots(&self) -> &[StructuralHotspot] {
        &self.hotspots
    }
    pub fn call_diff(&self) -> &[CallDiffChange] {
        &self.call_diff
    }
    pub fn changed_hunks(&self) -> &[ChangedHunk] {
        &self.changed_hunks
    }
    pub fn errors(&self) -> &[AnalysisError] {
        &self.errors
    }
    pub fn truncation(&self) -> Option<&ReviewTruncation> {
        self.truncation.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LanguageCoverage {
    language: SyntaxLanguage,
    coverage: ReviewCoverage,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LanguageCoverageWire {
    language: SyntaxLanguage,
    coverage: ReviewCoverage,
}
impl TryFrom<LanguageCoverageWire> for LanguageCoverage {
    type Error = ModelError;
    fn try_from(value: LanguageCoverageWire) -> Result<Self, Self::Error> {
        Ok(Self::new(value.language, value.coverage))
    }
}
impl<'de> Deserialize<'de> for LanguageCoverage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        LanguageCoverageWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}
impl LanguageCoverage {
    pub fn new(language: SyntaxLanguage, coverage: ReviewCoverage) -> Self {
        Self { language, coverage }
    }
    pub fn language(&self) -> SyntaxLanguage {
        self.language
    }
    pub fn coverage(&self) -> &ReviewCoverage {
        &self.coverage
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReviewStructure {
    comparison: ImmutableResolvedComparison,
    files: Vec<StructuredFile>,
    coverage: ReviewCoverage,
    language_coverage: Vec<LanguageCoverage>,
    errors: Vec<AnalysisError>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewStructureWire {
    comparison: ImmutableResolvedComparison,
    files: Vec<StructuredFile>,
    coverage: ReviewCoverage,
    language_coverage: Vec<LanguageCoverage>,
    errors: Vec<AnalysisError>,
}
impl TryFrom<ReviewStructureWire> for ReviewStructure {
    type Error = ModelError;
    fn try_from(value: ReviewStructureWire) -> Result<Self, Self::Error> {
        Self::new(
            value.comparison,
            value.files,
            value.coverage,
            value.language_coverage,
            value.errors,
        )
    }
}
impl<'de> Deserialize<'de> for ReviewStructure {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        ReviewStructureWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}
impl ReviewStructure {
    pub fn new(
        comparison: ImmutableResolvedComparison,
        files: Vec<StructuredFile>,
        coverage: ReviewCoverage,
        language_coverage: Vec<LanguageCoverage>,
        errors: Vec<AnalysisError>,
    ) -> Result<Self, ModelError> {
        if let Some(truncation) = coverage.truncation() {
            validate_review_truncation(truncation)?;
        }
        validate_coverage(&files, &coverage, None)?;
        let mut seen = HashSet::new();
        for language in &language_coverage {
            if !seen.insert(language.language()) {
                return Err(ModelError::new("language coverage entries must be unique"));
            }
            if let Some(truncation) = language.coverage().truncation() {
                validate_review_truncation(truncation)?;
            }
            validate_coverage(&files, language.coverage(), Some(language.language()))?;
        }
        let covered_languages: HashSet<_> =
            files.iter().filter_map(StructuredFile::language).collect();
        if seen != covered_languages {
            return Err(ModelError::new(
                "language coverage must account for every detected language exactly once",
            ));
        }
        Ok(Self {
            comparison,
            files,
            coverage,
            language_coverage,
            errors,
        })
    }
    pub fn comparison(&self) -> &ImmutableResolvedComparison {
        &self.comparison
    }
    pub fn files(&self) -> &[StructuredFile] {
        &self.files
    }
    pub fn coverage(&self) -> &ReviewCoverage {
        &self.coverage
    }
    pub fn language_coverage(&self) -> &[LanguageCoverage] {
        &self.language_coverage
    }
    pub fn errors(&self) -> &[AnalysisError] {
        &self.errors
    }
}

fn validate_navigation(target: &ReviewNavigationTarget) -> Result<(), ModelError> {
    if target.path.trim().is_empty() {
        Err(ModelError::new("navigation path must not be empty"))
    } else {
        Ok(())
    }
}

fn validate_review_truncation(truncation: &ReviewTruncation) -> Result<(), ModelError> {
    let measured = match (truncation.limit, truncation.observed) {
        (Some(limit), Some(observed)) if limit > 0 && observed >= limit => true,
        (None, None) => false,
        _ => {
            return Err(ModelError::new(
                "review truncation measurements must be paired and meet a positive limit",
            ));
        }
    };
    match truncation.reason {
        TruncationReason::Cancelled if measured => Err(ModelError::new(
            "cancelled review truncation cannot carry numeric measurements",
        )),
        TruncationReason::Cancelled => Ok(()),
        TruncationReason::Other
            if truncation
                .detail
                .as_ref()
                .is_some_and(|detail| !detail.trim().is_empty()) =>
        {
            Ok(())
        }
        TruncationReason::Other => Err(ModelError::new(
            "other review truncation requires non-empty detail",
        )),
        _ if measured => Ok(()),
        _ => Err(ModelError::new(
            "bounded review truncation requires limit and observed values",
        )),
    }
}

fn is_pending_truncation(reason: TruncationReason) -> bool {
    matches!(
        reason,
        TruncationReason::ItemLimit
            | TruncationReason::ByteLimit
            | TruncationReason::TimeLimit
            | TruncationReason::CaptureLimit
            | TruncationReason::ResponseLimit
            | TruncationReason::Cancelled
    )
}

fn validate_coverage(
    files: &[StructuredFile],
    coverage: &ReviewCoverage,
    language: Option<SyntaxLanguage>,
) -> Result<(), ModelError> {
    let selected: Vec<_> = files
        .iter()
        .filter(|file| language.is_none_or(|language| file.language() == Some(language)))
        .collect();
    let mut counts: HashMap<FileAnalysisStatus, u64> = HashMap::new();
    for file in &selected {
        *counts.entry(file.status()).or_default() += 1;
    }
    let analyzed = counts
        .get(&FileAnalysisStatus::Parsed)
        .copied()
        .unwrap_or(0)
        + counts
            .get(&FileAnalysisStatus::Partial)
            .copied()
            .unwrap_or(0);
    let pending = counts
        .get(&FileAnalysisStatus::Pending)
        .copied()
        .unwrap_or(0);
    let matches = coverage.total_items() == u64::try_from(selected.len()).unwrap_or(u64::MAX)
        && coverage.analyzed_items() == analyzed
        && coverage.pending_items() == pending
        && coverage.skipped_items()
            == counts
                .get(&FileAnalysisStatus::Skipped)
                .copied()
                .unwrap_or(0)
        && coverage.unsupported_items()
            == counts
                .get(&FileAnalysisStatus::Unsupported)
                .copied()
                .unwrap_or(0)
        && coverage.failed_items()
            == counts
                .get(&FileAnalysisStatus::Failed)
                .copied()
                .unwrap_or(0);
    if matches {
        Ok(())
    } else {
        Err(ModelError::new(
            "coverage does not account for its structured files",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_core::review::{
        ComparisonStrategy, GitObjectId, ResolvedComparison, ReviewComparisonId, ReviewSnapshot,
    };
    use okena_core::types::DiffMode;
    use okena_syntax::{SymbolKind, SymbolVisibility};
    use serde_json::json;

    fn nz(value: u32) -> NonZeroU32 {
        NonZeroU32::new(value).unwrap()
    }
    fn range(start: u64, end: u64, start_line: u32, end_line: u32) -> SourceRange {
        SourceRange::new(start, end, nz(start_line), nz(end_line)).unwrap()
    }
    fn provenance() -> SyntaxProvenance {
        SyntaxProvenance::tree_sitter(SyntaxLanguage::Rust, "tree-sitter-rust@0.24").unwrap()
    }
    fn symbol(signature: &str) -> SymbolFact {
        SymbolFact::new(
            provenance(),
            SymbolKey::new(vec!["worker".into()], SymbolKind::Function, "run").unwrap(),
            SymbolVisibility::Public,
            range(0, 30, 1, 3),
            range(0, 12, 1, 1),
            Some(range(13, 30, 2, 3)),
            signature,
            0,
            1,
            0,
        )
        .unwrap()
    }
    fn navigation(side: ComparisonSide) -> ReviewNavigationTarget {
        ReviewNavigationTarget {
            path: "src/lib.rs".into(),
            side,
            line: nz(1),
            byte_offset: Some(0),
            symbol_context: None,
        }
    }
    fn immutable_comparison() -> ImmutableResolvedComparison {
        let base = GitObjectId::new("1111111111111111111111111111111111111111").unwrap();
        let head = GitObjectId::new("2222222222222222222222222222222222222222").unwrap();
        ResolvedComparison::new(
            DiffMode::BranchCompare {
                base: "origin/main".into(),
                head: "feature".into(),
            },
            Some(GitObjectId::new("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap()),
            Some(head.clone()),
            ComparisonStrategy::MergeBaseToHead,
            ReviewSnapshot::Commit { oid: base.clone() },
            ReviewSnapshot::Commit { oid: head.clone() },
            Some(base),
            ReviewComparisonId("comparison-1".into()),
        )
        .unwrap()
        .try_into()
        .unwrap()
    }
    fn coverage(total: u64, analyzed: u64) -> ReviewCoverage {
        ReviewCoverage::new(total, analyzed, 0, 0, 0, 0, None).unwrap()
    }
    fn new_hunk() -> ChangedHunk {
        ChangedHunk::new(None, Some(ChangedLineRange::new(nz(1), nz(3)).unwrap())).unwrap()
    }
    fn paired_hunk() -> ChangedHunk {
        ChangedHunk::new(
            Some(ChangedLineRange::new(nz(1), nz(3)).unwrap()),
            Some(ChangedLineRange::new(nz(1), nz(3)).unwrap()),
        )
        .unwrap()
    }
    fn parsed_file() -> StructuredFile {
        let change = SymbolChange::new(
            SymbolChangeKind::Added,
            None,
            Some(symbol("pub fn run()")),
            None,
            false,
            vec![new_hunk()],
            navigation(ComparisonSide::Head),
        )
        .unwrap();
        StructuredFile::new(
            None,
            Some("src/lib.rs".into()),
            Some(SyntaxLanguage::Rust),
            None,
            Some(provenance()),
            FileAnalysisStatus::Parsed,
            Vec::new(),
            Vec::new(),
            vec![change],
            Vec::new(),
            Vec::new(),
            vec![new_hunk()],
            Vec::new(),
            None,
        )
        .unwrap()
    }

    fn pending_file(
        path: &str,
        language: Option<SyntaxLanguage>,
        truncation: Option<ReviewTruncation>,
        errors: Vec<AnalysisError>,
    ) -> StructuredFile {
        StructuredFile::new(
            None,
            Some(path.into()),
            language,
            None,
            None,
            FileAnalysisStatus::Pending,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            errors,
            truncation,
        )
        .unwrap()
    }

    fn measured_truncation(
        reason: TruncationReason,
        limit: u64,
        observed: u64,
    ) -> ReviewTruncation {
        ReviewTruncation {
            reason,
            limit: Some(limit),
            observed: Some(observed),
            detail: None,
        }
    }

    #[test]
    fn invalid_wire_change_shapes_are_rejected() {
        assert!(serde_json::from_value::<ChangedLineRange>(json!({"start":3,"end":2})).is_err());
        assert!(serde_json::from_value::<ChangedHunk>(json!({"old":null,"new":null})).is_err());
        let mut value = serde_json::to_value(
            SymbolChange::new(
                SymbolChangeKind::Added,
                None,
                Some(symbol("pub fn run()")),
                None,
                false,
                vec![new_hunk()],
                navigation(ComparisonSide::Head),
            )
            .unwrap(),
        )
        .unwrap();
        value["old"] = serde_json::to_value(symbol("pub fn run()")).unwrap();
        assert!(serde_json::from_value::<SymbolChange>(value).is_err());
    }

    #[test]
    fn signature_wire_must_match_paired_facts() {
        let old = symbol("pub fn run()");
        let new = symbol("pub fn run(value: u32)");
        let signature = SignatureChange::new(
            old.normalized_signature(),
            new.normalized_signature(),
            old.signature_range(),
            new.signature_range(),
        )
        .unwrap();
        let change = SymbolChange::new(
            SymbolChangeKind::Modified,
            Some(old),
            Some(new),
            Some(signature),
            true,
            vec![paired_hunk()],
            navigation(ComparisonSide::Head),
        )
        .unwrap();
        let mut value = serde_json::to_value(change).unwrap();
        value["signature_change"]["new_signature"] = json!("wrong");
        assert!(serde_json::from_value::<SymbolChange>(value).is_err());
    }

    #[test]
    fn modified_symbol_can_change_signature_and_body_together() {
        let old = symbol("pub fn run()");
        let new = symbol("pub fn run(value: u32)");
        let signature = SignatureChange::new(
            old.normalized_signature(),
            new.normalized_signature(),
            old.signature_range(),
            new.signature_range(),
        )
        .unwrap();
        let change = SymbolChange::new(
            SymbolChangeKind::Modified,
            Some(old),
            Some(new),
            Some(signature),
            true,
            vec![paired_hunk()],
            navigation(ComparisonSide::Head),
        )
        .unwrap();
        assert!(change.signature_change().is_some());
        assert!(change.body_changed());
        assert_eq!(
            serde_json::from_value::<SymbolChange>(serde_json::to_value(&change).unwrap()).unwrap(),
            change
        );
    }

    #[test]
    fn modified_symbol_requires_dimension_specific_hunks() {
        let old = symbol("pub fn run()");
        let new = symbol("pub fn run(value: u32)");
        let signature = SignatureChange::new(
            old.normalized_signature(),
            new.normalized_signature(),
            old.signature_range(),
            new.signature_range(),
        )
        .unwrap();
        let signature_only = ChangedHunk::new(
            Some(ChangedLineRange::new(nz(1), nz(1)).unwrap()),
            Some(ChangedLineRange::new(nz(1), nz(1)).unwrap()),
        )
        .unwrap();
        assert!(
            SymbolChange::new(
                SymbolChangeKind::Modified,
                Some(old.clone()),
                Some(new.clone()),
                Some(signature.clone()),
                true,
                vec![signature_only],
                navigation(ComparisonSide::Head),
            )
            .is_err()
        );

        let body_only = ChangedHunk::new(
            Some(ChangedLineRange::new(nz(2), nz(3)).unwrap()),
            Some(ChangedLineRange::new(nz(2), nz(3)).unwrap()),
        )
        .unwrap();
        assert!(
            SymbolChange::new(
                SymbolChangeKind::Modified,
                Some(old),
                Some(new),
                Some(signature),
                false,
                vec![body_only],
                navigation(ComparisonSide::Head),
            )
            .is_err()
        );
    }

    #[test]
    fn modified_symbol_accepts_one_sided_body_evidence_and_derives_zero_other_count() {
        let old = symbol("pub fn run()");
        let new = symbol("pub fn run()");
        let insertion =
            ChangedHunk::new(None, Some(ChangedLineRange::new(nz(2), nz(2)).unwrap())).unwrap();
        let inserted = SymbolChange::new(
            SymbolChangeKind::Modified,
            Some(old.clone()),
            Some(new.clone()),
            None,
            true,
            vec![insertion],
            navigation(ComparisonSide::Head),
        )
        .unwrap();
        assert_eq!(inserted.changed_old_lines(), 0);
        assert_eq!(inserted.changed_new_lines(), 1);

        let deletion =
            ChangedHunk::new(Some(ChangedLineRange::new(nz(3), nz(3)).unwrap()), None).unwrap();
        let deleted = SymbolChange::new(
            SymbolChangeKind::Modified,
            Some(old),
            Some(new),
            None,
            true,
            vec![deletion],
            navigation(ComparisonSide::Head),
        )
        .unwrap();
        assert_eq!(deleted.changed_old_lines(), 1);
        assert_eq!(deleted.changed_new_lines(), 0);
        assert_eq!(
            serde_json::from_value::<SymbolChange>(serde_json::to_value(&deleted).unwrap())
                .unwrap(),
            deleted
        );
    }

    #[test]
    fn modified_signature_accepts_one_sided_evidence_but_rejects_present_unrelated_sides() {
        let old = symbol("pub fn run()");
        let new = symbol("pub fn run(value: u32)");
        let signature = SignatureChange::new(
            old.normalized_signature(),
            new.normalized_signature(),
            old.signature_range(),
            new.signature_range(),
        )
        .unwrap();
        let insertion =
            ChangedHunk::new(None, Some(ChangedLineRange::new(nz(1), nz(1)).unwrap())).unwrap();
        let inserted = SymbolChange::new(
            SymbolChangeKind::Modified,
            Some(old.clone()),
            Some(new.clone()),
            Some(signature.clone()),
            false,
            vec![insertion],
            navigation(ComparisonSide::Head),
        )
        .unwrap();
        assert_eq!(inserted.changed_old_lines(), 0);
        assert_eq!(inserted.changed_new_lines(), 1);

        let unrelated_old_side = ChangedHunk::new(
            Some(ChangedLineRange::new(nz(2), nz(2)).unwrap()),
            Some(ChangedLineRange::new(nz(1), nz(1)).unwrap()),
        )
        .unwrap();
        assert!(
            SymbolChange::new(
                SymbolChangeKind::Modified,
                Some(old.clone()),
                Some(new.clone()),
                Some(signature.clone()),
                false,
                vec![unrelated_old_side],
                navigation(ComparisonSide::Head),
            )
            .is_err()
        );

        let deletion =
            ChangedHunk::new(Some(ChangedLineRange::new(nz(1), nz(1)).unwrap()), None).unwrap();
        let deleted = SymbolChange::new(
            SymbolChangeKind::Modified,
            Some(old),
            Some(new),
            Some(signature),
            false,
            vec![deletion],
            navigation(ComparisonSide::Head),
        )
        .unwrap();
        assert_eq!(deleted.changed_old_lines(), 1);
        assert_eq!(deleted.changed_new_lines(), 0);
    }

    #[test]
    fn symbol_hunks_are_unique_and_counts_are_derived() {
        assert!(
            SymbolChange::new(
                SymbolChangeKind::Added,
                None,
                Some(symbol("pub fn run()")),
                None,
                false,
                vec![new_hunk(), new_hunk()],
                navigation(ComparisonSide::Head),
            )
            .is_err()
        );
        let out_of_symbol =
            ChangedHunk::new(None, Some(ChangedLineRange::new(nz(8), nz(9)).unwrap())).unwrap();
        assert!(
            SymbolChange::new(
                SymbolChangeKind::Added,
                None,
                Some(symbol("pub fn run()")),
                None,
                false,
                vec![out_of_symbol],
                navigation(ComparisonSide::Head),
            )
            .is_err()
        );

        let change = SymbolChange::new(
            SymbolChangeKind::Added,
            None,
            Some(symbol("pub fn run()")),
            None,
            false,
            vec![new_hunk()],
            navigation(ComparisonSide::Head),
        )
        .unwrap();
        assert_eq!(change.changed_old_lines(), 0);
        assert_eq!(change.changed_new_lines(), 3);
        let mut inflated = serde_json::to_value(change).unwrap();
        inflated["changed_new_lines"] = json!(30);
        assert!(serde_json::from_value::<SymbolChange>(inflated).is_err());
    }

    #[test]
    fn structured_file_rejects_fabricated_symbol_hunks() {
        let change = SymbolChange::new(
            SymbolChangeKind::Added,
            None,
            Some(symbol("pub fn run()")),
            None,
            false,
            vec![new_hunk()],
            navigation(ComparisonSide::Head),
        )
        .unwrap();
        let different_file_hunk =
            ChangedHunk::new(None, Some(ChangedLineRange::new(nz(1), nz(1)).unwrap())).unwrap();
        assert!(
            StructuredFile::new(
                None,
                Some("src/lib.rs".into()),
                Some(SyntaxLanguage::Rust),
                None,
                Some(provenance()),
                FileAnalysisStatus::Parsed,
                Vec::new(),
                Vec::new(),
                vec![change],
                Vec::new(),
                Vec::new(),
                vec![different_file_hunk],
                Vec::new(),
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn modified_calls_require_a_changed_dimension_and_same_context() {
        let call = CallFact::new(
            provenance(),
            "work",
            "value",
            range(20, 22, 2, 2),
            range(16, 23, 2, 2),
            Some(symbol("pub fn run()").key().clone()),
            Vec::new(),
        )
        .unwrap();
        assert!(
            CallDiffChange::new(
                CallChangeKind::Modified,
                Some(call.clone()),
                Some(call.clone()),
                false,
                false,
                None,
                navigation(ComparisonSide::Head)
            )
            .is_err()
        );
        let changed = CallFact::new(
            provenance(),
            "work",
            "other",
            range(20, 22, 2, 2),
            range(16, 23, 2, 2),
            Some(symbol("pub fn run()").key().clone()),
            Vec::new(),
        )
        .unwrap();
        let change = CallDiffChange::new(
            CallChangeKind::Modified,
            Some(call.clone()),
            Some(changed.clone()),
            true,
            false,
            Some(
                CallPairingEvidence::new(
                    CallPairingStrategy::UniqueOccurrenceWithinEnclosingRange,
                    call.call_site_range(),
                    changed.call_site_range(),
                    symbol("pub fn run()").full_range(),
                    symbol("pub fn run()").full_range(),
                    1,
                    1,
                )
                .unwrap(),
            ),
            navigation(ComparisonSide::Head),
        )
        .unwrap();
        let mut invalid_wire = serde_json::to_value(change).unwrap();
        invalid_wire["arguments_changed"] = json!(false);
        assert!(serde_json::from_value::<CallDiffChange>(invalid_wire).is_err());
    }

    #[test]
    fn ambiguous_repeated_calls_must_remain_unpaired() {
        let old = CallFact::new(
            provenance(),
            "work",
            "value",
            range(20, 22, 2, 2),
            range(16, 23, 2, 2),
            Some(symbol("pub fn run()").key().clone()),
            Vec::new(),
        )
        .unwrap();
        let new = CallFact::new(
            provenance(),
            "work",
            "other",
            range(20, 22, 2, 2),
            range(16, 23, 2, 2),
            Some(symbol("pub fn run()").key().clone()),
            Vec::new(),
        )
        .unwrap();
        let repeated_old = CallFact::new(
            provenance(),
            "work",
            "value",
            range(25, 27, 3, 3),
            range(24, 28, 3, 3),
            Some(symbol("pub fn run()").key().clone()),
            Vec::new(),
        )
        .unwrap();
        let old_candidates = [old.clone(), repeated_old];
        assert!(
            CallPairingEvidence::new(
                CallPairingStrategy::UniqueOccurrenceWithinEnclosingRange,
                old.call_site_range(),
                new.call_site_range(),
                symbol("pub fn run()").full_range(),
                symbol("pub fn run()").full_range(),
                u32::try_from(old_candidates.len()).unwrap(),
                1,
            )
            .is_err()
        );
        assert!(
            CallDiffChange::new(
                CallChangeKind::Modified,
                Some(old.clone()),
                Some(new.clone()),
                true,
                false,
                None,
                navigation(ComparisonSide::Head),
            )
            .is_err()
        );
        assert!(
            CallDiffChange::new(
                CallChangeKind::Removed,
                Some(old),
                None,
                false,
                false,
                None,
                navigation(ComparisonSide::Base),
            )
            .is_ok()
        );
        assert!(
            CallDiffChange::new(
                CallChangeKind::Added,
                None,
                Some(new),
                false,
                false,
                None,
                navigation(ComparisonSide::Head),
            )
            .is_ok()
        );
    }

    #[test]
    fn failed_structured_file_rejects_successful_facts_on_the_wire() {
        let mut value = serde_json::to_value(parsed_file()).unwrap();
        value["status"] = json!("failed");
        assert!(serde_json::from_value::<StructuredFile>(value).is_err());
    }

    #[test]
    fn pending_files_accept_explicit_budget_time_and_cancellation_evidence() {
        let cases = [
            measured_truncation(TruncationReason::ItemLimit, 100, 100),
            measured_truncation(TruncationReason::ByteLimit, 1_000_000, 1_000_001),
            measured_truncation(TruncationReason::TimeLimit, 50_000, 50_000),
            ReviewTruncation {
                reason: TruncationReason::Cancelled,
                limit: None,
                observed: None,
                detail: Some("daemon request cancelled".into()),
            },
        ];

        for (index, truncation) in cases.into_iter().enumerate() {
            let file = pending_file(
                &format!("src/pending-{index}.rs"),
                Some(SyntaxLanguage::Rust),
                Some(truncation),
                Vec::new(),
            );
            assert_eq!(file.status(), FileAnalysisStatus::Pending);
            assert!(file.old_provenance().is_none());
            assert!(file.new_provenance().is_none());
            assert!(file.old_outline().is_empty());
            assert!(file.new_outline().is_empty());
            assert_eq!(
                serde_json::from_value::<StructuredFile>(serde_json::to_value(&file).unwrap())
                    .unwrap(),
                file
            );
        }

        let error = AnalysisError::new(
            Some("src/pending-error.rs".into()),
            AnalysisStage::Budget,
            "analysis slot was not available",
        )
        .unwrap();
        let pending = pending_file("src/pending-error.rs", None, None, vec![error]);
        assert_eq!(pending.status(), FileAnalysisStatus::Pending);
        assert_eq!(
            serde_json::from_value::<StructuredFile>(serde_json::to_value(&pending).unwrap())
                .unwrap(),
            pending
        );
    }

    #[test]
    fn pending_files_reject_successful_facts_provenance_and_invalid_evidence() {
        let mut with_facts = serde_json::to_value(parsed_file()).unwrap();
        with_facts["status"] = json!("pending");
        with_facts["truncation"] =
            serde_json::to_value(measured_truncation(TruncationReason::ItemLimit, 1, 1)).unwrap();
        assert!(serde_json::from_value::<StructuredFile>(with_facts).is_err());

        assert!(
            StructuredFile::new(
                None,
                Some("src/pending.rs".into()),
                Some(SyntaxLanguage::Rust),
                None,
                Some(provenance()),
                FileAnalysisStatus::Pending,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Some(measured_truncation(TruncationReason::ByteLimit, 100, 100)),
            )
            .is_err()
        );
        assert!(
            StructuredFile::new(
                None,
                Some("src/pending.rs".into()),
                Some(SyntaxLanguage::Rust),
                None,
                None,
                FileAnalysisStatus::Pending,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                None,
            )
            .is_err()
        );
        assert!(
            StructuredFile::new(
                None,
                Some("src/pending.rs".into()),
                None,
                None,
                None,
                FileAnalysisStatus::Pending,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Some(ReviewTruncation {
                    reason: TruncationReason::Other,
                    limit: None,
                    observed: None,
                    detail: Some("not a scheduling or budget reason".into()),
                }),
            )
            .is_err()
        );

        for stage in [
            AnalysisStage::Detection,
            AnalysisStage::Parsing,
            AnalysisStage::Comparison,
        ] {
            let invalid_error =
                AnalysisError::new(Some("src/pending.rs".into()), stage, "not pending evidence")
                    .unwrap();
            assert!(
                StructuredFile::new(
                    None,
                    Some("src/pending.rs".into()),
                    Some(SyntaxLanguage::Rust),
                    None,
                    None,
                    FileAnalysisStatus::Pending,
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    vec![invalid_error.clone()],
                    Some(measured_truncation(TruncationReason::TimeLimit, 50, 50)),
                )
                .is_err()
            );

            let budget_error = AnalysisError::new(
                Some("src/pending.rs".into()),
                AnalysisStage::Budget,
                "scheduler deferred analysis",
            )
            .unwrap();
            let valid = pending_file(
                "src/pending.rs",
                None,
                Some(measured_truncation(TruncationReason::ByteLimit, 100, 100)),
                vec![budget_error],
            );
            let mut wire = serde_json::to_value(valid).unwrap();
            wire["errors"] = json!([invalid_error]);
            assert!(serde_json::from_value::<StructuredFile>(wire).is_err());
        }
    }

    #[test]
    fn partial_and_failed_files_require_explicit_evidence() {
        let mut partial = serde_json::to_value(parsed_file()).unwrap();
        partial["status"] = json!("partial");
        assert!(serde_json::from_value::<StructuredFile>(partial).is_err());

        let mut failed = serde_json::to_value(parsed_file()).unwrap();
        failed["status"] = json!("failed");
        failed["symbol_changes"] = json!([]);
        assert!(serde_json::from_value::<StructuredFile>(failed).is_err());
    }

    #[test]
    fn outlines_must_match_snapshot_provenance() {
        let snapshot_provenance = provenance();
        let other_provenance =
            SyntaxProvenance::tree_sitter(SyntaxLanguage::Rust, "different-parser").unwrap();
        let outline = OutlineFact::new(
            other_provenance,
            SymbolReference::new(
                ComparisonSide::Head,
                symbol("pub fn run()").full_range(),
                symbol("pub fn run()").key().clone(),
            ),
            Vec::new(),
        )
        .unwrap();
        assert!(
            StructuredFile::new(
                None,
                Some("src/lib.rs".into()),
                Some(SyntaxLanguage::Rust),
                None,
                Some(snapshot_provenance),
                FileAnalysisStatus::Parsed,
                Vec::new(),
                vec![outline],
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn aggregate_wire_rejects_duplicate_language_coverage() {
        let review = ReviewStructure::new(
            immutable_comparison(),
            vec![parsed_file()],
            coverage(1, 1),
            vec![LanguageCoverage::new(SyntaxLanguage::Rust, coverage(1, 1))],
            Vec::new(),
        )
        .unwrap();
        let mut value = serde_json::to_value(review).unwrap();
        let duplicate = value["language_coverage"][0].clone();
        value["language_coverage"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert!(serde_json::from_value::<ReviewStructure>(value).is_err());
    }

    #[test]
    fn aggregate_and_language_coverage_count_pending_files_exactly() {
        let files = vec![
            parsed_file(),
            pending_file(
                "src/pending-rust.rs",
                Some(SyntaxLanguage::Rust),
                Some(measured_truncation(TruncationReason::ByteLimit, 100, 101)),
                Vec::new(),
            ),
            pending_file(
                "src/pending-ts.ts",
                Some(SyntaxLanguage::TypeScript),
                Some(measured_truncation(TruncationReason::TimeLimit, 50, 50)),
                Vec::new(),
            ),
            pending_file(
                "src/pending-undetected",
                None,
                Some(ReviewTruncation {
                    reason: TruncationReason::Cancelled,
                    limit: None,
                    observed: None,
                    detail: None,
                }),
                Vec::new(),
            ),
        ];
        let aggregate = ReviewCoverage::new(4, 1, 3, 0, 0, 0, None).unwrap();
        let rust = ReviewCoverage::new(2, 1, 1, 0, 0, 0, None).unwrap();
        let typescript = ReviewCoverage::new(1, 0, 1, 0, 0, 0, None).unwrap();
        let review = ReviewStructure::new(
            immutable_comparison(),
            files,
            aggregate,
            vec![
                LanguageCoverage::new(SyntaxLanguage::Rust, rust),
                LanguageCoverage::new(SyntaxLanguage::TypeScript, typescript),
            ],
            Vec::new(),
        )
        .unwrap();
        assert_eq!(review.coverage().pending_items(), 3);
        assert_eq!(review.language_coverage()[0].coverage().pending_items(), 1);
        assert_eq!(review.language_coverage()[1].coverage().pending_items(), 1);

        let value = serde_json::to_value(&review).unwrap();
        assert_eq!(
            serde_json::from_value::<ReviewStructure>(value.clone()).unwrap(),
            review
        );
        let mut invalid = value;
        invalid["language_coverage"][0]["coverage"] =
            serde_json::to_value(ReviewCoverage::new(2, 2, 0, 0, 0, 0, None).unwrap()).unwrap();
        assert!(serde_json::from_value::<ReviewStructure>(invalid).is_err());
    }

    #[test]
    fn pending_exact_comparison_response_has_stable_golden_json() {
        let truncation = measured_truncation(TruncationReason::ByteLimit, 1_000, 1_250);
        let pending = pending_file(
            "src/pending.rs",
            Some(SyntaxLanguage::Rust),
            Some(truncation.clone()),
            Vec::new(),
        );
        let pending_coverage = ReviewCoverage::new(1, 0, 1, 0, 0, 0, None).unwrap();
        let review = ReviewStructure::new(
            immutable_comparison(),
            vec![pending],
            pending_coverage.clone(),
            vec![LanguageCoverage::new(
                SyntaxLanguage::Rust,
                pending_coverage,
            )],
            Vec::new(),
        )
        .unwrap();
        let value = serde_json::to_value(&review).unwrap();
        assert_eq!(
            value,
            json!({
                "comparison": {
                    "requested": { "branch_compare": { "base": "origin/main", "head": "feature" } },
                    "requested_base_oid": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "requested_head_oid": "2222222222222222222222222222222222222222",
                    "strategy": "merge_base_to_head",
                    "base": { "kind": "commit", "oid": "1111111111111111111111111111111111111111" },
                    "head": { "kind": "commit", "oid": "2222222222222222222222222222222222222222" },
                    "merge_base_oid": "1111111111111111111111111111111111111111",
                    "identity": "comparison-1"
                },
                "files": [{
                    "old_path": null,
                    "new_path": "src/pending.rs",
                    "language": "rust",
                    "old_provenance": null,
                    "new_provenance": null,
                    "status": "pending",
                    "old_outline": [],
                    "new_outline": [],
                    "symbol_changes": [],
                    "hotspots": [],
                    "call_diff": [],
                    "changed_hunks": [],
                    "errors": [],
                    "truncation": {
                        "reason": "byte_limit",
                        "limit": 1_000,
                        "observed": 1_250
                    }
                }],
                "coverage": {
                    "total_items": 1,
                    "analyzed_items": 0,
                    "pending_items": 1,
                    "skipped_items": 0,
                    "unsupported_items": 0,
                    "failed_items": 0
                },
                "language_coverage": [{
                    "language": "rust",
                    "coverage": {
                        "total_items": 1,
                        "analyzed_items": 0,
                        "pending_items": 1,
                        "skipped_items": 0,
                        "unsupported_items": 0,
                        "failed_items": 0
                    }
                }],
                "errors": []
            })
        );
        assert_eq!(
            serde_json::from_value::<ReviewStructure>(value).unwrap(),
            review
        );
    }

    #[test]
    fn exact_comparison_response_has_stable_golden_json() {
        let review = ReviewStructure::new(
            immutable_comparison(),
            Vec::new(),
            coverage(0, 0),
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        let value = serde_json::to_value(&review).unwrap();
        assert_eq!(
            value,
            json!({
                "comparison": {
                    "requested": { "branch_compare": { "base": "origin/main", "head": "feature" } },
                    "requested_base_oid": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "requested_head_oid": "2222222222222222222222222222222222222222",
                    "strategy": "merge_base_to_head",
                    "base": { "kind": "commit", "oid": "1111111111111111111111111111111111111111" },
                    "head": { "kind": "commit", "oid": "2222222222222222222222222222222222222222" },
                    "merge_base_oid": "1111111111111111111111111111111111111111",
                    "identity": "comparison-1"
                },
                "files": [],
                "coverage": {
                    "total_items": 0,
                    "analyzed_items": 0,
                    "pending_items": 0,
                    "skipped_items": 0,
                    "unsupported_items": 0,
                    "failed_items": 0
                },
                "language_coverage": [],
                "errors": []
            })
        );
        assert_eq!(
            serde_json::from_value::<ReviewStructure>(value.clone()).unwrap(),
            review
        );
    }
}
