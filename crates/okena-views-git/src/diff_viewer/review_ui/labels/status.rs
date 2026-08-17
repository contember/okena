//! Analysis-status wording — unit S. Spec §10: the pill and its details popover
//! say everything in words, so `{:?}` never reaches the screen.

use super::super::model::CoverageSummary;
use super::{format_lines, short_sha};

pub(crate) const LOADING_INVENTORY: &str = "Loading inventory\u{2026}";
/// No count: the structure request is one-shot, there is no progress channel.
pub(crate) const ANALYZING_STRUCTURE: &str = "Analyzing structure\u{2026}";
pub(crate) const UNAVAILABLE: &str = "Structure unavailable \u{00B7} diff still works";
pub(crate) const DETAILS_LINK: &str = "details";
pub(crate) const POPOVER_TITLE: &str = "Structure analysis \u{00B7} this comparison";
pub(crate) const ANALYZED_ROW: &str = "Analyzed";
pub(crate) const FAILED_ROW: &str = "Structure analysis failed";
pub(crate) const FOOTER: &str = "Not analyzed files stay in the tree (dimmed), open as a plain \
     diff, and are ranked from git facts.";

const DOT: &str = " \u{00B7} ";

/// Joins the parts with ` · `, dropping the ones that have nothing to say.
fn join_dots(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(DOT)
}

/// `385 files` / `1 file`.
pub(crate) fn files_phrase(count: u64) -> String {
    if count == 1 {
        "1 file".to_string()
    } else {
        format!("{} files", format_lines(count))
    }
}

/// `TS, TSX, Rust`; empty when structure parsed nothing.
pub(crate) fn language_list(languages: &[String]) -> String {
    languages.join(", ")
}

/// `Structure ready · 385 files · TS, TSX, Rust`.
pub(crate) fn ready_sentence(files: u64, languages: &[String]) -> String {
    join_dots(&[
        "Structure ready",
        &files_phrase(files),
        &language_list(languages),
    ])
}

/// `Structure limited · 200 of 385 files` — a capped run is limited, never complete.
pub(crate) fn limited_sentence(analyzed: u64, total: u64) -> String {
    format!(
        "Structure limited{DOT}{} of {}",
        format_lines(analyzed),
        files_phrase(total)
    )
}

/// `Structure ready · 3 files failed to parse`.
pub(crate) fn failures_sentence(failed: u64) -> String {
    format!(
        "Structure ready{DOT}{} failed to parse",
        files_phrase(failed)
    )
}

/// Right column of the analyzed row: `200 files · TypeScript, TSX`.
pub(crate) fn analyzed_detail(files: u64, languages: &[String]) -> String {
    join_dots(&[&files_phrase(files), &language_list(languages)])
}

/// Right column of an omission row: `21 files · .astro 14, .js 5`.
pub(crate) fn omission_detail(count: u64, detail: &str) -> String {
    if count == 0 {
        return detail.to_string();
    }
    join_dots(&[&files_phrase(count), detail])
}

/// `Base 8f2c1a0 · head 3e91d7c · merge-base 8f2c1a0`; unresolved sides drop out.
pub(crate) fn oid_line(coverage: &CoverageSummary) -> String {
    let base = named_oid("Base", &coverage.base_oid);
    let head = named_oid("head", &coverage.head_oid);
    let merge_base = coverage
        .merge_base_oid
        .as_deref()
        .map(|oid| named_oid("merge-base", oid))
        .unwrap_or_default();
    join_dots(&[&base, &head, &merge_base])
}

fn named_oid(name: &str, oid: &str) -> String {
    if oid.is_empty() {
        String::new()
    } else {
        format!("{name} {}", short_sha(oid))
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::model::CoverageSummary;
    use super::{
        FOOTER, analyzed_detail, failures_sentence, files_phrase, limited_sentence, oid_line,
        omission_detail, ready_sentence,
    };

    fn languages() -> Vec<String> {
        vec!["TS".to_string(), "TSX".to_string(), "Rust".to_string()]
    }

    #[test]
    fn file_counts_are_singular_at_one_and_grouped_above_a_thousand() {
        assert_eq!(files_phrase(0), "0 files");
        assert_eq!(files_phrase(1), "1 file");
        assert_eq!(files_phrase(385), "385 files");
        assert_eq!(files_phrase(1_200), "1\u{2009}200 files");
    }

    #[test]
    fn pill_sentences_match_the_spec_wording() {
        assert_eq!(
            ready_sentence(385, &languages()),
            "Structure ready \u{00B7} 385 files \u{00B7} TS, TSX, Rust"
        );
        assert_eq!(
            limited_sentence(200, 385),
            "Structure limited \u{00B7} 200 of 385 files"
        );
        assert_eq!(
            failures_sentence(3),
            "Structure ready \u{00B7} 3 files failed to parse"
        );
        assert_eq!(
            failures_sentence(1),
            "Structure ready \u{00B7} 1 file failed to parse"
        );
    }

    #[test]
    fn a_missing_language_list_does_not_leave_a_dangling_separator() {
        assert_eq!(ready_sentence(12, &[]), "Structure ready \u{00B7} 12 files");
        assert_eq!(analyzed_detail(12, &[]), "12 files");
    }

    #[test]
    fn omission_details_pair_the_count_with_the_wording() {
        assert_eq!(
            omission_detail(21, ".astro 14, .js 5"),
            "21 files \u{00B7} .astro 14, .js 5"
        );
        assert_eq!(omission_detail(9, ""), "9 files");
        assert_eq!(omission_detail(0, "nothing skipped"), "nothing skipped");
    }

    /// The literal is wrapped with a line continuation; guard the joined result.
    #[test]
    fn the_footer_reads_as_one_sentence() {
        assert!(FOOTER.starts_with("Not analyzed files stay in the tree (dimmed), "));
        assert!(FOOTER.ends_with("open as a plain diff, and are ranked from git facts."));
        assert!(!FOOTER.contains("  "), "{FOOTER}");
    }

    #[test]
    fn resolved_oids_are_shortened_and_unresolved_sides_drop_out() {
        let coverage = CoverageSummary {
            base_oid: "8f2c1a0abcdef".to_string(),
            head_oid: "3e91d7cabcdef".to_string(),
            merge_base_oid: Some("8f2c1a0abcdef".to_string()),
            ..CoverageSummary::default()
        };
        assert_eq!(
            oid_line(&coverage),
            "Base 8f2c1a0 \u{00B7} head 3e91d7c \u{00B7} merge-base 8f2c1a0"
        );

        let no_merge_base = CoverageSummary {
            head_oid: "3e91d7cabcdef".to_string(),
            ..CoverageSummary::default()
        };
        assert_eq!(oid_line(&no_merge_base), "head 3e91d7c");
        assert_eq!(oid_line(&CoverageSummary::default()), "");
    }
}
