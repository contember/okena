//! Which changed symbol the viewport shows, and the outline as flat rows.
//! Pure; no GPUI.

use super::super::labels::symbol_glyph;
use super::super::model::{KindGlyph, SymbolEntry};
use okena_review::OutlineFact;
use std::collections::HashMap;

/// The changed symbol the symbol bar follows — spec §9. The narrowest hunk that
/// contains the top visible row wins, so the deepest symbol is named; with
/// nothing under the row the nearest symbol below it takes over.
pub(super) fn viewport_symbol(
    entries: &[SymbolEntry],
    top_row_old: Option<u32>,
    top_row_new: Option<u32>,
) -> Option<usize> {
    let mut containing: Option<(u32, usize)> = None;
    let mut following: Option<(u32, usize)> = None;
    for (index, entry) in entries.iter().enumerate() {
        for (row, hunks) in [
            (top_row_old, &entry.old_hunks),
            (top_row_new, &entry.new_hunks),
        ] {
            let Some(row) = row else {
                continue;
            };
            for &(start, end) in hunks {
                if start <= row && row <= end {
                    let width = end.saturating_sub(start);
                    if containing.is_none_or(|(best, _)| width < best) {
                        containing = Some((width, index));
                    }
                } else if start > row {
                    let distance = start.saturating_sub(row);
                    if following.is_none_or(|(best, _)| distance < best) {
                        following = Some((distance, index));
                    }
                }
            }
        }
    }
    containing.or(following).map(|(_, index)| index)
}

/// One outline entry, already flattened to a row with its nesting depth.
pub(super) struct OutlineRow {
    pub depth: usize,
    pub glyph: KindGlyph,
    pub name: String,
    /// Index into the file's `symbol_changes` when this symbol changed.
    pub change_index: Option<usize>,
}

/// `changed` maps a qualified symbol name to its change index — spec §9.
pub(super) fn outline_rows(
    outline: &[OutlineFact],
    changed: &HashMap<String, usize>,
) -> Vec<OutlineRow> {
    let mut rows = Vec::new();
    collect(outline, 0, changed, &mut rows);
    rows
}

fn collect(
    facts: &[OutlineFact],
    depth: usize,
    changed: &HashMap<String, usize>,
    rows: &mut Vec<OutlineRow>,
) {
    for fact in facts {
        let key = fact.symbol().key();
        rows.push(OutlineRow {
            depth,
            glyph: symbol_glyph(&key.kind()),
            name: key.name().to_string(),
            change_index: changed.get(&key.qualified_name()).copied(),
        });
        collect(fact.children(), depth.saturating_add(1), changed, rows);
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::fixtures;
    use super::super::super::model::{KindGlyph, SymbolEntry};
    use super::{outline_rows, viewport_symbol};
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
        let key = SymbolKey::new(
            path.iter().map(|part| (*part).to_string()).collect(),
            kind,
            name,
        )
        .expect("symbol key");
        OutlineFact::new(
            provenance,
            SymbolReference::new(ComparisonSide::Head, range(lines.0, lines.1), key),
            children,
        )
        .expect("outline fact")
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
    fn nothing_below_the_last_symbol_names_no_symbol() {
        let entries = symbols();
        assert_eq!(viewport_symbol(&entries, Some(9_000), Some(9_000)), None);
        assert_eq!(viewport_symbol(&entries, None, None), None);
        assert_eq!(viewport_symbol(&[], Some(1), Some(1)), None);
    }

    #[test]
    fn the_narrowest_hunk_wins_so_the_deepest_symbol_is_named() {
        let mut entries = symbols();
        entries.truncate(2);
        entries[0].old_hunks = vec![(10, 200)];
        entries[0].new_hunks = vec![(10, 200)];
        entries[1].old_hunks = vec![(40, 60)];
        entries[1].new_hunks = vec![(40, 60)];
        assert_eq!(viewport_symbol(&entries, Some(50), Some(50)), Some(1));
        assert_eq!(viewport_symbol(&entries, Some(30), Some(30)), Some(0));
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
        let mut changed = HashMap::new();
        changed.insert("Engine::run".to_string(), 3);

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
}
