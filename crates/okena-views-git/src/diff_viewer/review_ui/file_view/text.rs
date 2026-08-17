//! File-view wording and arithmetic — spec §9 and §12. Pure; no GPUI, and every
//! enum reaches the screen through `labels`, never through `{:?}`.

use super::super::labels::{format_signed, language_from_path};
use super::super::model::{AttentionTarget, FileAnalysis, FileEntry, PublicApiFact, ReviewModel};

pub(super) const DOT: &str = " \u{00B7} ";
pub(super) const ARROW: &str = "\u{2192}";
pub(super) const AT_LEAST: &str = "\u{2265} ";
/// Stands in for the position when the queue target is filtered out of view.
const UNPLACED: &str = "\u{2014}";
const BINARY: &str = "binary";

/// Directory (with its trailing slash) and basename; the header dims the first.
pub(super) fn split_path(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(index) => path.split_at(index + 1),
        None => ("", path),
    }
}

/// `TypeScript · parsed`, `JavaScript · not analyzed`, `binary`.
pub(super) fn analysis_label(entry: &FileEntry) -> String {
    if entry.binary {
        return BINARY.to_string();
    }
    let path = entry
        .new_path
        .as_deref()
        .or(entry.old_path.as_deref())
        .unwrap_or_default();
    let language = entry
        .analysis
        .language()
        .or_else(|| language_from_path(path));
    let state = match &entry.analysis {
        FileAnalysis::Parsed { .. } => "parsed",
        FileAnalysis::Partial { .. } => "partly analyzed",
        FileAnalysis::Failed => "failed to parse",
        FileAnalysis::NotInStructure
        | FileAnalysis::Pending
        | FileAnalysis::Unsupported
        | FileAnalysis::Skipped => "not analyzed",
    };
    match language {
        Some(language) => format!("{language}{DOT}{state}"),
        None => state.to_string(),
    }
}

/// The one-line summary a small comparison gets instead of the Overview — §12.
pub(super) fn header_summary(model: &ReviewModel) -> Option<String> {
    if !model.small_change {
        return None;
    }
    let mut parts = vec![file_count(model.files.len())];
    let added: u64 = model.files.iter().map(|entry| entry.lines_added).sum();
    let deleted: u64 = model.files.iter().map(|entry| entry.lines_deleted).sum();
    if let Some(churn) = churn_words(added, deleted) {
        parts.push(churn);
    }
    if let Some(clause) = public_api_clause(model.facts.public_api.as_ref()) {
        parts.push(clause);
    }
    Some(parts.join(DOT))
}

/// `+40 −12`; a side that changed nothing is left out — spec §2.
pub(super) fn churn_words(added: u64, deleted: u64) -> Option<String> {
    let (plus, minus) = format_signed(added, deleted);
    match (added > 0, deleted > 0) {
        (true, true) => Some(format!("{plus} {minus}")),
        (true, false) => Some(plus),
        (false, true) => Some(minus),
        (false, false) => None,
    }
}

/// The public-API half of the small-change summary; the strongest fact wins.
fn public_api_clause(fact: Option<&PublicApiFact>) -> Option<String> {
    let fact = fact?;
    let bound = if fact.lower_bound { AT_LEAST } else { "" };
    if fact.signatures > 0 {
        return Some(format!("{bound}{} changed", signatures(fact.signatures)));
    }
    if fact.removed > 0 {
        return Some(format!("{bound}{} removed", public_symbols(fact.removed)));
    }
    if fact.added > 0 {
        return Some(format!("{bound}{} added", public_symbols(fact.added)));
    }
    None
}

fn file_count(files: usize) -> String {
    if files == 1 {
        "1 file".to_string()
    } else {
        format!("{files} files")
    }
}

fn signatures(count: u64) -> String {
    if count == 1 {
        "1 public signature".to_string()
    } else {
        format!("{count} public signatures")
    }
}

fn public_symbols(count: u64) -> String {
    if count == 1 {
        "1 public symbol".to_string()
    } else {
        format!("{count} public symbols")
    }
}

/// One-based position of the queue target in the visible Attention order.
pub(super) fn queue_position(
    visible: &[usize],
    model: &ReviewModel,
    target: Option<&AttentionTarget>,
) -> Option<(usize, usize)> {
    let index = model.attention_index(target?)?;
    let row = visible.iter().position(|candidate| *candidate == index)?;
    Some((row + 1, visible.len()))
}

/// `3 of 236`, or `— of 236` when a filter hides the target. The steps stay
/// usable either way, so the label is only missing when there is nothing to step.
pub(super) fn queue_label(
    visible: &[usize],
    model: &ReviewModel,
    target: Option<&AttentionTarget>,
) -> Option<String> {
    if visible.is_empty() {
        return None;
    }
    Some(match queue_position(visible, model, target) {
        Some((position, total)) => format!("{position} of {total}"),
        None => format!("{UNPLACED} of {}", visible.len()),
    })
}

/// The call wording the details bar prints lives in `labels::calls`; the
/// navigator's inline outline reads the same functions.
pub(super) use super::super::labels::calls::{CallLine, call_lines, call_marker};

/// `changed symbol 1 of 4`.
pub(super) fn symbol_counter(index: usize, total: usize) -> String {
    format!("changed symbol {} of {total}", index.saturating_add(1))
}

/// `base 120–168 · head 120–190` — the outermost lines the symbol's hunks
/// touch on each side; a side without hunks is left out.
pub(super) fn line_span(old: &[(u32, u32)], new: &[(u32, u32)]) -> String {
    let span = |ranges: &[(u32, u32)]| {
        let start = ranges.iter().map(|(start, _)| *start).min()?;
        let end = ranges.iter().map(|(_, end)| *end).max()?;
        Some(if start == end {
            start.to_string()
        } else {
            format!("{start}\u{2013}{end}")
        })
    };
    let mut parts = Vec::new();
    if let Some(span) = span(old) {
        parts.push(format!("base {span}"));
    }
    if let Some(span) = span(new) {
        parts.push(format!("head {span}"));
    }
    if parts.is_empty() {
        return UNPLACED.to_string();
    }
    parts.join(DOT)
}

#[cfg(test)]
mod tests {
    use super::super::super::fixtures;
    use super::super::super::model::{AttentionTarget, ReviewModel};
    use super::super::super::ranking::{ModelInputs, StructureLoad, build_review_model};
    use super::{
        analysis_label, churn_words, header_summary, line_span, queue_label, queue_position,
        split_path, symbol_counter,
    };
    use okena_git::DiffMode;

    fn mode() -> DiffMode {
        DiffMode::BranchCompare {
            base: "main".into(),
            head: "feature".into(),
        }
    }

    fn small_model() -> ReviewModel {
        let inventory = fixtures::inventory_small();
        build_review_model(ModelInputs {
            inventory: Some(&inventory),
            inventory_error: None,
            structure: None,
            structure_state: StructureLoad::Loading,
            diff_mode: &mode(),
        })
    }

    fn entry<'a>(model: &'a ReviewModel, path: &str) -> &'a super::FileEntry {
        model
            .files
            .iter()
            .find(|entry| entry.display_path == path)
            .expect("fixture file")
    }

    #[test]
    fn paths_split_at_the_last_directory_boundary() {
        assert_eq!(
            split_path("src/build/compile.ts"),
            ("src/build/", "compile.ts")
        );
        assert_eq!(split_path("Cargo.toml"), ("", "Cargo.toml"));
        assert_eq!(split_path("src/"), ("src/", ""));
    }

    #[test]
    fn the_language_line_states_how_far_analysis_got() {
        let model = fixtures::model();
        assert_eq!(
            analysis_label(entry(&model, "src/engine.rs")),
            "Rust \u{00B7} parsed"
        );
        assert_eq!(
            analysis_label(entry(&model, "src/app.js")),
            "JavaScript \u{00B7} not analyzed"
        );
        assert_eq!(
            analysis_label(entry(&model, "worker/handler.rs")),
            "Rust \u{00B7} failed to parse"
        );
        assert_eq!(
            analysis_label(entry(&model, "README.md")),
            "Markdown \u{00B7} not analyzed"
        );
        assert_eq!(analysis_label(entry(&model, "assets/logo.png")), "binary");
    }

    #[test]
    fn only_a_small_comparison_carries_a_header_summary() {
        assert_eq!(header_summary(&fixtures::model()), None);
        assert_eq!(
            header_summary(&small_model()),
            Some("3 files \u{00B7} +16 \u{2212}4".to_string())
        );
    }

    #[test]
    fn churn_words_drop_the_side_that_did_not_change() {
        assert_eq!(churn_words(0, 0), None);
        assert_eq!(churn_words(388, 0), Some("+388".to_string()));
        assert_eq!(churn_words(0, 41), Some("\u{2212}41".to_string()));
        assert_eq!(
            churn_words(1_388, 41),
            Some("+1\u{2009}388 \u{2212}41".to_string())
        );
    }

    #[test]
    fn the_queue_position_counts_visible_rows_and_starts_at_one() {
        let model = fixtures::model();
        let visible: Vec<usize> = (0..model.attention.len()).collect();
        let first = model
            .attention
            .first()
            .expect("ranked items")
            .target
            .clone();
        assert_eq!(
            queue_position(&visible, &model, Some(&first)),
            Some((1, visible.len()))
        );

        let third = model.attention.get(2).expect("ranked items").target.clone();
        let narrowed = vec![2usize, 0];
        assert_eq!(
            queue_position(&narrowed, &model, Some(&third)),
            Some((1, 2))
        );
        assert_eq!(queue_position(&visible, &model, None), None);
        assert_eq!(
            queue_position(
                &visible,
                &model,
                Some(&AttentionTarget::Directory("nowhere".into()))
            ),
            None
        );
    }

    #[test]
    fn a_filtered_out_target_still_counts_the_rows_it_could_step_through() {
        let model = fixtures::model();
        let visible: Vec<usize> = (0..model.attention.len()).collect();
        let first = model
            .attention
            .first()
            .expect("ranked items")
            .target
            .clone();
        assert_eq!(
            queue_label(&visible, &model, Some(&first)),
            Some(format!("1 of {}", visible.len()))
        );
        assert_eq!(
            queue_label(&visible, &model, None),
            Some(format!("\u{2014} of {}", visible.len()))
        );
        assert_eq!(
            queue_label(
                &visible,
                &model,
                Some(&AttentionTarget::Directory("nowhere".into()))
            ),
            Some(format!("\u{2014} of {}", visible.len()))
        );
        assert_eq!(queue_label(&[], &model, Some(&first)), None);
    }

    #[test]
    fn the_symbol_counter_is_one_based() {
        assert_eq!(symbol_counter(0, 4), "changed symbol 1 of 4");
        assert_eq!(symbol_counter(3, 4), "changed symbol 4 of 4");
    }

    #[test]
    fn line_span_covers_the_outermost_hunk_lines_per_side() {
        assert_eq!(
            line_span(&[(120, 130), (150, 168)], &[(120, 190)]),
            "base 120\u{2013}168 \u{00B7} head 120\u{2013}190"
        );
        assert_eq!(line_span(&[], &[(7, 7)]), "head 7");
        assert_eq!(line_span(&[], &[]), "\u{2014}");
    }
}
