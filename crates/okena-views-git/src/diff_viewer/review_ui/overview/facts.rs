//! The five Overview facts, as finished sentences plus the one thing each links
//! to — spec §8. Pure; the render pass turns a link into a click.

use super::super::labels::facts as words;
use super::super::model::{CommitRow, Facts, FileEntry, ReasonKind, ReviewModel};
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
    /// The commit ledger, inline under the facts.
    CommitLedger,
    /// The files this comparison also touched, by role.
    Also,
}

impl FactLink {
    /// Link text. Only the ledger has two states, so only it reads `ledger_open`.
    pub(crate) fn label(&self, ledger_open: bool) -> &'static str {
        match self {
            Self::Attention => words::ATTENTION_LINK,
            Self::MechanicalMoves => words::FILTER_LINK,
            Self::CommitLedger => words::ledger_link(ledger_open),
            Self::Directory(_) | Self::Also => words::SHOW_LINK,
        }
    }
}

/// One line per fact that has something to say; empty facts never reach the screen.
/// Every sentence reads off the fact itself — the coverage caveat lives in §8's
/// second block, not here.
pub(crate) fn fact_sentences(facts: &Facts) -> Vec<FactLine> {
    let mut lines: Vec<FactLine> = Vec::with_capacity(5);

    if let Some(fact) = facts.public_api.as_ref() {
        let text = words::public_api_sentence(fact);
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

    if let Some(fact) = facts.commits.as_ref() {
        lines.push(FactLine {
            label: words::COMMITS,
            text: words::commits_sentence(fact),
            link: Some(FactLink::CommitLedger),
        });
    }

    if let Some(fact) = facts.also.as_ref() {
        let text = words::also_sentence(fact);
        if !text.is_empty() {
            // Deleted implementation files are named but not linked: their role is
            // Implementation, and filtering to it would show the whole change.
            let linkable = fact.lockfiles > 0 || fact.submodules > 0 || fact.binaries > 0;
            lines.push(FactLine {
                label: words::ALSO,
                text,
                link: linkable.then_some(FactLink::Also),
            });
        }
    }

    lines
}

/// The ledger, oldest commit first — the order the branch was written in.
pub(crate) fn ledger_rows(commits: &[CommitRow]) -> Vec<&CommitRow> {
    let mut rows: Vec<&CommitRow> = commits.iter().collect();
    // Stable, so commits that share a timestamp keep their inventory order.
    rows.sort_by_key(|commit| commit.timestamp);
    rows
}

/// A file the "Also" link can narrow to. Deleted implementation files are left
/// out on purpose — their role is Implementation, so filtering to it would widen
/// the view to the whole change instead of narrowing it.
fn is_also_file(entry: &FileEntry) -> bool {
    entry.binary
        || entry
            .reasons
            .iter()
            .any(|reason| matches!(reason.kind, ReasonKind::Lockfile | ReasonKind::Submodule))
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
    use super::super::super::model::{AlsoFact, Facts, ReasonKind, ReviewModel};
    use super::super::super::ranking::{ModelInputs, StructureLoad, build_review_model};
    use super::{FactLine, FactLink, also_roles, fact_sentences, is_also_file, ledger_rows};
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
        let lines = fact_sentences(&model.facts);
        let labels: Vec<&str> = lines.iter().map(|line| line.label).collect();
        assert_eq!(labels, ["Public API", "Tests", "Moves", "Commits", "Also"]);
        assert_eq!(line(&lines, "Public API").link, Some(FactLink::Attention));
        assert_eq!(line(&lines, "Moves").link, Some(FactLink::MechanicalMoves));
        assert_eq!(line(&lines, "Also").link, Some(FactLink::Also));
        assert_eq!(line(&lines, "Commits").link, Some(FactLink::CommitLedger));
        assert!(
            matches!(line(&lines, "Tests").link, Some(FactLink::Directory(_))),
            "the tests link opens the directory it names"
        );
    }

    #[test]
    fn an_empty_fact_set_produces_no_lines() {
        assert!(fact_sentences(&Facts::default()).is_empty());
    }

    #[test]
    fn every_link_carries_the_wording_the_spec_gives_it() {
        assert_eq!(FactLink::Attention.label(false), "\u{2192} Attention");
        assert_eq!(FactLink::MechanicalMoves.label(false), "filter");
        assert_eq!(FactLink::Also.label(false), "show");
        assert_eq!(FactLink::Directory("src".into()).label(false), "show");
    }

    #[test]
    fn only_the_ledger_link_changes_with_the_open_state() {
        assert_eq!(FactLink::CommitLedger.label(false), "show ledger");
        assert_eq!(FactLink::CommitLedger.label(true), "hide ledger");
        assert_eq!(FactLink::Also.label(true), "show");
    }

    #[test]
    fn the_ledger_runs_oldest_first() {
        let model = fixtures::model();
        let rows = ledger_rows(&model.commits);
        assert_eq!(rows.len(), model.commits.len());
        assert!(
            rows.windows(2)
                .all(|pair| pair[0].timestamp <= pair[1].timestamp),
            "the ledger reads in the order the branch was written"
        );
        assert!(
            rows.iter().any(|commit| commit.is_merge),
            "the fixture has a merge commit to mark"
        );
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
        let lines = fact_sentences(&model.facts);
        let public_api = line(&lines, "Public API");
        assert_eq!(public_api.text, "no supported language in this comparison");
        assert_eq!(public_api.link, None);
    }

    #[test]
    fn a_small_comparison_has_no_moves_and_no_also_line() {
        let inventory = fixtures::inventory_small();
        let model = model_of(&inventory);
        let labels: Vec<&str> = fact_sentences(&model.facts)
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
            !roles.contains(FileRole::Implementation),
            "a deleted implementation file must not widen the link to the whole change"
        );
        assert!(!roles.contains(FileRole::Documentation));
    }

    /// Drift guard: the link predicate must keep counting the same files the
    /// ranking's `also_fact` counts, minus the deleted implementation files.
    #[test]
    fn the_also_predicate_tracks_the_fact_it_links_from() {
        let model = fixtures::model();
        let fact = model
            .facts
            .also
            .as_ref()
            .expect("the fixture has an Also fact");
        let counted = model
            .files
            .iter()
            .filter(|entry| is_also_file(entry))
            .count();
        assert_eq!(
            counted,
            fact.lockfiles + fact.submodules + fact.binaries,
            "one file per lockfile, submodule and binary the fact counts"
        );
        assert!(
            fact.deleted_impl > 0,
            "the fixture deletes an implementation file, so the split is exercised"
        );
        assert_eq!(
            counted + fact.deleted_impl,
            model
                .files
                .iter()
                .filter(|entry| {
                    is_also_file(entry)
                        || entry
                            .reasons
                            .iter()
                            .any(|reason| reason.kind == ReasonKind::DeletedImpl)
                })
                .count(),
            "the sentence still names the deleted files the link leaves out"
        );
    }

    #[test]
    fn a_deleted_implementation_file_alone_leaves_the_also_line_unlinked() {
        let facts = Facts {
            also: Some(AlsoFact {
                lockfiles: 0,
                submodules: 0,
                binaries: 0,
                deleted_impl: 2,
            }),
            ..Facts::default()
        };
        let lines = fact_sentences(&facts);
        let also = line(&lines, "Also");
        assert_eq!(also.text, "2 deleted implementation files");
        assert_eq!(also.link, None, "nothing to narrow to");
    }

    #[test]
    fn the_tests_link_opens_the_directory_the_sentence_names() {
        let model = fixtures::model();
        let lines = fact_sentences(&model.facts);
        let tests = line(&lines, "Tests");
        let Some(FactLink::Directory(path)) = tests.link.as_ref() else {
            panic!("the tests fact links to a directory: {tests:?}");
        };
        assert!(
            tests.text.contains(path.as_str()),
            "the link goes where the sentence points: {} vs {path}",
            tests.text
        );
    }

    #[test]
    fn a_binary_only_comparison_still_has_an_also_line() {
        let inventory = fixtures::inventory_binary_only();
        let model = model_of(&inventory);
        let lines = fact_sentences(&model.facts);
        assert_eq!(line(&lines, "Also").text, "2 binary files");
        assert!(also_roles(&model).contains(FileRole::Unclassified));
    }
}
