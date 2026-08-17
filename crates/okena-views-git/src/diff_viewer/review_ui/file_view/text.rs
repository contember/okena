//! File-view wording and arithmetic — spec §9 and §12. Pure; no GPUI, and every
//! enum reaches the screen through `labels`, never through `{:?}`.

use super::super::labels::{format_signed, language_from_path};
use super::super::model::{
    AttentionTarget, CallRow, FileAnalysis, FileEntry, PublicApiFact, ReviewModel,
};
use okena_review::CallChangeKind;

pub(super) const DOT: &str = " \u{00B7} ";
pub(super) const ARROW: &str = "\u{2192}";
pub(super) const AT_LEAST: &str = "\u{2265} ";
/// Stands in for the position when the queue target is filtered out of view.
const UNPLACED: &str = "\u{2014}";
const BINARY: &str = "binary";
/// A call outside every branch, when the other side of a move had one.
const TOP_LEVEL: &str = "top level";

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

/// `changed symbol 1 of 4`.
pub(super) fn symbol_counter(index: usize, total: usize) -> String {
    format!("changed symbol {} of {total}", index.saturating_add(1))
}

/// `+`, `−` or `~` — which way the call went.
pub(super) fn call_marker(change: CallChangeKind) -> &'static str {
    match change {
        CallChangeKind::Added => "+",
        CallChangeKind::Removed => "\u{2212}",
        CallChangeKind::Modified => "~",
    }
}

/// `retry(3) → (retries)` for a modified call whose arguments changed,
/// `callee(args)` otherwise — a call that only moved between branches keeps
/// its one text, and `call_context` tells the move.
pub(super) fn call_text(row: &CallRow) -> String {
    let old = row.old_args.as_deref().unwrap_or_default();
    let new = row.new_args.as_deref().unwrap_or_default();
    match row.change {
        CallChangeKind::Added => format!("{}{new}", row.callee),
        CallChangeKind::Removed => format!("{}{old}", row.callee),
        CallChangeKind::Modified if old == new => format!("{}{new}", row.callee),
        CallChangeKind::Modified => format!("{}{old} {ARROW} {new}", row.callee),
    }
}

/// `in condition` — the branch the call sits in, outermost first; a call that
/// moved reads `in loop → loop · closure`. Top level on both sides says nothing.
pub(super) fn call_context(row: &CallRow) -> Option<String> {
    let stack = |context: &[String]| {
        if context.is_empty() {
            TOP_LEVEL.to_string()
        } else {
            context.join(DOT)
        }
    };
    match row.old_context.as_deref() {
        Some(old) => Some(format!("in {} {ARROW} {}", stack(old), stack(&row.context))),
        None if row.context.is_empty() => None,
        None => Some(format!("in {}", stack(&row.context))),
    }
}

/// Widest a call reads in the details block before it is cut.
const CALL_TEXT_CHARS: usize = 96;
const ELLIPSIS: char = '\u{2026}';

/// One call as the details block lists it: one line, whatever the source did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CallLine {
    pub change: CallChangeKind,
    pub text: String,
    pub context: Option<String>,
}

/// The lines the details block shows, and how many it left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CallLines {
    pub shown: Vec<CallLine>,
    pub hidden: usize,
}

impl CallLines {
    /// `… 12 more`, or nothing when every call is listed.
    pub(super) fn hidden_note(&self) -> Option<String> {
        (self.hidden > 0).then(|| format!("{ELLIPSIS} {} more", self.hidden))
    }
}

/// Removed and modified calls first — they are what changed behaviour; then
/// added, each side in source order. At most `limit` lines.
pub(super) fn call_lines(calls: &[CallRow], limit: usize) -> CallLines {
    let mut ordered: Vec<&CallRow> = calls.iter().collect();
    ordered.sort_by_key(|row| match row.change {
        CallChangeKind::Removed => 0,
        CallChangeKind::Modified => 1,
        CallChangeKind::Added => 2,
    });
    let shown = ordered
        .iter()
        .take(limit)
        .map(|row| CallLine {
            change: row.change,
            text: one_line(&call_text(row), CALL_TEXT_CHARS),
            context: call_context(row),
        })
        .collect();
    CallLines {
        shown,
        hidden: ordered.len().saturating_sub(limit),
    }
}

/// Collapse every whitespace run to one space and cut at `max_chars`.
fn one_line(text: &str, max_chars: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= max_chars {
        return joined;
    }
    let mut out: String = joined.chars().take(max_chars.saturating_sub(1)).collect();
    out.push(ELLIPSIS);
    out
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
    use super::super::super::model::{AttentionTarget, CallRow, ReviewModel};
    use super::super::super::ranking::{ModelInputs, StructureLoad, build_review_model};
    use super::{
        analysis_label, call_context, call_lines, call_marker, call_text, churn_words,
        header_summary, line_span, one_line, queue_label, queue_position, split_path,
        symbol_counter,
    };
    use okena_git::DiffMode;
    use okena_review::CallChangeKind;

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
    fn call_rows_read_as_signed_lines_with_their_branch() {
        let row = |change, old: Option<&str>, new: Option<&str>, context: Vec<String>| CallRow {
            change,
            callee: "retry".into(),
            old_args: old.map(str::to_string),
            new_args: new.map(str::to_string),
            context,
            old_context: None,
        };

        let added = row(CallChangeKind::Added, None, Some("(value)"), Vec::new());
        assert_eq!(call_marker(added.change), "+");
        assert_eq!(call_text(&added), "retry(value)");
        assert_eq!(call_context(&added), None);

        let removed = row(
            CallChangeKind::Removed,
            Some("(input)"),
            None,
            vec!["error branch".into()],
        );
        assert_eq!(call_marker(removed.change), "\u{2212}");
        assert_eq!(call_text(&removed), "retry(input)");
        assert_eq!(call_context(&removed), Some("in error branch".to_string()));

        let modified = row(
            CallChangeKind::Modified,
            Some("(3)"),
            Some("(retries)"),
            vec!["condition".into(), "loop".into()],
        );
        assert_eq!(call_marker(modified.change), "~");
        assert_eq!(call_text(&modified), "retry(3) \u{2192} (retries)");
        assert_eq!(
            call_context(&modified),
            Some("in condition \u{00B7} loop".to_string())
        );

        // Same arguments, moved into a closure: one text, the move in the context.
        let mut moved = row(
            CallChangeKind::Modified,
            Some("(3)"),
            Some("(3)"),
            vec!["loop".into(), "closure".into()],
        );
        moved.old_context = Some(vec!["loop".into()]);
        assert_eq!(call_text(&moved), "retry(3)");
        assert_eq!(
            call_context(&moved),
            Some("in loop \u{2192} loop \u{00B7} closure".to_string())
        );
        moved.old_context = Some(Vec::new());
        assert_eq!(
            call_context(&moved),
            Some("in top level \u{2192} loop \u{00B7} closure".to_string())
        );
    }

    #[test]
    fn call_lines_read_one_line_each_and_list_what_changed_first() {
        let row = |change: CallChangeKind, callee: &str, args: &str| CallRow {
            change,
            callee: callee.to_string(),
            old_args: Some(args.to_string()),
            new_args: Some(args.to_string()),
            context: Vec::new(),
            old_context: None,
        };
        let calls = vec![
            row(CallChangeKind::Added, "log", "(a)"),
            row(
                CallChangeKind::Removed,
                "result.set",
                "(name, {\n    fields: [],\n})",
            ),
            row(CallChangeKind::Added, "warn", "(b)"),
            row(CallChangeKind::Modified, "retry", "(3)"),
        ];
        let lines = call_lines(&calls, 3);
        assert_eq!(lines.hidden, 1);
        assert_eq!(lines.hidden_note().as_deref(), Some("\u{2026} 1 more"));
        assert_eq!(
            lines
                .shown
                .iter()
                .map(|line| line.change)
                .collect::<Vec<_>>(),
            vec![
                CallChangeKind::Removed,
                CallChangeKind::Modified,
                CallChangeKind::Added
            ]
        );
        assert_eq!(lines.shown[0].text, "result.set(name, { fields: [], })");
        assert_eq!(lines.shown[2].text, "log(a)");
        assert_eq!(call_lines(&calls, 8).hidden_note(), None);
    }

    #[test]
    fn one_line_collapses_whitespace_and_cuts_with_an_ellipsis() {
        assert_eq!(one_line("a  b\n\t c", 10), "a b c");
        assert_eq!(one_line("abcdefghij", 10), "abcdefghij");
        assert_eq!(one_line("abcdefghijk", 10), "abcdefghi\u{2026}");
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
