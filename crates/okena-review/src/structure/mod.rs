//! Deterministic structural comparison for one exact file pair.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt;

use okena_core::review::{
    ComparisonSide, ReviewNavigationTarget, ReviewTruncation, TruncationReason,
};
use okena_syntax::{
    CallFact, ControlContext, DiagnosticSeverity, DocumentStatus, DocumentStructure, SourceRange,
    SymbolFact, SymbolKey, SymbolKind, SyntaxLanguage, SyntaxTruncation, SyntaxTruncationReason,
};

use crate::call_diff::{CallDiffError, CallDiffInput, compare_calls};
use crate::{
    AnalysisError, AnalysisStage, CallChangeKind, CallDiffChange, ChangedHunk, ChangedLineRange,
    FileAnalysisStatus, ModelError, OutlineFact, SignatureChange, StructuralHotspot,
    StructuralMetric, StructuredFile, SymbolChange, SymbolChangeKind, SymbolReference,
};

/// Invalid comparator input or a result rejected by the frozen review model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructureError(String);

impl StructureError {
    fn invalid(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for StructureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for StructureError {}

impl From<ModelError> for StructureError {
    fn from(error: ModelError) -> Self {
        Self(error.to_string())
    }
}

impl From<CallDiffError> for StructureError {
    fn from(error: CallDiffError) -> Self {
        Self(error.to_string())
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
    validate_inputs(old_path, new_path, old_document, new_document)?;

    if old_path.is_some() && old_document.is_none() || new_path.is_some() && new_document.is_none()
    {
        return unsuccessful_file(
            old_path,
            new_path,
            FileAnalysisStatus::Skipped,
            changed_hunks,
            Vec::new(),
        );
    }

    let documents: Vec<_> = old_document.into_iter().chain(new_document).collect();
    if documents
        .iter()
        .any(|document| document.status() == DocumentStatus::Failed)
    {
        let mut errors = document_errors(&documents, true)?;
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
        );
    }
    if documents
        .iter()
        .any(|document| document.status() == DocumentStatus::Unsupported)
    {
        let errors = document_errors(&documents, true)?;
        return unsuccessful_file(
            old_path,
            new_path,
            FileAnalysisStatus::Unsupported,
            changed_hunks,
            errors,
        );
    }
    if documents
        .iter()
        .any(|document| document.status() == DocumentStatus::Skipped)
    {
        let errors = document_errors(&documents, true)?;
        return unsuccessful_file(
            old_path,
            new_path,
            FileAnalysisStatus::Skipped,
            changed_hunks,
            errors,
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
        );
    }

    let old_outline = old_document
        .map(|document| build_outline(document, ComparisonSide::Base))
        .transpose()?
        .unwrap_or_default();
    let new_outline = new_document
        .map(|document| build_outline(document, ComparisonSide::Head))
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
    )?;
    let call_diff = compare_matched_calls(old_path, new_path, old_document, new_document)?;
    let hotspots = build_hotspots(new_path, new_document, &symbol_changes)?;

    let mut errors = document_errors(&documents, false)?;
    let truncations = document_truncations(old_document, new_document);
    for (side, document, truncation) in &truncations {
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

    Ok(StructuredFile::new(
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
        changed_hunks.to_vec(),
        errors,
        review_truncation,
    )?)
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
) -> Result<StructuredFile, StructureError> {
    Ok(StructuredFile::new(
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
        changed_hunks.to_vec(),
        errors,
        None,
    )?)
}

fn selected_path<'a>(old_path: Option<&'a str>, new_path: Option<&'a str>) -> Option<&'a str> {
    new_path.or(old_path)
}

fn document_errors(
    documents: &[&DocumentStructure],
    include_all: bool,
) -> Result<Vec<AnalysisError>, StructureError> {
    let mut errors = Vec::new();
    for document in documents {
        for diagnostic in document.diagnostics() {
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
        SyntaxTruncationReason::SourceBytes => TruncationReason::ByteLimit,
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
) -> Result<Vec<OutlineFact>, StructureError> {
    let symbols = document.symbols();
    let mut order: Vec<usize> = (0..symbols.len()).collect();
    order.sort_by(|left, right| symbol_source_order(&symbols[*left], &symbols[*right]));

    let mut parents = vec![None; symbols.len()];
    let mut stack = Vec::<usize>::new();
    for index in order.iter().copied() {
        while stack
            .last()
            .is_some_and(|candidate| !is_outline_parent(&symbols[*candidate], &symbols[index]))
        {
            stack.pop();
        }
        parents[index] = stack.last().copied();
        stack.push(index);
    }

    let mut children = vec![Vec::new(); symbols.len()];
    let mut roots = Vec::new();
    for index in order.iter().copied() {
        if let Some(parent) = parents[index] {
            children[parent].push(index);
        } else {
            roots.push(index);
        }
    }
    let mut built = vec![None; symbols.len()];
    for index in order.iter().rev().copied() {
        let child_facts = children[index]
            .iter()
            .map(|child| {
                built[*child]
                    .take()
                    .ok_or_else(|| StructureError::invalid("outline child was not built"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let fact = &symbols[index];
        built[index] = Some(OutlineFact::new(
            document.provenance().clone(),
            SymbolReference::new(side, fact.full_range(), fact.key().clone()),
            child_facts,
        )?);
    }
    roots
        .into_iter()
        .map(|root| {
            built[root]
                .take()
                .ok_or_else(|| StructureError::invalid("outline root was not built"))
        })
        .collect()
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
) -> Result<Vec<SymbolChange>, StructureError> {
    let mut matched_old = HashSet::new();
    let mut matched_new = HashSet::new();
    let mut changes = Vec::new();

    for (old_index, new_index) in unique_pair_indices(old_symbols, new_symbols) {
        let old = &old_symbols[old_index];
        let new = &new_symbols[new_index];
        match compare_unique_pair(
            old_path,
            new_path,
            old,
            new,
            old_symbols,
            new_symbols,
            changed_hunks,
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
        if matched_old.contains(&index) {
            continue;
        }
        let hunks = one_side_hunks(changed_hunks, ComparisonSide::Base, fact.full_range());
        if hunks.is_empty() {
            continue;
        }
        changes.push(SymbolChange::new(
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
        )?);
    }
    for (index, fact) in new_symbols.iter().enumerate() {
        if matched_new.contains(&index) {
            continue;
        }
        let hunks = one_side_hunks(changed_hunks, ComparisonSide::Head, fact.full_range());
        if hunks.is_empty() {
            continue;
        }
        changes.push(SymbolChange::new(
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
        )?);
    }
    changes.sort_by(symbol_change_order);
    Ok(changes)
}

fn group_by_key(symbols: &[SymbolFact]) -> HashMap<SymbolKey, Vec<usize>> {
    let mut grouped = HashMap::<SymbolKey, Vec<usize>>::new();
    for (index, symbol) in symbols.iter().enumerate() {
        grouped.entry(symbol.key().clone()).or_default().push(index);
    }
    grouped
}

fn unique_pair_indices(
    old_symbols: &[SymbolFact],
    new_symbols: &[SymbolFact],
) -> Vec<(usize, usize)> {
    let old_by_key = group_by_key(old_symbols);
    let new_by_key = group_by_key(new_symbols);
    old_symbols
        .iter()
        .enumerate()
        .filter_map(|(old_index, old)| {
            let old_occurrences = old_by_key.get(old.key())?;
            let new_occurrences = new_by_key.get(old.key())?;
            if old_occurrences.len() != 1 || new_occurrences.len() != 1 {
                return None;
            }
            let new_index = new_occurrences[0];
            (old.provenance() == new_symbols[new_index].provenance())
                .then_some((old_index, new_index))
        })
        .collect()
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
    old_symbols: &[SymbolFact],
    new_symbols: &[SymbolFact],
    changed_hunks: &[ChangedHunk],
) -> Result<UniquePairResult, StructureError> {
    let candidate_hunks = pair_hunks(changed_hunks, old, new, old_symbols, new_symbols);
    let signature_text_changed = old.normalized_signature() != new.normalized_signature();
    let signature_has_evidence = side_has_intersection(
        &candidate_hunks,
        ComparisonSide::Base,
        old.signature_range(),
    ) || side_has_intersection(
        &candidate_hunks,
        ComparisonSide::Head,
        new.signature_range(),
    );
    if signature_text_changed && !signature_has_evidence {
        return Ok(UniquePairResult::Unpaired);
    }
    let body_changed = body_has_evidence(&candidate_hunks, old, new);
    let hunks = dimension_hunks(
        &candidate_hunks,
        old,
        new,
        signature_text_changed,
        body_changed,
    );
    let signature_has_evidence =
        side_has_intersection(&hunks, ComparisonSide::Base, old.signature_range())
            || side_has_intersection(&hunks, ComparisonSide::Head, new.signature_range());
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
    let body_changed = body_has_evidence(&hunks, old, new);
    if signature_change.is_none() && !body_changed {
        return Ok(UniquePairResult::Unchanged);
    }
    Ok(UniquePairResult::Changed(Box::new(SymbolChange::new(
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
    )?)))
}

fn dimension_hunks(
    hunks: &[ChangedHunk],
    old: &SymbolFact,
    new: &SymbolFact,
    signature_changed: bool,
    body_changed: bool,
) -> Vec<ChangedHunk> {
    hunks
        .iter()
        .filter(|hunk| {
            let old_relevant = hunk
                .old()
                .map(|lines| dimension_intersects(lines, old, signature_changed, body_changed));
            let new_relevant = hunk
                .new_range()
                .map(|lines| dimension_intersects(lines, new, signature_changed, body_changed));
            old_relevant.is_none_or(|relevant| relevant)
                && new_relevant.is_none_or(|relevant| relevant)
                && (old_relevant.unwrap_or(false) || new_relevant.unwrap_or(false))
        })
        .cloned()
        .collect()
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
    old: &SymbolFact,
    new: &SymbolFact,
    old_symbols: &[SymbolFact],
    new_symbols: &[SymbolFact],
) -> Vec<ChangedHunk> {
    hunks
        .iter()
        .filter(|hunk| {
            let old_valid = hunk
                .old()
                .is_none_or(|range| line_range_intersects(range, old.full_range()));
            let new_valid = hunk
                .new_range()
                .is_none_or(|range| line_range_intersects(range, new.full_range()));
            let intersects_own = hunk
                .old()
                .is_some_and(|range| hunk_intersects_own_symbol(range, old, old_symbols))
                || hunk
                    .new_range()
                    .is_some_and(|range| hunk_intersects_own_symbol(range, new, new_symbols));
            old_valid && new_valid && intersects_own
        })
        .cloned()
        .collect()
}

fn hunk_intersects_own_symbol(
    lines: ChangedLineRange,
    fact: &SymbolFact,
    symbols: &[SymbolFact],
) -> bool {
    if !line_range_intersects(lines, fact.full_range()) {
        return false;
    }
    let clipped_start = lines
        .start()
        .get()
        .max(fact.full_range().start_line().get());
    let clipped_end = lines.end().get().min(fact.full_range().end_line().get());
    !symbols.iter().any(|candidate| {
        is_descendant(fact, candidate)
            && candidate.full_range().start_line().get() <= clipped_start
            && clipped_end <= candidate.full_range().end_line().get()
    })
}

fn is_descendant(parent: &SymbolFact, candidate: &SymbolFact) -> bool {
    let mut parent_identity = parent.key().qualified_path().to_vec();
    parent_identity.push(parent.key().name().to_owned());
    candidate
        .key()
        .qualified_path()
        .starts_with(&parent_identity)
        && parent.full_range() != candidate.full_range()
        && parent.full_range().contains(candidate.full_range())
}

fn one_side_hunks(
    hunks: &[ChangedHunk],
    side: ComparisonSide,
    range: SourceRange,
) -> Vec<ChangedHunk> {
    hunks
        .iter()
        .filter(|hunk| {
            hunk_on_side(hunk, side).is_some_and(|lines| line_range_intersects(lines, range))
        })
        .cloned()
        .collect()
}

fn body_has_evidence(hunks: &[ChangedHunk], old: &SymbolFact, new: &SymbolFact) -> bool {
    [
        (ComparisonSide::Base, old.body_range()),
        (ComparisonSide::Head, new.body_range()),
    ]
    .into_iter()
    .any(|(side, range)| range.is_some_and(|range| side_has_intersection(hunks, side, range)))
}

fn side_has_intersection(hunks: &[ChangedHunk], side: ComparisonSide, range: SourceRange) -> bool {
    hunks.iter().any(|hunk| {
        hunk_on_side(hunk, side).is_some_and(|lines| line_range_intersects(lines, range))
    })
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
) -> Result<Vec<CallDiffChange>, StructureError> {
    let (Some(old_path), Some(new_path), Some(old_document), Some(new_document)) =
        (old_path, new_path, old_document, new_document)
    else {
        return Ok(Vec::new());
    };
    let mut changes = Vec::new();
    for (old_index, new_index) in
        unique_pair_indices(old_document.symbols(), new_document.symbols())
    {
        let old = &old_document.symbols()[old_index];
        let new = &new_document.symbols()[new_index];
        if !is_function(old.key().kind()) || !is_function(new.key().kind()) {
            continue;
        }
        changes.extend(compare_calls(CallDiffInput::new(
            old_path,
            new_path,
            old.key(),
            old.body_range().unwrap_or_else(|| old.full_range()),
            new.body_range().unwrap_or_else(|| new.full_range()),
            old_document.calls(),
            new_document.calls(),
        )?)?);
    }
    changes.sort_by(compare_call_changes);
    Ok(changes)
}

fn compare_call_changes(left: &CallDiffChange, right: &CallDiffChange) -> Ordering {
    left.navigation()
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
        .then_with(|| side_rank(left.navigation().side).cmp(&side_rank(right.navigation().side)))
        .then_with(|| match (call_change_fact(left), call_change_fact(right)) {
            (Some(left), Some(right)) => compare_call_facts(left, right),
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        })
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

fn compare_call_facts(left: &CallFact, right: &CallFact) -> Ordering {
    left.call_site_range()
        .start_byte()
        .cmp(&right.call_site_range().start_byte())
        .then_with(|| {
            left.call_site_range()
                .end_byte()
                .cmp(&right.call_site_range().end_byte())
        })
        .then_with(|| left.argument_text().cmp(right.argument_text()))
        .then_with(|| compare_control_contexts(left.control_context(), right.control_context()))
        .then_with(|| {
            syntax_language_rank(left.provenance().language())
                .cmp(&syntax_language_rank(right.provenance().language()))
        })
        .then_with(|| left.provenance().parser().cmp(right.provenance().parser()))
}

fn compare_control_contexts(left: &[ControlContext], right: &[ControlContext]) -> Ordering {
    for (left, right) in left.iter().zip(right) {
        let ordering = control_context_rank(left)
            .cmp(&control_context_rank(right))
            .then_with(|| match (left, right) {
                (ControlContext::Other(left), ControlContext::Other(right)) => left.cmp(right),
                _ => Ordering::Equal,
            });
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    left.len().cmp(&right.len())
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
) -> Result<Vec<StructuralHotspot>, StructureError> {
    let mut candidates = Vec::new();
    for change in changes {
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
    candidates.sort_by(HotspotCandidate::compare);
    Ok(candidates
        .into_iter()
        .map(|candidate| candidate.hotspot)
        .collect())
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
    fn line_helpers_use_fixed_width_nonzero_values() {
        assert_eq!(lines(1, 3).line_count(), 3);
        assert_eq!(NonZeroU64::new(1).unwrap().get(), 1);
    }
}
