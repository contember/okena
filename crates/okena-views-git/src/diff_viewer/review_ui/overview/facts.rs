//! The five Overview facts, as finished sentences plus the one thing each links
//! to — spec §8. Pure; the render pass turns a link into a click.

use super::super::labels::facts as words;
use super::super::model::{CoverageSummary, Facts, FileEntry, ReasonKind, ReviewModel};
use super::super::state::RoleSet;

/// One fact line: label, sentence, and the link that ends it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FactLine {
    pub label: &'static str,
    pub text: String,
    pub link: Option<FactLink>,
}

/// Where a fact's link goes. Every one of these lands somewhere — spec §2.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FactLink {
    /// The ordered list, in the navigator.
    Attention,
    /// The directory item that has no test changes next to it.
    Directory(String),
    /// Narrow the file filter to the moves that only moved.
    MechanicalMoves,
    /// The files this comparison also touched, by role.
    Also,
}

impl FactLink {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Attention => words::ATTENTION_LINK,
            Self::MechanicalMoves => words::FILTER_LINK,
            Self::Directory(_) | Self::Also => words::SHOW_LINK,
        }
    }
}

/// One line per fact that has something to say; empty facts never reach the screen.
pub(crate) fn fact_sentences(facts: &Facts, coverage: &CoverageSummary) -> Vec<FactLine> {
    let mut lines: Vec<FactLine> = Vec::with_capacity(5);

    if let Some(fact) = facts.public_api.as_ref() {
        let text = words::public_api_sentence(fact, &coverage.languages);
        if !text.is_empty() {
            // Nothing to rank when no language was analyzed, so no link either.
            let link = (!fact.no_supported_language).then_some(FactLink::Attention);
            lines.push(FactLine {
                label: words::PUBLIC_API,
                text,
                link,
            });
        }
    }

    if let Some(fact) = facts.tests.as_ref() {
        let link = fact
            .without
            .first()
            .map(|dir| FactLink::Directory(dir.path.clone()));
        lines.push(FactLine {
            label: words::TESTS,
            text: words::tests_sentence(fact),
            link,
        });
    }

    if let Some(fact) = facts.moves.as_ref() {
        let link = (fact.likely_mechanical > 0).then_some(FactLink::MechanicalMoves);
        lines.push(FactLine {
            label: words::MOVES,
            text: words::moves_sentence(fact),
            link,
        });
    }

    // The ledger needs a toggle `ReviewUiState` does not have, so the line has no link.
    if let Some(fact) = facts.commits.as_ref() {
        lines.push(FactLine {
            label: words::COMMITS,
            text: words::commits_sentence(fact),
            link: None,
        });
    }

    if let Some(fact) = facts.also.as_ref() {
        let text = words::also_sentence(fact);
        if !text.is_empty() {
            lines.push(FactLine {
                label: words::ALSO,
                text,
                link: Some(FactLink::Also),
            });
        }
    }

    lines
}

/// A file the "Also" fact counts — the same set `also_fact` sums up.
fn is_also_file(entry: &FileEntry) -> bool {
    entry.binary
        || entry.reasons.iter().any(|reason| {
            matches!(
                reason.kind,
                ReasonKind::Lockfile | ReasonKind::Submodule | ReasonKind::DeletedImpl
            )
        })
}

/// Roles of the files "Also" counts. The role filter is the narrowest filter the
/// navigator has, so the shown set is these roles, not only these files.
pub(crate) fn also_roles(model: &ReviewModel) -> RoleSet {
    model
        .files
        .iter()
        .filter(|entry| is_also_file(entry))
        .fold(RoleSet::empty(), |roles, entry| roles.with(entry.role))
}

#[cfg(test)]
mod tests {
    use super::super::super::fixtures;
    use super::super::super::model::{CoverageSummary, Facts, ReviewModel};
    use super::super::super::ranking::{ModelInputs, StructureLoad, build_review_model};
    use super::{FactLink, FactLine, also_roles, fact_sentences};
    use okena_core::review::{FileRole, ReviewInventory};
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

    fn line<'a>(lines: &'a [FactLine], label: &str) -> &'a FactLine {
        lines
            .iter()
            .find(|line| line.label == label)
            .unwrap_or_else(|| panic!("no {label} line in {lines:?}"))
    }

    #[test]
    fn the_fixture_comparison_says_every_fact_it_has() {
        let model = fixtures::model();
        let lines = fact_sentences(&model.facts, &model.coverage);
        let labels: Vec<&str> = lines.iter().map(|line| line.label).collect();
        assert_eq!(labels, ["Public API", "Tests", "Moves", "Commits", "Also"]);
        assert_eq!(line(&lines, "Public API").link, Some(FactLink::Attention));
        assert_eq!(line(&lines, "Moves").link, Some(FactLink::MechanicalMoves));
        assert_eq!(line(&lines, "Also").link, Some(FactLink::Also));
        assert!(
            matches!(line(&lines, "Tests").link, Some(FactLink::Directory(_))),
            "the tests link opens the directory it names"
        );
    }

    #[test]
    fn an_empty_fact_set_produces_no_lines() {
        assert!(fact_sentences(&Facts::default(), &CoverageSummary::default()).is_empty());
    }

    #[test]
    fn every_link_carries_the_wording_the_spec_gives_it() {
        assert_eq!(FactLink::Attention.label(), "\u{2192} Attention");
        assert_eq!(FactLink::MechanicalMoves.label(), "filter");
        assert_eq!(FactLink::Also.label(), "show");
        assert_eq!(FactLink::Directory("src".into()).label(), "show");
    }

    #[test]
    fn a_comparison_without_a_supported_language_keeps_the_fact_but_drops_the_link() {
        let inventory = fixtures::inventory_all_unsupported();
        let structure = fixtures::structure_empty();
        let model = build_review_model(ModelInputs {
            inventory: Some(&inventory),
            inventory_error: None,
            structure: Some(&structure),
            structure_state: StructureLoad::Ready,
            diff_mode: &DiffMode::BranchCompare {
                base: "main".into(),
                head: "feature".into(),
            },
        });
        let lines = fact_sentences(&model.facts, &model.coverage);
        let public_api = line(&lines, "Public API");
        assert_eq!(public_api.text, "no supported language in this comparison");
        assert_eq!(public_api.link, None);
    }

    #[test]
    fn a_small_comparison_has_no_moves_and_no_also_line() {
        let inventory = fixtures::inventory_small();
        let model = model_of(&inventory);
        let labels: Vec<&str> = fact_sentences(&model.facts, &model.coverage)
            .iter()
            .map(|line| line.label)
            .collect();
        assert!(!labels.contains(&"Moves"), "{labels:?}");
        assert!(!labels.contains(&"Also"), "{labels:?}");
        assert!(!labels.contains(&"Commits"), "the fixture has no commits");
    }

    #[test]
    fn the_also_link_narrows_to_the_roles_those_files_carry() {
        let model = fixtures::model();
        let roles = also_roles(&model);
        assert!(roles.contains(FileRole::Lockfile), "pnpm-lock.yaml");
        assert!(roles.contains(FileRole::Unclassified), "assets/logo.png");
        assert!(
            roles.contains(FileRole::Implementation),
            "src/legacy.rs was deleted"
        );
        assert!(!roles.contains(FileRole::Documentation));
    }

    #[test]
    fn a_binary_only_comparison_still_has_an_also_line() {
        let inventory = fixtures::inventory_binary_only();
        let model = model_of(&inventory);
        let lines = fact_sentences(&model.facts, &model.coverage);
        assert_eq!(line(&lines, "Also").text, "2 binary files");
        assert!(also_roles(&model).contains(FileRole::Unclassified));
    }
}
