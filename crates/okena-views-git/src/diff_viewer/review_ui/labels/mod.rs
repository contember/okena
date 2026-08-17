//! The review UI's shared vocabulary. Every user-visible word for an enum comes
//! from here — `{:?}` never reaches the screen.
// Frozen surface: the wave-1 view units call these.
#![allow(dead_code)]

pub(crate) mod facts;
pub(crate) mod nav;
pub(crate) mod reasons;
pub(crate) mod status;

use super::model::KindGlyph;
use okena_core::review::{FileRole, ReviewFileStatus};
use okena_syntax::{ControlContext, SymbolKind, SyntaxLanguage};

/// Thin space; groups digits without the width of a full space.
const DIGIT_GROUP_SEPARATOR: char = '\u{2009}';

pub(crate) fn role_label(role: FileRole) -> &'static str {
    match role {
        FileRole::Implementation => "Implementation",
        FileRole::Test => "Tests",
        FileRole::Fixture => "Fixtures",
        FileRole::Snapshot => "Snapshots",
        FileRole::Example => "Examples",
        FileRole::Documentation => "Documentation",
        FileRole::Generated => "Generated",
        FileRole::Vendored => "Vendored",
        FileRole::Lockfile => "Lockfiles",
        FileRole::Configuration => "Configuration",
        FileRole::Unclassified => "Unclassified",
    }
}

/// Badge-sized role name.
pub(crate) fn role_short(role: FileRole) -> &'static str {
    match role {
        FileRole::Implementation => "Impl",
        FileRole::Test => "Tests",
        FileRole::Fixture => "Fixtures",
        FileRole::Snapshot => "Snapshots",
        FileRole::Example => "Examples",
        FileRole::Documentation => "Docs",
        FileRole::Generated => "Generated",
        FileRole::Vendored => "Vendored",
        FileRole::Lockfile => "Lockfiles",
        FileRole::Configuration => "Config",
        FileRole::Unclassified => "Unclassified",
    }
}

pub(crate) fn status_label(status: ReviewFileStatus) -> &'static str {
    match status {
        ReviewFileStatus::Added => "added",
        ReviewFileStatus::Deleted => "deleted",
        ReviewFileStatus::Modified => "modified",
        ReviewFileStatus::Renamed => "renamed",
        ReviewFileStatus::Copied => "copied",
        ReviewFileStatus::TypeChanged => "type changed",
        ReviewFileStatus::ModeChanged => "mode changed",
        ReviewFileStatus::SubmoduleChanged => "submodule changed",
        ReviewFileStatus::Unmerged => "unmerged",
        ReviewFileStatus::Unknown => "unknown",
    }
}

pub(crate) fn glyph(kind: KindGlyph) -> &'static str {
    match kind {
        KindGlyph::Function => "\u{0192}",
        KindGlyph::Method => "m",
        KindGlyph::Class => "C",
        KindGlyph::Type => "T",
        KindGlyph::Module => "M",
        KindGlyph::File => "\u{2261}",
        KindGlyph::Directory => "\u{25B8}",
    }
}

/// Members (constants, fields, variants) share the type glyph — the spec has no
/// glyph of their own.
pub(crate) fn symbol_glyph(kind: &SymbolKind) -> KindGlyph {
    match kind {
        SymbolKind::Module => KindGlyph::Module,
        SymbolKind::Function | SymbolKind::Macro => KindGlyph::Function,
        SymbolKind::Method => KindGlyph::Method,
        SymbolKind::Struct | SymbolKind::Class | SymbolKind::Impl | SymbolKind::Union => {
            KindGlyph::Class
        }
        SymbolKind::Enum
        | SymbolKind::Trait
        | SymbolKind::Interface
        | SymbolKind::TypeAlias
        | SymbolKind::Constant
        | SymbolKind::Static
        | SymbolKind::Field
        | SymbolKind::Variant => KindGlyph::Type,
    }
}

pub(crate) fn language_label(language: &SyntaxLanguage) -> &'static str {
    language.display_name()
}

/// Language name for a path, including languages structure analysis cannot parse.
pub(crate) fn language_from_path(path: &str) -> Option<&'static str> {
    let extension = path.rsplit('/').next()?.rsplit_once('.')?.1.to_lowercase();
    let label = match extension.as_str() {
        "js" | "jsx" | "mjs" | "cjs" => "JavaScript",
        "ts" => "TypeScript",
        "tsx" => "TSX",
        "rs" => "Rust",
        "astro" => "Astro",
        "py" => "Python",
        "go" => "Go",
        "md" => "Markdown",
        "json" => "JSON",
        "yml" | "yaml" => "YAML",
        "toml" => "TOML",
        "css" | "scss" => "CSS",
        "html" => "HTML",
        _ => return None,
    };
    Some(label)
}

/// Digits grouped in threes, e.g. `15 692`.
pub(crate) fn format_lines(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    let leading = digits.len() % 3;
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && index % 3 == leading {
            out.push(DIGIT_GROUP_SEPARATOR);
        }
        out.push(digit);
    }
    out
}

/// The `+A` / `−D` pair; both are always produced, callers hide the zero side.
pub(crate) fn format_signed(added: u64, deleted: u64) -> (String, String) {
    (
        format!("+{}", format_lines(added)),
        format!("\u{2212}{}", format_lines(deleted)),
    )
}

pub(crate) fn relative_time(timestamp: i64) -> String {
    okena_git::format_relative_time(timestamp)
}

pub(crate) fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

pub(crate) fn control_context_word(context: &ControlContext) -> String {
    match context {
        ControlContext::Condition => "condition".to_string(),
        ControlContext::Loop => "loop".to_string(),
        ControlContext::MatchArm => "match arm".to_string(),
        ControlContext::ErrorBranch => "error branch".to_string(),
        ControlContext::Callback => "callback".to_string(),
        ControlContext::Closure => "closure".to_string(),
        ControlContext::Other(word) => word.clone(),
    }
}

/// Why a file carries its role, in words. Unknown ids keep their identity.
pub(crate) fn rule_sentence(rule_id: &str) -> String {
    let what = match rule_id {
        "builtin.path.generated.v1" => "generated output",
        "builtin.path.vendored.v1" => "vendored dependencies",
        "builtin.path.lockfile.v1" => "a lockfile name",
        "builtin.path.snapshot.v1" => "snapshot files",
        "builtin.path.fixture.v1" => "fixture files",
        "builtin.path.test.v1" => "test paths",
        "builtin.path.documentation.v1" => "documentation files",
        "builtin.path.example.v1" => "example paths",
        "builtin.path.configuration.v1" => "configuration files",
        "builtin.path.implementation.v1" => "a source file extension",
        "builtin.path.unclassified.v1" => "nothing more specific",
        _ => return format!("matched by rule {rule_id}"),
    };
    format!("matched by path rule: {what}")
}

#[cfg(test)]
mod tests {
    use super::{format_lines, format_signed, language_from_path, relative_time, rule_sentence};

    #[test]
    fn line_counts_group_digits_in_threes() {
        assert_eq!(format_lines(0), "0");
        assert_eq!(format_lines(999), "999");
        assert_eq!(format_lines(1_000), "1\u{2009}000");
        assert_eq!(format_lines(15_692), "15\u{2009}692");
        assert_eq!(format_lines(1_234_567), "1\u{2009}234\u{2009}567");
    }

    #[test]
    fn signed_pairs_carry_their_own_sign() {
        let (added, deleted) = format_signed(30_925, 3_715);
        assert_eq!(added, "+30\u{2009}925");
        assert_eq!(deleted, "\u{2212}3\u{2009}715");
    }

    #[test]
    fn relative_time_reads_the_distance_from_now() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(0))
            .unwrap_or(0);
        assert_eq!(relative_time(now), "just now");
        assert_eq!(relative_time(now - 3_600), "1h ago");
        assert_eq!(relative_time(now - 6 * 86_400), "6d ago");
    }

    #[test]
    fn languages_come_from_the_extension_not_the_parser() {
        assert_eq!(language_from_path("src/app.tsx"), Some("TSX"));
        assert_eq!(language_from_path("src/app.MJS"), Some("JavaScript"));
        assert_eq!(language_from_path("a/b/Cargo.toml"), Some("TOML"));
        assert_eq!(language_from_path("docs/readme.md"), Some("Markdown"));
        assert_eq!(language_from_path("Makefile"), None);
        assert_eq!(language_from_path("src.dir/Makefile"), None);
    }

    #[test]
    fn rule_ids_are_spelled_out_and_unknown_ids_keep_their_identity() {
        assert_eq!(
            rule_sentence("builtin.path.test.v1"),
            "matched by path rule: test paths"
        );
        assert_eq!(
            rule_sentence("builtin.path.implementation.v1"),
            "matched by path rule: a source file extension"
        );
        assert_eq!(rule_sentence("custom.rule"), "matched by rule custom.rule");
    }
}
