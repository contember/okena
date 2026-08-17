//! Attention-mode row model — spec §7.
//!
//! Pure. The ordered list is `ReviewModel::attention` narrowed by the navigator
//! filters; the grouped variant re-buckets the same rows by file without
//! reordering them.

use super::super::labels::nav as words;
use super::super::model::{AttentionTarget, KindGlyph, Reason, ReasonKind, ReviewModel, Tier};
use super::super::state::{AttentionFilter, NavRowId, RoleFilter};
use super::rows::matches_filter;

/// A two-line row has room for two chips — spec §7.
const MAX_CHIPS: usize = 2;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AttentionRow {
    /// Tier separators are not navigable, so they carry no id.
    pub id: Option<NavRowId>,
    pub kind: AttentionRowKind,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AttentionRowKind {
    Tier(&'static str),
    Group(GroupRow),
    Item(ItemRow),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GroupRow {
    pub path: String,
    pub count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ItemRow {
    pub target: AttentionTarget,
    pub glyph: KindGlyph,
    pub name: String,
    /// The file the item lives in, or what a directory row counts.
    pub path: String,
    pub added: u64,
    pub deleted: u64,
    /// At most [`MAX_CHIPS`], already shortened for the column.
    pub chips: Vec<Reason>,
    /// Ranked from git facts only — structure never reached it.
    pub dimmed: bool,
    /// Indented under a file header in the grouped variant.
    pub nested: bool,
}

/// Indices into `ReviewModel::attention` that pass every navigator filter.
///
/// Mirrors `DiffViewer::review_visible_attention`; `]` `[` walk that one and
/// `↑` `↓` walk this one, so the two predicates must stay identical.
pub(crate) fn visible_attention(
    model: &ReviewModel,
    filter: &AttentionFilter,
    role_filter: &RoleFilter,
    filter_text: &str,
) -> Vec<usize> {
    let needle = filter_text.to_lowercase();
    model
        .attention
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            filter.kinds.is_empty()
                || item
                    .reasons
                    .iter()
                    .any(|reason| filter.kinds.contains(&reason.kind))
        })
        .filter(|(_, item)| filter.include_tests || !item.is_test)
        .filter(|(_, item)| match item.target.file() {
            Some(key) => model
                .file_index(key)
                .and_then(|index| model.files.get(index))
                .is_some_and(|entry| role_filter.allows(entry)),
            None => true,
        })
        .filter(|(_, item)| {
            matches_filter(&item.path, &needle) || matches_filter(&item.name, &needle)
        })
        .map(|(index, _)| index)
        .collect()
}

/// Every visible row of the Attention list, in display order.
pub(crate) fn attention_rows(
    model: &ReviewModel,
    attention_filter: &AttentionFilter,
    role_filter: &RoleFilter,
    filter_text: &str,
) -> Vec<AttentionRow> {
    let visible = visible_attention(model, attention_filter, role_filter, filter_text);
    if attention_filter.grouped_by_file {
        grouped_rows(model, &visible)
    } else {
        ordered_rows(model, &visible)
    }
}

/// The ranked list with one separator per tier that has rows — spec §7.
fn ordered_rows(model: &ReviewModel, visible: &[usize]) -> Vec<AttentionRow> {
    let mut out = Vec::new();
    let mut tier: Option<Tier> = None;
    for index in visible {
        let Some(item) = model.attention.get(*index) else {
            continue;
        };
        if tier != Some(item.tier) {
            tier = Some(item.tier);
            out.push(AttentionRow {
                id: None,
                kind: AttentionRowKind::Tier(words::tier_label(item.tier)),
            });
        }
        out.push(item_row(model, *index, false));
    }
    out
}

/// The same rows bucketed by file; a file keeps the rank of its first item.
fn grouped_rows(model: &ReviewModel, visible: &[usize]) -> Vec<AttentionRow> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    let mut loose: Vec<usize> = Vec::new();
    for index in visible {
        let Some(item) = model.attention.get(*index) else {
            continue;
        };
        // Directory rows belong to no file, so they stay in the flat order.
        if item.target.file().is_none() {
            loose.push(*index);
            continue;
        }
        match groups.iter_mut().find(|(path, _)| *path == item.path) {
            Some((_, members)) => members.push(*index),
            None => groups.push((item.path.clone(), vec![*index])),
        }
    }
    let mut out = Vec::new();
    for (path, members) in groups {
        out.push(AttentionRow {
            id: Some(NavRowId::Dir(path.clone())),
            kind: AttentionRowKind::Group(GroupRow {
                path,
                count: members.len(),
            }),
        });
        out.extend(members.iter().map(|index| item_row(model, *index, true)));
    }
    out.extend(loose.iter().map(|index| item_row(model, *index, false)));
    out
}

fn item_row(model: &ReviewModel, index: usize, nested: bool) -> AttentionRow {
    let item = &model.attention[index];
    AttentionRow {
        id: Some(NavRowId::Item(item.target.clone())),
        kind: AttentionRowKind::Item(ItemRow {
            target: item.target.clone(),
            glyph: item.glyph,
            name: item.name.clone(),
            path: item.path.clone(),
            added: item.lines_added,
            deleted: item.lines_deleted,
            chips: item_chips(&item.reasons),
            dimmed: item.dimmed,
            nested,
        }),
    }
}

/// The two chips that say the most; `body` never displaces a measurement.
fn item_chips(reasons: &[Reason]) -> Vec<Reason> {
    let mut ordered: Vec<&Reason> = reasons.iter().collect();
    ordered.sort_by_key(|reason| u8::from(reason.kind == ReasonKind::Body));
    let mut out: Vec<Reason> = Vec::with_capacity(MAX_CHIPS);
    for reason in ordered {
        let label = words::short_chip(&reason.label).to_string();
        if out.iter().any(|kept| kept.label == label) {
            continue;
        }
        out.push(Reason {
            kind: reason.kind,
            label,
        });
        if out.len() == MAX_CHIPS {
            break;
        }
    }
    out
}

// -- reason filter chips -----------------------------------------------------

/// One OR filter over a group of reason kinds — spec §7.
pub(crate) struct ChipSpec {
    pub word: &'static str,
    pub kinds: &'static [ReasonKind],
}

pub(crate) const REASON_CHIPS: [ChipSpec; 6] = [
    ChipSpec {
        word: "sig",
        kinds: &[ReasonKind::PublicSignature, ReasonKind::ExportedSignature],
    },
    ChipSpec {
        word: "removed",
        kinds: &[
            ReasonKind::PublicRemoved,
            ReasonKind::Removed,
            ReasonKind::DeletedImpl,
        ],
    },
    ChipSpec {
        word: "calls",
        kinds: &[ReasonKind::Calls],
    },
    ChipSpec {
        word: "new",
        kinds: &[ReasonKind::New, ReasonKind::NewPublic],
    },
    ChipSpec {
        word: words::NO_TESTS_MARKER,
        kinds: &[ReasonKind::NoTestChanges],
    },
    ChipSpec {
        word: "git facts",
        kinds: &[
            ReasonKind::CiConfig,
            ReasonKind::Lockfile,
            ReasonKind::Submodule,
            ReasonKind::Binary,
            ReasonKind::Moved,
            ReasonKind::LargeChurn,
        ],
    },
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChipView {
    pub word: &'static str,
    pub label: String,
    pub count: usize,
    pub active: bool,
    /// The kinds one click has to flip.
    pub kinds: &'static [ReasonKind],
}

/// The chip row, counted over the *unfiltered* list so the numbers never move
/// under the pointer. Chips nothing matches are dropped — spec §2, no zeros.
pub(crate) fn reason_chips(model: &ReviewModel, filter: &AttentionFilter) -> Vec<ChipView> {
    REASON_CHIPS
        .iter()
        .filter_map(|spec| {
            let count = model
                .attention
                .iter()
                .filter(|item| {
                    item.reasons
                        .iter()
                        .any(|reason| spec.kinds.contains(&reason.kind))
                })
                .count();
            (count > 0).then(|| ChipView {
                word: spec.word,
                label: format!("{} {count}", spec.word),
                count,
                active: spec.kinds.iter().any(|kind| filter.kinds.contains(kind)),
                kinds: spec.kinds,
            })
        })
        .collect()
}

/// The kinds one chip click has to hand to `review_toggle_reason_filter`.
pub(crate) fn chip_toggle_kinds(chip: &ChipView, filter: &AttentionFilter) -> Vec<ReasonKind> {
    chip.kinds
        .iter()
        .copied()
        .filter(|kind| filter.kinds.contains(kind) == chip.active)
        .collect()
}

/// The active chip words, for the footer sentence.
pub(crate) fn active_chip_words(chips: &[ChipView]) -> Vec<&'static str> {
    chips
        .iter()
        .filter(|chip| chip.active)
        .map(|chip| chip.word)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::super::fixtures;
    use super::super::super::model::{AttentionTarget, ReasonKind, Tier};
    use super::super::super::state::{AttentionFilter, NavRowId, RoleFilter, RolePreset};
    use super::{
        AttentionRow, AttentionRowKind, active_chip_words, attention_rows, chip_toggle_kinds,
        reason_chips, visible_attention,
    };
    use std::collections::BTreeSet;

    fn labels(rows: &[AttentionRow]) -> Vec<String> {
        rows.iter()
            .map(|row| match &row.kind {
                AttentionRowKind::Tier(label) => (*label).to_string(),
                AttentionRowKind::Group(group) => format!("[{}] {}", group.count, group.path),
                AttentionRowKind::Item(item) => item.name.clone(),
            })
            .collect()
    }

    #[test]
    fn a_separator_opens_every_tier_that_has_rows() {
        let model = fixtures::model();
        let rows = attention_rows(
            &model,
            &AttentionFilter::default(),
            &RoleFilter::everything(),
            "",
        );
        let separators: Vec<String> = rows
            .iter()
            .filter_map(|row| match &row.kind {
                AttentionRowKind::Tier(label) => Some((*label).to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(
            separators,
            ["CONTRACT", "BEHAVIOUR", "VOLUME", "GIT FACTS", "REST"]
        );

        // Every separator sits directly above a row of its own tier.
        let mut tier = None;
        for row in &rows {
            match &row.kind {
                AttentionRowKind::Tier(label) => tier = Some(*label),
                AttentionRowKind::Item(item) => {
                    let expected = model
                        .attention
                        .iter()
                        .find(|candidate| candidate.target == item.target)
                        .map(|candidate| candidate.tier)
                        .unwrap_or(Tier::Rest);
                    assert_eq!(tier, Some(super::words::tier_label(expected)));
                }
                AttentionRowKind::Group(_) => panic!("the ordered list has no groups"),
            }
        }
    }

    #[test]
    fn separators_are_never_navigable_but_every_item_is() {
        let model = fixtures::model();
        let rows = attention_rows(
            &model,
            &AttentionFilter::default(),
            &RoleFilter::everything(),
            "",
        );
        for row in &rows {
            match &row.kind {
                AttentionRowKind::Tier(_) => assert!(row.id.is_none()),
                _ => assert!(row.id.is_some()),
            }
        }
    }

    #[test]
    fn the_reason_filter_keeps_only_items_that_carry_one_of_its_kinds() {
        let model = fixtures::model();
        let filter = AttentionFilter {
            kinds: BTreeSet::from([ReasonKind::PublicRemoved]),
            ..AttentionFilter::default()
        };
        let visible = visible_attention(&model, &filter, &RoleFilter::everything(), "");
        assert!(!visible.is_empty());
        for index in visible {
            assert!(
                model.attention[index]
                    .reasons
                    .iter()
                    .any(|reason| reason.kind == ReasonKind::PublicRemoved)
            );
        }
    }

    #[test]
    fn tests_stay_out_of_the_list_until_the_toggle_asks_for_them() {
        let model = fixtures::model();
        let without = visible_attention(
            &model,
            &AttentionFilter::default(),
            &RoleFilter::everything(),
            "",
        );
        assert!(without.iter().all(|index| !model.attention[*index].is_test));

        let filter = AttentionFilter {
            include_tests: true,
            ..AttentionFilter::default()
        };
        let with = visible_attention(&model, &filter, &RoleFilter::everything(), "");
        assert!(with.len() > without.len());
        assert!(with.iter().any(|index| model.attention[*index].is_test));
    }

    #[test]
    fn the_role_filter_and_the_text_filter_narrow_the_same_list() {
        let model = fixtures::model();
        let role_filter = RoleFilter::preset(RolePreset::Supporting);
        let supporting = visible_attention(
            &model,
            &AttentionFilter {
                include_tests: true,
                ..AttentionFilter::default()
            },
            &role_filter,
            "",
        );
        assert!(!supporting.is_empty());
        assert!(supporting.iter().all(|index| {
            match model.attention[*index].target.file() {
                // A directory row belongs to no file, so no role excludes it.
                None => true,
                Some(key) => model
                    .file_index(key)
                    .and_then(|file| model.files.get(file))
                    .is_some_and(|entry| role_filter.allows(entry)),
            }
        }));

        let typed = visible_attention(
            &model,
            &AttentionFilter::default(),
            &RoleFilter::everything(),
            "engine",
        );
        assert!(!typed.is_empty());
        assert!(typed.iter().all(|index| {
            let item = &model.attention[*index];
            item.path.contains("engine") || item.name.contains("engine")
        }));
    }

    #[test]
    fn grouping_puts_a_header_over_every_file_and_keeps_the_ranked_order() {
        let model = fixtures::model();
        let filter = AttentionFilter {
            grouped_by_file: true,
            ..AttentionFilter::default()
        };
        let rows = attention_rows(&model, &filter, &RoleFilter::everything(), "");
        assert!(
            !rows
                .iter()
                .any(|row| matches!(row.kind, AttentionRowKind::Tier(_))),
            "the grouped variant drops the tier separators"
        );
        let first = labels(&rows)
            .into_iter()
            .next()
            .expect("the fixture has rows");
        assert!(first.starts_with('['), "a header opens the list: {first}");

        // Every header counts exactly the rows nested under it.
        let mut counted = 0usize;
        let mut expected = 0usize;
        for row in &rows {
            match &row.kind {
                AttentionRowKind::Group(group) => {
                    assert_eq!(counted, expected, "the previous header miscounted");
                    counted = 0;
                    expected = group.count;
                    assert!(matches!(row.id, Some(NavRowId::Dir(_))));
                }
                AttentionRowKind::Item(item) if item.nested => counted += 1,
                _ => {}
            }
        }
        assert_eq!(counted, expected);
    }

    #[test]
    fn chips_count_the_unfiltered_list_and_drop_the_empty_ones() {
        let model = fixtures::model();
        let chips = reason_chips(&model, &AttentionFilter::default());
        let words: Vec<&str> = chips.iter().map(|chip| chip.word).collect();
        assert_eq!(
            words,
            ["sig", "removed", "calls", "new", "no tests", "git facts"]
        );
        assert!(chips.iter().all(|chip| chip.count > 0));

        let sig = chips
            .iter()
            .find(|chip| chip.word == "sig")
            .expect("the fixture changes signatures");
        assert_eq!(sig.label, format!("sig {}", sig.count));
        assert!(!sig.active);

        let filter = AttentionFilter {
            kinds: BTreeSet::from([ReasonKind::PublicSignature]),
            ..AttentionFilter::default()
        };
        let chips = reason_chips(&model, &filter);
        assert_eq!(active_chip_words(&chips), ["sig"]);

        // Turning the chip off has to clear both signature kinds.
        let sig = chips
            .iter()
            .find(|chip| chip.word == "sig")
            .expect("the sig chip");
        assert_eq!(
            chip_toggle_kinds(sig, &filter),
            [ReasonKind::PublicSignature]
        );

        let off = AttentionFilter::default();
        let chips = reason_chips(&model, &off);
        let sig = chips
            .iter()
            .find(|chip| chip.word == "sig")
            .expect("the sig chip");
        assert_eq!(
            chip_toggle_kinds(sig, &off),
            [ReasonKind::PublicSignature, ReasonKind::ExportedSignature]
        );
    }

    #[test]
    fn a_row_shows_two_chips_and_body_is_the_first_to_go() {
        let model = fixtures::model();
        let rows = attention_rows(
            &model,
            &AttentionFilter::default(),
            &RoleFilter::everything(),
            "",
        );
        let item = rows
            .iter()
            .find_map(|row| match &row.kind {
                AttentionRowKind::Item(item) if item.name.ends_with("run") => Some(item),
                _ => None,
            })
            .expect("Engine::run is in the fixture");
        let chips: Vec<&str> = item.chips.iter().map(|chip| chip.label.as_str()).collect();
        assert_eq!(chips, ["sig", "2 calls \u{00B7} error branch"]);

        assert!(rows.iter().all(|row| match &row.kind {
            AttentionRowKind::Item(item) => item.chips.len() <= 2,
            _ => true,
        }));
    }

    #[test]
    fn a_directory_row_names_what_it_counts() {
        let model = fixtures::model();
        let rows = attention_rows(
            &model,
            &AttentionFilter::default(),
            &RoleFilter::everything(),
            "",
        );
        let directory = rows
            .iter()
            .find_map(|row| match &row.kind {
                AttentionRowKind::Item(item)
                    if matches!(item.target, AttentionTarget::Directory(_)) =>
                {
                    Some(item)
                }
                _ => None,
            })
            .expect("src/ changed no tests");
        assert_eq!(
            directory
                .chips
                .iter()
                .map(|chip| chip.label.as_str())
                .collect::<Vec<_>>(),
            ["no tests"]
        );
        assert!(directory.path.contains("implementation files"));
    }
}
