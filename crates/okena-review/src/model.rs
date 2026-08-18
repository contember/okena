use okena_core::review::{
    ComparisonSide, ImmutableResolvedComparison, ReviewCoverage, ReviewNavigationTarget,
    ReviewTruncation, TruncationReason,
};
use okena_syntax::{
    CallFact, SourceRange, SymbolFact, SymbolKey, SyntaxLanguage, SyntaxProvenance,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
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

#[derive(Debug)]
pub(crate) enum ControlledModelError<E> {
    Invalid(ModelError),
    Stopped(E),
}

impl<E> From<ModelError> for ControlledModelError<E> {
    fn from(error: ModelError) -> Self {
        Self::Invalid(error)
    }
}

pub(crate) fn checked_stable_sort_by<T, E>(
    items: &mut Vec<T>,
    mut compare: impl FnMut(&T, &T, &mut dyn FnMut() -> Result<(), E>) -> Result<std::cmp::Ordering, E>,
    checkpoint: &mut dyn FnMut() -> Result<(), E>,
) -> Result<(), E> {
    let len = items.len();
    if len <= 1 {
        return checkpoint();
    }
    let mut operations = 0_u8;
    let mut order = Vec::with_capacity(len);
    for index in 0..len {
        checked_sort_tick(&mut operations, checkpoint)?;
        order.push(index);
    }

    let mut width = 1_usize;
    while width < len {
        let mut merged = Vec::with_capacity(len);
        let mut start = 0_usize;
        while start < len {
            checked_sort_tick(&mut operations, checkpoint)?;
            let middle = start.saturating_add(width).min(len);
            let end = middle.saturating_add(width).min(len);
            let (mut left, mut right) = (start, middle);
            while left < middle && right < end {
                checked_sort_tick(&mut operations, checkpoint)?;
                if compare(&items[order[left]], &items[order[right]], checkpoint)?
                    != std::cmp::Ordering::Greater
                {
                    merged.push(order[left]);
                    left += 1;
                } else {
                    merged.push(order[right]);
                    right += 1;
                }
            }
            while left < middle {
                checked_sort_tick(&mut operations, checkpoint)?;
                merged.push(order[left]);
                left += 1;
            }
            while right < end {
                checked_sort_tick(&mut operations, checkpoint)?;
                merged.push(order[right]);
                right += 1;
            }
            start = end;
        }
        order = merged;
        width = width.saturating_mul(2);
    }

    let mut slots = Vec::with_capacity(len);
    for item in std::mem::take(items) {
        checked_sort_tick(&mut operations, checkpoint)?;
        slots.push(Some(item));
    }
    for index in order {
        checked_sort_tick(&mut operations, checkpoint)?;
        let Some(item) = slots[index].take() else {
            unreachable!("checked sort order contains each source index once");
        };
        items.push(item);
    }
    checkpoint()
}

fn checked_sort_tick<E>(
    operations: &mut u8,
    checkpoint: &mut dyn FnMut() -> Result<(), E>,
) -> Result<(), E> {
    *operations = operations.wrapping_add(1);
    if *operations % 64 == 1 {
        checkpoint()
    } else {
        Ok(())
    }
}

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
        match Self::new_controlled(provenance, symbol, children, &mut || {
            Ok::<(), Infallible>(())
        }) {
            Ok(fact) => Ok(fact),
            Err(ControlledModelError::Invalid(error)) => Err(error),
            Err(ControlledModelError::Stopped(never)) => match never {},
        }
    }

    pub(crate) fn new_controlled<E>(
        provenance: SyntaxProvenance,
        symbol: SymbolReference,
        children: Vec<Self>,
        checkpoint: &mut dyn FnMut() -> Result<(), E>,
    ) -> Result<Self, ControlledModelError<E>> {
        checkpoint().map_err(ControlledModelError::Stopped)?;
        for child in &children {
            checkpoint().map_err(ControlledModelError::Stopped)?;
            if child.provenance != provenance
                || child.symbol.side() != symbol.side()
                || !symbol.range().contains(child.symbol.range())
            {
                return Err(ModelError::new(
                    "outline children must share provenance and be on the same side inside their parent",
                )
                .into());
            }
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
    pub(crate) fn new_controlled<E>(
        kind: SymbolChangeKind,
        old: Option<SymbolFact>,
        new: Option<SymbolFact>,
        signature_change: Option<SignatureChange>,
        body_changed: bool,
        hunks: Vec<ChangedHunk>,
        navigation: ReviewNavigationTarget,
        checkpoint: &mut dyn FnMut() -> Result<(), E>,
    ) -> Result<Self, ControlledModelError<E>> {
        Self::new_validated_controlled(
            kind,
            old,
            new,
            signature_change,
            body_changed,
            hunks,
            navigation,
            None,
            checkpoint,
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
        match Self::new_validated_controlled(
            kind,
            old,
            new,
            signature_change,
            body_changed,
            hunks,
            navigation,
            reported_counts,
            &mut || Ok::<(), Infallible>(()),
        ) {
            Ok(change) => Ok(change),
            Err(ControlledModelError::Invalid(error)) => Err(error),
            Err(ControlledModelError::Stopped(never)) => match never {},
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn new_validated_controlled<E>(
        kind: SymbolChangeKind,
        old: Option<SymbolFact>,
        new: Option<SymbolFact>,
        signature_change: Option<SignatureChange>,
        body_changed: bool,
        hunks: Vec<ChangedHunk>,
        navigation: ReviewNavigationTarget,
        reported_counts: Option<(u32, u32)>,
        checkpoint: &mut dyn FnMut() -> Result<(), E>,
    ) -> Result<Self, ControlledModelError<E>> {
        checkpoint().map_err(ControlledModelError::Stopped)?;
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
            return Err(ModelError::new("symbol change shape does not match its kind").into());
        }
        if let (Some(old), Some(new)) = (&old, &new) {
            if !symbol_keys_equal_controlled(old.key(), new.key(), checkpoint)? {
                return Err(
                    ModelError::new("matched symbols must have the same qualified key").into(),
                );
            }
            if old.provenance().language() != new.provenance().language() {
                return Err(
                    ModelError::new("matched symbols must use the same syntax language").into(),
                );
            }
            if signature_change.is_none()
                && old.normalized_signature() != new.normalized_signature()
            {
                return Err(ModelError::new(
                    "body-only changes require an unchanged normalized signature",
                )
                .into());
            }
            if let Some(signature) = &signature_change {
                signature.validate_facts(old, new)?;
            }
        }
        if hunks.is_empty() {
            return Err(ModelError::new("symbol changes require changed hunks").into());
        }
        let mut unique_hunks = HashSet::with_capacity(hunks.len());
        for hunk in &hunks {
            checkpoint().map_err(ControlledModelError::Stopped)?;
            if !unique_hunks.insert(hunk) {
                return Err(ModelError::new("symbol changes cannot cite duplicate hunks").into());
            }
        }
        for hunk in &hunks {
            checkpoint().map_err(ControlledModelError::Stopped)?;
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
                )
                .into());
            }
        }
        if let (Some(signature), Some(old), Some(new)) = (&signature_change, &old, &new) {
            if !side_has_intersection_controlled(
                &hunks,
                ComparisonSide::Base,
                signature.old_range(),
                checkpoint,
            )? && !side_has_intersection_controlled(
                &hunks,
                ComparisonSide::Head,
                signature.new_range(),
                checkpoint,
            )? {
                return Err(ModelError::new(
                    "signature changes require hunk evidence on at least one exact signature range",
                )
                .into());
            }
            signature.validate_facts(old, new)?;
        }
        if body_changed {
            let mut body_has_evidence = false;
            for (side, fact) in [
                (ComparisonSide::Base, old.as_ref()),
                (ComparisonSide::Head, new.as_ref()),
            ] {
                checkpoint().map_err(ControlledModelError::Stopped)?;
                if let Some(body) = fact.and_then(SymbolFact::body_range)
                    && side_has_intersection_controlled(&hunks, side, body, checkpoint)?
                {
                    body_has_evidence = true;
                    break;
                }
            }
            if !body_has_evidence {
                return Err(ModelError::new(
                    "body changes require hunk evidence on at least one body range",
                )
                .into());
            }
        }
        if kind == SymbolChangeKind::Modified {
            for hunk in &hunks {
                checkpoint().map_err(ControlledModelError::Stopped)?;
                for (side, fact) in [
                    (ComparisonSide::Base, old.as_ref()),
                    (ComparisonSide::Head, new.as_ref()),
                ] {
                    checkpoint().map_err(ControlledModelError::Stopped)?;
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
                        )
                        .into());
                    }
                }
            }
        }
        let changed_old_lines = match old.as_ref() {
            Some(fact) => changed_line_count_controlled(
                &hunks,
                ComparisonSide::Base,
                fact.full_range(),
                checkpoint,
            )?,
            None => 0,
        };
        let changed_new_lines = match new.as_ref() {
            Some(fact) => changed_line_count_controlled(
                &hunks,
                ComparisonSide::Head,
                fact.full_range(),
                checkpoint,
            )?,
            None => 0,
        };
        if changed_old_lines == 0 && changed_new_lines == 0 {
            return Err(ModelError::new("symbol changes require changed-line evidence").into());
        }
        if reported_counts.is_some_and(|counts| counts != (changed_old_lines, changed_new_lines)) {
            return Err(ModelError::new(
                "serialized changed-line counts must equal derived hunk intersections",
            )
            .into());
        }
        validate_navigation(&navigation)?;
        checkpoint().map_err(ControlledModelError::Stopped)?;
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

fn side_has_intersection_controlled<E>(
    hunks: &[ChangedHunk],
    side: ComparisonSide,
    source: SourceRange,
    checkpoint: &mut dyn FnMut() -> Result<(), E>,
) -> Result<bool, ControlledModelError<E>> {
    for hunk in hunks {
        checkpoint().map_err(ControlledModelError::Stopped)?;
        if hunk_range(hunk, side).is_some_and(|range| range.intersects_source(source)) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn hunk_range(hunk: &ChangedHunk, side: ComparisonSide) -> Option<ChangedLineRange> {
    match side {
        ComparisonSide::Base => hunk.old(),
        ComparisonSide::Head => hunk.new_range(),
    }
}

fn changed_line_count_controlled<E>(
    hunks: &[ChangedHunk],
    side: ComparisonSide,
    source: SourceRange,
    checkpoint: &mut dyn FnMut() -> Result<(), E>,
) -> Result<u32, ControlledModelError<E>> {
    let mut intersections = Vec::with_capacity(hunks.len());
    for hunk in hunks {
        checkpoint().map_err(ControlledModelError::Stopped)?;
        if let Some(range) = hunk_range(hunk, side) {
            let start = range.start().get().max(source.start_line().get());
            let end = range.end().get().min(source.end_line().get());
            if start <= end {
                intersections.push((start, end));
            }
        }
    }
    checked_stable_sort_by(
        &mut intersections,
        |left, right, _| Ok(left.cmp(right)),
        checkpoint,
    )
    .map_err(ControlledModelError::Stopped)?;
    let mut total = 0_u64;
    let mut current: Option<(u32, u32)> = None;
    for (start, end) in intersections {
        checkpoint().map_err(ControlledModelError::Stopped)?;
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
    u32::try_from(total)
        .map_err(|_| ModelError::new("derived changed-line count overflowed").into())
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
    /// The callee repeats, but once the occurrences identical on both sides were cancelled
    /// exactly one changed occurrence remained inside each enclosing range.
    UniqueChangedOccurrenceWithinEnclosingRange,
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
        match Self::new_controlled(
            kind,
            old,
            new,
            arguments_changed,
            control_context_changed,
            pairing,
            navigation,
            &mut || Ok::<(), Infallible>(()),
        ) {
            Ok(change) => Ok(change),
            Err(ControlledModelError::Invalid(error)) => Err(error),
            Err(ControlledModelError::Stopped(never)) => match never {},
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_controlled<E>(
        kind: CallChangeKind,
        old: Option<CallFact>,
        new: Option<CallFact>,
        arguments_changed: bool,
        control_context_changed: bool,
        pairing: Option<CallPairingEvidence>,
        navigation: ReviewNavigationTarget,
        checkpoint: &mut dyn FnMut() -> Result<(), E>,
    ) -> Result<Self, ControlledModelError<E>> {
        checkpoint().map_err(ControlledModelError::Stopped)?;
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
            )
            .into());
        }
        if let (Some(old), Some(new)) = (&old, &new)
            && (old.callee_text() != new.callee_text()
                || !optional_symbol_keys_equal_controlled(
                    old.enclosing_symbol(),
                    new.enclosing_symbol(),
                    checkpoint,
                )?)
        {
            return Err(ModelError::new(
                "paired call modifications require the same callee and enclosing symbol",
            )
            .into());
        }
        if let (Some(old), Some(new)) = (&old, &new) {
            let actual_arguments_changed = old.argument_text() != new.argument_text();
            let actual_control_context_changed = !control_contexts_equal_controlled(
                old.control_context(),
                new.control_context(),
                checkpoint,
            )?;
            if arguments_changed != actual_arguments_changed
                || control_context_changed != actual_control_context_changed
            {
                return Err(ModelError::new(
                    "call modification flags must match the paired syntactic facts",
                )
                .into());
            }
            let evidence = pairing.as_ref().ok_or_else(|| {
                ModelError::new("modified calls require explicit pairing evidence")
            })?;
            if evidence.old_call_range() != old.call_site_range()
                || evidence.new_call_range() != new.call_site_range()
            {
                return Err(ModelError::new(
                    "call pairing evidence must name the paired call-site locations",
                )
                .into());
            }
        }
        validate_navigation(&navigation)?;
        checkpoint().map_err(ControlledModelError::Stopped)?;
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

fn control_contexts_equal_controlled<E>(
    old: &[okena_syntax::ControlContext],
    new: &[okena_syntax::ControlContext],
    checkpoint: &mut dyn FnMut() -> Result<(), E>,
) -> Result<bool, ControlledModelError<E>> {
    checkpoint().map_err(ControlledModelError::Stopped)?;
    if old.len() != new.len() {
        return Ok(false);
    }
    for (old, new) in old.iter().zip(new) {
        checkpoint().map_err(ControlledModelError::Stopped)?;
        if old != new {
            return Ok(false);
        }
    }
    Ok(true)
}

fn optional_symbol_keys_equal_controlled<E>(
    old: Option<&SymbolKey>,
    new: Option<&SymbolKey>,
    checkpoint: &mut dyn FnMut() -> Result<(), E>,
) -> Result<bool, ControlledModelError<E>> {
    match (old, new) {
        (Some(old), Some(new)) => symbol_keys_equal_controlled(old, new, checkpoint),
        (None, None) => Ok(true),
        (Some(_), None) | (None, Some(_)) => Ok(false),
    }
}

fn symbol_keys_equal_controlled<E>(
    old: &SymbolKey,
    new: &SymbolKey,
    checkpoint: &mut dyn FnMut() -> Result<(), E>,
) -> Result<bool, ControlledModelError<E>> {
    checkpoint().map_err(ControlledModelError::Stopped)?;
    if old.kind() != new.kind()
        || old.name() != new.name()
        || old.qualified_path().len() != new.qualified_path().len()
    {
        return Ok(false);
    }
    for (old, new) in old.qualified_path().iter().zip(new.qualified_path()) {
        checkpoint().map_err(ControlledModelError::Stopped)?;
        if old != new {
            return Ok(false);
        }
    }
    Ok(true)
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
        match Self::new_controlled(
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
            &mut || Ok::<(), Infallible>(()),
        ) {
            Ok(file) => Ok(file),
            Err(ControlledModelError::Invalid(error)) => Err(error),
            Err(ControlledModelError::Stopped(never)) => match never {},
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_controlled<E>(
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
        checkpoint: &mut dyn FnMut() -> Result<(), E>,
    ) -> Result<Self, ControlledModelError<E>> {
        checkpoint().map_err(ControlledModelError::Stopped)?;
        if old_path.is_none() && new_path.is_none() {
            return Err(ModelError::new("structured file requires at least one path").into());
        }
        if old_path.as_ref().is_some_and(|path| path.trim().is_empty())
            || new_path.as_ref().is_some_and(|path| path.trim().is_empty())
        {
            return Err(ModelError::new("structured file paths must not be empty").into());
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
            return Err(
                ModelError::new("unsuccessful files cannot contain structured facts").into(),
            );
        }
        if status == FileAnalysisStatus::Unsupported && language.is_some() {
            return Err(ModelError::new("unsupported files cannot claim a syntax language").into());
        }
        if status == FileAnalysisStatus::Unsupported
            && (old_provenance.is_some() || new_provenance.is_some())
        {
            return Err(ModelError::new("unsupported files cannot claim syntax provenance").into());
        }
        if status == FileAnalysisStatus::Pending
            && (old_provenance.is_some() || new_provenance.is_some())
        {
            return Err(ModelError::new("pending files cannot claim syntax provenance").into());
        }
        if matches!(
            status,
            FileAnalysisStatus::Parsed | FileAnalysisStatus::Partial
        ) && language.is_none()
        {
            return Err(
                ModelError::new("successful structured files require a syntax language").into(),
            );
        }
        if matches!(
            status,
            FileAnalysisStatus::Parsed | FileAnalysisStatus::Partial
        ) && (old_path.is_some() != old_provenance.is_some()
            || new_path.is_some() != new_provenance.is_some())
        {
            return Err(ModelError::new(
                "analyzed snapshot paths require matching syntax provenance",
            )
            .into());
        }
        if status == FileAnalysisStatus::Parsed && (truncation.is_some() || !errors.is_empty()) {
            return Err(ModelError::new("parsed files cannot carry errors or truncation").into());
        }
        if let Some(truncation) = &truncation {
            validate_review_truncation(truncation)?;
        }
        if status == FileAnalysisStatus::Partial && truncation.is_none() && errors.is_empty() {
            return Err(ModelError::new("partial files require errors or truncation").into());
        }
        if status == FileAnalysisStatus::Pending {
            if truncation.is_none() && errors.is_empty() {
                return Err(ModelError::new(
                    "pending files require budget, cancellation, time, or analysis error evidence",
                )
                .into());
            }
            for error in &errors {
                checkpoint().map_err(ControlledModelError::Stopped)?;
                if error.stage() != AnalysisStage::Budget {
                    return Err(ModelError::new(
                        "pending files can only carry budget-stage analysis errors",
                    )
                    .into());
                }
            }
            if truncation
                .as_ref()
                .is_some_and(|truncation| !is_pending_truncation(truncation.reason))
            {
                return Err(ModelError::new(
                    "pending file truncation must describe a budget, cancellation, or time limit",
                )
                .into());
            }
        }
        if status == FileAnalysisStatus::Failed && errors.is_empty() {
            return Err(ModelError::new("failed files require analysis error evidence").into());
        }
        let mut unique_file_hunks = HashSet::with_capacity(changed_hunks.len());
        for hunk in &changed_hunks {
            checkpoint().map_err(ControlledModelError::Stopped)?;
            if !unique_file_hunks.insert(hunk) {
                return Err(ModelError::new(
                    "structured files cannot contain duplicate changed hunks",
                )
                .into());
            }
        }
        for change in &symbol_changes {
            checkpoint().map_err(ControlledModelError::Stopped)?;
            for hunk in change.hunks() {
                checkpoint().map_err(ControlledModelError::Stopped)?;
                if !unique_file_hunks.contains(hunk) {
                    return Err(ModelError::new(
                        "symbol changes can only cite hunks from their structured file",
                    )
                    .into());
                }
            }
        }
        for fact in old_outline.iter() {
            checkpoint().map_err(ControlledModelError::Stopped)?;
            if fact.symbol().side() != ComparisonSide::Base
                || Some(fact.provenance()) != old_provenance.as_ref()
            {
                return Err(ModelError::new(
                    "old outline must use the base side and old document provenance",
                )
                .into());
            }
        }
        for fact in new_outline.iter() {
            checkpoint().map_err(ControlledModelError::Stopped)?;
            if fact.symbol().side() != ComparisonSide::Head
                || Some(fact.provenance()) != new_provenance.as_ref()
            {
                return Err(ModelError::new(
                    "new outline must use the head side and new document provenance",
                )
                .into());
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
            for provenance in file.old_provenance.iter().chain(file.new_provenance.iter()) {
                checkpoint().map_err(ControlledModelError::Stopped)?;
                if provenance.language() != language {
                    return Err(ModelError::new(
                        "document provenance must use the file syntax language",
                    )
                    .into());
                }
            }
            for change in &file.symbol_changes {
                checkpoint().map_err(ControlledModelError::Stopped)?;
                for fact in change.old().into_iter().chain(change.new_fact()) {
                    checkpoint().map_err(ControlledModelError::Stopped)?;
                    if fact.provenance().language() != language {
                        return Err(ModelError::new(
                            "structured facts must use the file syntax language",
                        )
                        .into());
                    }
                }
            }
            for hotspot in &file.hotspots {
                checkpoint().map_err(ControlledModelError::Stopped)?;
                if hotspot.provenance().language() != language {
                    return Err(ModelError::new(
                        "structured facts must use the file syntax language",
                    )
                    .into());
                }
            }
            for change in &file.call_diff {
                checkpoint().map_err(ControlledModelError::Stopped)?;
                for fact in change.old().into_iter().chain(change.new_fact()) {
                    checkpoint().map_err(ControlledModelError::Stopped)?;
                    if fact.provenance().language() != language {
                        return Err(ModelError::new(
                            "structured facts must use the file syntax language",
                        )
                        .into());
                    }
                }
            }
        }
        for navigation in file
            .symbol_changes
            .iter()
            .map(SymbolChange::navigation)
            .chain(file.hotspots.iter().map(StructuralHotspot::navigation))
            .chain(file.call_diff.iter().map(CallDiffChange::navigation))
        {
            checkpoint().map_err(ControlledModelError::Stopped)?;
            let expected = file
                .path_on(navigation.side)
                .ok_or_else(|| ModelError::new("navigation targets a missing comparison side"))?;
            if navigation.path != expected {
                return Err(ModelError::new(
                    "navigation path does not match its structured file side",
                )
                .into());
            }
        }
        checkpoint().map_err(ControlledModelError::Stopped)?;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OmittedFileReason {
    UnsupportedLanguage,
    Binary,
    Submodule,
    ModeOnly,
    WhitespaceIgnored,
    FileLimit,
    SourceByteLimit,
    AggregateByteLimit,
    TimeLimit,
    FactLimit,
    ResponseLimit,
    Cancelled,
}

impl OmittedFileReason {
    pub fn status(self) -> FileAnalysisStatus {
        match self {
            Self::UnsupportedLanguage | Self::Binary | Self::Submodule => {
                FileAnalysisStatus::Unsupported
            }
            Self::ModeOnly | Self::WhitespaceIgnored => FileAnalysisStatus::Skipped,
            Self::FileLimit
            | Self::SourceByteLimit
            | Self::AggregateByteLimit
            | Self::TimeLimit
            | Self::FactLimit
            | Self::ResponseLimit
            | Self::Cancelled => FileAnalysisStatus::Pending,
        }
    }

    fn truncation_reason(self) -> Option<TruncationReason> {
        match self {
            Self::UnsupportedLanguage
            | Self::Binary
            | Self::Submodule
            | Self::ModeOnly
            | Self::WhitespaceIgnored => None,
            Self::FileLimit => Some(TruncationReason::ItemLimit),
            Self::SourceByteLimit | Self::AggregateByteLimit => Some(TruncationReason::ByteLimit),
            Self::TimeLimit => Some(TruncationReason::TimeLimit),
            Self::FactLimit => Some(TruncationReason::CaptureLimit),
            Self::ResponseLimit => Some(TruncationReason::ResponseLimit),
            Self::Cancelled => Some(TruncationReason::Cancelled),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OmittedFileGroup {
    count: u64,
    language: Option<SyntaxLanguage>,
    reason: OmittedFileReason,
    status: FileAnalysisStatus,
    truncation: Option<ReviewTruncation>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OmittedFileGroupWire {
    count: u64,
    language: Option<SyntaxLanguage>,
    reason: OmittedFileReason,
    status: FileAnalysisStatus,
    truncation: Option<ReviewTruncation>,
}

impl TryFrom<OmittedFileGroupWire> for OmittedFileGroup {
    type Error = ModelError;

    fn try_from(value: OmittedFileGroupWire) -> Result<Self, Self::Error> {
        Self::new_with_status(
            value.count,
            value.language,
            value.reason,
            value.status,
            value.truncation,
        )
    }
}

impl<'de> Deserialize<'de> for OmittedFileGroup {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        OmittedFileGroupWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}

impl OmittedFileGroup {
    pub fn new(
        count: u64,
        language: Option<SyntaxLanguage>,
        reason: OmittedFileReason,
        truncation: Option<ReviewTruncation>,
    ) -> Result<Self, ModelError> {
        Self::new_with_status(count, language, reason, reason.status(), truncation)
    }

    fn new_with_status(
        count: u64,
        language: Option<SyntaxLanguage>,
        reason: OmittedFileReason,
        status: FileAnalysisStatus,
        truncation: Option<ReviewTruncation>,
    ) -> Result<Self, ModelError> {
        if count == 0 {
            return Err(ModelError::new("omitted file group count must be positive"));
        }
        if status != reason.status() {
            return Err(ModelError::new(
                "omitted file status must match its deterministic reason category",
            ));
        }
        if reason == OmittedFileReason::UnsupportedLanguage && language.is_some() {
            return Err(ModelError::new(
                "unsupported-language omissions cannot claim a supported syntax language",
            ));
        }
        match (reason.truncation_reason(), truncation.as_ref()) {
            (None, None) => {}
            (None, Some(_)) => {
                return Err(ModelError::new(
                    "non-pending omission reasons cannot carry truncation evidence",
                ));
            }
            (Some(_), None) => {
                return Err(ModelError::new(
                    "pending omission reasons require truncation evidence",
                ));
            }
            (Some(expected), Some(actual)) if actual.reason != expected => {
                return Err(ModelError::new(
                    "omission truncation reason does not match its omission reason",
                ));
            }
            (Some(_), Some(actual)) => validate_review_truncation(actual)?,
        }
        Ok(Self {
            count,
            language,
            reason,
            status,
            truncation,
        })
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn language(&self) -> Option<SyntaxLanguage> {
        self.language
    }

    pub fn reason(&self) -> OmittedFileReason {
        self.reason
    }

    pub fn status(&self) -> FileAnalysisStatus {
        self.status
    }

    pub fn truncation(&self) -> Option<&ReviewTruncation> {
        self.truncation.as_ref()
    }

    fn equivalent_to(&self, other: &Self) -> bool {
        self.language == other.language
            && self.reason == other.reason
            && self.status == other.status
            && self.truncation == other.truncation
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReviewStructure {
    comparison: ImmutableResolvedComparison,
    files: Vec<StructuredFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    omissions: Vec<OmittedFileGroup>,
    coverage: ReviewCoverage,
    language_coverage: Vec<LanguageCoverage>,
    errors: Vec<AnalysisError>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewStructureWire {
    comparison: ImmutableResolvedComparison,
    files: Vec<StructuredFile>,
    #[serde(default)]
    omissions: Vec<OmittedFileGroup>,
    coverage: ReviewCoverage,
    language_coverage: Vec<LanguageCoverage>,
    errors: Vec<AnalysisError>,
}
impl TryFrom<ReviewStructureWire> for ReviewStructure {
    type Error = ModelError;
    fn try_from(value: ReviewStructureWire) -> Result<Self, Self::Error> {
        Self::new_with_omissions(
            value.comparison,
            value.files,
            value.omissions,
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
        Self::new_with_omissions(
            comparison,
            files,
            Vec::new(),
            coverage,
            language_coverage,
            errors,
        )
    }

    pub fn new_with_omissions(
        comparison: ImmutableResolvedComparison,
        files: Vec<StructuredFile>,
        mut omissions: Vec<OmittedFileGroup>,
        coverage: ReviewCoverage,
        language_coverage: Vec<LanguageCoverage>,
        errors: Vec<AnalysisError>,
    ) -> Result<Self, ModelError> {
        if let Some(truncation) = coverage.truncation() {
            validate_review_truncation(truncation)?;
        }
        if omissions.iter().enumerate().any(|(index, omission)| {
            omissions[..index]
                .iter()
                .any(|earlier| omission.equivalent_to(earlier))
        }) {
            return Err(ModelError::new(
                "equivalent omitted file groups must be combined",
            ));
        }
        omissions.sort_by(compare_omitted_file_groups);
        validate_coverage(&files, &omissions, &coverage, None)?;
        let mut seen = HashSet::new();
        for language in &language_coverage {
            if !seen.insert(language.language()) {
                return Err(ModelError::new("language coverage entries must be unique"));
            }
            if let Some(truncation) = language.coverage().truncation() {
                validate_review_truncation(truncation)?;
            }
            validate_coverage(
                &files,
                &omissions,
                language.coverage(),
                Some(language.language()),
            )?;
        }
        let covered_languages: HashSet<_> = files
            .iter()
            .filter_map(StructuredFile::language)
            .chain(omissions.iter().filter_map(OmittedFileGroup::language))
            .collect();
        if seen != covered_languages {
            return Err(ModelError::new(
                "language coverage must account for every detected language exactly once",
            ));
        }
        Ok(Self {
            comparison,
            files,
            omissions,
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
    pub fn omissions(&self) -> &[OmittedFileGroup] {
        &self.omissions
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

fn compare_omitted_file_groups(
    left: &OmittedFileGroup,
    right: &OmittedFileGroup,
) -> std::cmp::Ordering {
    optional_language_rank(left.language())
        .cmp(&optional_language_rank(right.language()))
        .then_with(|| {
            omission_reason_rank(left.reason()).cmp(&omission_reason_rank(right.reason()))
        })
        .then_with(|| {
            left.truncation()
                .and_then(|truncation| truncation.limit)
                .cmp(&right.truncation().and_then(|truncation| truncation.limit))
        })
        .then_with(|| {
            left.truncation()
                .and_then(|truncation| truncation.observed)
                .cmp(
                    &right
                        .truncation()
                        .and_then(|truncation| truncation.observed),
                )
        })
        .then_with(|| {
            left.truncation()
                .and_then(|truncation| truncation.detail.as_deref())
                .cmp(
                    &right
                        .truncation()
                        .and_then(|truncation| truncation.detail.as_deref()),
                )
        })
        .then_with(|| left.count().cmp(&right.count()))
}

fn optional_language_rank(language: Option<SyntaxLanguage>) -> u8 {
    match language {
        None => 0,
        Some(SyntaxLanguage::Rust) => 1,
        Some(SyntaxLanguage::TypeScript) => 2,
        Some(SyntaxLanguage::Tsx) => 3,
    }
}

fn omission_reason_rank(reason: OmittedFileReason) -> u8 {
    match reason {
        OmittedFileReason::UnsupportedLanguage => 0,
        OmittedFileReason::Binary => 1,
        OmittedFileReason::Submodule => 2,
        OmittedFileReason::ModeOnly => 3,
        OmittedFileReason::WhitespaceIgnored => 4,
        OmittedFileReason::FileLimit => 5,
        OmittedFileReason::SourceByteLimit => 6,
        OmittedFileReason::AggregateByteLimit => 7,
        OmittedFileReason::TimeLimit => 8,
        OmittedFileReason::FactLimit => 9,
        OmittedFileReason::ResponseLimit => 10,
        OmittedFileReason::Cancelled => 11,
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
    omissions: &[OmittedFileGroup],
    coverage: &ReviewCoverage,
    language: Option<SyntaxLanguage>,
) -> Result<(), ModelError> {
    let mut counts: HashMap<FileAnalysisStatus, u64> = HashMap::new();
    for file in files
        .iter()
        .filter(|file| language.is_none_or(|language| file.language() == Some(language)))
    {
        add_coverage_count(&mut counts, file.status(), 1)?;
    }
    for omission in omissions
        .iter()
        .filter(|omission| language.is_none_or(|language| omission.language() == Some(language)))
    {
        add_coverage_count(&mut counts, omission.status(), omission.count())?;
    }
    let analyzed = counts
        .get(&FileAnalysisStatus::Parsed)
        .copied()
        .unwrap_or(0)
        .checked_add(
            counts
                .get(&FileAnalysisStatus::Partial)
                .copied()
                .unwrap_or(0),
        )
        .ok_or_else(|| ModelError::new("coverage count overflow"))?;
    let pending = counts
        .get(&FileAnalysisStatus::Pending)
        .copied()
        .unwrap_or(0);
    let total = counts.values().try_fold(0_u64, |total, count| {
        total
            .checked_add(*count)
            .ok_or_else(|| ModelError::new("coverage count overflow"))
    })?;
    let matches = coverage.total_items() == total
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
            "coverage does not account for its structured files and omissions",
        ))
    }
}

fn add_coverage_count(
    counts: &mut HashMap<FileAnalysisStatus, u64>,
    status: FileAnalysisStatus,
    count: u64,
) -> Result<(), ModelError> {
    let current = counts.entry(status).or_default();
    *current = current
        .checked_add(count)
        .ok_or_else(|| ModelError::new("coverage count overflow"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use okena_core::review::{
        ComparisonStrategy, GitObjectId, ResolvedComparison, ReviewComparisonId, ReviewSnapshot,
    };
    use okena_core::types::DiffMode;
    use okena_syntax::{ControlContext, SymbolKind, SymbolVisibility};
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

    fn omission(
        count: u64,
        language: Option<SyntaxLanguage>,
        reason: OmittedFileReason,
    ) -> OmittedFileGroup {
        let truncation = match reason {
            OmittedFileReason::UnsupportedLanguage
            | OmittedFileReason::Binary
            | OmittedFileReason::Submodule
            | OmittedFileReason::ModeOnly
            | OmittedFileReason::WhitespaceIgnored => None,
            OmittedFileReason::FileLimit => {
                Some(measured_truncation(TruncationReason::ItemLimit, 100, 100))
            }
            OmittedFileReason::SourceByteLimit | OmittedFileReason::AggregateByteLimit => Some(
                measured_truncation(TruncationReason::ByteLimit, 1_000, 1_001),
            ),
            OmittedFileReason::TimeLimit => Some(measured_truncation(
                TruncationReason::TimeLimit,
                50_000,
                50_001,
            )),
            OmittedFileReason::FactLimit => Some(measured_truncation(
                TruncationReason::CaptureLimit,
                500,
                500,
            )),
            OmittedFileReason::ResponseLimit => Some(measured_truncation(
                TruncationReason::ResponseLimit,
                10_000,
                10_001,
            )),
            OmittedFileReason::Cancelled => Some(ReviewTruncation {
                reason: TruncationReason::Cancelled,
                limit: None,
                observed: None,
                detail: None,
            }),
        };
        OmittedFileGroup::new(count, language, reason, truncation).unwrap()
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
    fn controlled_structured_file_validation_stops_inside_hunk_membership_work() {
        let hunks: Vec<_> = (1_u32..=100)
            .map(|line| {
                ChangedHunk::new(
                    None,
                    Some(ChangedLineRange::new(nz(line), nz(line)).unwrap()),
                )
                .unwrap()
            })
            .collect();
        let mut checks = 0_u32;
        let error = StructuredFile::new_controlled(
            None,
            Some("src/lib.rs".into()),
            Some(SyntaxLanguage::Rust),
            None,
            Some(provenance()),
            FileAnalysisStatus::Parsed,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            hunks,
            Vec::new(),
            None,
            &mut || {
                checks += 1;
                if checks == 40 { Err("stopped") } else { Ok(()) }
            },
        )
        .unwrap_err();

        assert!(matches!(error, ControlledModelError::Stopped("stopped")));
    }

    #[test]
    fn checked_stable_sort_can_stop_after_comparisons_begin() {
        let comparisons = Cell::new(0_u32);
        let mut values: Vec<_> = (0_u32..100).rev().collect();
        let error = checked_stable_sort_by(
            &mut values,
            |left, right, _| {
                comparisons.set(comparisons.get() + 1);
                Ok(left.cmp(right))
            },
            &mut || {
                if comparisons.get() >= 10 {
                    Err("stopped")
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();

        assert_eq!(error, "stopped");
        assert!(comparisons.get() >= 10);
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
    fn controlled_modified_call_validation_stops_inside_context_comparison() {
        let old_contexts: Vec<_> = (0_u32..100)
            .map(|index| ControlContext::Other(format!("context-{index}")))
            .collect();
        let mut new_contexts = old_contexts.clone();
        new_contexts[99] = ControlContext::Other("changed".into());
        let enclosing = symbol("pub fn run()");
        let old = CallFact::new(
            provenance(),
            "work",
            "value",
            range(20, 22, 2, 2),
            range(16, 23, 2, 2),
            Some(enclosing.key().clone()),
            old_contexts,
        )
        .unwrap();
        let new = CallFact::new(
            provenance(),
            "work",
            "value",
            range(20, 22, 2, 2),
            range(16, 23, 2, 2),
            Some(enclosing.key().clone()),
            new_contexts,
        )
        .unwrap();
        let pairing = CallPairingEvidence::new(
            CallPairingStrategy::UniqueOccurrenceWithinEnclosingRange,
            old.call_site_range(),
            new.call_site_range(),
            enclosing.full_range(),
            enclosing.full_range(),
            1,
            1,
        )
        .unwrap();
        let mut checks = 0_u32;
        let error = CallDiffChange::new_controlled(
            CallChangeKind::Modified,
            Some(old),
            Some(new),
            false,
            true,
            Some(pairing),
            navigation(ComparisonSide::Head),
            &mut || {
                checks += 1;
                if checks == 50 { Err("stopped") } else { Ok(()) }
            },
        )
        .unwrap_err();

        assert!(matches!(error, ControlledModelError::Stopped("stopped")));
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
    fn every_omission_reason_has_a_fixed_status_and_evidence_shape() {
        let cases = [
            (
                OmittedFileReason::UnsupportedLanguage,
                FileAnalysisStatus::Unsupported,
                None,
            ),
            (
                OmittedFileReason::Binary,
                FileAnalysisStatus::Unsupported,
                None,
            ),
            (
                OmittedFileReason::Submodule,
                FileAnalysisStatus::Unsupported,
                None,
            ),
            (
                OmittedFileReason::ModeOnly,
                FileAnalysisStatus::Skipped,
                None,
            ),
            (
                OmittedFileReason::WhitespaceIgnored,
                FileAnalysisStatus::Skipped,
                None,
            ),
            (
                OmittedFileReason::FileLimit,
                FileAnalysisStatus::Pending,
                Some(TruncationReason::ItemLimit),
            ),
            (
                OmittedFileReason::SourceByteLimit,
                FileAnalysisStatus::Pending,
                Some(TruncationReason::ByteLimit),
            ),
            (
                OmittedFileReason::AggregateByteLimit,
                FileAnalysisStatus::Pending,
                Some(TruncationReason::ByteLimit),
            ),
            (
                OmittedFileReason::TimeLimit,
                FileAnalysisStatus::Pending,
                Some(TruncationReason::TimeLimit),
            ),
            (
                OmittedFileReason::FactLimit,
                FileAnalysisStatus::Pending,
                Some(TruncationReason::CaptureLimit),
            ),
            (
                OmittedFileReason::ResponseLimit,
                FileAnalysisStatus::Pending,
                Some(TruncationReason::ResponseLimit),
            ),
            (
                OmittedFileReason::Cancelled,
                FileAnalysisStatus::Pending,
                Some(TruncationReason::Cancelled),
            ),
        ];

        for (reason, status, truncation_reason) in cases {
            let language =
                (reason != OmittedFileReason::UnsupportedLanguage).then_some(SyntaxLanguage::Rust);
            let group = omission(3, language, reason);
            assert_eq!(group.count(), 3);
            assert_eq!(group.reason(), reason);
            assert_eq!(group.status(), status);
            assert_eq!(
                group.truncation().map(|truncation| truncation.reason),
                truncation_reason
            );
            assert_eq!(
                serde_json::from_value::<OmittedFileGroup>(serde_json::to_value(&group).unwrap())
                    .unwrap(),
                group
            );
        }
    }

    #[test]
    fn grouped_omissions_support_huge_counts_without_file_expansion() {
        let group = omission(u64::MAX, None, OmittedFileReason::FileLimit);
        let review = ReviewStructure::new_with_omissions(
            immutable_comparison(),
            Vec::new(),
            vec![group],
            ReviewCoverage::new(u64::MAX, 0, u64::MAX, 0, 0, 0, None).unwrap(),
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        assert!(review.files().is_empty());
        assert_eq!(review.omissions().len(), 1);
        assert_eq!(review.omissions()[0].count(), u64::MAX);
    }

    #[test]
    fn unknown_language_omissions_only_contribute_to_aggregate_coverage() {
        let omissions = vec![
            omission(3, None, OmittedFileReason::FileLimit),
            omission(
                2,
                Some(SyntaxLanguage::TypeScript),
                OmittedFileReason::ModeOnly,
            ),
        ];
        let review = ReviewStructure::new_with_omissions(
            immutable_comparison(),
            vec![parsed_file()],
            omissions,
            ReviewCoverage::new(6, 1, 3, 2, 0, 0, None).unwrap(),
            vec![
                LanguageCoverage::new(
                    SyntaxLanguage::Rust,
                    ReviewCoverage::new(1, 1, 0, 0, 0, 0, None).unwrap(),
                ),
                LanguageCoverage::new(
                    SyntaxLanguage::TypeScript,
                    ReviewCoverage::new(2, 0, 0, 2, 0, 0, None).unwrap(),
                ),
            ],
            Vec::new(),
        )
        .unwrap();
        assert_eq!(review.coverage().total_items(), 6);
        assert_eq!(review.language_coverage()[0].coverage().total_items(), 1);
        assert_eq!(review.language_coverage()[1].coverage().total_items(), 2);
    }

    #[test]
    fn invalid_omission_shapes_duplicates_and_coverage_are_rejected() {
        assert!(OmittedFileGroup::new(0, None, OmittedFileReason::FileLimit, None).is_err());
        assert!(
            OmittedFileGroup::new(
                1,
                Some(SyntaxLanguage::Rust),
                OmittedFileReason::UnsupportedLanguage,
                None,
            )
            .is_err()
        );
        assert!(OmittedFileGroup::new(1, None, OmittedFileReason::FileLimit, None).is_err());
        assert!(
            OmittedFileGroup::new(
                1,
                None,
                OmittedFileReason::SourceByteLimit,
                Some(measured_truncation(TruncationReason::ItemLimit, 1, 1)),
            )
            .is_err()
        );
        assert!(
            OmittedFileGroup::new(
                1,
                None,
                OmittedFileReason::Binary,
                Some(measured_truncation(TruncationReason::ByteLimit, 1, 1)),
            )
            .is_err()
        );

        let group = omission(1, None, OmittedFileReason::FileLimit);
        let mut invalid_wire = serde_json::to_value(&group).unwrap();
        invalid_wire["count"] = json!(0);
        assert!(serde_json::from_value::<OmittedFileGroup>(invalid_wire).is_err());
        for invalid_status in [
            FileAnalysisStatus::Parsed,
            FileAnalysisStatus::Partial,
            FileAnalysisStatus::Failed,
            FileAnalysisStatus::Skipped,
        ] {
            let mut invalid_wire = serde_json::to_value(&group).unwrap();
            invalid_wire["status"] = serde_json::to_value(invalid_status).unwrap();
            assert!(serde_json::from_value::<OmittedFileGroup>(invalid_wire).is_err());
        }

        assert!(
            ReviewStructure::new_with_omissions(
                immutable_comparison(),
                Vec::new(),
                vec![group.clone(), group.clone()],
                ReviewCoverage::new(2, 0, 2, 0, 0, 0, None).unwrap(),
                Vec::new(),
                Vec::new(),
            )
            .is_err()
        );
        assert!(
            ReviewStructure::new_with_omissions(
                immutable_comparison(),
                Vec::new(),
                vec![group],
                ReviewCoverage::new(2, 0, 2, 0, 0, 0, None).unwrap(),
                Vec::new(),
                Vec::new(),
            )
            .is_err()
        );
    }

    #[test]
    fn non_empty_omission_response_has_stable_golden_json() {
        let pending_coverage = ReviewCoverage::new(3, 0, 3, 0, 0, 0, None).unwrap();
        let review = ReviewStructure::new_with_omissions(
            immutable_comparison(),
            Vec::new(),
            vec![omission(
                3,
                Some(SyntaxLanguage::Rust),
                OmittedFileReason::TimeLimit,
            )],
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
                "files": [],
                "omissions": [{
                    "count": 3,
                    "language": "rust",
                    "reason": "time_limit",
                    "status": "pending",
                    "truncation": {
                        "reason": "time_limit",
                        "limit": 50_000,
                        "observed": 50_001
                    }
                }],
                "coverage": {
                    "total_items": 3,
                    "analyzed_items": 0,
                    "pending_items": 3,
                    "skipped_items": 0,
                    "unsupported_items": 0,
                    "failed_items": 0
                },
                "language_coverage": [{
                    "language": "rust",
                    "coverage": {
                        "total_items": 3,
                        "analyzed_items": 0,
                        "pending_items": 3,
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
        let mut legacy = value;
        legacy.as_object_mut().unwrap().remove("omissions");
        assert_eq!(
            serde_json::from_value::<ReviewStructure>(legacy).unwrap(),
            review
        );
    }
}
