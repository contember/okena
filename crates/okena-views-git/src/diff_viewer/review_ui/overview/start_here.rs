//! "Start here" — the first rows of the ordered list, the coverage caveat, and
//! the tone each reason chip wears. Pure; spec §8.

use super::super::labels::facts as words;
use super::super::model::{AttentionItem, CoverageSummary, ReasonKind, ReviewModel, Tier};

/// How many rows the Overview shows before "all N → Attention" takes over.
pub(crate) const START_HERE_ROWS: usize = 10;

/// The first ten items. The list is tier-ordered, so rest-tier rows only appear
/// once the higher tiers run out — spec §6.
pub(crate) fn start_here(model: &ReviewModel) -> &[AttentionItem] {
    let end = model.attention.len().min(START_HERE_ROWS);
    &model.attention[..end]
}

/// How far structure reached, in words — only when it did not reach everything.
pub(crate) fn caveat(coverage: &CoverageSummary) -> Option<String> {
    if !coverage.partial {
        return None;
    }
    if coverage.impl_total == 0 {
        // Without implementation files the sentence already counts every file, so
        // "(first N in path order)" would only repeat the number next to it.
        return Some(words::caveat_sentence(
            coverage.analyzed_files,
            coverage.total_files,
            false,
            None,
        ));
    }
    let reached = u64::try_from(coverage.impl_analyzed).unwrap_or(u64::MAX);
    let total = u64::try_from(coverage.impl_total).unwrap_or(u64::MAX);
    // The bias clause earns its place only when it names a different number.
    let path_order = coverage
        .path_order_bias
        .then_some(coverage.analyzed_files)
        .filter(|first| *first != reached);
    Some(words::caveat_sentence(reached, total, true, path_order))
}

/// How loud a reason chip is. Colours come from the theme, not from here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChipTone {
    /// Something left the public surface.
    Contract,
    /// The code behind the surface moved.
    Behaviour,
    /// New surface.
    Addition,
    /// A git fact worth a second look.
    Caution,
    /// Context, not a finding.
    Muted,
}

pub(crate) fn chip_tone(kind: ReasonKind) -> ChipTone {
    match kind {
        ReasonKind::PublicRemoved | ReasonKind::Removed | ReasonKind::DeletedImpl => {
            ChipTone::Contract
        }
        ReasonKind::PublicSignature
        | ReasonKind::ExportedSignature
        | ReasonKind::Body
        | ReasonKind::Calls => ChipTone::Behaviour,
        ReasonKind::New | ReasonKind::NewPublic => ChipTone::Addition,
        ReasonKind::NoTestChanges
        | ReasonKind::CiConfig
        | ReasonKind::Lockfile
        | ReasonKind::Submodule
        | ReasonKind::Binary
        | ReasonKind::Complex
        | ReasonKind::LargeChurn => ChipTone::Caution,
        ReasonKind::Moved | ReasonKind::NotAnalyzed => ChipTone::Muted,
    }
}

/// Tier of the loudest reason on a row, for the row's own glyph colour.
pub(crate) fn row_tone(item: &AttentionItem) -> ChipTone {
    match item.tier {
        Tier::Contract => ChipTone::Contract,
        Tier::Behaviour => ChipTone::Behaviour,
        Tier::Volume | Tier::GitFacts => ChipTone::Caution,
        Tier::Rest => ChipTone::Muted,
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::fixtures;
    use super::super::super::model::{CoverageSummary, ReasonKind, ReviewModel, Tier};
    use super::super::super::ranking::{ModelInputs, StructureLoad, build_review_model};
    use super::{ChipTone, START_HERE_ROWS, caveat, chip_tone, row_tone, start_here};
    use okena_core::review::ReviewInventory;
    use okena_git::DiffMode;

    fn model_of(inventory: &ReviewInventory) -> ReviewModel {
        build_review_model(ModelInputs {
            inventory: Some(inventory),
            inventory_error: None,
            structure: None,
            structure_state: StructureLoad::NotStarted,
            diff_mode: &DiffMode::BranchCompare {
                base: "main".into(),
                head: "feature".into(),
            },
        })
    }

    #[test]
    fn start_here_takes_the_top_of_the_ordered_list() {
        let model = fixtures::model();
        let rows = start_here(&model);
        assert_eq!(rows.len(), START_HERE_ROWS.min(model.attention.len()));
        assert_eq!(rows.first(), model.attention.first());
        assert!(
            rows.windows(2).all(|pair| pair[0].tier <= pair[1].tier),
            "the rows keep the ranking order"
        );
    }

    #[test]
    fn rest_tier_rows_only_appear_when_the_higher_tiers_run_out() {
        let model = fixtures::model();
        let higher = model
            .attention
            .iter()
            .filter(|item| item.tier < Tier::Rest)
            .count();
        let shown_rest = start_here(&model)
            .iter()
            .filter(|item| item.tier == Tier::Rest)
            .count();
        if higher >= START_HERE_ROWS {
            assert_eq!(shown_rest, 0);
        } else {
            assert_eq!(
                shown_rest,
                START_HERE_ROWS
                    .saturating_sub(higher)
                    .min(model.attention.len().saturating_sub(higher))
            );
        }
    }

    #[test]
    fn a_short_list_is_shown_whole() {
        let inventory = fixtures::inventory_small();
        let model = model_of(&inventory);
        assert!(model.attention.len() < START_HERE_ROWS);
        assert_eq!(start_here(&model).len(), model.attention.len());
    }

    #[test]
    fn complete_coverage_has_no_caveat() {
        assert_eq!(caveat(&CoverageSummary::default()), None);
    }

    #[test]
    fn a_partial_run_names_the_reach_and_only_then_the_bias() {
        let coverage = CoverageSummary {
            analyzed_files: 200,
            total_files: 385,
            impl_analyzed: 63,
            impl_total: 97,
            path_order_bias: true,
            partial: true,
            ..CoverageSummary::default()
        };
        assert_eq!(
            caveat(&coverage).as_deref(),
            Some(
                "structure reached 63 of 97 implementation files (first 200 in path order) \
                 \u{2014} the rest ranked from git facts"
            )
        );

        let unbiased = CoverageSummary {
            path_order_bias: false,
            ..coverage
        };
        assert_eq!(
            caveat(&unbiased).as_deref(),
            Some(
                "structure reached 63 of 97 implementation files \u{2014} the rest ranked \
                 from git facts"
            )
        );
    }

    #[test]
    fn a_comparison_without_implementation_files_counts_files_instead() {
        let coverage = CoverageSummary {
            analyzed_files: 0,
            total_files: 2,
            partial: true,
            ..CoverageSummary::default()
        };
        assert_eq!(
            caveat(&coverage).as_deref(),
            Some("structure reached 0 of 2 files \u{2014} the rest ranked from git facts")
        );
    }

    #[test]
    fn the_bias_clause_is_dropped_when_it_would_only_repeat_a_number() {
        let no_implementation = CoverageSummary {
            analyzed_files: 1,
            total_files: 3,
            impl_analyzed: 0,
            impl_total: 0,
            path_order_bias: true,
            partial: true,
            ..CoverageSummary::default()
        };
        let sentence = caveat(&no_implementation).expect("a partial run has a caveat");
        assert_eq!(
            sentence,
            "structure reached 1 of 3 files \u{2014} the rest ranked from git facts"
        );
        assert!(!sentence.contains("path order"), "{sentence}");

        let same_number = CoverageSummary {
            analyzed_files: 63,
            total_files: 200,
            impl_analyzed: 63,
            impl_total: 97,
            path_order_bias: true,
            partial: true,
            ..CoverageSummary::default()
        };
        let sentence = caveat(&same_number).expect("a partial run has a caveat");
        assert_eq!(
            sentence,
            "structure reached 63 of 97 implementation files \u{2014} the rest ranked from \
             git facts"
        );
        assert!(!sentence.contains("path order"), "{sentence}");
    }

    #[test]
    fn a_rows_tone_follows_the_tier_that_placed_it() {
        let model = fixtures::model();
        for item in &model.attention {
            let expected = match item.tier {
                Tier::Contract => ChipTone::Contract,
                Tier::Behaviour => ChipTone::Behaviour,
                Tier::Volume | Tier::GitFacts => ChipTone::Caution,
                Tier::Rest => ChipTone::Muted,
            };
            assert_eq!(row_tone(item), expected, "{} | {:?}", item.name, item.tier);
        }
        let contract = model
            .attention
            .first()
            .expect("the fixture ranks something first");
        assert_eq!(row_tone(contract), ChipTone::Contract);
    }

    #[test]
    fn chip_tones_separate_the_contract_from_the_context() {
        assert_eq!(chip_tone(ReasonKind::PublicRemoved), ChipTone::Contract);
        assert_eq!(chip_tone(ReasonKind::Body), ChipTone::Behaviour);
        assert_eq!(chip_tone(ReasonKind::NewPublic), ChipTone::Addition);
        assert_eq!(chip_tone(ReasonKind::Lockfile), ChipTone::Caution);
        assert_eq!(chip_tone(ReasonKind::NotAnalyzed), ChipTone::Muted);
    }
}
