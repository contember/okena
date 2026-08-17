//! Where the diff viewport is, which changed symbol it shows, and the outline
//! as flat rows. Pure; no GPUI element building.

use super::super::super::types::{DiffViewMode, DisplayItem, SideBySideLine};
use super::super::labels::symbol_glyph;
use super::super::model::{KindGlyph, SymbolEntry};
use okena_review::OutlineFact;
use okena_syntax::SymbolKey;
use std::collections::HashMap;

/// Hunk headers carry no line numbers, so the top row may need a look ahead.
const LOOKAHEAD_ROWS: usize = 16;

/// Index of the topmost visible row of a `uniform_list`.
///
/// The list never fills `ScrollHandle::child_bounds`, so `logical_scroll_top`
/// stays at zero; the rows are uniform, so the index is `offset / row height`.
/// A binary search does that division without a float-to-integer cast.
pub(super) fn top_item_index(scroll_y: f32, contents_height: f32, item_count: usize) -> usize {
    if item_count == 0
        || !contents_height.is_finite()
        || contents_height <= 0.0
        || !scroll_y.is_finite()
        || scroll_y <= 0.0
    {
        return 0;
    }
    let contents = f64::from(contents_height);
    // row height = contents / count, so `row * contents <= offset * count`.
    let limit = f64::from(scroll_y) * as_f64(item_count);
    let mut low = 0;
    let mut high = item_count - 1;
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if as_f64(middle) * contents <= limit {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    low
}

fn as_f64(value: usize) -> f64 {
    f64::from(u32::try_from(value).unwrap_or(u32::MAX))
}

/// Base and head line of the diff row at `top`, in the mode the pane renders.
pub(super) fn top_row_lines(
    items: &[DisplayItem],
    side_by_side_lines: &[SideBySideLine],
    mode: DiffViewMode,
    top: usize,
) -> (Option<u32>, Option<u32>) {
    let count = match mode {
        DiffViewMode::Unified => items.len(),
        DiffViewMode::SideBySide => side_by_side_lines.len(),
    };
    let last = count.min(top.saturating_add(LOOKAHEAD_ROWS));
    for row in top..last {
        let lines = match mode {
            DiffViewMode::Unified => items.get(row).map(unified_lines),
            DiffViewMode::SideBySide => side_by_side_lines.get(row).map(split_lines),
        };
        if let Some((old, new)) = lines
            && (old.is_some() || new.is_some())
        {
            return (old, new);
        }
    }
    (None, None)
}

fn unified_lines(item: &DisplayItem) -> (Option<u32>, Option<u32>) {
    match item {
        DisplayItem::Line(line) => (
            line_number(line.old_line_num),
            line_number(line.new_line_num),
        ),
        DisplayItem::Expander(expander) => (
            line_number(Some(expander.old_range.0)),
            line_number(Some(expander.new_range.0)),
        ),
    }
}

fn split_lines(line: &SideBySideLine) -> (Option<u32>, Option<u32>) {
    if let Some(expander) = line.expander.as_ref() {
        return (
            line_number(Some(expander.old_range.0)),
            line_number(Some(expander.new_range.0)),
        );
    }
    (
        line_number(line.left.as_ref().map(|side| side.line_num)),
        line_number(line.right.as_ref().map(|side| side.line_num)),
    )
}

fn line_number(value: Option<usize>) -> Option<u32> {
    value.and_then(|line| u32::try_from(line).ok())
}

/// The changed symbol the viewport is looking at — spec §9: the symbol around
/// the top row, else the next one below it, else the last one above it.
pub(super) fn viewport_symbol(
    entries: &[SymbolEntry],
    top_row_old: Option<u32>,
    top_row_new: Option<u32>,
) -> Option<usize> {
    enclosing(entries, top_row_old, top_row_new)
        .or_else(|| nearest_following(entries, top_row_old, top_row_new))
        .or_else(|| nearest_preceding(entries, top_row_old, top_row_new))
}

/// What the symbol bar names. An explicit selection holds while the viewport
/// top is inside it or just above it; past that the bar follows the view again.
pub(super) fn followed_symbol(
    entries: &[SymbolEntry],
    selected: Option<usize>,
    top_row_old: Option<u32>,
    top_row_new: Option<u32>,
) -> Option<usize> {
    let Some(selected) = selected.filter(|index| *index < entries.len()) else {
        return viewport_symbol(entries, top_row_old, top_row_new);
    };
    if top_row_old.is_none() && top_row_new.is_none() {
        return Some(selected);
    }
    let holds = entries
        .get(selected)
        .is_some_and(|entry| covers(entry, top_row_old, top_row_new))
        || nearest_following(entries, top_row_old, top_row_new) == Some(selected);
    if holds {
        return Some(selected);
    }
    viewport_symbol(entries, top_row_old, top_row_new).or(Some(selected))
}

/// The viewport row of one snapshot with the symbol's hunks on that same side.
type SideRows<'a> = (Option<u32>, &'a [(u32, u32)]);

/// One symbol's hunks paired with the viewport row of the same snapshot, so a
/// base line number is never measured against a head one.
fn sides(
    entry: &SymbolEntry,
    top_row_old: Option<u32>,
    top_row_new: Option<u32>,
) -> [SideRows<'_>; 2] {
    [
        (top_row_old, entry.old_hunks.as_slice()),
        (top_row_new, entry.new_hunks.as_slice()),
    ]
}

fn covers(entry: &SymbolEntry, top_row_old: Option<u32>, top_row_new: Option<u32>) -> bool {
    sides(entry, top_row_old, top_row_new)
        .into_iter()
        .any(|(row, hunks)| {
            row.is_some_and(|row| {
                hunks
                    .iter()
                    .any(|(start, end)| *start <= row && row <= *end)
            })
        })
}

/// Nested symbols both cover the row; the deeper qualified path is the specific
/// one, so it wins — spec §9.
fn enclosing(
    entries: &[SymbolEntry],
    top_row_old: Option<u32>,
    top_row_new: Option<u32>,
) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    for (index, entry) in entries.iter().enumerate() {
        if !covers(entry, top_row_old, top_row_new) {
            continue;
        }
        let depth = qualified_depth(entry);
        if best.is_none_or(|(deepest, _)| depth > deepest) {
            best = Some((depth, index));
        }
    }
    best.map(|(_, index)| index)
}

fn qualified_depth(entry: &SymbolEntry) -> usize {
    entry.qualified.matches("::").count()
}

fn nearest_following(
    entries: &[SymbolEntry],
    top_row_old: Option<u32>,
    top_row_new: Option<u32>,
) -> Option<usize> {
    nearest(entries, top_row_old, top_row_new, |row, hunks| {
        hunks
            .iter()
            .filter(|(start, _)| *start > row)
            .map(|(start, _)| start.saturating_sub(row))
            .min()
    })
}

fn nearest_preceding(
    entries: &[SymbolEntry],
    top_row_old: Option<u32>,
    top_row_new: Option<u32>,
) -> Option<usize> {
    nearest(entries, top_row_old, top_row_new, |row, hunks| {
        hunks
            .iter()
            .filter(|(_, end)| *end < row)
            .map(|(_, end)| row.saturating_sub(*end))
            .min()
    })
}

/// The smallest distance wins; distances are line counts within one snapshot,
/// so the two sides stay comparable. A tie keeps the earlier symbol.
fn nearest(
    entries: &[SymbolEntry],
    top_row_old: Option<u32>,
    top_row_new: Option<u32>,
    distance: impl Fn(u32, &[(u32, u32)]) -> Option<u32>,
) -> Option<usize> {
    let mut best: Option<(u32, usize)> = None;
    for (index, entry) in entries.iter().enumerate() {
        for (row, hunks) in sides(entry, top_row_old, top_row_new) {
            let Some(row) = row else {
                continue;
            };
            let Some(found) = distance(row, hunks) else {
                continue;
            };
            if best.is_none_or(|(closest, _)| found < closest) {
                best = Some((found, index));
            }
        }
    }
    best.map(|(_, index)| index)
}

/// One outline entry, already flattened to a row with its nesting depth.
pub(super) struct OutlineRow {
    pub depth: usize,
    pub glyph: KindGlyph,
    pub name: String,
    /// Index into the file's `symbol_changes` when this symbol changed.
    pub change_index: Option<usize>,
}

/// `changed` is keyed by the full [`SymbolKey`], so a type and a function of the
/// same qualified name never mark each other — spec §9.
pub(super) fn outline_rows(
    outline: &[OutlineFact],
    changed: &HashMap<SymbolKey, usize>,
) -> Vec<OutlineRow> {
    let mut rows = Vec::new();
    collect(outline, 0, changed, &mut rows);
    rows
}

fn collect(
    facts: &[OutlineFact],
    depth: usize,
    changed: &HashMap<SymbolKey, usize>,
    rows: &mut Vec<OutlineRow>,
) {
    for fact in facts {
        let key = fact.symbol().key();
        rows.push(OutlineRow {
            depth,
            glyph: symbol_glyph(&key.kind()),
            name: key.name().to_string(),
            change_index: changed.get(key).copied(),
        });
        collect(fact.children(), depth.saturating_add(1), changed, rows);
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::fixtures;
    use super::super::super::model::{KindGlyph, SymbolEntry};
    use super::{followed_symbol, outline_rows, top_item_index, top_row_lines, viewport_symbol};
    use crate::diff_viewer::types::{
        DiffViewMode, DisplayItem, DisplayLine, ExpanderRow, SideBySideLine, SideContent,
    };
    use okena_git::DiffLineType;
    use okena_review::{ComparisonSide, OutlineFact, SymbolReference};
    use okena_syntax::{SourceRange, SymbolKey, SymbolKind, SyntaxLanguage, SyntaxProvenance};
    use std::collections::HashMap;
    use std::num::NonZeroU32;

    fn symbols() -> Vec<SymbolEntry> {
        fixtures::model()
            .files
            .iter()
            .find(|entry| entry.display_path == "src/engine.rs")
            .expect("the analyzed fixture file")
            .symbols
            .clone()
    }

    fn named(entries: &[SymbolEntry], index: Option<usize>) -> Option<&str> {
        index
            .and_then(|index| entries.get(index))
            .map(|entry| entry.name.as_str())
    }

    fn position(entries: &[SymbolEntry], name: &str) -> usize {
        entries
            .iter()
            .position(|entry| entry.name == name)
            .expect("fixture symbol")
    }

    fn key(path: &[&str], kind: SymbolKind, name: &str) -> SymbolKey {
        SymbolKey::new(
            path.iter().map(|part| (*part).to_string()).collect(),
            kind,
            name,
        )
        .expect("symbol key")
    }

    fn range(start: u32, end: u32) -> SourceRange {
        SourceRange::new(
            u64::from(start) * 100,
            u64::from(end) * 100 + 99,
            NonZeroU32::new(start).expect("one-based"),
            NonZeroU32::new(end).expect("one-based"),
        )
        .expect("source range")
    }

    fn outline(
        path: &[&str],
        name: &str,
        kind: SymbolKind,
        lines: (u32, u32),
        children: Vec<OutlineFact>,
    ) -> OutlineFact {
        let provenance = SyntaxProvenance::tree_sitter(SyntaxLanguage::Rust, "tree-sitter-rust")
            .expect("provenance");
        OutlineFact::new(
            provenance,
            SymbolReference::new(
                ComparisonSide::Head,
                range(lines.0, lines.1),
                key(path, kind, name),
            ),
            children,
        )
        .expect("outline fact")
    }

    fn line(old: Option<usize>, new: Option<usize>, line_type: DiffLineType) -> DisplayItem {
        DisplayItem::Line(DisplayLine {
            line_type,
            old_line_num: old,
            new_line_num: new,
            spans: Vec::new(),
            plain_text: String::new(),
        })
    }

    fn side(line_num: usize) -> SideContent {
        SideContent {
            line_num,
            line_type: DiffLineType::Context,
            spans: Vec::new(),
            plain_text: String::new(),
            changed_ranges: Vec::new(),
        }
    }

    fn split(left: Option<usize>, right: Option<usize>, is_header: bool) -> SideBySideLine {
        SideBySideLine {
            left: left.map(side),
            right: right.map(side),
            is_header,
            header_text: String::new(),
            expander: None,
        }
    }

    #[test]
    fn the_top_index_divides_the_scroll_offset_by_one_row() {
        // 10 rows, 200 px of content, so each row is 20 px tall.
        assert_eq!(top_item_index(0.0, 200.0, 10), 0);
        assert_eq!(top_item_index(19.0, 200.0, 10), 0);
        assert_eq!(top_item_index(20.0, 200.0, 10), 1);
        assert_eq!(top_item_index(105.0, 200.0, 10), 5);
        assert_eq!(top_item_index(10_000.0, 200.0, 10), 9);
    }

    #[test]
    fn an_unmeasured_or_impossible_list_stays_at_the_first_row() {
        assert_eq!(top_item_index(50.0, 200.0, 0), 0);
        assert_eq!(top_item_index(50.0, 0.0, 10), 0);
        assert_eq!(top_item_index(-50.0, 200.0, 10), 0);
        assert_eq!(top_item_index(f32::NAN, 200.0, 10), 0);
        assert_eq!(top_item_index(50.0, f32::NAN, 10), 0);
        assert_eq!(top_item_index(50.0, 200.0, 1), 0);
    }

    #[test]
    fn unified_rows_report_both_line_numbers_and_skip_hunk_headers() {
        let items = [
            line(None, None, DiffLineType::Header),
            line(Some(18), Some(18), DiffLineType::Context),
            line(Some(19), None, DiffLineType::Removed),
            DisplayItem::Expander(ExpanderRow {
                old_range: (30, 40),
                new_range: (31, 41),
            }),
        ];
        let mode = DiffViewMode::Unified;
        assert_eq!(
            top_row_lines(&items, &[], mode, 0),
            (Some(18), Some(18)),
            "the header has no numbers, so the next row answers"
        );
        assert_eq!(top_row_lines(&items, &[], mode, 2), (Some(19), None));
        assert_eq!(top_row_lines(&items, &[], mode, 3), (Some(30), Some(31)));
        assert_eq!(top_row_lines(&items, &[], mode, 9), (None, None));
        assert_eq!(top_row_lines(&[], &[], mode, 0), (None, None));
    }

    #[test]
    fn split_rows_come_from_the_side_by_side_list_not_the_unified_items() {
        let items = [line(Some(1), Some(1), DiffLineType::Context)];
        let rows = [
            split(None, None, true),
            split(Some(18), Some(20), false),
            split(None, Some(21), false),
            SideBySideLine {
                left: None,
                right: None,
                is_header: false,
                header_text: String::new(),
                expander: Some(ExpanderRow {
                    old_range: (50, 60),
                    new_range: (52, 62),
                }),
            },
        ];
        let mode = DiffViewMode::SideBySide;
        assert_eq!(top_row_lines(&items, &rows, mode, 0), (Some(18), Some(20)));
        assert_eq!(top_row_lines(&items, &rows, mode, 2), (None, Some(21)));
        assert_eq!(top_row_lines(&items, &rows, mode, 3), (Some(50), Some(52)));
        assert_eq!(top_row_lines(&items, &[], mode, 0), (None, None));
    }

    #[test]
    fn the_symbol_under_the_top_row_wins_on_either_side() {
        let entries = symbols();
        assert_eq!(
            named(&entries, viewport_symbol(&entries, Some(22), Some(22))),
            Some("run")
        );
        assert_eq!(
            named(&entries, viewport_symbol(&entries, Some(60), None)),
            Some("legacy_run")
        );
        assert_eq!(
            named(&entries, viewport_symbol(&entries, None, Some(700))),
            Some("orchestrate")
        );
    }

    #[test]
    fn a_row_between_symbols_falls_back_to_the_nearest_one_below() {
        let entries = symbols();
        assert_eq!(
            named(&entries, viewport_symbol(&entries, Some(100), Some(100))),
            Some("dispatch")
        );
        assert_eq!(
            named(&entries, viewport_symbol(&entries, Some(1), Some(1))),
            Some("run")
        );
    }

    #[test]
    fn a_row_past_the_last_symbol_falls_back_to_the_one_above_it() {
        let entries = symbols();
        assert_eq!(
            named(
                &entries,
                viewport_symbol(&entries, Some(9_000), Some(9_000))
            ),
            Some("orchestrate"),
            "scrolling past the end never jumps back to the first symbol"
        );
        assert_eq!(viewport_symbol(&entries, None, None), None);
        assert_eq!(viewport_symbol(&[], Some(1), Some(1)), None);
    }

    #[test]
    fn the_deepest_qualified_path_wins_when_two_symbols_cover_the_row() {
        let mut entries = symbols();
        entries.truncate(2);
        entries[0].qualified = "orchestrate".into();
        entries[0].old_hunks = vec![(10, 200)];
        entries[0].new_hunks = vec![(10, 200)];
        entries[1].qualified = "Engine::run".into();
        entries[1].old_hunks = vec![(10, 200)];
        entries[1].new_hunks = vec![(10, 200)];
        assert_eq!(viewport_symbol(&entries, Some(50), Some(50)), Some(1));

        entries[1].qualified = "run".into();
        assert_eq!(
            viewport_symbol(&entries, Some(50), Some(50)),
            Some(0),
            "equal depth keeps the earlier symbol"
        );
    }

    #[test]
    fn the_selection_holds_inside_and_just_above_it_then_lets_the_view_lead() {
        let entries = symbols();
        let configure = position(&entries, "configure");
        let selected = Some(configure);

        assert_eq!(
            followed_symbol(&entries, selected, Some(75), Some(75)),
            selected,
            "the top row sits inside the selected symbol"
        );
        assert_eq!(
            followed_symbol(&entries, selected, Some(72), Some(72)),
            selected,
            "the top row is just above it, with nothing in between"
        );
        assert_eq!(
            named(
                &entries,
                followed_symbol(&entries, selected, Some(410), Some(410))
            ),
            Some("normalize"),
            "once the view moves on, the bar follows it"
        );
        assert_eq!(
            followed_symbol(&entries, selected, None, None),
            selected,
            "an unmeasured viewport keeps the selection"
        );
        assert_eq!(
            named(
                &entries,
                followed_symbol(&entries, None, Some(22), Some(22))
            ),
            Some("run"),
            "without a selection the view alone decides"
        );
    }

    #[test]
    fn the_outline_flattens_depth_first_and_marks_changed_symbols() {
        let tree = vec![
            outline(
                &[],
                "Engine",
                SymbolKind::Struct,
                (10, 90),
                vec![
                    outline(&["Engine"], "run", SymbolKind::Method, (20, 40), Vec::new()),
                    outline(
                        &["Engine"],
                        "stop",
                        SymbolKind::Method,
                        (50, 60),
                        Vec::new(),
                    ),
                ],
            ),
            outline(
                &[],
                "normalize",
                SymbolKind::Function,
                (100, 120),
                Vec::new(),
            ),
        ];
        let changed = HashMap::from([(key(&["Engine"], SymbolKind::Method, "run"), 3)]);

        let rows = outline_rows(&tree, &changed);
        let shape: Vec<(usize, &str, Option<usize>)> = rows
            .iter()
            .map(|row| (row.depth, row.name.as_str(), row.change_index))
            .collect();
        assert_eq!(
            shape,
            [
                (0, "Engine", None),
                (1, "run", Some(3)),
                (1, "stop", None),
                (0, "normalize", None),
            ]
        );
        assert_eq!(rows[0].glyph, KindGlyph::Class);
        assert_eq!(rows[1].glyph, KindGlyph::Method);
        assert_eq!(rows[3].glyph, KindGlyph::Function);
        assert!(outline_rows(&[], &changed).is_empty());
    }

    #[test]
    fn a_same_named_symbol_of_another_kind_is_not_marked_changed() {
        let tree = vec![outline(
            &[],
            "normalize",
            SymbolKind::TypeAlias,
            (10, 20),
            Vec::new(),
        )];
        let changed = HashMap::from([(key(&[], SymbolKind::Function, "normalize"), 0)]);
        assert_eq!(outline_rows(&tree, &changed)[0].change_index, None);
    }
}
