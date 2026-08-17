//! Overview fact sentences — unit O. Spec §8: every number the Overview shows is
//! worded here, so the render pass only paints finished strings.

use super::super::model::{AlsoFact, CommitsFact, MovesFact, PublicApiFact, TestsFact};
use super::super::state::MECHANICAL_RESIDUAL_LINES;
use super::format_lines;
use super::reasons::short_language;
use super::status::files_phrase;

const DOT: &str = " \u{00B7} ";
const DASH: &str = " \u{2014} ";
/// Counts are lower bounds while coverage is partial — spec §2.
const AT_LEAST: &str = "\u{2265} ";

pub(crate) const GLANCE_HEADER: &str = "CHANGE AT A GLANCE";
pub(crate) const GLANCE_HINT: &str = "changed lines = added + deleted";
pub(crate) const START_HERE_HEADER: &str = "START HERE";
pub(crate) const START_HERE_HINT: &str =
    "one ordered list \u{00B7} every row names its reasons";
pub(crate) const TIERS_FOOTER: &str = "Tiers: contract \u{2192} behaviour \u{2192} volume \
     \u{2192} git facts \u{2192} everything else. Same order in the navigator's Attention \
     mode; ] steps through it from any file.";
/// Shown instead of the headline when the comparison changed nothing.
pub(crate) const NOTHING_CHANGED: &str = "No files changed";

pub(crate) const PUBLIC_API: &str = "Public API";
pub(crate) const TESTS: &str = "Tests";
pub(crate) const MOVES: &str = "Moves";
pub(crate) const COMMITS: &str = "Commits";
pub(crate) const ALSO: &str = "Also";

pub(crate) const ATTENTION_LINK: &str = "\u{2192} Attention";
pub(crate) const SHOW_LINK: &str = "show";
pub(crate) const FILTER_LINK: &str = "filter";
const SHOW_LEDGER_LINK: &str = "show ledger";
const HIDE_LEDGER_LINK: &str = "hide ledger";
/// Marks a commit with more than one parent in the ledger.
pub(crate) const MERGE_BADGE: &str = "merge";
pub(crate) const NO_SUPPORTED_LANGUAGE: &str = "no supported language in this comparison";
/// A directory row with no path of its own — the comparison touched the root.
const REPOSITORY_ROOT: &str = "repository root";

/// `12 files` from a `usize` count, without a lossy cast.
fn count_files(files: usize) -> String {
    files_phrase(u64::try_from(files).unwrap_or(u64::MAX))
}

/// A count grouped in threes, without a lossy cast.
fn format_count(count: usize) -> String {
    format_lines(u64::try_from(count).unwrap_or(u64::MAX))
}

/// `1 line` / `6 118 lines`.
pub(crate) fn lines_phrase(lines: u64) -> String {
    if lines == 1 {
        "1 line".to_string()
    } else {
        format!("{} lines", format_lines(lines))
    }
}

/// `Implementation 15 692 lines`; the sign shows when the role lost more lines
/// than it gained. The number itself stays the changed-line total.
pub(crate) fn headline_lines(role: &str, lines: u64, deletion_heavy: bool) -> String {
    let sign = if deletion_heavy { "\u{2212}" } else { "" };
    let unit = if lines == 1 { "line" } else { "lines" };
    format!("{role} {sign}{} {unit}", format_lines(lines))
}

/// `Implementation 12 files` — the fallback when nothing has a line count.
pub(crate) fn headline_files(role: &str, files: usize) -> String {
    format!("{role} {}", count_files(files))
}

/// `45 % of 34 640 · 97 files`.
pub(crate) fn headline_share_of_lines(percent: f32, total_lines: u64, files: usize) -> String {
    format!(
        "{} of {}{DOT}{}",
        rounded_percent(percent),
        format_lines(total_lines),
        count_files(files)
    )
}

/// `40 % of 30 files`.
pub(crate) fn headline_share_of_files(percent: f32, total_files: usize) -> String {
    format!("{} of {}", rounded_percent(percent), count_files(total_files))
}

/// Headline share, without the decimal the legend carries.
fn rounded_percent(percent: f32) -> String {
    format!("{percent:.0} %")
}

/// `45.3 %` — the legend keeps one decimal so small roles stay visible.
pub(crate) fn percent_label(percent: f32) -> String {
    format!("{percent:.1} %")
}

/// Legend count column: `97 files`.
pub(crate) fn legend_files(files: usize) -> String {
    count_files(files)
}

/// `≥ 3 removed · ≥ 12 signatures changed · ≥ 34 added — analyzed subset, TS/TSX`.
/// Empty when the fact carries nothing worth a line.
pub(crate) fn public_api_sentence(fact: &PublicApiFact, languages: &[String]) -> String {
    if fact.no_supported_language {
        return NO_SUPPORTED_LANGUAGE.to_string();
    }
    let at_least = if fact.lower_bound { AT_LEAST } else { "" };
    let mut parts: Vec<String> = Vec::with_capacity(3);
    if fact.removed > 0 {
        parts.push(format!("{at_least}{} removed", format_lines(fact.removed)));
    }
    if fact.signatures > 0 {
        let unit = if fact.signatures == 1 {
            "signature"
        } else {
            "signatures"
        };
        parts.push(format!(
            "{at_least}{} {unit} changed",
            format_lines(fact.signatures)
        ));
    }
    if fact.added > 0 {
        parts.push(format!("{at_least}{} added", format_lines(fact.added)));
    }
    if parts.is_empty() {
        return String::new();
    }
    let counts = parts.join(DOT);
    if !fact.lower_bound {
        return counts;
    }
    let subset = language_slashes(languages);
    if subset.is_empty() {
        format!("{counts}{DASH}analyzed subset")
    } else {
        format!("{counts}{DASH}analyzed subset, {subset}")
    }
}

/// `TS/TSX` — chip-sized language names, the way the analyzed subset is named.
fn language_slashes(languages: &[String]) -> String {
    languages
        .iter()
        .map(|language| short_language(language))
        .collect::<Vec<_>>()
        .join("/")
}

/// `Test files changed next to 4 of 6 implementation directories · none next to
/// packages/workers/src (26 files, 6 118 lines)`.
pub(crate) fn tests_sentence(fact: &TestsFact) -> String {
    let unit = if fact.impl_dirs == 1 {
        "directory"
    } else {
        "directories"
    };
    let head = format!(
        "Test files changed next to {} of {} implementation {unit}",
        fact.with_tests, fact.impl_dirs
    );
    let Some(first) = fact.without.first() else {
        return head;
    };
    let mut sentence = format!(
        "{head}{DOT}none next to {} ({}, {})",
        directory_name(&first.path),
        count_files(first.files),
        lines_phrase(first.lines)
    );
    let rest = fact.without.len().saturating_sub(1);
    if rest > 0 {
        sentence.push_str(&format!(" and {rest} more"));
    }
    sentence
}

fn directory_name(path: &str) -> &str {
    if path.is_empty() { REPOSITORY_ROOT } else { path }
}

/// `21 high-similarity moves · 17 likely mechanical (≤ 20 residual lines) ·
/// 4 with edits, ranked below`.
pub(crate) fn moves_sentence(fact: &MovesFact) -> String {
    let unit = if fact.total == 1 { "move" } else { "moves" };
    let mut parts = vec![format!("{} high-similarity {unit}", fact.total)];
    if fact.likely_mechanical > 0 {
        parts.push(format!(
            "{} likely mechanical (\u{2264} {MECHANICAL_RESIDUAL_LINES} residual lines)",
            fact.likely_mechanical
        ));
    }
    if fact.with_edits > 0 {
        parts.push(format!("{} with edits, ranked below", fact.with_edits));
    }
    parts.join(DOT)
}

/// `14 · 1 merge · Ada, Bob · 6 days · 305b0f0 … 9a7be3f`. The label supplies the noun.
pub(crate) fn commits_sentence(fact: &CommitsFact) -> String {
    let mut parts = vec![format_count(fact.count)];
    if fact.merges > 0 {
        let unit = if fact.merges == 1 { "merge" } else { "merges" };
        parts.push(format!("{} {unit}", fact.merges));
    }
    let authors = authors_phrase(&fact.authors);
    if !authors.is_empty() {
        parts.push(authors);
    }
    let span = span_phrase(fact.span_secs);
    if !span.is_empty() {
        parts.push(span);
    }
    let range = sha_range(&fact.first_sha, &fact.last_sha);
    if !range.is_empty() {
        parts.push(range);
    }
    parts.join(DOT)
}

/// Three names at most; the rest become `+2`.
fn authors_phrase(authors: &[String]) -> String {
    const SHOWN: usize = 3;
    if authors.is_empty() {
        return String::new();
    }
    let named = authors
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let rest = authors.len().saturating_sub(SHOWN);
    if rest == 0 {
        named
    } else {
        format!("{named} +{rest}")
    }
}

/// How long the branch took, in the coarsest unit that still says something.
fn span_phrase(seconds: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    if seconds <= 0 {
        return String::new();
    }
    if seconds < MINUTE {
        return "under a minute".to_string();
    }
    let (value, singular, plural) = if seconds < HOUR {
        (seconds / MINUTE, "minute", "minutes")
    } else if seconds < DAY {
        (seconds / HOUR, "hour", "hours")
    } else {
        (seconds / DAY, "day", "days")
    };
    let unit = if value == 1 { singular } else { plural };
    format!("{value} {unit}")
}

/// `305b0f0 … 9a7be3f`; a single commit shows one sha.
fn sha_range(first: &str, last: &str) -> String {
    if first.is_empty() && last.is_empty() {
        return String::new();
    }
    if first == last || last.is_empty() {
        return first.to_string();
    }
    if first.is_empty() {
        return last.to_string();
    }
    format!("{first} \u{2026} {last}")
}

/// `2 lockfiles · 1 submodule pointer · 3 binary files · 1 deleted implementation file`.
pub(crate) fn also_sentence(fact: &AlsoFact) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(4);
    if fact.lockfiles > 0 {
        parts.push(counted(fact.lockfiles, "lockfile", "lockfiles"));
    }
    if fact.submodules > 0 {
        parts.push(counted(
            fact.submodules,
            "submodule pointer",
            "submodule pointers",
        ));
    }
    if fact.binaries > 0 {
        parts.push(counted(fact.binaries, "binary file", "binary files"));
    }
    if fact.deleted_impl > 0 {
        parts.push(counted(
            fact.deleted_impl,
            "deleted implementation file",
            "deleted implementation files",
        ));
    }
    parts.join(DOT)
}

fn counted(count: usize, singular: &str, plural: &str) -> String {
    let unit = if count == 1 { singular } else { plural };
    format!("{count} {unit}")
}

/// `structure reached 63 of 97 implementation files (first 200 in path order)
/// — the rest ranked from git facts`.
pub(crate) fn caveat_sentence(
    reached: u64,
    total: u64,
    implementation: bool,
    path_order_first: Option<u64>,
) -> String {
    let noun = if implementation {
        "implementation files"
    } else {
        "files"
    };
    let bias = match path_order_first {
        Some(first) if first > 0 => format!(" (first {} in path order)", format_lines(first)),
        _ => String::new(),
    };
    format!(
        "structure reached {} of {} {noun}{bias}{DASH}the rest ranked from git facts",
        format_lines(reached),
        format_lines(total)
    )
}

/// The commit-ledger link says what the click does next.
pub(crate) fn ledger_link(open: bool) -> &'static str {
    if open {
        HIDE_LEDGER_LINK
    } else {
        SHOW_LEDGER_LINK
    }
}

/// `all 236 → Attention` — the link to the whole ordered list.
pub(crate) fn all_attention(count: usize) -> String {
    format!("all {} {ATTENTION_LINK}", format_count(count))
}

#[cfg(test)]
mod tests {
    use super::super::super::model::{AlsoFact, CommitsFact, DirRef, MovesFact, PublicApiFact, TestsFact};
    use super::{
        all_attention, also_sentence, caveat_sentence, commits_sentence, headline_files,
        headline_lines, headline_share_of_files, headline_share_of_lines, ledger_link,
        moves_sentence, percent_label, public_api_sentence, tests_sentence,
    };

    fn public_api(removed: u64, signatures: u64, added: u64, lower_bound: bool) -> PublicApiFact {
        PublicApiFact {
            removed,
            signatures,
            added,
            lower_bound,
            languages: Vec::new(),
            no_supported_language: false,
        }
    }

    fn languages() -> Vec<String> {
        vec!["TypeScript".to_string(), "TSX".to_string()]
    }

    #[test]
    fn the_headline_names_the_role_then_its_share() {
        assert_eq!(
            headline_lines("Implementation", 15_692, false),
            "Implementation 15\u{2009}692 lines"
        );
        assert_eq!(
            headline_share_of_lines(45.3, 34_640, 97),
            "45 % of 34\u{2009}640 \u{00B7} 97 files"
        );
        assert_eq!(headline_lines("Documentation", 1, false), "Documentation 1 line");
    }

    #[test]
    fn a_deletion_heavy_role_shows_the_sign() {
        assert_eq!(
            headline_lines("Implementation", 16_000, true),
            "Implementation \u{2212}16\u{2009}000 lines"
        );
    }

    #[test]
    fn a_comparison_without_line_counts_falls_back_to_files() {
        assert_eq!(headline_files("Implementation", 12), "Implementation 12 files");
        assert_eq!(headline_files("Unclassified", 1), "Unclassified 1 file");
        assert_eq!(headline_share_of_files(40.0, 30), "40 % of 30 files");
    }

    #[test]
    fn legend_percentages_keep_one_decimal() {
        assert_eq!(percent_label(45.3), "45.3 %");
        assert_eq!(percent_label(0.7), "0.7 %");
    }

    #[test]
    fn public_api_counts_are_lower_bounds_only_while_coverage_is_partial() {
        assert_eq!(
            public_api_sentence(&public_api(3, 12, 34, true), &languages()),
            "\u{2265} 3 removed \u{00B7} \u{2265} 12 signatures changed \u{00B7} \u{2265} 34 \
             added \u{2014} analyzed subset, TS/TSX"
        );
        assert_eq!(
            public_api_sentence(&public_api(3, 12, 34, false), &languages()),
            "3 removed \u{00B7} 12 signatures changed \u{00B7} 34 added"
        );
    }

    #[test]
    fn public_api_never_prints_a_zero_or_a_plural_of_one() {
        assert_eq!(
            public_api_sentence(&public_api(0, 1, 0, false), &[]),
            "1 signature changed"
        );
        assert_eq!(public_api_sentence(&public_api(0, 0, 0, false), &[]), "");
    }

    #[test]
    fn a_comparison_without_a_supported_language_says_so() {
        let mut fact = public_api(0, 0, 0, true);
        fact.no_supported_language = true;
        assert_eq!(
            public_api_sentence(&fact, &languages()),
            "no supported language in this comparison"
        );
    }

    #[test]
    fn the_tests_fact_names_the_largest_directory_without_test_changes() {
        let fact = TestsFact {
            impl_dirs: 6,
            with_tests: 4,
            without: vec![
                DirRef {
                    path: "packages/workers/src".to_string(),
                    files: 26,
                    lines: 6_118,
                },
                DirRef {
                    path: "packages/core/src".to_string(),
                    files: 2,
                    lines: 30,
                },
            ],
        };
        assert_eq!(
            tests_sentence(&fact),
            "Test files changed next to 4 of 6 implementation directories \u{00B7} none next to \
             packages/workers/src (26 files, 6\u{2009}118 lines) and 1 more"
        );
    }

    #[test]
    fn every_implementation_directory_with_tests_needs_no_second_clause() {
        let fact = TestsFact {
            impl_dirs: 1,
            with_tests: 1,
            without: Vec::new(),
        };
        assert_eq!(
            tests_sentence(&fact),
            "Test files changed next to 1 of 1 implementation directory"
        );
    }

    #[test]
    fn the_moves_fact_splits_mechanical_moves_from_moves_with_edits() {
        let fact = MovesFact {
            total: 21,
            likely_mechanical: 17,
            with_edits: 4,
            avg_similarity: 94,
            residual_lines: 400,
        };
        assert_eq!(
            moves_sentence(&fact),
            "21 high-similarity moves \u{00B7} 17 likely mechanical (\u{2264} 20 residual lines) \
             \u{00B7} 4 with edits, ranked below"
        );

        let only_mechanical = MovesFact {
            total: 1,
            likely_mechanical: 1,
            with_edits: 0,
            avg_similarity: 98,
            residual_lines: 3,
        };
        assert_eq!(
            moves_sentence(&only_mechanical),
            "1 high-similarity move \u{00B7} 1 likely mechanical (\u{2264} 20 residual lines)"
        );
    }

    #[test]
    fn the_commits_fact_reads_count_merges_authors_span_and_range() {
        let fact = CommitsFact {
            count: 14,
            merges: 1,
            authors: vec!["David Matejka".to_string()],
            span_secs: 6 * 86_400,
            first_sha: "305b0f0".to_string(),
            last_sha: "9a7be3f".to_string(),
        };
        assert_eq!(
            commits_sentence(&fact),
            "14 \u{00B7} 1 merge \u{00B7} David Matejka \u{00B7} 6 days \u{00B7} 305b0f0 \
             \u{2026} 9a7be3f"
        );
    }

    #[test]
    fn a_single_commit_has_no_merge_no_span_and_one_sha() {
        let fact = CommitsFact {
            count: 1,
            merges: 0,
            authors: vec!["Ada".to_string()],
            span_secs: 0,
            first_sha: "abc1234".to_string(),
            last_sha: "abc1234".to_string(),
        };
        assert_eq!(commits_sentence(&fact), "1 \u{00B7} Ada \u{00B7} abc1234");
    }

    #[test]
    fn long_author_lists_and_short_spans_stay_readable() {
        let fact = CommitsFact {
            count: 9,
            merges: 2,
            authors: vec![
                "Ada".to_string(),
                "Bob".to_string(),
                "Cy".to_string(),
                "Dee".to_string(),
                "Eve".to_string(),
            ],
            span_secs: 7_200,
            first_sha: "1111111".to_string(),
            last_sha: "2222222".to_string(),
        };
        assert_eq!(
            commits_sentence(&fact),
            "9 \u{00B7} 2 merges \u{00B7} Ada, Bob, Cy +2 \u{00B7} 2 hours \u{00B7} 1111111 \
             \u{2026} 2222222"
        );
    }

    #[test]
    fn the_also_fact_omits_every_kind_that_did_not_change() {
        let fact = AlsoFact {
            lockfiles: 2,
            submodules: 1,
            binaries: 3,
            deleted_impl: 0,
        };
        assert_eq!(
            also_sentence(&fact),
            "2 lockfiles \u{00B7} 1 submodule pointer \u{00B7} 3 binary files"
        );

        let one_each = AlsoFact {
            lockfiles: 1,
            submodules: 0,
            binaries: 0,
            deleted_impl: 1,
        };
        assert_eq!(
            also_sentence(&one_each),
            "1 lockfile \u{00B7} 1 deleted implementation file"
        );
    }

    #[test]
    fn the_caveat_names_the_reach_and_the_selection_bias() {
        assert_eq!(
            caveat_sentence(63, 97, true, Some(200)),
            "structure reached 63 of 97 implementation files (first 200 in path order) \
             \u{2014} the rest ranked from git facts"
        );
        assert_eq!(
            caveat_sentence(3, 12, false, None),
            "structure reached 3 of 12 files \u{2014} the rest ranked from git facts"
        );
    }

    #[test]
    fn the_attention_link_counts_the_whole_list() {
        assert_eq!(all_attention(236), "all 236 \u{2192} Attention");
        assert_eq!(all_attention(1_236), "all 1\u{2009}236 \u{2192} Attention");
    }

    #[test]
    fn the_ledger_link_names_the_next_state() {
        assert_eq!(ledger_link(false), "show ledger");
        assert_eq!(ledger_link(true), "hide ledger");
    }
}
