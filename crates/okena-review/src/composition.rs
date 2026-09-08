//! What a comparison is made of: role volumes, with inline test scopes moved
//! out of the implementation they are written inside.
//!
//! Analysis is per file. A module gated at its *declaration*
//! (`#[cfg(test)] mod fixtures;` in the parent) is therefore not recognised as
//! a test scope — resolving that needs the declaring file, which this pass
//! never opens.

use okena_core::review::{
    AnalysisCoverage, ChangeComposition, CompositionFile, CompositionTotals, FileAnalysis,
    FileRole, RoleVolume,
};
use okena_syntax::{
    AnalysisLimits, DocumentSymbols, SymbolFact, SymbolKind, SyntaxLanguage, Truncation,
};

use crate::classification;

/// One changed file, as the caller's diff already describes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangedFile {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub is_binary: bool,
    /// One-based head-side line numbers that the diff added, ascending.
    pub added_lines: Vec<u32>,
    /// One-based base-side line numbers that the diff deleted, ascending.
    pub deleted_lines: Vec<u32>,
}

impl ChangedFile {
    /// Head path when there is one, else the base path.
    pub fn path(&self) -> &str {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .unwrap_or_default()
    }
}

/// Source text for the two sides of one file.
pub trait SourceLoader {
    /// Head-side content, or `None` when it cannot be read.
    fn head(&self, path: &str) -> Option<String>;
    /// Base-side content, or `None` when it cannot be read.
    fn base(&self, path: &str) -> Option<String>;
}

/// Ceilings for one comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompositionLimits {
    pub syntax: AnalysisLimits,
    /// Files handed to tree-sitter. Past it, roles still hold — only the
    /// inline-test split goes unmeasured, and coverage says so.
    pub max_analyzed_files: usize,
}

impl Default for CompositionLimits {
    fn default() -> Self {
        Self {
            syntax: AnalysisLimits::default(),
            max_analyzed_files: 500,
        }
    }
}

/// Classify every file, then measure how much of the implementation volume is
/// tests written inside implementation files.
pub fn compose(
    files: &[ChangedFile],
    loader: &dyn SourceLoader,
    limits: CompositionLimits,
) -> ChangeComposition {
    let mut out = Vec::with_capacity(files.len());
    let mut coverage = AnalysisCoverage::default();
    let mut budget = limits.max_analyzed_files;

    for file in files {
        let path = file.path().to_string();
        let classification = classification::classify(&path);
        let added = count(&file.added_lines);
        let deleted = count(&file.deleted_lines);
        let language = analysis_candidate(file, classification.role);
        if let Some(language) = language {
            coverage.candidates += 1;
            record_language(&mut coverage, language);
        }

        let (inline_test_lines, analysis) = match language {
            None => (0, FileAnalysis::Skipped),
            Some(_) if budget == 0 => {
                coverage.not_reached += 1;
                (0, FileAnalysis::NotReached)
            }
            Some(language) => {
                budget -= 1;
                let measured = inline_tests(file, language, loader, limits.syntax);
                match measured.analysis {
                    FileAnalysis::Analyzed => coverage.analyzed += 1,
                    FileAnalysis::Partial => coverage.partial += 1,
                    _ => coverage.unavailable += 1,
                }
                (measured.lines, measured.analysis)
            }
        };

        out.push(CompositionFile {
            path,
            old_path: file.old_path.clone(),
            new_path: file.new_path.clone(),
            role: classification.role,
            rule_id: classification.rule_id,
            added,
            deleted,
            is_binary: file.is_binary,
            inline_test_lines,
            analysis,
        });
    }

    let totals = totals(&out);
    let roles = role_volumes(&out, totals.changed_lines);
    ChangeComposition {
        files: out,
        roles,
        totals,
        coverage,
    }
}

/// Whether a file is worth parsing: only an implementation-like file can hide
/// tests inside it, and only in a language this build understands.
fn analysis_candidate(file: &ChangedFile, role: FileRole) -> Option<SyntaxLanguage> {
    let has_lines = !file.added_lines.is_empty() || !file.deleted_lines.is_empty();
    if file.is_binary || !has_lines || !role.is_implementation() {
        return None;
    }
    SyntaxLanguage::from_path(file.path())
}

fn record_language(coverage: &mut AnalysisCoverage, language: SyntaxLanguage) {
    let label = language.label().to_string();
    if !coverage.languages.contains(&label) {
        coverage.languages.push(label);
    }
}

struct InlineTests {
    lines: u64,
    analysis: FileAnalysis,
}

/// Changed lines that sit inside a test scope, counted once each — a nested
/// case inside a changed `mod tests` cannot be counted twice because scopes are
/// merged into line ranges before the lines are matched against them.
fn inline_tests(
    file: &ChangedFile,
    language: SyntaxLanguage,
    loader: &dyn SourceLoader,
    limits: AnalysisLimits,
) -> InlineTests {
    let mut lines = 0;
    let mut analysis: Option<FileAnalysis> = None;

    for (side_lines, source) in [
        (
            &file.added_lines,
            file.new_path.as_deref().and_then(|path| loader.head(path)),
        ),
        (
            &file.deleted_lines,
            file.old_path.as_deref().and_then(|path| loader.base(path)),
        ),
    ] {
        if side_lines.is_empty() {
            continue;
        }
        let Some(source) = source else {
            return InlineTests {
                lines: 0,
                analysis: FileAnalysis::Unavailable,
            };
        };
        let document = okena_syntax::analyze(language, &source, limits);
        let state = analysis_of(&document);
        analysis = Some(analysis.map_or(state, |seen| less_complete(seen, state)));
        lines += lines_within(side_lines, &test_scopes(&document));
    }

    InlineTests {
        lines,
        analysis: analysis.unwrap_or(FileAnalysis::Skipped),
    }
}

/// The less complete of two states, so a file reads as honestly as its worse side.
fn less_complete(left: FileAnalysis, right: FileAnalysis) -> FileAnalysis {
    let rank = |state| match state {
        FileAnalysis::Analyzed => 0,
        FileAnalysis::Skipped => 1,
        FileAnalysis::Partial => 2,
        FileAnalysis::NotReached => 3,
        FileAnalysis::Unavailable => 4,
    };
    if rank(left) >= rank(right) {
        left
    } else {
        right
    }
}

fn analysis_of(document: &DocumentSymbols) -> FileAnalysis {
    match document.truncation {
        None => FileAnalysis::Analyzed,
        Some(Truncation::ParseFailed) => FileAnalysis::Unavailable,
        Some(_) => FileAnalysis::Partial,
    }
}

/// Merged, ascending line ranges of every test scope in the document.
fn test_scopes(document: &DocumentSymbols) -> Vec<(u32, u32)> {
    let mut ranges: Vec<(u32, u32)> = document
        .symbols
        .iter()
        .filter(|symbol| is_test_scope(symbol))
        .map(|symbol| (symbol.start_line, symbol.end_line))
        .collect();
    ranges.sort_unstable();
    let mut merged: Vec<(u32, u32)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1.saturating_add(1) => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// Whether a declaration opens a scope that holds tests.
///
/// Rust says so with an attribute — `#[cfg(test)]` or `#[test]` — which is a
/// fact, not a guess. A `mod tests` with no attribute is the fallback, for the
/// module somebody forgot to gate.
fn is_test_scope(symbol: &SymbolFact) -> bool {
    if symbol.kind == SymbolKind::TestGroup || symbol.has_attribute("test") {
        return true;
    }
    if symbol
        .attributes
        .iter()
        .any(|attribute| cfg_names_test(attribute))
    {
        return true;
    }
    symbol.kind == SymbolKind::Module && classification::is_test_directory(&symbol.name)
}

/// Whether a `cfg(…)` attribute enables the item for tests. String literals are
/// dropped first, so `cfg(feature = "testing")` is not a test gate.
fn cfg_names_test(attribute: &str) -> bool {
    let Some(arguments) = attribute
        .strip_prefix("cfg")
        .and_then(|rest| rest.trim_start().strip_prefix('('))
    else {
        return false;
    };
    let mut in_string = false;
    arguments
        .chars()
        .map(|character| {
            if character == '"' {
                in_string = !in_string;
            }
            if in_string || character == '"' {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .any(|token| token == "test")
}

/// How many of `lines` fall inside `ranges`. Both are ascending.
fn lines_within(lines: &[u32], ranges: &[(u32, u32)]) -> u64 {
    let mut range = ranges.iter().peekable();
    let mut current = range.next();
    let mut hits = 0;
    for &line in lines {
        while current.is_some_and(|&(_, end)| end < line) {
            current = range.next();
        }
        match current {
            Some(&(start, _)) if start <= line => hits += 1,
            Some(_) => {}
            None => break,
        }
    }
    hits
}

fn count(lines: &[u32]) -> u64 {
    u64::try_from(lines.len()).unwrap_or(u64::MAX)
}

fn totals(files: &[CompositionFile]) -> CompositionTotals {
    files.iter().fold(
        CompositionTotals {
            files: files.len(),
            ..CompositionTotals::default()
        },
        |totals, file| CompositionTotals {
            files: totals.files,
            added: totals.added.saturating_add(file.added),
            deleted: totals.deleted.saturating_add(file.deleted),
            changed_lines: totals.changed_lines.saturating_add(file.changed_lines()),
        },
    )
}

/// One row per role that has files or lines, largest first.
fn role_volumes(files: &[CompositionFile], total_changed: u64) -> Vec<RoleVolume> {
    let mut volumes: Vec<RoleVolume> = FileRole::ALL
        .iter()
        .map(|&role| RoleVolume {
            role,
            files: 0,
            changed_lines: 0,
            percent: 0.0,
            borrowed_lines: 0,
        })
        .collect();
    let index = |role: FileRole| {
        FileRole::ALL
            .iter()
            .position(|&candidate| candidate == role)
    };

    for file in files {
        let Some(own) = index(file.role) else {
            continue;
        };
        volumes[own].files += 1;
        let inline = file.inline_test_lines.min(file.changed_lines());
        volumes[own].changed_lines += file.changed_lines() - inline;
        if inline == 0 {
            continue;
        }
        let Some(test) = index(FileRole::Test) else {
            continue;
        };
        volumes[test].changed_lines += inline;
        volumes[test].borrowed_lines += inline;
    }

    volumes.retain(|volume| volume.files > 0 || volume.changed_lines > 0);
    for volume in &mut volumes {
        volume.percent = share(volume.changed_lines, total_changed);
    }
    volumes.sort_by(|left, right| {
        right
            .changed_lines
            .cmp(&left.changed_lines)
            .then_with(|| index(left.role).cmp(&index(right.role)))
    });
    volumes
}

/// A percentage in 0.0..=100.0. A comparison with no line totals has no shares.
fn share(part: u64, total: u64) -> f32 {
    if total == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let ratio = part as f64 / total as f64;
    #[allow(clippy::cast_possible_truncation)]
    let percent = (ratio * 100.0) as f32;
    percent
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    struct Sources {
        head: HashMap<String, String>,
        base: HashMap<String, String>,
    }

    impl Sources {
        fn with_head(mut self, path: &str, source: &str) -> Self {
            self.head.insert(path.to_string(), source.to_string());
            self
        }
        fn with_base(mut self, path: &str, source: &str) -> Self {
            self.base.insert(path.to_string(), source.to_string());
            self
        }
    }

    impl SourceLoader for Sources {
        fn head(&self, path: &str) -> Option<String> {
            self.head.get(path).cloned()
        }
        fn base(&self, path: &str) -> Option<String> {
            self.base.get(path).cloned()
        }
    }

    fn added(path: &str, lines: &[u32]) -> ChangedFile {
        ChangedFile {
            old_path: Some(path.to_string()),
            new_path: Some(path.to_string()),
            is_binary: false,
            added_lines: lines.to_vec(),
            deleted_lines: Vec::new(),
        }
    }

    fn volume(composition: &ChangeComposition, role: FileRole) -> RoleVolume {
        composition
            .roles
            .iter()
            .copied()
            .find(|volume| volume.role == role)
            .unwrap_or_else(|| panic!("no {role:?} row in {:#?}", composition.roles))
    }

    /// Lines 1-4 are implementation, 6-11 the gated test module.
    const ENGINE: &str = "\
pub fn run() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    #[test]
    fn runs() {
        assert_eq!(super::run(), 1);
    }
}
";

    #[test]
    fn a_cfg_test_module_moves_its_lines_from_implementation_to_tests() {
        let sources = Sources::default().with_head("src/engine.rs", ENGINE);
        let composition = compose(
            &[added("src/engine.rs", &[2, 8, 9, 10])],
            &sources,
            CompositionLimits::default(),
        );

        assert_eq!(composition.files[0].inline_test_lines, 3);
        assert_eq!(
            volume(&composition, FileRole::Implementation).changed_lines,
            1
        );
        let tests = volume(&composition, FileRole::Test);
        assert_eq!(tests.changed_lines, 3);
        assert_eq!(tests.borrowed_lines, 3);
        // The lines came from an implementation file, so Tests has no file of its own.
        assert_eq!(tests.files, 0);
    }

    #[test]
    fn a_nested_case_inside_a_changed_test_module_is_counted_once() {
        let sources = Sources::default().with_head("src/engine.rs", ENGINE);
        // Every line of the module, including its `#[test] fn` inside it.
        let composition = compose(
            &[added("src/engine.rs", &[5, 6, 7, 8, 9, 10, 11])],
            &sources,
            CompositionLimits::default(),
        );
        assert_eq!(composition.files[0].inline_test_lines, 7);
        assert_eq!(composition.totals.changed_lines, 7);
        assert_eq!(volume(&composition, FileRole::Test).changed_lines, 7);
    }

    #[test]
    fn deleted_test_lines_are_measured_against_the_base_side() {
        let sources = Sources::default().with_base("src/engine.rs", ENGINE);
        let file = ChangedFile {
            old_path: Some("src/engine.rs".into()),
            new_path: Some("src/engine.rs".into()),
            is_binary: false,
            added_lines: Vec::new(),
            deleted_lines: vec![2, 8, 9],
        };
        let composition = compose(&[file], &sources, CompositionLimits::default());
        assert_eq!(composition.files[0].inline_test_lines, 2);
    }

    #[test]
    fn a_test_file_keeps_all_its_lines_and_is_never_parsed() {
        let composition = compose(
            &[added("tests/integration.rs", &[1, 2, 3])],
            &Sources::default(),
            CompositionLimits::default(),
        );
        assert_eq!(composition.files[0].role, FileRole::Test);
        assert_eq!(composition.files[0].analysis, FileAnalysis::Skipped);
        assert_eq!(composition.coverage.candidates, 0);
        let tests = volume(&composition, FileRole::Test);
        assert_eq!(
            (tests.files, tests.changed_lines, tests.borrowed_lines),
            (1, 3, 0)
        );
    }

    #[test]
    fn a_cfg_gate_that_only_mentions_a_feature_named_testing_is_not_a_test_scope() {
        let source = "\
#[cfg(feature = \"testing\")]
mod helpers {
    pub fn seed() {}
}
";
        let sources = Sources::default().with_head("src/engine.rs", source);
        let composition = compose(
            &[added("src/engine.rs", &[1, 2, 3])],
            &sources,
            CompositionLimits::default(),
        );
        assert_eq!(composition.files[0].inline_test_lines, 0);
    }

    #[test]
    fn a_compound_cfg_that_includes_test_is_a_test_scope() {
        let source = "\
#[cfg(all(test, unix))]
mod tests {
    fn case() {}
}
";
        let sources = Sources::default().with_head("src/engine.rs", source);
        let composition = compose(
            &[added("src/engine.rs", &[1, 2, 3])],
            &sources,
            CompositionLimits::default(),
        );
        assert_eq!(composition.files[0].inline_test_lines, 3);
    }

    #[test]
    fn a_typescript_describe_block_counts_as_tests() {
        let source = "\
export function run() {
  return 1;
}

describe('run', () => {
  it('returns one', () => {
    expect(run()).toBe(1);
  });
});
";
        let sources = Sources::default().with_head("web/src/engine.ts", source);
        let composition = compose(
            &[added("web/src/engine.ts", &[2, 6, 7])],
            &sources,
            CompositionLimits::default(),
        );
        assert_eq!(composition.files[0].inline_test_lines, 2);
    }

    #[test]
    fn percentages_are_shares_of_the_whole_comparison() {
        let composition = compose(
            &[
                added("src/engine.rs", &[1, 2, 3]),
                added("docs/guide.md", &[1]),
            ],
            &Sources::default().with_head("src/engine.rs", "pub fn run() {}\n"),
            CompositionLimits::default(),
        );
        assert_eq!(composition.totals.changed_lines, 4);
        assert!((volume(&composition, FileRole::Implementation).percent - 75.0).abs() < 0.01);
        assert!((volume(&composition, FileRole::Documentation).percent - 25.0).abs() < 0.01);
    }

    #[test]
    fn roles_are_ordered_by_volume_and_empty_ones_are_left_out() {
        let composition = compose(
            &[
                added("docs/guide.md", &[1, 2, 3, 4, 5]),
                added("src/engine.rs", &[1]),
            ],
            &Sources::default().with_head("src/engine.rs", "pub fn run() {}\n"),
            CompositionLimits::default(),
        );
        let roles: Vec<FileRole> = composition.roles.iter().map(|volume| volume.role).collect();
        assert_eq!(
            roles,
            vec![FileRole::Documentation, FileRole::Implementation]
        );
    }

    #[test]
    fn the_file_budget_stops_parsing_without_losing_the_roles() {
        let sources = Sources::default()
            .with_head("src/a.rs", ENGINE)
            .with_head("src/b.rs", ENGINE);
        let composition = compose(
            &[added("src/a.rs", &[8]), added("src/b.rs", &[8])],
            &sources,
            CompositionLimits {
                max_analyzed_files: 1,
                ..CompositionLimits::default()
            },
        );
        assert_eq!(composition.files[0].analysis, FileAnalysis::Analyzed);
        assert_eq!(composition.files[1].analysis, FileAnalysis::NotReached);
        assert_eq!(composition.files[1].inline_test_lines, 0);
        assert_eq!(composition.coverage.candidates, 2);
        assert_eq!(composition.coverage.analyzed, 1);
        assert_eq!(composition.coverage.not_reached, 1);
        assert!(!composition.coverage.is_complete());
        // Both files still carry their role and their lines.
        assert_eq!(volume(&composition, FileRole::Implementation).files, 2);
    }

    #[test]
    fn a_file_whose_source_cannot_be_read_says_so_rather_than_reporting_zero_tests() {
        let composition = compose(
            &[added("src/engine.rs", &[1])],
            &Sources::default(),
            CompositionLimits::default(),
        );
        assert_eq!(composition.files[0].analysis, FileAnalysis::Unavailable);
        assert_eq!(composition.coverage.unavailable, 1);
        assert!(!composition.coverage.is_complete());
    }

    #[test]
    fn a_binary_file_is_never_parsed_but_still_carries_its_role() {
        let file = ChangedFile {
            old_path: None,
            new_path: Some("assets/icon.png".into()),
            is_binary: true,
            added_lines: Vec::new(),
            deleted_lines: Vec::new(),
        };
        let composition = compose(&[file], &Sources::default(), CompositionLimits::default());
        assert_eq!(composition.files[0].role, FileRole::Unclassified);
        assert_eq!(composition.files[0].analysis, FileAnalysis::Skipped);
        assert_eq!(composition.coverage.candidates, 0);
    }

    #[test]
    fn an_empty_comparison_has_no_roles_and_no_shares() {
        let composition = compose(&[], &Sources::default(), CompositionLimits::default());
        assert!(composition.roles.is_empty());
        assert_eq!(composition.totals, CompositionTotals::default());
        assert!(composition.coverage.is_complete());
    }

    #[test]
    fn languages_are_reported_once_each_in_the_order_they_appear() {
        let sources = Sources::default()
            .with_head("src/a.rs", "fn a() {}\n")
            .with_head("web/b.ts", "export const b = 1;\n")
            .with_head("src/c.rs", "fn c() {}\n");
        let composition = compose(
            &[
                added("src/a.rs", &[1]),
                added("web/b.ts", &[1]),
                added("src/c.rs", &[1]),
            ],
            &sources,
            CompositionLimits::default(),
        );
        assert_eq!(composition.coverage.languages, vec!["Rust", "TypeScript"]);
    }

    #[test]
    fn lines_within_matches_only_lines_inside_a_range() {
        assert_eq!(lines_within(&[5, 12, 20, 25], &[(10, 20)]), 2);
        assert_eq!(lines_within(&[1, 2], &[]), 0);
        assert_eq!(lines_within(&[], &[(1, 100)]), 0);
    }

    #[test]
    fn overlapping_and_adjacent_scopes_merge_into_one_range() {
        let document = okena_syntax::analyze(
            SyntaxLanguage::Rust,
            "#[cfg(test)]\nmod tests {\n    #[test]\n    fn a() {}\n}\n",
            AnalysisLimits::default(),
        );
        assert_eq!(test_scopes(&document), vec![(1, 5)]);
    }
}
