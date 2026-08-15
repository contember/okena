//! Deterministic same-file comparison of direct outgoing calls.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

use okena_core::review::{ComparisonSide, ReviewNavigationTarget};
use okena_syntax::{CallFact, ControlContext, SourceRange, SymbolKey, SyntaxLanguage};

use crate::{CallChangeKind, CallDiffChange, CallPairingEvidence, CallPairingStrategy, ModelError};

/// Exact inputs for comparing direct calls in one uniquely matched descriptive symbol.
///
/// This input does not claim stable symbol identity. The caller selects a same-file old/new symbol
/// match and supplies each side's exact source range. Calls outside those ranges, calls owned by a
/// nested symbol, and calls owned by another same-named symbol are ignored.
#[derive(Clone, Copy, Debug)]
pub struct CallDiffInput<'a> {
    old_path: &'a str,
    new_path: &'a str,
    enclosing_symbol: &'a SymbolKey,
    old_enclosing_range: SourceRange,
    new_enclosing_range: SourceRange,
    old_calls: &'a [CallFact],
    new_calls: &'a [CallFact],
}

impl<'a> CallDiffInput<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        old_path: &'a str,
        new_path: &'a str,
        enclosing_symbol: &'a SymbolKey,
        old_enclosing_range: SourceRange,
        new_enclosing_range: SourceRange,
        old_calls: &'a [CallFact],
        new_calls: &'a [CallFact],
    ) -> Result<Self, CallDiffError> {
        if old_path.trim().is_empty() {
            return Err(CallDiffError::EmptyPath(ComparisonSide::Base));
        }
        if new_path.trim().is_empty() {
            return Err(CallDiffError::EmptyPath(ComparisonSide::Head));
        }
        Ok(Self {
            old_path,
            new_path,
            enclosing_symbol,
            old_enclosing_range,
            new_enclosing_range,
            old_calls,
            new_calls,
        })
    }
}

#[derive(Debug)]
pub enum CallDiffError {
    EmptyPath(ComparisonSide),
    InvalidChange(ModelError),
}

impl fmt::Display for CallDiffError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPath(ComparisonSide::Base) => {
                formatter.write_str("old call-diff path must not be empty")
            }
            Self::EmptyPath(ComparisonSide::Head) => {
                formatter.write_str("new call-diff path must not be empty")
            }
            Self::InvalidChange(error) => write!(formatter, "invalid call-diff change: {error}"),
        }
    }
}

impl std::error::Error for CallDiffError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidChange(error) => Some(error),
            Self::EmptyPath(_) => None,
        }
    }
}

impl From<ModelError> for CallDiffError {
    fn from(value: ModelError) -> Self {
        Self::InvalidChange(value)
    }
}

#[derive(Default)]
struct Candidates<'a> {
    old: Vec<&'a CallFact>,
    new: Vec<&'a CallFact>,
}

/// Compare direct outgoing calls for one selected same-file enclosing symbol.
///
/// A modified call is emitted only when its exact callee and descriptive enclosing key produce one
/// candidate on each side with identical syntax provenance. Any repetition or provenance mismatch
/// deliberately degrades to removed and added occurrences.
pub fn compare_calls(input: CallDiffInput<'_>) -> Result<Vec<CallDiffChange>, CallDiffError> {
    let mut candidates = BTreeMap::<&str, Candidates<'_>>::new();
    for call in calls_in_scope(
        input.old_calls,
        input.enclosing_symbol,
        input.old_enclosing_range,
    ) {
        candidates
            .entry(call.callee_text())
            .or_default()
            .old
            .push(call);
    }
    for call in calls_in_scope(
        input.new_calls,
        input.enclosing_symbol,
        input.new_enclosing_range,
    ) {
        candidates
            .entry(call.callee_text())
            .or_default()
            .new
            .push(call);
    }

    let mut changes = Vec::new();
    for group in candidates.values_mut() {
        group.old.sort_by(|left, right| compare_facts(left, right));
        group.new.sort_by(|left, right| compare_facts(left, right));
        if let ([old], [new]) = (group.old.as_slice(), group.new.as_slice())
            && old.provenance() == new.provenance()
        {
            let arguments_changed = old.argument_text() != new.argument_text();
            let control_context_changed = old.control_context() != new.control_context();
            if !arguments_changed && !control_context_changed {
                continue;
            }
            let pairing = CallPairingEvidence::new(
                CallPairingStrategy::UniqueOccurrenceWithinEnclosingRange,
                old.call_site_range(),
                new.call_site_range(),
                input.old_enclosing_range,
                input.new_enclosing_range,
                1,
                1,
            )?;
            changes.push(CallDiffChange::new(
                CallChangeKind::Modified,
                Some((*old).clone()),
                Some((*new).clone()),
                arguments_changed,
                control_context_changed,
                Some(pairing),
                navigation(input.new_path, ComparisonSide::Head, new.call_site_range()),
            )?);
            continue;
        }

        for call in &group.old {
            changes.push(CallDiffChange::new(
                CallChangeKind::Removed,
                Some((*call).clone()),
                None,
                false,
                false,
                None,
                navigation(input.old_path, ComparisonSide::Base, call.call_site_range()),
            )?);
        }
        for call in &group.new {
            changes.push(CallDiffChange::new(
                CallChangeKind::Added,
                None,
                Some((*call).clone()),
                false,
                false,
                None,
                navigation(input.new_path, ComparisonSide::Head, call.call_site_range()),
            )?);
        }
    }
    changes.sort_by(compare_changes);
    Ok(changes)
}

fn calls_in_scope<'a>(
    calls: &'a [CallFact],
    enclosing_symbol: &SymbolKey,
    enclosing_range: SourceRange,
) -> impl Iterator<Item = &'a CallFact> {
    calls.iter().filter(move |call| {
        call.enclosing_symbol() == Some(enclosing_symbol)
            && enclosing_range.contains(call.call_site_range())
    })
}

fn navigation(path: &str, side: ComparisonSide, call_range: SourceRange) -> ReviewNavigationTarget {
    ReviewNavigationTarget {
        path: path.to_string(),
        side,
        line: call_range.start_line(),
        byte_offset: Some(call_range.start_byte()),
        symbol_context: None,
    }
}

fn compare_facts(left: &CallFact, right: &CallFact) -> Ordering {
    left.call_site_range()
        .start_byte()
        .cmp(&right.call_site_range().start_byte())
        .then_with(|| {
            left.call_site_range()
                .end_byte()
                .cmp(&right.call_site_range().end_byte())
        })
        .then_with(|| left.argument_text().cmp(right.argument_text()))
        .then_with(|| compare_contexts(left.control_context(), right.control_context()))
        .then_with(|| {
            language_rank(left.provenance().language())
                .cmp(&language_rank(right.provenance().language()))
        })
        .then_with(|| left.provenance().parser().cmp(right.provenance().parser()))
}

fn language_rank(language: SyntaxLanguage) -> u8 {
    match language {
        SyntaxLanguage::Rust => 0,
        SyntaxLanguage::TypeScript => 1,
        SyntaxLanguage::Tsx => 2,
    }
}

fn compare_contexts(left: &[ControlContext], right: &[ControlContext]) -> Ordering {
    for (left, right) in left.iter().zip(right) {
        let ordering =
            context_rank(left)
                .cmp(&context_rank(right))
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

fn context_rank(context: &ControlContext) -> u8 {
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

fn compare_changes(left: &CallDiffChange, right: &CallDiffChange) -> Ordering {
    left.navigation()
        .line
        .cmp(&right.navigation().line)
        .then_with(|| {
            left.navigation()
                .byte_offset
                .cmp(&right.navigation().byte_offset)
        })
        .then_with(|| change_rank(left.kind()).cmp(&change_rank(right.kind())))
        .then_with(|| change_callee(left).cmp(change_callee(right)))
        .then_with(|| left.navigation().path.cmp(&right.navigation().path))
        .then_with(|| side_rank(left.navigation().side).cmp(&side_rank(right.navigation().side)))
        .then_with(|| match (change_fact(left), change_fact(right)) {
            (Some(left), Some(right)) => compare_facts(left, right),
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        })
}

fn change_rank(kind: CallChangeKind) -> u8 {
    match kind {
        CallChangeKind::Removed => 0,
        CallChangeKind::Modified => 1,
        CallChangeKind::Added => 2,
    }
}

fn side_rank(side: ComparisonSide) -> u8 {
    match side {
        ComparisonSide::Base => 0,
        ComparisonSide::Head => 1,
    }
}

fn change_callee(change: &CallDiffChange) -> &str {
    change_fact(change)
        .map(CallFact::callee_text)
        .unwrap_or_default()
}

fn change_fact(change: &CallDiffChange) -> Option<&CallFact> {
    change.new_fact().or_else(|| change.old())
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use okena_syntax::{SymbolKind, SyntaxLanguage, SyntaxProvenance};

    use super::*;

    fn source_range(start: u64, end: u64, line: u32) -> SourceRange {
        let line = NonZeroU32::new(line).unwrap();
        SourceRange::new(start, end, line, line).unwrap()
    }

    fn enclosing_range() -> SourceRange {
        SourceRange::new(
            0,
            10_000,
            NonZeroU32::new(1).unwrap(),
            NonZeroU32::new(1_000).unwrap(),
        )
        .unwrap()
    }

    fn key(path: &[&str], name: &str) -> SymbolKey {
        SymbolKey::new(
            path.iter().map(|segment| (*segment).to_string()).collect(),
            SymbolKind::Function,
            name,
        )
        .unwrap()
    }

    fn call(
        callee: &str,
        arguments: &str,
        start: u64,
        line: u32,
        enclosing: &SymbolKey,
        contexts: Vec<ControlContext>,
    ) -> CallFact {
        call_with_provenance(
            SyntaxProvenance::tree_sitter(SyntaxLanguage::TypeScript, "call-diff-test").unwrap(),
            callee,
            arguments,
            start,
            line,
            enclosing,
            contexts,
        )
    }

    fn call_with_provenance(
        provenance: SyntaxProvenance,
        callee: &str,
        arguments: &str,
        start: u64,
        line: u32,
        enclosing: &SymbolKey,
        contexts: Vec<ControlContext>,
    ) -> CallFact {
        let call_range = source_range(start, start.saturating_add(12), line);
        CallFact::new(
            provenance,
            callee,
            arguments,
            call_range,
            call_range,
            Some(enclosing.clone()),
            contexts,
        )
        .unwrap()
    }

    fn compare(
        enclosing: &SymbolKey,
        old_calls: &[CallFact],
        new_calls: &[CallFact],
    ) -> Vec<CallDiffChange> {
        compare_paths("src/old.ts", "src/new.ts", enclosing, old_calls, new_calls)
    }

    fn compare_paths(
        old_path: &str,
        new_path: &str,
        enclosing: &SymbolKey,
        old_calls: &[CallFact],
        new_calls: &[CallFact],
    ) -> Vec<CallDiffChange> {
        compare_calls(
            CallDiffInput::new(
                old_path,
                new_path,
                enclosing,
                enclosing_range(),
                enclosing_range(),
                old_calls,
                new_calls,
            )
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn unchanged_unique_call_is_omitted() {
        let enclosing = key(&[], "review");
        let old = vec![call("load", "(value)", 10, 2, &enclosing, Vec::new())];
        let new = vec![call("load", "(value)", 30, 4, &enclosing, Vec::new())];

        assert!(compare(&enclosing, &old, &new).is_empty());
    }

    #[test]
    fn additions_and_removals_navigate_to_their_own_side_and_path() {
        let enclosing = key(&[], "review");
        let old = vec![call("removed", "()", 20, 3, &enclosing, Vec::new())];
        let new = vec![call("added", "()", 40, 5, &enclosing, Vec::new())];
        let changes = compare_paths("old/name.ts", "new/name.ts", &enclosing, &old, &new);

        let removed = changes
            .iter()
            .find(|change| change.kind() == CallChangeKind::Removed)
            .unwrap();
        assert_eq!(removed.navigation().path, "old/name.ts");
        assert_eq!(removed.navigation().side, ComparisonSide::Base);
        assert_eq!(removed.navigation().line.get(), 3);
        assert_eq!(removed.navigation().byte_offset, Some(20));
        let added = changes
            .iter()
            .find(|change| change.kind() == CallChangeKind::Added)
            .unwrap();
        assert_eq!(added.navigation().path, "new/name.ts");
        assert_eq!(added.navigation().side, ComparisonSide::Head);
        assert_eq!(added.navigation().line.get(), 5);
        assert_eq!(added.navigation().byte_offset, Some(40));
    }

    #[test]
    fn unique_pair_reports_argument_control_and_combined_modifications() {
        let enclosing = key(&[], "review");
        for (old_arguments, new_arguments, old_context, new_context, expected) in [
            ("(old)", "(new)", vec![], vec![], (true, false)),
            (
                "(same)",
                "(same)",
                vec![],
                vec![ControlContext::Condition],
                (false, true),
            ),
            (
                "(old)",
                "(new)",
                vec![ControlContext::Loop],
                vec![ControlContext::Condition],
                (true, true),
            ),
        ] {
            let old = vec![call("load", old_arguments, 10, 2, &enclosing, old_context)];
            let new = vec![call("load", new_arguments, 30, 4, &enclosing, new_context)];
            let changes = compare(&enclosing, &old, &new);

            assert_eq!(changes.len(), 1);
            let change = &changes[0];
            assert_eq!(change.kind(), CallChangeKind::Modified);
            assert_eq!(change.arguments_changed(), expected.0);
            assert_eq!(change.control_context_changed(), expected.1);
            assert_eq!(change.navigation().side, ComparisonSide::Head);
            assert_eq!(change.navigation().path, "src/new.ts");
            assert_eq!(change.navigation().line.get(), 4);
            let evidence = change.pairing().unwrap();
            assert_eq!(
                evidence.strategy(),
                CallPairingStrategy::UniqueOccurrenceWithinEnclosingRange
            );
            assert_eq!(evidence.old_candidate_count(), 1);
            assert_eq!(evidence.new_candidate_count(), 1);
            assert_eq!(evidence.old_call_range(), old[0].call_site_range());
            assert_eq!(evidence.new_call_range(), new[0].call_site_range());
            assert_eq!(evidence.old_enclosing_range(), enclosing_range());
            assert_eq!(evidence.new_enclosing_range(), enclosing_range());
        }
    }

    #[test]
    fn repeated_candidates_on_one_or_both_sides_never_pair_by_ordinal() {
        let enclosing = key(&[], "review");
        let old = vec![
            call("load", "(first)", 10, 2, &enclosing, Vec::new()),
            call("load", "(second)", 30, 4, &enclosing, Vec::new()),
        ];
        let one_new = vec![call("load", "(new)", 50, 6, &enclosing, Vec::new())];
        let one_sided = compare(&enclosing, &old, &one_new);
        assert_eq!(
            one_sided
                .iter()
                .filter(|change| change.kind() == CallChangeKind::Removed)
                .count(),
            2
        );
        assert_eq!(
            one_sided
                .iter()
                .filter(|change| change.kind() == CallChangeKind::Added)
                .count(),
            1
        );
        assert!(one_sided.iter().all(|change| change.pairing().is_none()));

        let two_new = vec![
            call("load", "(first)", 50, 6, &enclosing, Vec::new()),
            call("load", "(second)", 70, 8, &enclosing, Vec::new()),
        ];
        let both_sides = compare(&enclosing, &old, &two_new);
        assert_eq!(both_sides.len(), 4);
        assert!(
            both_sides
                .iter()
                .all(|change| change.kind() != CallChangeKind::Modified)
        );
    }

    #[test]
    fn exact_callee_change_degrades_to_added_and_removed() {
        let enclosing = key(&[], "review");
        let old = vec![call("load", "(value)", 10, 2, &enclosing, Vec::new())];
        let new = vec![call("loadCached", "(value)", 30, 4, &enclosing, Vec::new())];
        let changes = compare(&enclosing, &old, &new);

        assert_eq!(changes.len(), 2);
        assert!(
            changes
                .iter()
                .any(|change| change.kind() == CallChangeKind::Removed)
        );
        assert!(
            changes
                .iter()
                .any(|change| change.kind() == CallChangeKind::Added)
        );
    }

    #[test]
    fn cross_language_provenance_never_pairs_as_modified() {
        let enclosing = key(&[], "review");
        let old = vec![call_with_provenance(
            SyntaxProvenance::tree_sitter(SyntaxLanguage::TypeScript, "typescript-parser").unwrap(),
            "load",
            "(old)",
            10,
            2,
            &enclosing,
            Vec::new(),
        )];
        let new = vec![call_with_provenance(
            SyntaxProvenance::tree_sitter(SyntaxLanguage::Rust, "rust-parser").unwrap(),
            "load",
            "(new)",
            30,
            4,
            &enclosing,
            Vec::new(),
        )];
        let changes = compare(&enclosing, &old, &new);

        assert_eq!(changes.len(), 2);
        assert!(
            changes
                .iter()
                .any(|change| change.kind() == CallChangeKind::Removed)
        );
        assert!(
            changes
                .iter()
                .any(|change| change.kind() == CallChangeKind::Added)
        );
        assert!(
            changes
                .iter()
                .all(|change| change.kind() != CallChangeKind::Modified)
        );
    }

    #[test]
    fn parser_version_mismatch_never_pairs_as_modified() {
        let enclosing = key(&[], "review");
        let old = vec![call_with_provenance(
            SyntaxProvenance::tree_sitter(SyntaxLanguage::TypeScript, "typescript-parser@1")
                .unwrap(),
            "load",
            "(old)",
            10,
            2,
            &enclosing,
            Vec::new(),
        )];
        let new = vec![call_with_provenance(
            SyntaxProvenance::tree_sitter(SyntaxLanguage::TypeScript, "typescript-parser@2")
                .unwrap(),
            "load",
            "(new)",
            30,
            4,
            &enclosing,
            Vec::new(),
        )];
        let changes = compare(&enclosing, &old, &new);

        assert_eq!(changes.len(), 2);
        assert!(
            changes
                .iter()
                .all(|change| change.kind() != CallChangeKind::Modified)
        );
    }

    #[test]
    fn nested_other_and_out_of_range_calls_are_excluded() {
        let enclosing = key(&[], "outer");
        let nested = key(&["outer"], "inner");
        let other = key(&[], "other");
        let old = vec![
            call("selected", "(old)", 10, 2, &enclosing, Vec::new()),
            call("nested", "()", 20, 3, &nested, Vec::new()),
            call("selected", "(other)", 30, 4, &other, Vec::new()),
            call("outside", "()", 20_000, 2_000, &enclosing, Vec::new()),
        ];
        let new = vec![
            call("selected", "(new)", 40, 5, &enclosing, Vec::new()),
            call("nested", "()", 50, 6, &nested, Vec::new()),
            call("selected", "(other)", 60, 7, &other, Vec::new()),
        ];
        let changes = compare(&enclosing, &old, &new);

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind(), CallChangeKind::Modified);
        assert_eq!(changes[0].new_fact().unwrap().callee_text(), "selected");
    }

    #[test]
    fn unicode_callee_and_ranges_are_preserved() {
        let enclosing = key(&["Nástroje"], "zkontroluj");
        let old = vec![call(
            "služba.načti",
            "(žlutý)",
            21,
            3,
            &enclosing,
            Vec::new(),
        )];
        let new = vec![call(
            "služba.načti",
            "(červený)",
            55,
            6,
            &enclosing,
            Vec::new(),
        )];
        let changes = compare(&enclosing, &old, &new);

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].new_fact().unwrap().callee_text(), "služba.načti");
        assert_eq!(changes[0].navigation().byte_offset, Some(55));
        assert_eq!(
            changes[0].pairing().unwrap().old_call_range(),
            source_range(21, 33, 3)
        );
    }

    #[test]
    fn output_order_is_stable_for_reordered_inputs() {
        let enclosing = key(&[], "review");
        let old = vec![
            call("zeta", "()", 90, 9, &enclosing, Vec::new()),
            call("alpha", "()", 10, 2, &enclosing, Vec::new()),
            call("repeat", "(one)", 50, 5, &enclosing, Vec::new()),
            call("repeat", "(two)", 70, 7, &enclosing, Vec::new()),
        ];
        let new = vec![call("beta", "()", 30, 3, &enclosing, Vec::new())];
        let mut reversed_old = old.clone();
        reversed_old.reverse();
        let mut reversed_new = new.clone();
        reversed_new.reverse();

        assert_eq!(
            compare(&enclosing, &old, &new),
            compare(&enclosing, &reversed_old, &reversed_new)
        );
    }

    #[test]
    fn provenance_tie_breaks_have_stable_serialization_for_reversed_inputs() {
        let enclosing = key(&[], "review");
        let make = |language, parser| {
            call_with_provenance(
                SyntaxProvenance::tree_sitter(language, parser).unwrap(),
                "same",
                "(value)",
                10,
                2,
                &enclosing,
                Vec::new(),
            )
        };
        let old = vec![
            make(SyntaxLanguage::TypeScript, "parser-z"),
            make(SyntaxLanguage::Rust, "parser-rust"),
        ];
        let new = vec![
            make(SyntaxLanguage::Tsx, "parser-tsx"),
            make(SyntaxLanguage::TypeScript, "parser-a"),
        ];
        let mut reversed_old = old.clone();
        reversed_old.reverse();
        let mut reversed_new = new.clone();
        reversed_new.reverse();

        let forward = serde_json::to_string(&compare(&enclosing, &old, &new)).unwrap();
        let reversed =
            serde_json::to_string(&compare(&enclosing, &reversed_old, &reversed_new)).unwrap();
        assert_eq!(forward, reversed);
    }

    #[test]
    fn empty_paths_are_rejected_even_when_there_are_no_changes() {
        let enclosing = key(&[], "review");
        let error = CallDiffInput::new(
            "",
            "src/new.ts",
            &enclosing,
            enclosing_range(),
            enclosing_range(),
            &[],
            &[],
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CallDiffError::EmptyPath(ComparisonSide::Base)
        ));
    }
}
