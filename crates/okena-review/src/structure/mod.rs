//! Deterministic structural comparison for one exact file pair.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;

use okena_core::review::{
    ComparisonSide, ReviewNavigationTarget, ReviewTruncation, TruncationReason,
};
use okena_syntax::{
    CallFact, ControlContext, DiagnosticSeverity, DocumentStatus, DocumentStructure, SourceRange,
    SymbolFact, SymbolKey, SymbolKind, SyntaxLanguage, SyntaxProvenance, SyntaxTruncation,
    SyntaxTruncationReason,
};

use crate::call_diff::{
    CallDiffError, ComparisonStopReason, ControlledCallDiffError, IndexedCallDiffInput,
    compare_indexed_calls_controlled,
};
use crate::model::{ControlledModelError, checked_stable_sort_by};
use crate::{
    AnalysisError, AnalysisStage, CallChangeKind, CallDiffChange, ChangedHunk, ChangedLineRange,
    FileAnalysisStatus, ModelError, OutlineFact, SignatureChange, StructuralHotspot,
    StructuralMetric, StructuredFile, SymbolChange, SymbolChangeKind, SymbolReference,
};

/// Invalid comparator input or a result rejected by the frozen review model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StructureError {
    Invalid(String),
    Stopped(ComparisonStopReason),
}

impl StructureError {
    fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    pub fn stop_reason(&self) -> Option<ComparisonStopReason> {
        match self {
            Self::Stopped(reason) => Some(*reason),
            Self::Invalid(_) => None,
        }
    }
}

impl fmt::Display for StructureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
            Self::Stopped(reason) => {
                write!(formatter, "structural comparison stopped: {reason:?}")
            }
        }
    }
}

impl std::error::Error for StructureError {}

impl From<ModelError> for StructureError {
    fn from(error: ModelError) -> Self {
        Self::Invalid(error.to_string())
    }
}

impl From<CallDiffError> for StructureError {
    fn from(error: CallDiffError) -> Self {
        Self::Invalid(error.to_string())
    }
}

impl From<ControlledCallDiffError> for StructureError {
    fn from(error: ControlledCallDiffError) -> Self {
        match error {
            ControlledCallDiffError::Comparison(error) => Self::Invalid(error.to_string()),
            ControlledCallDiffError::Stopped(reason) => Self::Stopped(reason),
        }
    }
}

impl From<ControlledModelError<ComparisonStopReason>> for StructureError {
    fn from(error: ControlledModelError<ComparisonStopReason>) -> Self {
        match error {
            ControlledModelError::Invalid(error) => Self::Invalid(error.to_string()),
            ControlledModelError::Stopped(reason) => Self::Stopped(reason),
        }
    }
}

/// Compare syntax facts for one exact old/new path pair.
///
/// A missing document is valid only for a missing comparison side. If a path exists but its
/// document is absent, the result is explicitly skipped. Matching uses a unique `SymbolKey`
/// occurrence and identical syntax provenance; ambiguity degrades to added/removed facts when
/// changed-hunk evidence exists.
pub fn compare_structured_file(
    old_path: Option<&str>,
    new_path: Option<&str>,
    old_document: Option<&DocumentStructure>,
    new_document: Option<&DocumentStructure>,
    changed_hunks: &[ChangedHunk],
) -> Result<StructuredFile, StructureError> {
    compare_structured_file_controlled(
        old_path,
        new_path,
        old_document,
        new_document,
        changed_hunks,
        &mut || None,
    )
}

/// Compare one exact file pair with cooperative cancellation/deadline checkpoints.
pub fn compare_structured_file_controlled(
    old_path: Option<&str>,
    new_path: Option<&str>,
    old_document: Option<&DocumentStructure>,
    new_document: Option<&DocumentStructure>,
    changed_hunks: &[ChangedHunk],
    checkpoint: &mut dyn FnMut() -> Option<ComparisonStopReason>,
) -> Result<StructuredFile, StructureError> {
    check(checkpoint)?;
    validate_inputs(old_path, new_path, old_document, new_document)?;

    if old_path.is_some() && old_document.is_none() || new_path.is_some() && new_document.is_none()
    {
        return unsuccessful_file(
            old_path,
            new_path,
            FileAnalysisStatus::Skipped,
            changed_hunks,
            Vec::new(),
            checkpoint,
        );
    }

    let documents: Vec<_> = old_document.into_iter().chain(new_document).collect();
    if documents
        .iter()
        .any(|document| document.status() == DocumentStatus::Failed)
    {
        let mut errors = document_errors(&documents, true, checkpoint)?;
        if errors.is_empty() {
            errors.push(AnalysisError::new(
                selected_path(old_path, new_path).map(str::to_owned),
                AnalysisStage::Parsing,
                "syntax analysis failed without a diagnostic",
            )?);
        }
        return unsuccessful_file(
            old_path,
            new_path,
            FileAnalysisStatus::Failed,
            changed_hunks,
            errors,
            checkpoint,
        );
    }
    if documents
        .iter()
        .any(|document| document.status() == DocumentStatus::Unsupported)
    {
        let errors = document_errors(&documents, true, checkpoint)?;
        return unsuccessful_file(
            old_path,
            new_path,
            FileAnalysisStatus::Unsupported,
            changed_hunks,
            errors,
            checkpoint,
        );
    }
    if documents
        .iter()
        .any(|document| document.status() == DocumentStatus::Skipped)
    {
        let errors = document_errors(&documents, true, checkpoint)?;
        return unsuccessful_file(
            old_path,
            new_path,
            FileAnalysisStatus::Skipped,
            changed_hunks,
            errors,
            checkpoint,
        );
    }

    let language = documents
        .first()
        .map(|document| document.provenance().language())
        .ok_or_else(|| StructureError::invalid("comparison has no analyzable document"))?;
    if documents
        .iter()
        .any(|document| document.provenance().language() != language)
    {
        return unsuccessful_file(
            old_path,
            new_path,
            FileAnalysisStatus::Failed,
            changed_hunks,
            vec![AnalysisError::new(
                selected_path(old_path, new_path).map(str::to_owned),
                AnalysisStage::Comparison,
                "old and new syntax languages differ; structural facts were not matched",
            )?],
            checkpoint,
        );
    }

    let old_outline = old_document
        .map(|document| build_outline(document, ComparisonSide::Base, checkpoint))
        .transpose()?
        .unwrap_or_default();
    let new_outline = new_document
        .map(|document| build_outline(document, ComparisonSide::Head, checkpoint))
        .transpose()?
        .unwrap_or_default();
    let symbol_changes = compare_symbols(
        old_path,
        new_path,
        old_document
            .map(DocumentStructure::symbols)
            .unwrap_or_default(),
        new_document
            .map(DocumentStructure::symbols)
            .unwrap_or_default(),
        changed_hunks,
        checkpoint,
    )?;
    let call_diff =
        compare_matched_calls(old_path, new_path, old_document, new_document, checkpoint)?;
    let hotspots = build_hotspots(new_path, new_document, &symbol_changes, checkpoint)?;

    let mut errors = document_errors(&documents, false, checkpoint)?;
    let truncations = document_truncations(old_document, new_document);
    for (side, document, truncation) in &truncations {
        check(checkpoint)?;
        errors.push(AnalysisError::new(
            Some(document.path().to_owned()),
            AnalysisStage::Budget,
            format!(
                "{} syntax analysis truncated: {:?}",
                side_name(*side),
                truncation.reason()
            ),
        )?);
    }
    let review_truncation = truncations
        .first()
        .map(|(side, _, truncation)| translate_truncation(*side, truncation));
    let status = if documents
        .iter()
        .any(|document| document.status() == DocumentStatus::Partial)
        || !errors.is_empty()
        || review_truncation.is_some()
    {
        FileAnalysisStatus::Partial
    } else {
        FileAnalysisStatus::Parsed
    };

    let file = StructuredFile::new_controlled(
        old_path.map(str::to_owned),
        new_path.map(str::to_owned),
        Some(language),
        old_document.map(|document| document.provenance().clone()),
        new_document.map(|document| document.provenance().clone()),
        status,
        old_outline,
        new_outline,
        symbol_changes,
        hotspots,
        call_diff,
        clone_hunks(changed_hunks, checkpoint)?,
        errors,
        review_truncation,
        &mut || checkpoint().map_or(Ok(()), Err),
    )?;
    Ok(file)
}

fn validate_inputs(
    old_path: Option<&str>,
    new_path: Option<&str>,
    old_document: Option<&DocumentStructure>,
    new_document: Option<&DocumentStructure>,
) -> Result<(), StructureError> {
    if old_path.is_none() && new_path.is_none() {
        return Err(StructureError::invalid(
            "structural comparison requires at least one exact path",
        ));
    }
    for (side, path, document) in [
        ("old", old_path, old_document),
        ("new", new_path, new_document),
    ] {
        if path.is_some_and(|path| path.trim().is_empty()) {
            return Err(StructureError::invalid(format!(
                "{side} comparison path must not be empty"
            )));
        }
        if path.is_none() && document.is_some() {
            return Err(StructureError::invalid(format!(
                "{side} document requires an exact {side} path"
            )));
        }
        if let (Some(path), Some(document)) = (path, document)
            && path != document.path()
        {
            return Err(StructureError::invalid(format!(
                "{side} document path does not match the exact comparison path"
            )));
        }
    }
    Ok(())
}

fn unsuccessful_file(
    old_path: Option<&str>,
    new_path: Option<&str>,
    status: FileAnalysisStatus,
    changed_hunks: &[ChangedHunk],
    errors: Vec<AnalysisError>,
    checkpoint: &mut dyn FnMut() -> Option<ComparisonStopReason>,
) -> Result<StructuredFile, StructureError> {
    let file = StructuredFile::new_controlled(
        old_path.map(str::to_owned),
        new_path.map(str::to_owned),
        None,
        None,
        None,
        status,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        clone_hunks(changed_hunks, checkpoint)?,
        errors,
        None,
        &mut || checkpoint().map_or(Ok(()), Err),
    )?;
    Ok(file)
}

fn clone_hunks(
    hunks: &[ChangedHunk],
    checkpoint: &mut dyn FnMut() -> Option<ComparisonStopReason>,
) -> Result<Vec<ChangedHunk>, StructureError> {
    let mut cloned = Vec::with_capacity(hunks.len());
    for hunk in hunks {
        check(checkpoint)?;
        cloned.push(hunk.clone());
    }
    Ok(cloned)
}

fn selected_path<'a>(old_path: Option<&'a str>, new_path: Option<&'a str>) -> Option<&'a str> {
    new_path.or(old_path)
}

fn document_errors(
    documents: &[&DocumentStructure],
    include_all: bool,
    checkpoint: &mut dyn FnMut() -> Option<ComparisonStopReason>,
) -> Result<Vec<AnalysisError>, StructureError> {
    let mut errors = Vec::new();
    for document in documents {
        check(checkpoint)?;
        for diagnostic in document.diagnostics() {
            check(checkpoint)?;
            if !include_all && diagnostic.severity() == DiagnosticSeverity::Info {
                continue;
            }
            errors.push(AnalysisError::new(
                Some(document.path().to_owned()),
                AnalysisStage::Parsing,
                format!("{:?}: {}", diagnostic.severity(), diagnostic.message()),
            )?);
        }
    }
    Ok(errors)
}

fn document_truncations<'a>(
    old_document: Option<&'a DocumentStructure>,
    new_document: Option<&'a DocumentStructure>,
) -> Vec<(ComparisonSide, &'a DocumentStructure, &'a SyntaxTruncation)> {
    let mut truncations = Vec::new();
    if let Some(document) = old_document
        && let Some(truncation) = document.truncation()
    {
        truncations.push((ComparisonSide::Base, document, truncation));
    }
    if let Some(document) = new_document
        && let Some(truncation) = document.truncation()
    {
        truncations.push((ComparisonSide::Head, document, truncation));
    }
    truncations
}

fn translate_truncation(side: ComparisonSide, truncation: &SyntaxTruncation) -> ReviewTruncation {
    let reason = match truncation.reason() {
        SyntaxTruncationReason::SourceBytes | SyntaxTruncationReason::CaptureBytes => {
            TruncationReason::ByteLimit
        }
        SyntaxTruncationReason::SymbolCount
        | SyntaxTruncationReason::CallCount
        | SyntaxTruncationReason::DiagnosticCount => TruncationReason::CaptureLimit,
        SyntaxTruncationReason::Time => TruncationReason::TimeLimit,
        SyntaxTruncationReason::Cancelled => TruncationReason::Cancelled,
    };
    ReviewTruncation {
        reason,
        limit: truncation.limit(),
        observed: truncation.observed(),
        detail: Some(format!("{} syntax analysis", side_name(side))),
    }
}

fn side_name(side: ComparisonSide) -> &'static str {
    match side {
        ComparisonSide::Base => "base",
        ComparisonSide::Head => "head",
    }
}

fn build_outline(
    document: &DocumentStructure,
    side: ComparisonSide,
    checkpoint: &mut dyn FnMut() -> Option<ComparisonStopReason>,
) -> Result<Vec<OutlineFact>, StructureError> {
    let symbols = document.symbols();
    let mut order: Vec<usize> = (0..symbols.len()).collect();
    checked_stable_sort_by(
        &mut order,
        |left, right, _| Ok(symbol_source_order(&symbols[*left], &symbols[*right])),
        &mut || check(checkpoint),
    )?;

    let mut parents = vec![None; symbols.len()];
    let mut stack = Vec::<usize>::new();
    for index in order.iter().copied() {
        check(checkpoint)?;
        while stack
            .last()
            .is_some_and(|candidate| !is_outline_parent(&symbols[*candidate], &symbols[index]))
        {
            check(checkpoint)?;
            stack.pop();
        }
        parents[index] = stack.last().copied();
        stack.push(index);
    }

    let mut children = vec![Vec::new(); symbols.len()];
    let mut roots = Vec::new();
    for index in order.iter().copied() {
        check(checkpoint)?;
        if let Some(parent) = parents[index] {
            children[parent].push(index);
        } else {
            roots.push(index);
        }
    }
    let mut built = vec![None; symbols.len()];
    for index in order.iter().rev().copied() {
        check(checkpoint)?;
        let mut child_facts = Vec::with_capacity(children[index].len());
        for child in &children[index] {
            check(checkpoint)?;
            child_facts.push(
                built[*child]
                    .take()
                    .ok_or_else(|| StructureError::invalid("outline child was not built"))?,
            );
        }
        let fact = &symbols[index];
        built[index] = Some(OutlineFact::new_controlled(
            document.provenance().clone(),
            SymbolReference::new(side, fact.full_range(), fact.key().clone()),
            child_facts,
            &mut || checkpoint().map_or(Ok(()), Err),
        )?);
    }
    let mut outline = Vec::with_capacity(roots.len());
    for root in roots {
        check(checkpoint)?;
        outline.push(
            built[root]
                .take()
                .ok_or_else(|| StructureError::invalid("outline root was not built"))?,
        );
    }
    Ok(outline)
}

fn is_outline_parent(parent: &SymbolFact, child: &SymbolFact) -> bool {
    let parent_path = parent.key().qualified_path();
    let child_path = child.key().qualified_path();
    child_path.len() == parent_path.len() + 1
        && child_path.starts_with(parent_path)
        && child_path
            .last()
            .is_some_and(|name| name == parent.key().name())
        && parent.full_range() != child.full_range()
        && parent.full_range().contains(child.full_range())
}

fn compare_symbols(
    old_path: Option<&str>,
    new_path: Option<&str>,
    old_symbols: &[SymbolFact],
    new_symbols: &[SymbolFact],
    changed_hunks: &[ChangedHunk],
    checkpoint: &mut dyn FnMut() -> Option<ComparisonStopReason>,
) -> Result<Vec<SymbolChange>, StructureError> {
    let old_hunks = attribute_hunks(old_symbols, changed_hunks, ComparisonSide::Base, checkpoint)?;
    let new_hunks = attribute_hunks(new_symbols, changed_hunks, ComparisonSide::Head, checkpoint)?;
    let mut matched_old = HashSet::new();
    let mut matched_new = HashSet::new();
    let mut changes = Vec::new();

    for (old_index, new_index) in unique_pair_indices(old_symbols, new_symbols, checkpoint)? {
        check(checkpoint)?;
        let old = &old_symbols[old_index];
        let new = &new_symbols[new_index];
        match compare_unique_pair(
            old_path,
            new_path,
            old,
            new,
            old_index,
            new_index,
            &old_hunks,
            &new_hunks,
            changed_hunks,
            checkpoint,
        )? {
            UniquePairResult::Unpaired => {}
            UniquePairResult::Unchanged => {
                matched_old.insert(old_index);
                matched_new.insert(new_index);
            }
            UniquePairResult::Changed(change) => {
                matched_old.insert(old_index);
                matched_new.insert(new_index);
                changes.push(*change);
            }
        }
    }

    for (index, fact) in old_symbols.iter().enumerate() {
        check(checkpoint)?;
        if matched_old.contains(&index) {
            continue;
        }
        let hunks = hunks_from_indices(changed_hunks, &old_hunks.intersecting[index], checkpoint)?;
        if hunks.is_empty() {
            continue;
        }
        changes.push(SymbolChange::new_controlled(
            SymbolChangeKind::Removed,
            Some(fact.clone()),
            None,
            None,
            false,
            hunks,
            navigation(
                required_path(old_path, "removed symbol requires an old path")?,
                ComparisonSide::Base,
                fact,
            ),
            &mut || checkpoint().map_or(Ok(()), Err),
        )?);
    }
    for (index, fact) in new_symbols.iter().enumerate() {
        check(checkpoint)?;
        if matched_new.contains(&index) {
            continue;
        }
        let hunks = hunks_from_indices(changed_hunks, &new_hunks.intersecting[index], checkpoint)?;
        if hunks.is_empty() {
            continue;
        }
        changes.push(SymbolChange::new_controlled(
            SymbolChangeKind::Added,
            None,
            Some(fact.clone()),
            None,
            false,
            hunks,
            navigation(
                required_path(new_path, "added symbol requires a new path")?,
                ComparisonSide::Head,
                fact,
            ),
            &mut || checkpoint().map_or(Ok(()), Err),
        )?);
    }
    checked_stable_sort_by(
        &mut changes,
        |left, right, _| Ok(symbol_change_order(left, right)),
        &mut || check(checkpoint),
    )?;
    Ok(changes)
}

fn group_by_key(
    symbols: &[SymbolFact],
    checkpoint: &mut dyn FnMut() -> Option<ComparisonStopReason>,
) -> Result<HashMap<SymbolKey, Vec<usize>>, StructureError> {
    let mut grouped = HashMap::<SymbolKey, Vec<usize>>::new();
    for (index, symbol) in symbols.iter().enumerate() {
        check(checkpoint)?;
        grouped.entry(symbol.key().clone()).or_default().push(index);
    }
    Ok(grouped)
}

fn unique_pair_indices(
    old_symbols: &[SymbolFact],
    new_symbols: &[SymbolFact],
    checkpoint: &mut dyn FnMut() -> Option<ComparisonStopReason>,
) -> Result<Vec<(usize, usize)>, StructureError> {
    let old_by_key = group_by_key(old_symbols, checkpoint)?;
    let new_by_key = group_by_key(new_symbols, checkpoint)?;
    let mut pairs = Vec::new();
    for (old_index, old) in old_symbols.iter().enumerate() {
        check(checkpoint)?;
        let Some(old_occurrences) = old_by_key.get(old.key()) else {
            continue;
        };
        let Some(new_occurrences) = new_by_key.get(old.key()) else {
            continue;
        };
        if old_occurrences.len() != 1 || new_occurrences.len() != 1 {
            continue;
        }
        let new_index = new_occurrences[0];
        if old.provenance() == new_symbols[new_index].provenance() {
            pairs.push((old_index, new_index));
        }
    }
    Ok(pairs)
}

enum UniquePairResult {
    Unpaired,
    Unchanged,
    Changed(Box<SymbolChange>),
}

#[allow(clippy::too_many_arguments)]
fn compare_unique_pair(
    old_path: Option<&str>,
    new_path: Option<&str>,
    old: &SymbolFact,
    new: &SymbolFact,
    old_index: usize,
    new_index: usize,
    old_hunks: &HunkAttribution,
    new_hunks: &HunkAttribution,
    changed_hunks: &[ChangedHunk],
    checkpoint: &mut dyn FnMut() -> Option<ComparisonStopReason>,
) -> Result<UniquePairResult, StructureError> {
    let candidate_hunks = pair_hunks(
        changed_hunks,
        old_index,
        new_index,
        old_hunks,
        new_hunks,
        checkpoint,
    )?;
    let signature_text_changed = old.normalized_signature() != new.normalized_signature();
    let signature_has_evidence = side_has_intersection(
        &candidate_hunks,
        ComparisonSide::Base,
        old.signature_range(),
        checkpoint,
    )? || side_has_intersection(
        &candidate_hunks,
        ComparisonSide::Head,
        new.signature_range(),
        checkpoint,
    )?;
    if signature_text_changed && !signature_has_evidence {
        return Ok(UniquePairResult::Unpaired);
    }
    let body_changed = body_has_evidence(&candidate_hunks, old, new, checkpoint)?;
    let hunks = dimension_hunks(
        &candidate_hunks,
        old,
        new,
        signature_text_changed,
        body_changed,
        checkpoint,
    )?;
    let signature_has_evidence = side_has_intersection(
        &hunks,
        ComparisonSide::Base,
        old.signature_range(),
        checkpoint,
    )? || side_has_intersection(
        &hunks,
        ComparisonSide::Head,
        new.signature_range(),
        checkpoint,
    )?;
    if signature_text_changed && !signature_has_evidence {
        return Ok(UniquePairResult::Unpaired);
    }
    let signature_change = signature_text_changed
        .then(|| {
            SignatureChange::new(
                old.normalized_signature(),
                new.normalized_signature(),
                old.signature_range(),
                new.signature_range(),
            )
        })
        .transpose()?;
    let body_changed = body_has_evidence(&hunks, old, new, checkpoint)?;
    if signature_change.is_none() && !body_changed {
        return Ok(UniquePairResult::Unchanged);
    }
    Ok(UniquePairResult::Changed(Box::new(
        SymbolChange::new_controlled(
            SymbolChangeKind::Modified,
            Some(old.clone()),
            Some(new.clone()),
            signature_change,
            body_changed,
            hunks,
            navigation(
                required_path(new_path.or(old_path), "modified symbol requires a path")?,
                if new_path.is_some() {
                    ComparisonSide::Head
                } else {
                    ComparisonSide::Base
                },
                if new_path.is_some() { new } else { old },
            ),
            &mut || checkpoint().map_or(Ok(()), Err),
        )?,
    )))
}

fn dimension_hunks(
    hunks: &[ChangedHunk],
    old: &SymbolFact,
    new: &SymbolFact,
    signature_changed: bool,
    body_changed: bool,
    checkpoint: &mut dyn FnMut() -> Option<ComparisonStopReason>,
) -> Result<Vec<ChangedHunk>, StructureError> {
    let mut relevant = Vec::new();
    for hunk in hunks {
        check(checkpoint)?;
        let old_relevant = hunk
            .old()
            .map(|lines| dimension_intersects(lines, old, signature_changed, body_changed));
        let new_relevant = hunk
            .new_range()
            .map(|lines| dimension_intersects(lines, new, signature_changed, body_changed));
        if old_relevant.is_none_or(|relevant| relevant)
            && new_relevant.is_none_or(|relevant| relevant)
            && (old_relevant.unwrap_or(false) || new_relevant.unwrap_or(false))
        {
            relevant.push(hunk.clone());
        }
    }
    Ok(relevant)
}

fn dimension_intersects(
    lines: ChangedLineRange,
    fact: &SymbolFact,
    signature_changed: bool,
    body_changed: bool,
) -> bool {
    signature_changed && line_range_intersects(lines, fact.signature_range())
        || body_changed
            && fact
                .body_range()
                .is_some_and(|body| line_range_intersects(lines, body))
}

fn pair_hunks(
    hunks: &[ChangedHunk],
    old_index: usize,
    new_index: usize,
    old_hunks: &HunkAttribution,
    new_hunks: &HunkAttribution,
    checkpoint: &mut dyn FnMut() -> Option<ComparisonStopReason>,
) -> Result<Vec<ChangedHunk>, StructureError> {
    let mut candidates = BTreeSet::new();
    for &hunk_index in &old_hunks.own[old_index] {
        check(checkpoint)?;
        candidates.insert(hunk_index);
    }
    for &hunk_index in &new_hunks.own[new_index] {
        check(checkpoint)?;
        candidates.insert(hunk_index);
    }
    let mut paired = Vec::new();
    for hunk_index in candidates {
        check(checkpoint)?;
        let hunk = &hunks[hunk_index];
        let old_valid = hunk.old().is_none()
            || old_hunks.intersecting[old_index]
                .binary_search(&hunk_index)
                .is_ok();
        let new_valid = hunk.new_range().is_none()
            || new_hunks.intersecting[new_index]
                .binary_search(&hunk_index)
                .is_ok();
        let old_own = old_hunks.own[old_index].binary_search(&hunk_index).is_ok();
        let new_own = new_hunks.own[new_index].binary_search(&hunk_index).is_ok();
        if old_valid && new_valid && (old_own || new_own) {
            paired.push(hunk.clone());
        }
    }
    Ok(paired)
}

struct HunkAttribution {
    intersecting: Vec<Vec<usize>>,
    own: Vec<Vec<usize>>,
}

fn attribute_hunks(
    symbols: &[SymbolFact],
    hunks: &[ChangedHunk],
    side: ComparisonSide,
    checkpoint: &mut dyn FnMut() -> Option<ComparisonStopReason>,
) -> Result<HunkAttribution, StructureError> {
    let mut symbol_order: Vec<_> = (0..symbols.len()).collect();
    checked_stable_sort_by(
        &mut symbol_order,
        |left, right, _| {
            Ok(symbols[*left]
                .full_range()
                .start_line()
                .cmp(&symbols[*right].full_range().start_line()))
        },
        &mut || check(checkpoint),
    )?;

    let mut hunk_order = Vec::new();
    for (index, hunk) in hunks.iter().enumerate() {
        check(checkpoint)?;
        if let Some(lines) = hunk_on_side(hunk, side) {
            hunk_order.push((index, lines));
        }
    }
    checked_stable_sort_by(
        &mut hunk_order,
        |(_, left), (_, right), _| Ok((left.start(), left.end()).cmp(&(right.start(), right.end()))),
        &mut || check(checkpoint),
    )?;

    let mut attribution = HunkAttribution {
        intersecting: vec![Vec::new(); symbols.len()],
        own: vec![Vec::new(); symbols.len()],
    };
    let mut next_symbol = 0;
    let mut active = Vec::<usize>::new();
    for (hunk_index, lines) in hunk_order {
        check(checkpoint)?;
        while next_symbol < symbol_order.len()
            && symbols[symbol_order[next_symbol]]
                .full_range()
                .start_line()
                .get()
                <= lines.end().get()
        {
            check(checkpoint)?;
            active.push(symbol_order[next_symbol]);
            next_symbol += 1;
        }
        let mut retained = Vec::with_capacity(active.len());
        for symbol_index in active.drain(..) {
            check(checkpoint)?;
            if symbols[symbol_index].full_range().end_line().get() >= lines.start().get() {
                retained.push(symbol_index);
            }
        }
        active = retained;
        for &symbol_index in &active {
            check(checkpoint)?;
            let fact = &symbols[symbol_index];
            if !line_range_intersects(lines, fact.full_range()) {
                continue;
            }
            attribution.intersecting[symbol_index].push(hunk_index);
            let clipped_start = lines
                .start()
                .get()
                .max(fact.full_range().start_line().get());
            let clipped_end = lines.end().get().min(fact.full_range().end_line().get());
            let mut belongs_to_descendant = false;
            for &candidate_index in &active {
                check(checkpoint)?;
                let candidate = &symbols[candidate_index];
                if is_descendant(fact, candidate)
                    && candidate.full_range().start_line().get() <= clipped_start
                    && clipped_end <= candidate.full_range().end_line().get()
                {
                    belongs_to_descendant = true;
                    break;
                }
            }
            if !belongs_to_descendant {
                attribution.own[symbol_index].push(hunk_index);
            }
        }
    }
    for indexes in attribution
        .intersecting
        .iter_mut()
        .chain(attribution.own.iter_mut())
    {
        checked_stable_sort_by(indexes, |left, right, _| Ok(left.cmp(right)), &mut || {
            check(checkpoint)
        })?;
    }
    Ok(attribution)
}

fn is_descendant(parent: &SymbolFact, candidate: &SymbolFact) -> bool {
    let parent_path = parent.key().qualified_path();
    let candidate_path = candidate.key().qualified_path();
    candidate_path.len() > parent_path.len()
        && candidate_path.starts_with(parent_path)
        && candidate_path[parent_path.len()] == parent.key().name()
        && parent.full_range() != candidate.full_range()
        && parent.full_range().contains(candidate.full_range())
}

fn hunks_from_indices(
    hunks: &[ChangedHunk],
    indices: &[usize],
    checkpoint: &mut dyn FnMut() -> Option<ComparisonStopReason>,
) -> Result<Vec<ChangedHunk>, StructureError> {
    let mut matching = Vec::with_capacity(indices.len());
    for &index in indices {
        check(checkpoint)?;
        matching.push(hunks[index].clone());
    }
    Ok(matching)
}

fn body_has_evidence(
    hunks: &[ChangedHunk],
    old: &SymbolFact,
    new: &SymbolFact,
    checkpoint: &mut dyn FnMut() -> Option<ComparisonStopReason>,
) -> Result<bool, StructureError> {
    if let Some(range) = old.body_range()
        && side_has_intersection(hunks, ComparisonSide::Base, range, checkpoint)?
    {
        return Ok(true);
    }
    if let Some(range) = new.body_range()
        && side_has_intersection(hunks, ComparisonSide::Head, range, checkpoint)?
    {
        return Ok(true);
    }
    Ok(false)
}

fn side_has_intersection(
    hunks: &[ChangedHunk],
    side: ComparisonSide,
    range: SourceRange,
    checkpoint: &mut dyn FnMut() -> Option<ComparisonStopReason>,
) -> Result<bool, StructureError> {
    for hunk in hunks {
        check(checkpoint)?;
        if hunk_on_side(hunk, side).is_some_and(|lines| line_range_intersects(lines, range)) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn hunk_on_side(hunk: &ChangedHunk, side: ComparisonSide) -> Option<ChangedLineRange> {
    match side {
        ComparisonSide::Base => hunk.old(),
        ComparisonSide::Head => hunk.new_range(),
    }
}

fn line_range_intersects(lines: ChangedLineRange, source: SourceRange) -> bool {
    lines.start().get() <= source.end_line().get() && source.start_line().get() <= lines.end().get()
}

fn compare_matched_calls(
    old_path: Option<&str>,
    new_path: Option<&str>,
    old_document: Option<&DocumentStructure>,
    new_document: Option<&DocumentStructure>,
    checkpoint: &mut dyn FnMut() -> Option<ComparisonStopReason>,
) -> Result<Vec<CallDiffChange>, StructureError> {
    let (Some(old_path), Some(new_path), Some(old_document), Some(new_document)) =
        (old_path, new_path, old_document, new_document)
    else {
        return Ok(Vec::new());
    };
    let old_index = index_calls(old_document.calls(), checkpoint)?;
    let new_index = index_calls(new_document.calls(), checkpoint)?;
    let mut changes = Vec::new();
    for (old_symbol_index, new_symbol_index) in
        unique_pair_indices(old_document.symbols(), new_document.symbols(), checkpoint)?
    {
        check(checkpoint)?;
        let old = &old_document.symbols()[old_symbol_index];
        let new = &new_document.symbols()[new_symbol_index];
        if !is_function(old.key().kind()) || !is_function(new.key().kind()) {
            continue;
        }
        let old_calls = old_index
            .get(&(old.key(), old.provenance()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let new_calls = new_index
            .get(&(new.key(), new.provenance()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        changes.extend(compare_indexed_calls_controlled(
            IndexedCallDiffInput::new(
                old_path,
                new_path,
                old.key(),
                old.body_range().unwrap_or_else(|| old.full_range()),
                new.body_range().unwrap_or_else(|| new.full_range()),
                old_calls,
                new_calls,
            )?,
            checkpoint,
        )?);
    }
    for change in &changes {
        check(checkpoint)?;
        if let Some(fact) = call_change_fact(change) {
            for _ in fact.control_context() {
                check(checkpoint)?;
            }
        }
    }
    checked_stable_sort_by(&mut changes, compare_call_changes_controlled, &mut || {
        check(checkpoint)
    })?;
    Ok(changes)
}

type CallIndex<'a> = HashMap<(&'a SymbolKey, &'a SyntaxProvenance), Vec<&'a CallFact>>;

fn index_calls<'a>(
    calls: &'a [CallFact],
    checkpoint: &mut dyn FnMut() -> Option<ComparisonStopReason>,
) -> Result<CallIndex<'a>, StructureError> {
    let mut index = HashMap::new();
    for call in calls {
        check(checkpoint)?;
        let Some(enclosing) = call.enclosing_symbol() else {
            continue;
        };
        index
            .entry((enclosing, call.provenance()))
            .or_insert_with(Vec::new)
            .push(call);
    }
    Ok(index)
}

fn compare_call_changes_controlled<E>(
    left: &CallDiffChange,
    right: &CallDiffChange,
    checkpoint: &mut dyn FnMut() -> Result<(), E>,
) -> Result<Ordering, E> {
    let ordering = left
        .navigation()
        .line
        .cmp(&right.navigation().line)
        .then_with(|| {
            left.navigation()
                .byte_offset
                .cmp(&right.navigation().byte_offset)
        })
        .then_with(|| call_change_rank(left.kind()).cmp(&call_change_rank(right.kind())))
        .then_with(|| compare_call_enclosing(left, right))
        .then_with(|| call_callee(left).cmp(call_callee(right)))
        .then_with(|| left.navigation().path.cmp(&right.navigation().path))
        .then_with(|| side_rank(left.navigation().side).cmp(&side_rank(right.navigation().side)));
    if ordering != Ordering::Equal {
        return Ok(ordering);
    }
    match (call_change_fact(left), call_change_fact(right)) {
        (Some(left), Some(right)) => compare_call_facts_controlled(left, right, checkpoint),
        (None, Some(_)) => Ok(Ordering::Less),
        (Some(_), None) => Ok(Ordering::Greater),
        (None, None) => Ok(Ordering::Equal),
    }
}

fn call_change_rank(kind: CallChangeKind) -> u8 {
    match kind {
        CallChangeKind::Removed => 0,
        CallChangeKind::Modified => 1,
        CallChangeKind::Added => 2,
    }
}

fn call_change_fact(change: &CallDiffChange) -> Option<&CallFact> {
    change.new_fact().or_else(|| change.old())
}

fn call_callee(change: &CallDiffChange) -> &str {
    call_change_fact(change)
        .map(CallFact::callee_text)
        .unwrap_or_default()
}

fn compare_call_enclosing(left: &CallDiffChange, right: &CallDiffChange) -> Ordering {
    let left = call_change_fact(left).and_then(CallFact::enclosing_symbol);
    let right = call_change_fact(right).and_then(CallFact::enclosing_symbol);
    match (left, right) {
        (Some(left), Some(right)) => left
            .qualified_path()
            .cmp(right.qualified_path())
            .then_with(|| left.name().cmp(right.name()))
            .then_with(|| symbol_kind_rank(left.kind()).cmp(&symbol_kind_rank(right.kind()))),
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn compare_call_facts_controlled<E>(
    left: &CallFact,
    right: &CallFact,
    checkpoint: &mut dyn FnMut() -> Result<(), E>,
) -> Result<Ordering, E> {
    let ordering = left
        .call_site_range()
        .start_byte()
        .cmp(&right.call_site_range().start_byte())
        .then_with(|| {
            left.call_site_range()
                .end_byte()
                .cmp(&right.call_site_range().end_byte())
        })
        .then_with(|| left.argument_text().cmp(right.argument_text()));
    if ordering != Ordering::Equal {
        return Ok(ordering);
    }
    let ordering = compare_control_contexts_controlled(
        left.control_context(),
        right.control_context(),
        checkpoint,
    )?;
    if ordering != Ordering::Equal {
        return Ok(ordering);
    }
    Ok(syntax_language_rank(left.provenance().language())
        .cmp(&syntax_language_rank(right.provenance().language()))
        .then_with(|| left.provenance().parser().cmp(right.provenance().parser())))
}

fn compare_control_contexts_controlled<E>(
    left: &[ControlContext],
    right: &[ControlContext],
    checkpoint: &mut dyn FnMut() -> Result<(), E>,
) -> Result<Ordering, E> {
    for (left, right) in left.iter().zip(right) {
        checkpoint()?;
        let ordering = control_context_rank(left)
            .cmp(&control_context_rank(right))
            .then_with(|| match (left, right) {
                (ControlContext::Other(left), ControlContext::Other(right)) => left.cmp(right),
                _ => Ordering::Equal,
            });
        if ordering != Ordering::Equal {
            return Ok(ordering);
        }
    }
    Ok(left.len().cmp(&right.len()))
}

fn control_context_rank(context: &ControlContext) -> u8 {
    match context {
        ControlContext::Condition => 0,
        ControlContext::Loop => 1,
        ControlContext::MatchArm => 2,
        ControlContext::ErrorBranch => 3,
        ControlContext::Callback => 4,
        ControlContext::Closure => 5,
        ControlContext::Other(_) => 6,
    }
}

fn syntax_language_rank(language: SyntaxLanguage) -> u8 {
    match language {
        SyntaxLanguage::Rust => 0,
        SyntaxLanguage::TypeScript => 1,
        SyntaxLanguage::Tsx => 2,
    }
}

fn build_hotspots(
    new_path: Option<&str>,
    new_document: Option<&DocumentStructure>,
    changes: &[SymbolChange],
    checkpoint: &mut dyn FnMut() -> Option<ComparisonStopReason>,
) -> Result<Vec<StructuralHotspot>, StructureError> {
    let mut candidates = Vec::new();
    for change in changes {
        check(checkpoint)?;
        let fact = change.new_fact().or_else(|| change.old());
        let Some(fact) = fact.filter(|fact| is_function(fact.key().kind())) else {
            continue;
        };
        let side = if change.new_fact().is_some() {
            ComparisonSide::Head
        } else {
            ComparisonSide::Base
        };
        let path = match side {
            ComparisonSide::Head => new_path,
            ComparisonSide::Base => Some(change.navigation().path.as_str()),
        }
        .ok_or_else(|| StructureError::invalid("changed-function hotspot requires its path"))?;
        candidates.push(HotspotCandidate::new(
            fact,
            side,
            path,
            StructuralMetric::ChangedLines {
                old: change.changed_old_lines(),
                new: change.changed_new_lines(),
            },
            0,
            u64::from(change.changed_old_lines()) + u64::from(change.changed_new_lines()),
        )?);
    }
    if let (Some(path), Some(document)) = (new_path, new_document) {
        for fact in document.symbols() {
            check(checkpoint)?;
            if is_function(fact.key().kind()) {
                candidates.push(HotspotCandidate::new(
                    fact,
                    ComparisonSide::Head,
                    path,
                    StructuralMetric::FunctionLineCount {
                        lines: fact.full_range().line_count(),
                    },
                    1,
                    u64::from(fact.full_range().line_count()),
                )?);
                candidates.push(HotspotCandidate::new(
                    fact,
                    ComparisonSide::Head,
                    path,
                    StructuralMetric::ParameterCount {
                        parameters: fact.parameter_count(),
                    },
                    2,
                    u64::from(fact.parameter_count()),
                )?);
                candidates.push(HotspotCandidate::new(
                    fact,
                    ComparisonSide::Head,
                    path,
                    StructuralMetric::SyntacticNestingDepth {
                        depth: fact.syntactic_nesting_depth(),
                    },
                    3,
                    u64::from(fact.syntactic_nesting_depth()),
                )?);
            }
            if is_type(fact.key().kind()) {
                candidates.push(HotspotCandidate::new(
                    fact,
                    ComparisonSide::Head,
                    path,
                    StructuralMetric::TypeMemberCount {
                        members: fact.type_member_count(),
                    },
                    4,
                    u64::from(fact.type_member_count()),
                )?);
            }
        }
    }
    checked_stable_sort_by(
        &mut candidates,
        |left, right, _| Ok(HotspotCandidate::compare(left, right)),
        &mut || check(checkpoint),
    )?;
    let mut hotspots = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        check(checkpoint)?;
        hotspots.push(candidate.hotspot);
    }
    Ok(hotspots)
}

struct HotspotCandidate {
    hotspot: StructuralHotspot,
    metric_rank: u8,
    value: u64,
    qualified_name: String,
    kind_rank: u8,
    start_byte: u64,
    side_rank: u8,
}

impl HotspotCandidate {
    fn new(
        fact: &SymbolFact,
        side: ComparisonSide,
        path: &str,
        metric: StructuralMetric,
        metric_rank: u8,
        value: u64,
    ) -> Result<Self, StructureError> {
        Ok(Self {
            hotspot: StructuralHotspot::new(
                SymbolReference::new(side, fact.full_range(), fact.key().clone()),
                metric,
                fact.provenance().clone(),
                navigation(path, side, fact),
            )?,
            metric_rank,
            value,
            qualified_name: fact.key().qualified_name(),
            kind_rank: symbol_kind_rank(fact.key().kind()),
            start_byte: fact.full_range().start_byte(),
            side_rank: side_rank(side),
        })
    }

    fn compare(left: &Self, right: &Self) -> Ordering {
        left.metric_rank
            .cmp(&right.metric_rank)
            .then_with(|| right.value.cmp(&left.value))
            .then_with(|| left.qualified_name.cmp(&right.qualified_name))
            .then_with(|| left.kind_rank.cmp(&right.kind_rank))
            .then_with(|| left.start_byte.cmp(&right.start_byte))
            .then_with(|| left.side_rank.cmp(&right.side_rank))
    }
}

fn check(
    checkpoint: &mut dyn FnMut() -> Option<ComparisonStopReason>,
) -> Result<(), StructureError> {
    match checkpoint() {
        Some(reason) => Err(StructureError::Stopped(reason)),
        None => Ok(()),
    }
}

fn navigation(path: &str, side: ComparisonSide, fact: &SymbolFact) -> ReviewNavigationTarget {
    ReviewNavigationTarget {
        path: path.to_owned(),
        side,
        line: fact.full_range().start_line(),
        byte_offset: Some(fact.full_range().start_byte()),
        symbol_context: None,
    }
}

fn required_path<'a>(path: Option<&'a str>, message: &str) -> Result<&'a str, StructureError> {
    path.ok_or_else(|| StructureError::invalid(message))
}

fn symbol_change_order(left: &SymbolChange, right: &SymbolChange) -> Ordering {
    left.navigation()
        .line
        .cmp(&right.navigation().line)
        .then_with(|| side_rank(left.navigation().side).cmp(&side_rank(right.navigation().side)))
        .then_with(|| {
            optional_change_fact(left)
                .map(|fact| fact.key().qualified_name())
                .cmp(&optional_change_fact(right).map(|fact| fact.key().qualified_name()))
        })
        .then_with(|| {
            optional_change_fact(left)
                .map(|fact| symbol_kind_rank(fact.key().kind()))
                .cmp(&optional_change_fact(right).map(|fact| symbol_kind_rank(fact.key().kind())))
        })
}

fn optional_change_fact(change: &SymbolChange) -> Option<&SymbolFact> {
    change.new_fact().or_else(|| change.old())
}

fn symbol_source_order(left: &SymbolFact, right: &SymbolFact) -> Ordering {
    left.full_range()
        .start_byte()
        .cmp(&right.full_range().start_byte())
        .then_with(|| {
            right
                .full_range()
                .end_byte()
                .cmp(&left.full_range().end_byte())
        })
        .then_with(|| {
            left.key()
                .qualified_name()
                .cmp(&right.key().qualified_name())
        })
        .then_with(|| {
            symbol_kind_rank(left.key().kind()).cmp(&symbol_kind_rank(right.key().kind()))
        })
}

fn side_rank(side: ComparisonSide) -> u8 {
    match side {
        ComparisonSide::Base => 0,
        ComparisonSide::Head => 1,
    }
}

fn is_function(kind: SymbolKind) -> bool {
    matches!(kind, SymbolKind::Function | SymbolKind::Method)
}

fn is_type(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::Struct
            | SymbolKind::Enum
            | SymbolKind::Union
            | SymbolKind::Trait
            | SymbolKind::Impl
            | SymbolKind::Class
            | SymbolKind::Interface
    )
}

fn symbol_kind_rank(kind: SymbolKind) -> u8 {
    match kind {
        SymbolKind::Module => 0,
        SymbolKind::Function => 1,
        SymbolKind::Method => 2,
        SymbolKind::Struct => 3,
        SymbolKind::Enum => 4,
        SymbolKind::Union => 5,
        SymbolKind::Trait => 6,
        SymbolKind::Impl => 7,
        SymbolKind::Class => 8,
        SymbolKind::Interface => 9,
        SymbolKind::TypeAlias => 10,
        SymbolKind::Constant => 11,
        SymbolKind::Static => 12,
        SymbolKind::Field => 13,
        SymbolKind::Variant => 14,
        SymbolKind::Macro => 15,
    }
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU32, NonZeroU64};

    use okena_syntax::{
        DocumentStatus, SymbolVisibility, SyntaxDiagnostic, SyntaxLanguage, SyntaxProvenance,
    };

    use super::*;

    fn range(start_byte: u64, end_byte: u64, start_line: u32, end_line: u32) -> SourceRange {
        SourceRange::new(
            start_byte,
            end_byte,
            NonZeroU32::new(start_line).unwrap(),
            NonZeroU32::new(end_line).unwrap(),
        )
        .unwrap()
    }

    fn lines(start: u32, end: u32) -> ChangedLineRange {
        ChangedLineRange::new(
            NonZeroU32::new(start).unwrap(),
            NonZeroU32::new(end).unwrap(),
        )
        .unwrap()
    }

    fn hunk(old: Option<(u32, u32)>, new: Option<(u32, u32)>) -> ChangedHunk {
        ChangedHunk::new(
            old.map(|(start, end)| lines(start, end)),
            new.map(|(start, end)| lines(start, end)),
        )
        .unwrap()
    }

    fn provenance(language: SyntaxLanguage, parser: &str) -> SyntaxProvenance {
        SyntaxProvenance::tree_sitter(language, parser).unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    fn fact(
        provenance: &SyntaxProvenance,
        parent: &[&str],
        kind: SymbolKind,
        name: &str,
        full: SourceRange,
        signature: SourceRange,
        body: Option<SourceRange>,
        signature_text: &str,
        parameters: u32,
        nesting: u32,
        members: u32,
    ) -> SymbolFact {
        SymbolFact::new(
            provenance.clone(),
            SymbolKey::new(
                parent.iter().map(|part| (*part).to_owned()).collect(),
                kind,
                name,
            )
            .unwrap(),
            SymbolVisibility::Private,
            full,
            signature,
            body,
            signature_text,
            parameters,
            nesting,
            members,
        )
        .unwrap()
    }

    fn document(
        path: &str,
        provenance: &SyntaxProvenance,
        status: DocumentStatus,
        symbols: Vec<SymbolFact>,
        diagnostics: Vec<SyntaxDiagnostic>,
        truncation: Option<SyntaxTruncation>,
    ) -> DocumentStructure {
        document_with_calls(
            path,
            provenance,
            status,
            symbols,
            Vec::new(),
            diagnostics,
            truncation,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn document_with_calls(
        path: &str,
        provenance: &SyntaxProvenance,
        status: DocumentStatus,
        symbols: Vec<SymbolFact>,
        calls: Vec<CallFact>,
        diagnostics: Vec<SyntaxDiagnostic>,
        truncation: Option<SyntaxTruncation>,
    ) -> DocumentStructure {
        DocumentStructure::new(
            path,
            provenance.clone(),
            status,
            symbols,
            calls,
            diagnostics,
            truncation,
        )
        .unwrap()
    }

    fn function(
        provenance: &SyntaxProvenance,
        parent: &[&str],
        name: &str,
        start_line: u32,
        signature_text: &str,
    ) -> SymbolFact {
        let start = u64::from(start_line) * 100;
        fact(
            provenance,
            parent,
            SymbolKind::Function,
            name,
            range(start, start + 90, start_line, start_line + 4),
            range(start, start + 20, start_line, start_line),
            Some(range(
                start + 21,
                start + 90,
                start_line + 1,
                start_line + 4,
            )),
            signature_text,
            1,
            2,
            0,
        )
    }

    fn parsed_with_calls(
        path: &str,
        provenance: &SyntaxProvenance,
        symbols: Vec<SymbolFact>,
        calls: Vec<CallFact>,
    ) -> DocumentStructure {
        document_with_calls(
            path,
            provenance,
            DocumentStatus::Parsed,
            symbols,
            calls,
            Vec::new(),
            None,
        )
    }

    fn direct_call(
        provenance: &SyntaxProvenance,
        enclosing: &SymbolFact,
        callee: &str,
        arguments: &str,
        start_byte: u64,
        line: u32,
        contexts: Vec<ControlContext>,
    ) -> CallFact {
        let call_range = range(start_byte, start_byte + 12, line, line);
        CallFact::new(
            provenance.clone(),
            callee,
            arguments,
            range(start_byte + 4, start_byte + 10, line, line),
            call_range,
            Some(enclosing.key().clone()),
            contexts,
        )
        .unwrap()
    }

    fn parsed(
        path: &str,
        provenance: &SyntaxProvenance,
        symbols: Vec<SymbolFact>,
    ) -> DocumentStructure {
        document(
            path,
            provenance,
            DocumentStatus::Parsed,
            symbols,
            Vec::new(),
            None,
        )
    }

    fn find_change<'a>(file: &'a StructuredFile, name: &str) -> &'a SymbolChange {
        file.symbol_changes()
            .iter()
            .find(|change| {
                optional_change_fact(change).is_some_and(|fact| fact.key().name() == name)
            })
            .unwrap()
    }

    #[test]
    fn compares_added_removed_body_signature_and_combined_changes() {
        let rust = provenance(SyntaxLanguage::Rust, "rust-test");
        let old_symbols = vec![
            function(&rust, &[], "removed", 1, "fn removed()"),
            function(&rust, &[], "body", 10, "fn body()"),
            function(&rust, &[], "signature", 20, "fn signature(value: u8)"),
            function(&rust, &[], "combined", 30, "fn combined(value: u8)"),
        ];
        let new_symbols = vec![
            function(&rust, &[], "added", 1, "fn added()"),
            function(&rust, &[], "body", 10, "fn body()"),
            function(&rust, &[], "signature", 20, "fn signature(value: u16)"),
            function(&rust, &[], "combined", 30, "fn combined(value: u16)"),
        ];
        let hunks = vec![
            hunk(Some((1, 1)), Some((1, 1))),
            hunk(Some((12, 12)), Some((12, 12))),
            hunk(Some((20, 20)), Some((20, 20))),
            hunk(Some((30, 30)), Some((30, 30))),
            hunk(Some((32, 32)), Some((32, 32))),
        ];
        let file = compare_structured_file(
            Some("old.rs"),
            Some("new.rs"),
            Some(&parsed("old.rs", &rust, old_symbols)),
            Some(&parsed("new.rs", &rust, new_symbols)),
            &hunks,
        )
        .unwrap();

        assert_eq!(
            find_change(&file, "removed").kind(),
            SymbolChangeKind::Removed
        );
        assert_eq!(find_change(&file, "added").kind(), SymbolChangeKind::Added);
        let body = find_change(&file, "body");
        assert!(body.body_changed());
        assert!(body.signature_change().is_none());
        let signature = find_change(&file, "signature");
        assert!(!signature.body_changed());
        assert!(signature.signature_change().is_some());
        let combined = find_change(&file, "combined");
        assert!(combined.body_changed());
        assert!(combined.signature_change().is_some());
    }

    #[test]
    fn compares_one_sided_body_insertions_and_deletions_as_modified() {
        let rust = provenance(SyntaxLanguage::Rust, "rust-test");
        let old = parsed(
            "src/lib.rs",
            &rust,
            vec![function(&rust, &[], "work", 1, "fn work()")],
        );
        let new = old.clone();

        let insertion = compare_structured_file(
            Some("src/lib.rs"),
            Some("src/lib.rs"),
            Some(&old),
            Some(&new),
            &[hunk(None, Some((3, 3)))],
        )
        .unwrap();
        let change = &insertion.symbol_changes()[0];
        assert_eq!(change.kind(), SymbolChangeKind::Modified);
        assert!(change.body_changed());
        assert_eq!(change.changed_old_lines(), 0);
        assert_eq!(change.changed_new_lines(), 1);

        let deletion = compare_structured_file(
            Some("src/lib.rs"),
            Some("src/lib.rs"),
            Some(&old),
            Some(&new),
            &[hunk(Some((4, 4)), None)],
        )
        .unwrap();
        let change = &deletion.symbol_changes()[0];
        assert_eq!(change.kind(), SymbolChangeKind::Modified);
        assert!(change.body_changed());
        assert_eq!(change.changed_old_lines(), 1);
        assert_eq!(change.changed_new_lines(), 0);
    }

    #[test]
    fn compares_multiline_signature_insertions_and_deletions_as_modified() {
        let rust = provenance(SyntaxLanguage::Rust, "rust-test");
        let compact = fact(
            &rust,
            &[],
            SymbolKind::Function,
            "work",
            range(0, 100, 1, 6),
            range(0, 20, 1, 1),
            Some(range(21, 100, 2, 6)),
            "fn work()",
            0,
            1,
            0,
        );
        let multiline = fact(
            &rust,
            &[],
            SymbolKind::Function,
            "work",
            range(0, 130, 1, 8),
            range(0, 50, 1, 3),
            Some(range(51, 130, 4, 8)),
            "fn work(value: u32)",
            1,
            1,
            0,
        );
        let compact_document = parsed("src/lib.rs", &rust, vec![compact]);
        let multiline_document = parsed("src/lib.rs", &rust, vec![multiline]);

        let insertion = compare_structured_file(
            Some("src/lib.rs"),
            Some("src/lib.rs"),
            Some(&compact_document),
            Some(&multiline_document),
            &[hunk(None, Some((2, 2)))],
        )
        .unwrap();
        let change = &insertion.symbol_changes()[0];
        assert!(change.signature_change().is_some());
        assert!(!change.body_changed());
        assert_eq!(change.changed_old_lines(), 0);
        assert_eq!(change.changed_new_lines(), 1);

        let deletion = compare_structured_file(
            Some("src/lib.rs"),
            Some("src/lib.rs"),
            Some(&multiline_document),
            Some(&compact_document),
            &[hunk(Some((3, 3)), None)],
        )
        .unwrap();
        let change = &deletion.symbol_changes()[0];
        assert!(change.signature_change().is_some());
        assert!(!change.body_changed());
        assert_eq!(change.changed_old_lines(), 1);
        assert_eq!(change.changed_new_lines(), 0);
    }

    #[test]
    fn duplicate_keys_degrade_to_added_and_removed() {
        let ts = provenance(SyntaxLanguage::TypeScript, "ts-test");
        let old = parsed(
            "old.ts",
            &ts,
            vec![
                function(
                    &ts,
                    &[],
                    "overload",
                    1,
                    "function overload(x: string): void",
                ),
                function(
                    &ts,
                    &[],
                    "overload",
                    10,
                    "function overload(x: number): void",
                ),
            ],
        );
        let new = parsed(
            "new.ts",
            &ts,
            vec![
                function(
                    &ts,
                    &[],
                    "overload",
                    1,
                    "function overload(x: string): void",
                ),
                function(
                    &ts,
                    &[],
                    "overload",
                    10,
                    "function overload(x: number): void",
                ),
            ],
        );
        let file = compare_structured_file(
            Some("old.ts"),
            Some("new.ts"),
            Some(&old),
            Some(&new),
            &[
                hunk(Some((1, 1)), Some((1, 1))),
                hunk(Some((10, 10)), Some((10, 10))),
            ],
        )
        .unwrap();
        assert_eq!(file.symbol_changes().len(), 4);
        assert_eq!(
            file.symbol_changes()
                .iter()
                .filter(|change| change.kind() == SymbolChangeKind::Added)
                .count(),
            2
        );
        assert_eq!(
            file.symbol_changes()
                .iter()
                .filter(|change| change.kind() == SymbolChangeKind::Removed)
                .count(),
            2
        );
    }

    #[test]
    fn preserves_unchanged_parent_in_both_outlines() {
        let rust = provenance(SyntaxLanguage::Rust, "rust-test");
        let parent = |body_end| {
            fact(
                &rust,
                &[],
                SymbolKind::Module,
                "outer",
                range(0, body_end, 1, 20),
                range(0, 10, 1, 1),
                Some(range(11, body_end, 2, 20)),
                "mod outer",
                0,
                0,
                0,
            )
        };
        let old = parsed(
            "src/lib.rs",
            &rust,
            vec![
                parent(2_000),
                function(&rust, &["outer"], "child", 5, "fn child()"),
            ],
        );
        let new = parsed(
            "src/lib.rs",
            &rust,
            vec![
                parent(2_000),
                function(&rust, &["outer"], "child", 5, "fn child()"),
            ],
        );
        let file = compare_structured_file(
            Some("src/lib.rs"),
            Some("src/lib.rs"),
            Some(&old),
            Some(&new),
            &[hunk(Some((7, 7)), Some((7, 7)))],
        )
        .unwrap();
        assert_eq!(file.symbol_changes().len(), 1);
        assert_eq!(file.old_outline()[0].symbol().key().name(), "outer");
        assert_eq!(
            file.old_outline()[0].children()[0].symbol().key().name(),
            "child"
        );
        assert_eq!(
            file.new_outline()[0].children()[0].symbol().key().name(),
            "child"
        );
    }

    #[test]
    fn navigation_uses_new_rename_path_and_old_deleted_path() {
        let rust = provenance(SyntaxLanguage::Rust, "rust-test");
        let old = parsed(
            "old.rs",
            &rust,
            vec![function(&rust, &[], "work", 2, "fn work()")],
        );
        let new = parsed(
            "new.rs",
            &rust,
            vec![function(&rust, &[], "work", 2, "fn work()")],
        );
        let renamed = compare_structured_file(
            Some("old.rs"),
            Some("new.rs"),
            Some(&old),
            Some(&new),
            &[hunk(Some((4, 4)), Some((4, 4)))],
        )
        .unwrap();
        assert_eq!(renamed.symbol_changes()[0].navigation().path, "new.rs");
        assert_eq!(
            renamed.symbol_changes()[0].navigation().side,
            ComparisonSide::Head
        );

        let deleted = compare_structured_file(
            Some("old.rs"),
            None,
            Some(&old),
            None,
            &[hunk(Some((2, 6)), None)],
        )
        .unwrap();
        assert_eq!(deleted.symbol_changes()[0].navigation().path, "old.rs");
        assert_eq!(
            deleted.symbol_changes()[0].navigation().side,
            ComparisonSide::Base
        );
    }

    #[test]
    fn unicode_offsets_and_inclusive_hunk_boundaries_are_preserved() {
        let rust = provenance(SyntaxLanguage::Rust, "rust-test");
        let source = "// žluť\nfn převeď() {\n    work();\n}\n";
        let start = u64::try_from(source.find("fn převeď").unwrap()).unwrap();
        let body_start = u64::try_from(source.find('{').unwrap()).unwrap();
        let end = u64::try_from(source.len()).unwrap();
        let unicode = fact(
            &rust,
            &[],
            SymbolKind::Function,
            "převeď",
            range(start, end, 2, 4),
            range(start, body_start, 2, 2),
            Some(range(body_start, end, 2, 4)),
            "fn převeď()",
            0,
            1,
            0,
        );
        unicode.full_range().validate_source(source).unwrap();
        unicode.signature_range().validate_source(source).unwrap();
        unicode
            .body_range()
            .unwrap()
            .validate_source(source)
            .unwrap();
        let old = parsed("unicode.rs", &rust, vec![unicode.clone()]);
        let new = parsed("unicode.rs", &rust, vec![unicode]);
        let file = compare_structured_file(
            Some("unicode.rs"),
            Some("unicode.rs"),
            Some(&old),
            Some(&new),
            &[hunk(Some((4, 4)), Some((4, 4)))],
        )
        .unwrap();
        let change = &file.symbol_changes()[0];
        assert_eq!(change.navigation().line.get(), 2);
        assert_eq!(change.navigation().byte_offset, Some(start));
        assert_eq!(change.changed_old_lines(), 1);
        assert_eq!(change.changed_new_lines(), 1);
    }

    #[test]
    fn hotspot_ties_have_stable_name_order_and_named_metrics() {
        let rust = provenance(SyntaxLanguage::Rust, "rust-test");
        let old = parsed(
            "src/lib.rs",
            &rust,
            vec![
                function(&rust, &[], "beta", 1, "fn beta()"),
                function(&rust, &[], "alpha", 10, "fn alpha()"),
                fact(
                    &rust,
                    &[],
                    SymbolKind::Struct,
                    "Container",
                    range(2_000, 2_090, 20, 24),
                    range(2_000, 2_020, 20, 20),
                    Some(range(2_021, 2_090, 21, 24)),
                    "struct Container",
                    0,
                    0,
                    3,
                ),
            ],
        );
        let new = old.clone();
        let file = compare_structured_file(
            Some("src/lib.rs"),
            Some("src/lib.rs"),
            Some(&old),
            Some(&new),
            &[
                hunk(Some((3, 3)), Some((3, 3))),
                hunk(Some((12, 12)), Some((12, 12))),
            ],
        )
        .unwrap();
        let changed: Vec<_> = file
            .hotspots()
            .iter()
            .filter(|hotspot| matches!(hotspot.metric(), StructuralMetric::ChangedLines { .. }))
            .collect();
        assert_eq!(changed[0].symbol().key().name(), "alpha");
        assert_eq!(changed[1].symbol().key().name(), "beta");
        let largest: Vec<_> = file
            .hotspots()
            .iter()
            .filter(|hotspot| {
                matches!(hotspot.metric(), StructuralMetric::FunctionLineCount { .. })
            })
            .collect();
        assert_eq!(largest[0].symbol().key().name(), "alpha");
        assert!(file.hotspots().iter().any(|hotspot| {
            matches!(
                hotspot.metric(),
                StructuralMetric::ParameterCount { parameters: 1 }
            )
        }));
        assert!(file.hotspots().iter().any(|hotspot| {
            matches!(
                hotspot.metric(),
                StructuralMetric::SyntacticNestingDepth { depth: 2 }
            )
        }));
        assert!(file.hotspots().iter().any(|hotspot| {
            matches!(
                hotspot.metric(),
                StructuralMetric::TypeMemberCount { members: 3 }
            )
        }));
    }

    #[test]
    fn translates_unsupported_partial_failed_truncated_and_skipped_statuses() {
        let ts = provenance(SyntaxLanguage::TypeScript, "ts-test");
        let unsupported = document(
            "file.ts",
            &ts,
            DocumentStatus::Unsupported,
            Vec::new(),
            Vec::new(),
            None,
        );
        let file =
            compare_structured_file(None, Some("file.ts"), None, Some(&unsupported), &[]).unwrap();
        assert_eq!(file.status(), FileAnalysisStatus::Unsupported);
        assert!(file.new_outline().is_empty());

        let warning =
            SyntaxDiagnostic::new(DiagnosticSeverity::Warning, "recovered", None).unwrap();
        let partial = document(
            "file.ts",
            &ts,
            DocumentStatus::Partial,
            Vec::new(),
            vec![warning],
            None,
        );
        let file =
            compare_structured_file(None, Some("file.ts"), None, Some(&partial), &[]).unwrap();
        assert_eq!(file.status(), FileAnalysisStatus::Partial);
        assert_eq!(file.errors().len(), 1);

        let failure = SyntaxDiagnostic::new(DiagnosticSeverity::Error, "failed", None).unwrap();
        let failed = document(
            "file.ts",
            &ts,
            DocumentStatus::Failed,
            Vec::new(),
            vec![failure],
            None,
        );
        let file =
            compare_structured_file(None, Some("file.ts"), None, Some(&failed), &[]).unwrap();
        assert_eq!(file.status(), FileAnalysisStatus::Failed);
        assert!(file.symbol_changes().is_empty());

        let truncation =
            SyntaxTruncation::new(SyntaxTruncationReason::SymbolCount, Some(10), Some(11)).unwrap();
        let truncated = document(
            "file.ts",
            &ts,
            DocumentStatus::Partial,
            vec![function(&ts, &[], "available", 1, "function available()")],
            Vec::new(),
            Some(truncation),
        );
        let file =
            compare_structured_file(None, Some("file.ts"), None, Some(&truncated), &[]).unwrap();
        assert_eq!(file.status(), FileAnalysisStatus::Partial);
        assert_eq!(
            file.truncation().unwrap().reason,
            TruncationReason::CaptureLimit
        );
        assert_eq!(file.new_outline().len(), 1);

        let file = compare_structured_file(None, Some("file.ts"), None, None, &[]).unwrap();
        assert_eq!(file.status(), FileAnalysisStatus::Skipped);
    }

    #[test]
    fn unchanged_unique_symbol_is_not_reported_as_a_change() {
        let rust = provenance(SyntaxLanguage::Rust, "rust-test");
        let document = parsed(
            "src/lib.rs",
            &rust,
            vec![function(&rust, &[], "unchanged", 1, "fn unchanged()")],
        );
        let file = compare_structured_file(
            Some("src/lib.rs"),
            Some("src/lib.rs"),
            Some(&document),
            Some(&document),
            &[],
        )
        .unwrap();
        assert!(file.symbol_changes().is_empty());
        assert_eq!(file.old_outline().len(), 1);
        assert_eq!(file.new_outline().len(), 1);
    }

    #[test]
    fn unique_callable_pairs_report_each_changed_call_dimension() {
        let ts = provenance(SyntaxLanguage::TypeScript, "ts-test");
        let arguments = function(&ts, &[], "arguments", 1, "function arguments() {}");
        let control = function(&ts, &[], "control", 10, "function control() {}");
        let combined = function(&ts, &[], "combined", 20, "function combined() {}");
        let symbols = vec![arguments.clone(), control.clone(), combined.clone()];
        let old_calls = vec![
            direct_call(&ts, &arguments, "load", "old", 130, 3, Vec::new()),
            direct_call(&ts, &control, "load", "same", 1_030, 12, Vec::new()),
            direct_call(
                &ts,
                &combined,
                "load",
                "old",
                2_030,
                22,
                vec![ControlContext::Loop],
            ),
        ];
        let new_calls = vec![
            direct_call(&ts, &arguments, "load", "new", 130, 3, Vec::new()),
            direct_call(
                &ts,
                &control,
                "load",
                "same",
                1_030,
                12,
                vec![ControlContext::Condition],
            ),
            direct_call(
                &ts,
                &combined,
                "load",
                "new",
                2_030,
                22,
                vec![ControlContext::Condition],
            ),
        ];
        let file = compare_structured_file(
            Some("src/old.ts"),
            Some("src/new.ts"),
            Some(&parsed_with_calls(
                "src/old.ts",
                &ts,
                symbols.clone(),
                old_calls,
            )),
            Some(&parsed_with_calls("src/new.ts", &ts, symbols, new_calls)),
            &[],
        )
        .unwrap();

        assert_eq!(file.call_diff().len(), 3);
        let find = |name: &str| {
            file.call_diff()
                .iter()
                .find(|change| {
                    call_change_fact(change)
                        .and_then(CallFact::enclosing_symbol)
                        .is_some_and(|key| key.name() == name)
                })
                .unwrap()
        };
        let argument_change = find("arguments");
        assert!(argument_change.arguments_changed());
        assert!(!argument_change.control_context_changed());
        let control_change = find("control");
        assert!(!control_change.arguments_changed());
        assert!(control_change.control_context_changed());
        let combined_change = find("combined");
        assert!(combined_change.arguments_changed());
        assert!(combined_change.control_context_changed());
        assert!(file.call_diff().iter().all(|change| {
            change.kind() == CallChangeKind::Modified
                && change.navigation().path == "src/new.ts"
                && change.navigation().side == ComparisonSide::Head
        }));
    }

    #[test]
    fn repeated_calls_degrade_and_duplicate_symbol_keys_suppress_pairing() {
        let rust = provenance(SyntaxLanguage::Rust, "rust-test");
        let run = function(&rust, &[], "run", 1, "fn run() {}");
        let repeated = compare_structured_file(
            Some("old.rs"),
            Some("new.rs"),
            Some(&parsed_with_calls(
                "old.rs",
                &rust,
                vec![run.clone()],
                vec![
                    direct_call(&rust, &run, "load", "old-a", 130, 3, Vec::new()),
                    direct_call(&rust, &run, "load", "old-b", 150, 4, Vec::new()),
                ],
            )),
            Some(&parsed_with_calls(
                "new.rs",
                &rust,
                vec![run.clone()],
                vec![direct_call(&rust, &run, "load", "new", 130, 3, Vec::new())],
            )),
            &[],
        )
        .unwrap();
        assert_eq!(repeated.call_diff().len(), 3);
        assert_eq!(
            repeated
                .call_diff()
                .iter()
                .filter(|change| change.kind() == CallChangeKind::Removed)
                .count(),
            2
        );
        assert_eq!(
            repeated
                .call_diff()
                .iter()
                .filter(|change| change.kind() == CallChangeKind::Added)
                .count(),
            1
        );
        assert!(
            repeated
                .call_diff()
                .iter()
                .all(|change| change.pairing().is_none())
        );

        let duplicate_old = vec![run.clone(), function(&rust, &[], "run", 10, "fn run() {}")];
        let duplicate_new = duplicate_old.clone();
        let duplicate = compare_structured_file(
            Some("old.rs"),
            Some("new.rs"),
            Some(&parsed_with_calls(
                "old.rs",
                &rust,
                duplicate_old,
                vec![direct_call(&rust, &run, "load", "old", 130, 3, Vec::new())],
            )),
            Some(&parsed_with_calls(
                "new.rs",
                &rust,
                duplicate_new,
                vec![direct_call(&rust, &run, "load", "new", 130, 3, Vec::new())],
            )),
            &[],
        )
        .unwrap();
        assert!(duplicate.call_diff().is_empty());
    }

    #[test]
    fn same_callee_is_scoped_to_function_and_method_across_renamed_paths() {
        let ts = provenance(SyntaxLanguage::TypeScript, "ts-test");
        let function = function(&ts, &[], "loadPage", 1, "function loadPage() {}");
        let method = fact(
            &ts,
            &["Store"],
            SymbolKind::Method,
            "refresh",
            range(1_000, 1_090, 10, 14),
            range(1_000, 1_020, 10, 10),
            Some(range(1_021, 1_090, 11, 14)),
            "refresh() {}",
            0,
            1,
            0,
        );
        let symbols = vec![function.clone(), method.clone()];
        let old_calls = vec![
            direct_call(&ts, &function, "load", "page-old", 130, 3, Vec::new()),
            direct_call(&ts, &method, "load", "store-old", 1_030, 12, Vec::new()),
        ];
        let new_calls = vec![
            direct_call(&ts, &function, "load", "page-new", 130, 3, Vec::new()),
            direct_call(&ts, &method, "load", "store-new", 1_030, 12, Vec::new()),
        ];
        let file = compare_structured_file(
            Some("src/before.ts"),
            Some("src/after.ts"),
            Some(&parsed_with_calls(
                "src/before.ts",
                &ts,
                symbols.clone(),
                old_calls,
            )),
            Some(&parsed_with_calls("src/after.ts", &ts, symbols, new_calls)),
            &[],
        )
        .unwrap();

        assert_eq!(file.call_diff().len(), 2);
        let enclosing_names: HashSet<_> = file
            .call_diff()
            .iter()
            .map(|change| {
                change
                    .new_fact()
                    .unwrap()
                    .enclosing_symbol()
                    .unwrap()
                    .qualified_name()
            })
            .collect();
        assert_eq!(
            enclosing_names,
            HashSet::from(["loadPage".to_owned(), "Store::refresh".to_owned()])
        );
        assert!(file.call_diff().iter().all(|change| {
            change.navigation().path == "src/after.ts"
                && change.navigation().side == ComparisonSide::Head
        }));
    }

    #[test]
    fn parser_mismatch_suppresses_call_diff() {
        let old_provenance = provenance(SyntaxLanguage::Rust, "rust-old");
        let new_provenance = provenance(SyntaxLanguage::Rust, "rust-new");
        let old = function(&old_provenance, &[], "run", 1, "fn run() {}");
        let new = function(&new_provenance, &[], "run", 1, "fn run() {}");
        let file = compare_structured_file(
            Some("old.rs"),
            Some("new.rs"),
            Some(&parsed_with_calls(
                "old.rs",
                &old_provenance,
                vec![old.clone()],
                vec![direct_call(
                    &old_provenance,
                    &old,
                    "load",
                    "old",
                    130,
                    3,
                    Vec::new(),
                )],
            )),
            Some(&parsed_with_calls(
                "new.rs",
                &new_provenance,
                vec![new.clone()],
                vec![direct_call(
                    &new_provenance,
                    &new,
                    "load",
                    "new",
                    130,
                    3,
                    Vec::new(),
                )],
            )),
            &[],
        )
        .unwrap();
        assert!(file.call_diff().is_empty());
    }

    #[test]
    fn multi_function_call_diff_order_is_stable_for_reversed_inputs() {
        let rust = provenance(SyntaxLanguage::Rust, "rust-test");
        let first = function(&rust, &[], "first", 1, "fn first() {}");
        let second = function(&rust, &[], "second", 10, "fn second() {}");
        let symbols = vec![first.clone(), second.clone()];
        let old_calls = vec![
            direct_call(&rust, &first, "zeta", "old", 150, 4, Vec::new()),
            direct_call(&rust, &second, "alpha", "old", 1_030, 12, Vec::new()),
        ];
        let new_calls = vec![
            direct_call(&rust, &first, "zeta", "new", 150, 4, Vec::new()),
            direct_call(&rust, &second, "alpha", "new", 1_030, 12, Vec::new()),
        ];
        let forward = compare_structured_file(
            Some("old.rs"),
            Some("new.rs"),
            Some(&parsed_with_calls(
                "old.rs",
                &rust,
                symbols.clone(),
                old_calls.clone(),
            )),
            Some(&parsed_with_calls(
                "new.rs",
                &rust,
                symbols.clone(),
                new_calls.clone(),
            )),
            &[],
        )
        .unwrap();
        let reversed = compare_structured_file(
            Some("old.rs"),
            Some("new.rs"),
            Some(&parsed_with_calls(
                "old.rs",
                &rust,
                symbols.iter().cloned().rev().collect(),
                old_calls.iter().cloned().rev().collect(),
            )),
            Some(&parsed_with_calls(
                "new.rs",
                &rust,
                symbols.into_iter().rev().collect(),
                new_calls.into_iter().rev().collect(),
            )),
            &[],
        )
        .unwrap();
        assert_eq!(forward.call_diff(), reversed.call_diff());
        assert_eq!(
            forward
                .call_diff()
                .iter()
                .map(|change| change.navigation().line.get())
                .collect::<Vec<_>>(),
            vec![4, 12]
        );
    }

    #[test]
    fn language_and_provenance_rules_are_conservative() {
        let ts = provenance(SyntaxLanguage::TypeScript, "ts-test");
        let tsx = provenance(SyntaxLanguage::Tsx, "tsx-test");
        let rust = provenance(SyntaxLanguage::Rust, "rust-test");
        let ts_document = parsed("file.ts", &ts, vec![function(&ts, &[], "work", 1, "work")]);
        let tsx_document = parsed(
            "file.tsx",
            &tsx,
            vec![function(&tsx, &[], "work", 1, "work")],
        );
        let mismatch = compare_structured_file(
            Some("file.ts"),
            Some("file.tsx"),
            Some(&ts_document),
            Some(&tsx_document),
            &[hunk(Some((1, 1)), Some((1, 1)))],
        )
        .unwrap();
        assert_eq!(mismatch.status(), FileAnalysisStatus::Failed);
        assert!(mismatch.symbol_changes().is_empty());

        let rust_old = parsed(
            "old.rs",
            &rust,
            vec![function(&rust, &[], "work", 1, "fn work()")],
        );
        let rust_new_provenance = provenance(SyntaxLanguage::Rust, "rust-new-parser");
        let rust_new = parsed(
            "new.rs",
            &rust_new_provenance,
            vec![function(&rust_new_provenance, &[], "work", 1, "fn work()")],
        );
        let file = compare_structured_file(
            Some("old.rs"),
            Some("new.rs"),
            Some(&rust_old),
            Some(&rust_new),
            &[hunk(Some((1, 1)), Some((1, 1)))],
        )
        .unwrap();
        assert_eq!(file.symbol_changes().len(), 2);
        assert!(
            file.symbol_changes()
                .iter()
                .all(|change| change.kind() != SymbolChangeKind::Modified)
        );
    }

    #[test]
    fn exact_document_paths_are_required() {
        let rust = provenance(SyntaxLanguage::Rust, "rust-test");
        let document = parsed("actual.rs", &rust, Vec::new());
        let error = compare_structured_file(None, Some("other.rs"), None, Some(&document), &[])
            .unwrap_err();
        assert!(error.to_string().contains("exact comparison path"));
    }

    #[test]
    fn cancelled_syntax_truncation_keeps_unmeasured_review_evidence() {
        let rust = provenance(SyntaxLanguage::Rust, "rust-test");
        let cancellation =
            SyntaxTruncation::new(SyntaxTruncationReason::Cancelled, None, None).unwrap();
        let document = document(
            "src/lib.rs",
            &rust,
            DocumentStatus::Partial,
            Vec::new(),
            Vec::new(),
            Some(cancellation),
        );
        let file =
            compare_structured_file(None, Some("src/lib.rs"), None, Some(&document), &[]).unwrap();
        assert_eq!(
            file.truncation().unwrap().reason,
            TruncationReason::Cancelled
        );
        assert_eq!(file.truncation().unwrap().limit, None);
        assert_eq!(file.truncation().unwrap().observed, None);
    }

    #[test]
    fn controlled_checkpoints_stop_outline_symbol_and_hunk_work() {
        let rust = provenance(SyntaxLanguage::Rust, "rust-test");
        let symbols: Vec<_> = (0_u32..100)
            .map(|index| {
                function(
                    &rust,
                    &[],
                    &format!("function_{index}"),
                    index * 10 + 1,
                    &format!("fn function_{index}()"),
                )
            })
            .collect();
        let document = parsed("src/lib.rs", &rust, symbols.clone());
        let mut sort_checks = 0_u32;
        let error = build_outline(&document, ComparisonSide::Head, &mut || {
            sort_checks += 1;
            (sort_checks == 3).then_some(ComparisonStopReason::Deadline)
        })
        .unwrap_err();
        assert_eq!(error.stop_reason(), Some(ComparisonStopReason::Deadline));

        let mut outline_checks = 0_u32;
        let error = build_outline(&document, ComparisonSide::Head, &mut || {
            outline_checks += 1;
            (outline_checks == 25).then_some(ComparisonStopReason::Cancelled)
        })
        .unwrap_err();
        assert_eq!(error.stop_reason(), Some(ComparisonStopReason::Cancelled));

        let mut symbol_checks = 0_u32;
        let error = compare_symbols(
            Some("old.rs"),
            Some("new.rs"),
            &symbols,
            &symbols,
            &[],
            &mut || {
                symbol_checks += 1;
                (symbol_checks == 150).then_some(ComparisonStopReason::Deadline)
            },
        )
        .unwrap_err();
        assert_eq!(error.stop_reason(), Some(ComparisonStopReason::Deadline));

        let wide = fact(
            &rust,
            &[],
            SymbolKind::Function,
            "wide",
            range(0, 50_000, 1, 500),
            range(0, 20, 1, 1),
            Some(range(21, 50_000, 2, 500)),
            "fn wide()",
            0,
            0,
            0,
        );
        let hunks: Vec<_> = (2_u32..102)
            .map(|line| hunk(Some((line, line)), Some((line, line))))
            .collect();
        let mut hunk_checks = 0_u32;
        let error = compare_symbols(
            Some("old.rs"),
            Some("new.rs"),
            std::slice::from_ref(&wide),
            std::slice::from_ref(&wide),
            &hunks,
            &mut || {
                hunk_checks += 1;
                (hunk_checks == 30).then_some(ComparisonStopReason::Disconnected)
            },
        )
        .unwrap_err();
        assert_eq!(
            error.stop_reason(),
            Some(ComparisonStopReason::Disconnected)
        );
    }

    #[test]
    fn wide_outline_parent_stops_inside_controlled_child_validation() {
        let rust = provenance(SyntaxLanguage::Rust, "rust-test");
        let parent = fact(
            &rust,
            &[],
            SymbolKind::Module,
            "wide",
            range(0, 200_000, 1, 2_000),
            range(0, 20, 1, 1),
            Some(range(21, 200_000, 2, 2_000)),
            "mod wide",
            0,
            0,
            100,
        );
        let mut symbols = vec![parent];
        symbols.extend((0_u32..100).map(|index| {
            function(
                &rust,
                &["wide"],
                &format!("child_{index}"),
                index * 10 + 10,
                &format!("fn child_{index}()"),
            )
        }));
        let document = parsed("src/lib.rs", &rust, symbols);
        let mut total_checks = 0_u32;
        build_outline(&document, ComparisonSide::Head, &mut || {
            total_checks += 1;
            None
        })
        .unwrap();
        let stop_at = total_checks - 50;
        let mut checks = 0_u32;
        let error = build_outline(&document, ComparisonSide::Head, &mut || {
            checks += 1;
            (checks == stop_at).then_some(ComparisonStopReason::Cancelled)
        })
        .unwrap_err();

        assert_eq!(error.stop_reason(), Some(ComparisonStopReason::Cancelled));
    }

    #[test]
    fn structure_call_index_is_checkpointed_near_linearly() {
        let rust = provenance(SyntaxLanguage::Rust, "rust-test");
        let symbols: Vec<_> = (0_u32..100)
            .map(|index| {
                function(
                    &rust,
                    &[],
                    &format!("function_{index}"),
                    index * 10 + 1,
                    &format!("fn function_{index}()"),
                )
            })
            .collect();
        let calls: Vec<_> = symbols
            .iter()
            .enumerate()
            .map(|(index, symbol)| {
                let start_line = u32::try_from(index).unwrap() * 10 + 3;
                direct_call(
                    &rust,
                    symbol,
                    "load",
                    "same",
                    u64::from(start_line - 3) * 100 + 130,
                    start_line,
                    Vec::new(),
                )
            })
            .collect();
        let old = parsed_with_calls("old.rs", &rust, symbols.clone(), calls.clone());
        let new = parsed_with_calls("new.rs", &rust, symbols, calls);
        let mut checkpoints = 0_usize;
        let file = compare_structured_file_controlled(
            Some("old.rs"),
            Some("new.rs"),
            Some(&old),
            Some(&new),
            &[],
            &mut || {
                checkpoints += 1;
                None
            },
        )
        .unwrap();

        assert!(file.call_diff().is_empty());
        assert!(checkpoints < 10_000, "checkpoint count was {checkpoints}");
    }

    #[test]
    fn controlled_file_comparison_reports_immediate_disconnect() {
        let error = compare_structured_file_controlled(
            None,
            Some("src/lib.rs"),
            None,
            None,
            &[],
            &mut || Some(ComparisonStopReason::Disconnected),
        )
        .unwrap_err();
        assert_eq!(
            error.stop_reason(),
            Some(ComparisonStopReason::Disconnected)
        );
    }

    #[test]
    fn controlled_file_comparison_stops_inside_final_model_validation() {
        let hunks: Vec<_> = (1_u32..=100)
            .map(|line| hunk(None, Some((line, line))))
            .collect();
        let mut checks = 0_u32;
        let error = compare_structured_file_controlled(
            None,
            Some("src/lib.rs"),
            None,
            None,
            &hunks,
            &mut || {
                checks += 1;
                (checks == 150).then_some(ComparisonStopReason::Disconnected)
            },
        )
        .unwrap_err();

        assert_eq!(
            error.stop_reason(),
            Some(ComparisonStopReason::Disconnected)
        );
    }

    #[test]
    fn line_helpers_use_fixed_width_nonzero_values() {
        assert_eq!(lines(1, 3).line_count(), 3);
        assert_eq!(NonZeroU64::new(1).unwrap().get(), 1);
    }
}
