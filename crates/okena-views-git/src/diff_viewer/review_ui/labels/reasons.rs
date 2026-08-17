//! Reason chip wording — unit R. Spec §6: every chip and every omission sentence
//! is already worded here, so no `{:?}` enum name ever reaches the screen.

use super::{control_context_word, format_lines};
use okena_review::{AnalysisStage, FileAnalysisStatus, OmittedFileReason};
use okena_syntax::ControlContext;

const DOT: &str = " \u{00B7} ";
const DASH: &str = " \u{2014} ";

pub(crate) const PUBLIC_REMOVED: &str = "public symbol removed";
pub(crate) const PUBLIC_SIGNATURE: &str = "public signature";
pub(crate) const EXPORTED_SIGNATURE: &str = "exported signature";
pub(crate) const BODY: &str = "body";
pub(crate) const REMOVED: &str = "removed";
pub(crate) const NEW_PUBLIC: &str = "new \u{00B7} exported";
pub(crate) const NEW_IMPLEMENTATION_FILE: &str = "new implementation file";
pub(crate) const DELETED_IMPLEMENTATION_FILE: &str = "deleted implementation file";
pub(crate) const NO_TEST_CHANGES: &str = "no tests changed next to it";
pub(crate) const CI_CONFIG: &str = "CI config";
pub(crate) const LOCKFILE: &str = "lockfile";
pub(crate) const SUBMODULE: &str = "submodule";
pub(crate) const BINARY: &str = "binary";
pub(crate) const LARGE_CHURN: &str = "large change";
pub(crate) const NOT_ANALYZED: &str = "not analyzed";
pub(crate) const FAILED_TO_PARSE: &str = "Failed to parse";

/// `2 calls · error branch`; the context is dropped when no call sits in one.
pub(crate) fn calls_label(count: usize, context: Option<&str>) -> String {
    let calls = if count == 1 {
        "1 call".to_string()
    } else {
        format!("{count} calls")
    };
    match context {
        Some(context) if !context.is_empty() => format!("{calls}{DOT}{context}"),
        _ => calls,
    }
}

/// `240 lines` — the size of a new function.
pub(crate) fn lines_label(lines: u32) -> String {
    if lines == 1 {
        "1 line".to_string()
    } else {
        format!("{} lines", format_lines(u64::from(lines)))
    }
}

/// `11 members` — the size of a new type.
pub(crate) fn members_label(members: u32) -> String {
    if members == 1 {
        "1 member".to_string()
    } else {
        format!("{} members", format_lines(u64::from(members)))
    }
}

/// `moved 98 %` — how much of the file survived the rename.
pub(crate) fn moved_label(similarity: u8) -> String {
    format!("moved {similarity} %")
}

/// `86 residual lines` — what the rename changed on top of the move.
pub(crate) fn residual_label(lines: u64) -> String {
    if lines == 1 {
        "1 residual line".to_string()
    } else {
        format!("{} residual lines", format_lines(lines))
    }
}

/// `nesting 6` — changed code in an already deeply nested function.
pub(crate) fn nesting_label(depth: u32) -> String {
    format!("nesting {depth}")
}

/// `8 params` — changed code in an already wide signature.
pub(crate) fn params_label(params: u32) -> String {
    format!("{params} params")
}

/// `not analyzed · JS`; the language drops out when the path does not name one.
pub(crate) fn not_analyzed_label(language: Option<&str>) -> String {
    match language {
        Some(language) => format!("{NOT_ANALYZED}{DOT}{}", short_language(language)),
        None => NOT_ANALYZED.to_string(),
    }
}

/// `26 implementation files` — what a directory row stands for.
pub(crate) fn implementation_files(count: usize) -> String {
    if count == 1 {
        "1 implementation file".to_string()
    } else {
        format!("{count} implementation files")
    }
}

/// Chip-sized language name; unknown names keep their full spelling.
pub(crate) fn short_language(language: &str) -> &str {
    match language {
        "JavaScript" => "JS",
        "TypeScript" => "TS",
        "Markdown" => "MD",
        "Python" => "Py",
        other => other,
    }
}

/// The context worth naming: the innermost kind of branch a call sits in.
pub(crate) fn most_severe_context<'a>(
    contexts: impl IntoIterator<Item = &'a ControlContext>,
) -> Option<String> {
    contexts
        .into_iter()
        .filter_map(|context| severity(context).map(|rank| (rank, context)))
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, context)| control_context_word(context))
}

/// Lower ranks first. `Other` is a parser escape hatch, never a headline.
fn severity(context: &ControlContext) -> Option<u8> {
    Some(match context {
        ControlContext::ErrorBranch => 0,
        ControlContext::Condition => 1,
        ControlContext::Loop => 2,
        ControlContext::MatchArm => 3,
        ControlContext::Callback => 4,
        ControlContext::Closure => 5,
        ControlContext::Other(_) => return None,
    })
}

/// One omission group in words — spec §10. Never the reason's Debug name.
pub(crate) fn omission_sentence(reason: OmittedFileReason, limit: Option<u64>) -> String {
    match reason {
        OmittedFileReason::UnsupportedLanguage => "Unsupported language".to_string(),
        OmittedFileReason::Binary => "Binary content".to_string(),
        OmittedFileReason::Submodule => "Submodule pointer".to_string(),
        OmittedFileReason::ModeOnly => format!("Skipped{DASH}mode-only change"),
        OmittedFileReason::WhitespaceIgnored => format!("Skipped{DASH}whitespace-only"),
        OmittedFileReason::FileLimit => format!(
            "Not analyzed{DASH}file limit{}, taken in path order",
            bracketed(limit)
        ),
        OmittedFileReason::SourceByteLimit => {
            format!("Not analyzed{DASH}file size limit{}", bracketed(limit))
        }
        OmittedFileReason::AggregateByteLimit => {
            format!("Not analyzed{DASH}total size limit{}", bracketed(limit))
        }
        OmittedFileReason::TimeLimit => {
            format!("Not analyzed{DASH}time limit{}", bracketed(limit))
        }
        OmittedFileReason::FactLimit => {
            format!("Not analyzed{DASH}fact limit{}", bracketed(limit))
        }
        OmittedFileReason::ResponseLimit => {
            format!("Not analyzed{DASH}response size limit{}", bracketed(limit))
        }
        OmittedFileReason::Cancelled => format!("Not analyzed{DASH}analysis cancelled"),
    }
}

/// A truncation the analysis hit is worth an amber row; a language it never
/// supported is not.
pub(crate) fn omission_warns(reason: OmittedFileReason) -> bool {
    reason.status() == FileAnalysisStatus::Pending
}

fn bracketed(limit: Option<u64>) -> String {
    limit
        .map(|limit| format!(" ({})", format_lines(limit)))
        .unwrap_or_default()
}

/// `parsing: unexpected token` — the right column of a failed-parse row.
pub(crate) fn failure_detail(stage: AnalysisStage, message: &str) -> String {
    format!("{}: {message}", stage_word(stage))
}

fn stage_word(stage: AnalysisStage) -> &'static str {
    match stage {
        AnalysisStage::Detection => "detection",
        AnalysisStage::Parsing => "parsing",
        AnalysisStage::Comparison => "comparison",
        AnalysisStage::Budget => "budget",
    }
}

/// `.astro 14, .js 5` — what the unsupported group actually contained.
pub(crate) fn extension_summary(counts: &[(String, u64)]) -> String {
    counts
        .iter()
        .map(|(extension, count)| format!("{extension} {count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::{
        calls_label, extension_summary, failure_detail, implementation_files, lines_label,
        members_label, most_severe_context, moved_label, not_analyzed_label, omission_sentence,
        omission_warns, residual_label,
    };
    use okena_review::{AnalysisStage, OmittedFileReason};
    use okena_syntax::ControlContext;

    #[test]
    fn counted_chips_stay_singular_at_one() {
        assert_eq!(calls_label(1, None), "1 call");
        assert_eq!(
            calls_label(2, Some("error branch")),
            "2 calls \u{00B7} error branch"
        );
        assert_eq!(calls_label(2, Some("")), "2 calls");
        assert_eq!(lines_label(1), "1 line");
        assert_eq!(lines_label(240), "240 lines");
        assert_eq!(lines_label(1_240), "1\u{2009}240 lines");
        assert_eq!(members_label(11), "11 members");
        assert_eq!(residual_label(1), "1 residual line");
        assert_eq!(residual_label(86), "86 residual lines");
        assert_eq!(implementation_files(1), "1 implementation file");
        assert_eq!(implementation_files(26), "26 implementation files");
        assert_eq!(moved_label(98), "moved 98 %");
    }

    #[test]
    fn the_named_context_is_the_most_severe_one_and_parser_escapes_never_win() {
        let contexts = [
            ControlContext::Closure,
            ControlContext::ErrorBranch,
            ControlContext::Condition,
        ];
        assert_eq!(most_severe_context(&contexts), Some("error branch".into()));

        let softer = [ControlContext::MatchArm, ControlContext::Loop];
        assert_eq!(most_severe_context(&softer), Some("loop".into()));

        let escapes = [ControlContext::Other("guard".into())];
        assert_eq!(most_severe_context(&escapes), None);
        assert_eq!(most_severe_context(&[]), None);
    }

    #[test]
    fn languages_are_shortened_for_chips_and_omitted_when_unknown() {
        assert_eq!(
            not_analyzed_label(Some("JavaScript")),
            "not analyzed \u{00B7} JS"
        );
        assert_eq!(
            not_analyzed_label(Some("Rust")),
            "not analyzed \u{00B7} Rust"
        );
        assert_eq!(not_analyzed_label(None), "not analyzed");
    }

    #[test]
    fn omission_sentences_carry_their_limit_and_never_a_debug_name() {
        assert_eq!(
            omission_sentence(OmittedFileReason::FileLimit, Some(200)),
            "Not analyzed \u{2014} file limit (200), taken in path order"
        );
        assert_eq!(
            omission_sentence(OmittedFileReason::FileLimit, None),
            "Not analyzed \u{2014} file limit, taken in path order"
        );
        assert_eq!(
            omission_sentence(OmittedFileReason::ModeOnly, None),
            "Skipped \u{2014} mode-only change"
        );
        assert_eq!(
            omission_sentence(OmittedFileReason::UnsupportedLanguage, None),
            "Unsupported language"
        );
        assert_eq!(
            omission_sentence(OmittedFileReason::SourceByteLimit, Some(1_048_576)),
            "Not analyzed \u{2014} file size limit (1\u{2009}048\u{2009}576)"
        );

        // Single-word variants spell ordinary English; only the run-together
        // names would betray a `{:?}`.
        let debug_names = [
            "UnsupportedLanguage",
            "ModeOnly",
            "WhitespaceIgnored",
            "FileLimit",
            "SourceByteLimit",
            "AggregateByteLimit",
            "TimeLimit",
            "FactLimit",
            "ResponseLimit",
        ];
        let reasons = [
            OmittedFileReason::UnsupportedLanguage,
            OmittedFileReason::Binary,
            OmittedFileReason::Submodule,
            OmittedFileReason::ModeOnly,
            OmittedFileReason::WhitespaceIgnored,
            OmittedFileReason::FileLimit,
            OmittedFileReason::SourceByteLimit,
            OmittedFileReason::AggregateByteLimit,
            OmittedFileReason::TimeLimit,
            OmittedFileReason::FactLimit,
            OmittedFileReason::ResponseLimit,
            OmittedFileReason::Cancelled,
        ];
        for reason in reasons {
            let sentence = omission_sentence(reason, Some(7));
            assert_ne!(sentence, format!("{reason:?}"), "a sentence, not a name");
            for name in debug_names {
                assert!(!sentence.contains(name), "{sentence} leaks {name}");
            }
        }
    }

    #[test]
    fn only_the_reasons_that_stopped_the_analysis_warn() {
        assert!(omission_warns(OmittedFileReason::FileLimit));
        assert!(omission_warns(OmittedFileReason::TimeLimit));
        assert!(!omission_warns(OmittedFileReason::UnsupportedLanguage));
        assert!(!omission_warns(OmittedFileReason::ModeOnly));
    }

    #[test]
    fn failure_and_extension_details_read_as_words() {
        assert_eq!(
            failure_detail(AnalysisStage::Parsing, "unexpected token"),
            "parsing: unexpected token"
        );
        assert_eq!(
            extension_summary(&[(".astro".into(), 14), (".js".into(), 5)]),
            ".astro 14, .js 5"
        );
        assert_eq!(extension_summary(&[]), "");
    }
}
