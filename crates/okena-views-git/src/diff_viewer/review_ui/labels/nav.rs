//! Navigator row wording — unit N. Spec §7: the segmented control, the filter
//! box, the Roles button, the row markers and the footer sentence.

use super::super::model::{ReasonKind, Tier};
use super::{format_lines, reasons};

const DOT: &str = " \u{00B7} ";

pub(crate) const FILES_TAB: &str = "Files";
pub(crate) const ATTENTION_TAB: &str = "Attention";
pub(crate) const FILTER_PLACEHOLDER_FILES: &str = "Filter files\u{2026}";
pub(crate) const FILTER_PLACEHOLDER_ITEMS: &str = "Filter\u{2026}";
/// The key that focuses the filter box, shown inside it — spec §11.
pub(crate) const FILTER_KEY_HINT: &str = "/";
pub(crate) const ROLES: &str = "Roles";
pub(crate) const CLEAR_GLYPH: &str = "\u{2715}";
pub(crate) const CHEVRON_DOWN: &str = "\u{25BE}";
pub(crate) const FLATTEN: &str = "flatten";
pub(crate) const OUTLINE: &str = "outline";
pub(crate) const OUTLINE_HINT: &str =
    "Changed symbols and what changed in them, inline under every file";
/// The label on a detail line that states a signature change.
pub(crate) const SIGNATURE_LINE: &str = "sig";
pub(crate) const SHOW_ALL: &str = "show all";
pub(crate) const GROUP_BY_FILE: &str = "group by file";
pub(crate) const ORDERED_LIST: &str = "ordered list";
pub(crate) const TESTS_EXCLUDED: &str = "tests excluded";
pub(crate) const TESTS_CHIP: &str = "tests";
pub(crate) const NO_FILE_MATCH: &str = "No file matches the filter";
pub(crate) const NO_ITEM_MATCH: &str = "No item matches the filters";
/// The way out of an empty list, next to the sentence that explains it.
pub(crate) const CLEAR: &str = "clear";
pub(crate) const NO_TESTS_MARKER: &str = "no tests";

pub(crate) const PRESETS_TITLE: &str = "PRESETS";
pub(crate) const ROLES_TITLE: &str = "ROLES \u{00B7} CLICK TOGGLES \u{00B7} OR";
pub(crate) const ALSO_TITLE: &str = "ALSO";
pub(crate) const LIKELY_MECHANICAL: &str = "Likely mechanical only";
pub(crate) const NOT_ANALYZED_ONLY: &str = "Not analyzed only";

/// The tier separators of the Attention list — spec §7.
pub(crate) fn tier_label(tier: Tier) -> &'static str {
    match tier {
        Tier::Contract => "CONTRACT",
        Tier::Behaviour => "BEHAVIOUR",
        Tier::Volume => "VOLUME",
        Tier::GitFacts => "GIT FACTS",
        Tier::Rest => "REST",
    }
}

/// `Roles · all 11`; the caller adds the ✕ when a filter is active.
pub(crate) fn roles_button(filter_label: &str) -> String {
    format!("{ROLES}{DOT}{filter_label}")
}

/// `sig` / `sig 2` — how many signatures changed in the file.
pub(crate) fn signature_marker(count: usize) -> String {
    if count <= 1 {
        "sig".to_string()
    } else {
        format!("sig {count}")
    }
}

/// The marker word for a file row; `None` for reasons a row never shows.
///
/// `signatures` is the number of signature reasons in the whole file, so the
/// `sig N` marker counts them even though only one of them produced it.
pub(crate) fn file_marker(kind: ReasonKind, label: &str, signatures: usize) -> Option<String> {
    match kind {
        // Dimming already says it; a badge would read as an error state.
        ReasonKind::NotAnalyzed => None,
        // Every edited function has a changed body; the churn cell says as much.
        ReasonKind::Body => None,
        ReasonKind::PublicSignature | ReasonKind::ExportedSignature => {
            Some(signature_marker(signatures))
        }
        ReasonKind::Calls => Some("calls".to_string()),
        ReasonKind::New | ReasonKind::NewPublic => Some("new".to_string()),
        ReasonKind::PublicRemoved | ReasonKind::Removed | ReasonKind::DeletedImpl => {
            Some("removed".to_string())
        }
        _ => Some(short_chip(label).to_string()),
    }
}

/// The marker word for a symbol row in the outline; `None` when the detail
/// lines under the row already state it.
pub(crate) fn symbol_marker(kind: ReasonKind, label: &str) -> Option<String> {
    match kind {
        // The lines under the row are the calls and the signature themselves.
        ReasonKind::Calls => None,
        // Every edited symbol has a changed body; the churn cell says as much.
        ReasonKind::Body => None,
        ReasonKind::NotAnalyzed => None,
        // The signature line shows the change but not who depends on it.
        ReasonKind::PublicSignature => Some("public".to_string()),
        ReasonKind::ExportedSignature => Some("exported".to_string()),
        _ => Some(short_chip(label).to_string()),
    }
}

/// Priority of a reason as a file marker — spec §7 keeps the two loudest.
pub(crate) fn marker_rank(kind: ReasonKind) -> u8 {
    match kind {
        ReasonKind::PublicRemoved => 0,
        ReasonKind::PublicSignature | ReasonKind::ExportedSignature => 1,
        ReasonKind::Calls => 2,
        ReasonKind::Removed | ReasonKind::New | ReasonKind::NewPublic | ReasonKind::DeletedImpl => {
            3
        }
        ReasonKind::Moved => 4,
        _ => 5,
    }
}

/// The navigator is one column wide, so the sentences the Overview spells out
/// are shortened here. Everything else keeps the wording it already has.
pub(crate) fn short_chip(label: &str) -> &str {
    match label {
        reasons::PUBLIC_REMOVED => "removed",
        reasons::PUBLIC_SIGNATURE | reasons::EXPORTED_SIGNATURE => "sig",
        reasons::NEW_PUBLIC | reasons::NEW_IMPLEMENTATION_FILE => "new",
        reasons::DELETED_IMPLEMENTATION_FILE => "deleted",
        reasons::NO_TEST_CHANGES => NO_TESTS_MARKER,
        other => other,
    }
}

/// `…/collection.ts → …/content/collection.ts` — the directories both paths
/// share are elided, the basenames always survive.
pub(crate) fn rename_display(old: &str, new: &str) -> String {
    let old_parts: Vec<&str> = old.split('/').collect();
    let new_parts: Vec<&str> = new.split('/').collect();
    // The basename is never elided, so the last segment is out of reach.
    let limit = old_parts.len().min(new_parts.len()).saturating_sub(1);
    let shared = (0..limit)
        .take_while(|index| old_parts[*index] == new_parts[*index])
        .count();
    format!(
        "{} \u{2192} {}",
        elided(&old_parts, shared),
        elided(&new_parts, shared)
    )
}

fn elided(parts: &[&str], shared: usize) -> String {
    let tail = parts[shared..].join("/");
    if shared == 0 {
        tail
    } else {
        format!("\u{2026}/{tail}")
    }
}

/// Row counts are `usize`; the shared formatter speaks `u64`.
fn wide(count: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX)
}

/// `385 files` / `1 file`.
pub(crate) fn files_phrase(count: usize) -> String {
    match count {
        1 => "1 file".to_string(),
        other => format!("{} files", format_lines(wide(other))),
    }
}

/// `312 changed symbols` / `1 changed symbol`.
pub(crate) fn symbols_phrase(count: usize) -> String {
    match count {
        1 => "1 changed symbol".to_string(),
        other => format!("{} changed symbols", format_lines(wide(other))),
    }
}

/// `236 items` / `1 item`.
pub(crate) fn items_phrase(count: usize) -> String {
    match count {
        1 => "1 item".to_string(),
        other => format!("{} items", format_lines(wide(other))),
    }
}

/// The sidebar footer: what the list currently shows, plus the link that undoes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FooterLine {
    pub text: String,
    /// `show all`, present only while a filter narrows the list.
    pub action: Option<&'static str>,
}

/// `385 files · 312 changed symbols · dimmed = not analyzed (185)` /
/// `113 of 385 files · Review code`. `symbols` is `None` unless the outline
/// is on, and then it says how long the list the user scrolls really is.
pub(crate) fn files_footer(
    visible: usize,
    total: usize,
    role_label: Option<&str>,
    not_analyzed: usize,
    symbols: Option<usize>,
) -> FooterLine {
    let narrowed = visible != total || role_label.is_some();
    if !narrowed {
        let mut text = files_phrase(total);
        if let Some(symbols) = symbols {
            text.push_str(DOT);
            text.push_str(&symbols_phrase(symbols));
        }
        if not_analyzed > 0 {
            text.push_str(DOT);
            text.push_str(&format!(
                "dimmed = not analyzed ({})",
                format_lines(wide(not_analyzed))
            ));
        }
        return FooterLine { text, action: None };
    }
    let mut text = format!("{} of {}", format_lines(wide(visible)), files_phrase(total));
    if let Some(symbols) = symbols {
        text.push_str(DOT);
        text.push_str(&symbols_phrase(symbols));
    }
    if let Some(role_label) = role_label {
        text.push_str(DOT);
        text.push_str(role_label);
    }
    FooterLine {
        text,
        action: Some(SHOW_ALL),
    }
}

/// `236 items · tests excluded` / `26 of 236 items · sig, removed`.
pub(crate) fn attention_footer(
    visible: usize,
    total: usize,
    chips: &[&str],
    tests_excluded: bool,
) -> FooterLine {
    let mut text = if visible == total {
        items_phrase(total)
    } else {
        format!("{} of {}", format_lines(wide(visible)), items_phrase(total))
    };
    if !chips.is_empty() {
        text.push_str(DOT);
        text.push_str(&chips.join(", "));
    }
    if tests_excluded {
        text.push_str(DOT);
        text.push_str(TESTS_EXCLUDED);
    }
    FooterLine { text, action: None }
}

#[cfg(test)]
mod tests {
    use super::super::super::model::{ReasonKind, Tier};
    use super::{
        attention_footer, file_marker, files_footer, rename_display, short_chip, signature_marker,
        symbol_marker, tier_label,
    };

    #[test]
    fn tiers_are_named_in_words_not_debug() {
        assert_eq!(tier_label(Tier::Contract), "CONTRACT");
        assert_eq!(tier_label(Tier::GitFacts), "GIT FACTS");
        assert_eq!(tier_label(Tier::Rest), "REST");
    }

    #[test]
    fn the_signature_marker_counts_only_when_there_is_more_than_one() {
        assert_eq!(signature_marker(0), "sig");
        assert_eq!(signature_marker(1), "sig");
        assert_eq!(signature_marker(2), "sig 2");
    }

    #[test]
    fn not_analyzed_never_becomes_a_marker() {
        assert_eq!(
            file_marker(ReasonKind::NotAnalyzed, "not analyzed \u{00B7} JS", 0),
            None
        );
    }

    #[test]
    fn markers_use_the_short_word_for_the_row() {
        assert_eq!(
            file_marker(ReasonKind::PublicRemoved, "public symbol removed", 0),
            Some("removed".to_string())
        );
        assert_eq!(
            file_marker(ReasonKind::ExportedSignature, "exported signature", 2),
            Some("sig 2".to_string())
        );
        assert_eq!(
            file_marker(ReasonKind::Calls, "2 calls \u{00B7} error branch", 0),
            Some("calls".to_string())
        );
        assert_eq!(
            file_marker(ReasonKind::New, "new implementation file", 0),
            Some("new".to_string())
        );
        assert_eq!(
            file_marker(ReasonKind::Moved, "moved 98 %", 0),
            Some("moved 98 %".to_string()),
            "the similarity is the whole point of the marker"
        );
        assert_eq!(
            file_marker(ReasonKind::CiConfig, "CI config", 0),
            Some("CI config".to_string())
        );
    }

    #[test]
    fn chips_shorten_the_sentences_and_leave_the_measurements_alone() {
        assert_eq!(short_chip("public symbol removed"), "removed");
        assert_eq!(short_chip("public signature"), "sig");
        assert_eq!(short_chip("exported signature"), "sig");
        assert_eq!(short_chip("new \u{00B7} exported"), "new");
        assert_eq!(short_chip("no test files changed next to it"), "no tests");
        assert_eq!(short_chip("240 lines"), "240 lines");
        assert_eq!(
            short_chip("2 calls \u{00B7} error branch"),
            "2 calls \u{00B7} error branch"
        );
    }

    #[test]
    fn renames_keep_the_basenames_and_elide_the_shared_directories() {
        assert_eq!(
            rename_display("core/src/collection.ts", "core/src/content/collection.ts"),
            "\u{2026}/collection.ts \u{2192} \u{2026}/content/collection.ts"
        );
        assert_eq!(
            rename_display("pletivo/src/render.ts", "core/src/render.ts"),
            "pletivo/src/render.ts \u{2192} core/src/render.ts",
            "nothing is shared, so nothing is elided"
        );
        assert_eq!(
            rename_display("src/old.rs", "src/new.rs"),
            "\u{2026}/old.rs \u{2192} \u{2026}/new.rs"
        );
        assert_eq!(
            rename_display("old.rs", "new.rs"),
            "old.rs \u{2192} new.rs",
            "a basename is never elided"
        );
    }

    #[test]
    fn the_files_footer_names_the_filter_and_offers_the_way_back() {
        let plain = files_footer(385, 385, None, 185, None);
        assert_eq!(plain.text, "385 files \u{00B7} dimmed = not analyzed (185)");
        assert_eq!(plain.action, None);

        let analyzed = files_footer(12, 12, None, 0, None);
        assert_eq!(analyzed.text, "12 files");
        assert_eq!(analyzed.action, None);

        let filtered = files_footer(113, 385, Some("Review code"), 185, None);
        assert_eq!(filtered.text, "113 of 385 files \u{00B7} Review code");
        assert_eq!(filtered.action, Some("show all"));

        let text_only = files_footer(3, 385, None, 0, None);
        assert_eq!(text_only.text, "3 of 385 files");
        assert_eq!(text_only.action, Some("show all"));
    }

    #[test]
    fn a_symbol_marker_never_repeats_the_lines_below_it() {
        // The call and signature lines under the row state these themselves.
        assert_eq!(symbol_marker(ReasonKind::Calls, "6 calls"), None);
        assert_eq!(symbol_marker(ReasonKind::Body, "body"), None);
        assert_eq!(
            symbol_marker(ReasonKind::PublicSignature, "public signature").as_deref(),
            Some("public")
        );
        assert_eq!(
            symbol_marker(ReasonKind::NewPublic, "new \u{00B7} exported").as_deref(),
            Some("new")
        );
    }

    #[test]
    fn the_outline_footer_says_how_long_the_list_really_is() {
        let outlined = files_footer(88, 88, None, 8, Some(312));
        assert_eq!(
            outlined.text,
            "88 files \u{00B7} 312 changed symbols \u{00B7} dimmed = not analyzed (8)"
        );

        let one = files_footer(1, 88, None, 0, Some(1));
        assert_eq!(one.text, "1 of 88 files \u{00B7} 1 changed symbol");
        assert_eq!(one.action, Some("show all"));
    }

    #[test]
    fn the_attention_footer_counts_items_and_names_the_active_chips() {
        let plain = attention_footer(236, 236, &[], false);
        assert_eq!(plain.text, "236 items");

        let excluded = attention_footer(236, 236, &[], true);
        assert_eq!(excluded.text, "236 items \u{00B7} tests excluded");

        let chipped = attention_footer(26, 236, &["sig", "removed"], false);
        assert_eq!(chipped.text, "26 of 236 items \u{00B7} sig, removed");

        assert_eq!(attention_footer(1, 1, &[], false).text, "1 item");
    }
}
