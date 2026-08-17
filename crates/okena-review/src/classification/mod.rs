//! Deterministic path-based file classification.

use std::fmt;

use okena_core::review::{FileClassification, FileRole, ReviewFileFact, ReviewFileStatus};

const GENERATED_RULE: &str = "builtin.path.generated.v1";
const VENDORED_RULE: &str = "builtin.path.vendored.v1";
const LOCKFILE_RULE: &str = "builtin.path.lockfile.v1";
const SNAPSHOT_RULE: &str = "builtin.path.snapshot.v1";
const FIXTURE_RULE: &str = "builtin.path.fixture.v1";
const TEST_RULE: &str = "builtin.path.test.v1";
const DOCUMENTATION_RULE: &str = "builtin.path.documentation.v1";
const EXAMPLE_RULE: &str = "builtin.path.example.v1";
const CONFIGURATION_RULE: &str = "builtin.path.configuration.v1";
const IMPLEMENTATION_RULE: &str = "builtin.path.implementation.v1";
const UNCLASSIFIED_RULE: &str = "builtin.path.unclassified.v1";

/// A path cannot be classified as a repository-relative file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassificationError(String);

impl fmt::Display for ClassificationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ClassificationError {}

/// Classify one raw Git fact without modifying its Git provenance.
///
/// The head path wins for additions, modifications, copies, and renames. The
/// base path is used for deletions.
pub fn classify_file_fact(
    file: &ReviewFileFact,
) -> Result<FileClassification, ClassificationError> {
    validate_optional_path(file.old_path.as_deref())?;
    validate_optional_path(file.new_path.as_deref())?;
    let selected = match file.status {
        ReviewFileStatus::Added => require_path(file.new_path.as_deref(), "added file head")?,
        ReviewFileStatus::Deleted => require_path(file.old_path.as_deref(), "deleted file base")?,
        ReviewFileStatus::Renamed | ReviewFileStatus::Copied => {
            require_path(file.old_path.as_deref(), "renamed/copied file base")?;
            require_path(file.new_path.as_deref(), "renamed/copied file head")?
        }
        ReviewFileStatus::Modified
        | ReviewFileStatus::TypeChanged
        | ReviewFileStatus::ModeChanged
        | ReviewFileStatus::SubmoduleChanged
        | ReviewFileStatus::Unmerged
        | ReviewFileStatus::Unknown => file
            .new_path
            .as_deref()
            .or(file.old_path.as_deref())
            .ok_or_else(missing_paths)?,
    };
    classify_path(selected)
}

/// Classify an old/new path pair, preferring the head path when present.
///
/// Rules use this fixed precedence:
/// Generated, Vendored, Lockfile, Snapshot, Fixture, Test, Documentation,
/// Example, Configuration, Implementation, Unclassified.
/// Whether a scope *inside* a file reads as a test scope — a Rust `mod tests`,
/// a `describe` named `spec`. Same vocabulary as the path rules use for
/// directories, so "what counts as a test" has one answer.
pub fn is_test_scope(name: &str) -> bool {
    TEST_SEGMENTS.contains(&name.to_ascii_lowercase().as_str())
}

pub fn classify_paths(
    old_path: Option<&str>,
    new_path: Option<&str>,
) -> Result<FileClassification, ClassificationError> {
    validate_optional_path(old_path)?;
    validate_optional_path(new_path)?;
    let selected = new_path.or(old_path).ok_or_else(missing_paths)?;
    classify_path(selected)
}

fn classify_path(selected: &str) -> Result<FileClassification, ClassificationError> {
    let normalized = normalize_path(selected)?;
    let lower = normalized.to_lowercase();
    let segments: Vec<&str> = lower.split('/').collect();
    let basename = segments.last().copied().unwrap_or_default();

    let (role, rule_id) = if is_generated(&segments, basename) {
        (FileRole::Generated, GENERATED_RULE)
    } else if has_segment(&segments, VENDORED_SEGMENTS) {
        (FileRole::Vendored, VENDORED_RULE)
    } else if LOCKFILE_NAMES.contains(&basename) {
        (FileRole::Lockfile, LOCKFILE_RULE)
    } else if is_snapshot(&segments, basename) {
        (FileRole::Snapshot, SNAPSHOT_RULE)
    } else if is_fixture(&segments, basename) {
        (FileRole::Fixture, FIXTURE_RULE)
    } else if is_test(&segments, basename) {
        (FileRole::Test, TEST_RULE)
    } else if is_documentation(&segments, basename) {
        (FileRole::Documentation, DOCUMENTATION_RULE)
    } else if has_segment(&segments, EXAMPLE_SEGMENTS) {
        (FileRole::Example, EXAMPLE_RULE)
    } else if is_configuration(&segments, basename) {
        (FileRole::Configuration, CONFIGURATION_RULE)
    } else if has_implementation_extension(basename) {
        (FileRole::Implementation, IMPLEMENTATION_RULE)
    } else {
        (FileRole::Unclassified, UNCLASSIFIED_RULE)
    };

    FileClassification::from_rule(role, rule_id)
        .map_err(|error| ClassificationError(error.to_string()))
}

const GENERATED_SEGMENTS: &[&str] = &[
    "generated",
    "__generated__",
    "gen",
    "dist",
    "target",
    ".next",
    ".nuxt",
    ".svelte-kit",
];
const VENDORED_SEGMENTS: &[&str] = &[
    "vendor",
    "vendored",
    "third_party",
    "third-party",
    "node_modules",
];
const SNAPSHOT_SEGMENTS: &[&str] = &["snapshots", "__snapshots__"];
const FIXTURE_SEGMENTS: &[&str] = &[
    "fixture",
    "fixtures",
    "__fixtures__",
    "testdata",
    "test-data",
    "golden",
];
const TEST_SEGMENTS: &[&str] = &["test", "tests", "__tests__", "spec", "specs"];
const DOCUMENTATION_SEGMENTS: &[&str] = &["doc", "docs", "documentation"];
const EXAMPLE_SEGMENTS: &[&str] = &["example", "examples", "playground"];
const CONFIGURATION_SEGMENTS: &[&str] = &["config", "configs", ".cargo", ".github"];

const LOCKFILE_NAMES: &[&str] = &[
    "cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "bun.lockb",
    "deno.lock",
    "poetry.lock",
    "uv.lock",
    "pipfile.lock",
    "gemfile.lock",
    "composer.lock",
    "flake.lock",
    "go.sum",
];

fn validate_optional_path(path: Option<&str>) -> Result<(), ClassificationError> {
    if path.is_some_and(|path| path.trim().is_empty()) {
        Err(ClassificationError(
            "classification paths must not be empty".to_string(),
        ))
    } else {
        Ok(())
    }
}

fn require_path<'a>(path: Option<&'a str>, context: &str) -> Result<&'a str, ClassificationError> {
    path.ok_or_else(|| ClassificationError(format!("classification requires the {context} path")))
}

fn missing_paths() -> ClassificationError {
    ClassificationError("classification requires an old path or a new path".to_string())
}

fn normalize_path(path: &str) -> Result<String, ClassificationError> {
    if path.contains('\0') {
        return Err(ClassificationError(
            "classification paths must not contain NUL".to_string(),
        ));
    }
    let slashed = path.replace('\\', "/");
    let bytes = slashed.as_bytes();
    let windows_drive_path = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if slashed.starts_with('/') || windows_drive_path {
        return Err(ClassificationError(
            "classification paths must be repository-relative".to_string(),
        ));
    }
    let mut normalized = Vec::new();
    for segment in slashed.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                return Err(ClassificationError(
                    "classification paths must not traverse parent directories".to_string(),
                ));
            }
            segment => normalized.push(segment),
        }
    }
    if normalized.is_empty() {
        return Err(ClassificationError(
            "classification path does not name a file".to_string(),
        ));
    }
    Ok(normalized.join("/"))
}

fn has_segment(segments: &[&str], candidates: &[&str]) -> bool {
    segments.iter().any(|segment| candidates.contains(segment))
}

fn is_generated(segments: &[&str], basename: &str) -> bool {
    has_segment(segments, GENERATED_SEGMENTS)
        || basename.starts_with("generated.")
        || basename.contains(".generated.")
        || basename.contains(".gen.")
        || basename.ends_with("_generated.rs")
        || basename.ends_with("-generated.rs")
}

fn is_snapshot(segments: &[&str], basename: &str) -> bool {
    has_segment(segments, SNAPSHOT_SEGMENTS)
        || basename.ends_with(".snap")
        || basename.ends_with(".snap.new")
        || basename.ends_with(".snapshot")
}

fn is_fixture(segments: &[&str], basename: &str) -> bool {
    has_segment(segments, FIXTURE_SEGMENTS)
        || basename.contains(".fixture.")
        || basename.contains("_fixture.")
}

fn is_test(segments: &[&str], basename: &str) -> bool {
    if has_segment(segments, TEST_SEGMENTS) {
        return true;
    }
    let stem = [".d.ts", ".d.mts", ".d.cts"]
        .iter()
        .find_map(|suffix| basename.strip_suffix(suffix))
        .unwrap_or_else(|| basename.rsplit_once('.').map_or(basename, |(stem, _)| stem));
    stem == "test"
        || stem == "tests"
        || stem.ends_with(".test")
        || stem.ends_with(".spec")
        || stem.ends_with("_test")
        || stem.ends_with("_tests")
        || stem.ends_with("-test")
        || stem.ends_with("-tests")
        || stem.starts_with("test_")
}

fn is_documentation(segments: &[&str], basename: &str) -> bool {
    has_segment(segments, DOCUMENTATION_SEGMENTS)
        || basename.ends_with(".md")
        || basename.ends_with(".mdx")
        || basename.ends_with(".rst")
        || basename.ends_with(".adoc")
        || CANONICAL_DOCUMENTATION_NAMES.contains(&basename)
}

const CANONICAL_DOCUMENTATION_NAMES: &[&str] = &[
    "readme",
    "changelog",
    "contributing",
    "code_of_conduct",
    "license",
    "license-mit",
    "license-apache",
    "copying",
    "notice",
];

fn is_configuration(segments: &[&str], basename: &str) -> bool {
    has_segment(segments, CONFIGURATION_SEGMENTS)
        || CONFIGURATION_NAMES.contains(&basename)
        || basename.starts_with("tsconfig.") && basename.ends_with(".json")
        || basename.starts_with(".eslintrc")
        || basename.starts_with(".prettierrc")
        || basename == ".env"
        || basename.starts_with(".env.")
        || CONFIGURATION_STEMS
            .iter()
            .any(|stem| basename.starts_with(stem))
}

const CONFIGURATION_NAMES: &[&str] = &[
    "cargo.toml",
    "package.json",
    "deno.json",
    "deno.jsonc",
    "biome.json",
    "biome.jsonc",
    "turbo.json",
    "nx.json",
    "rust-toolchain",
    "rust-toolchain.toml",
    "rustfmt.toml",
    "clippy.toml",
    "deny.toml",
    ".editorconfig",
    ".gitignore",
    ".gitattributes",
    "dockerfile",
    "makefile",
    "justfile",
];

const CONFIGURATION_STEMS: &[&str] = &[
    "eslint.config.",
    "prettier.config.",
    "vite.config.",
    "vitest.config.",
    "jest.config.",
    "webpack.config.",
    "rollup.config.",
    "tailwind.config.",
    "postcss.config.",
    "next.config.",
    "nuxt.config.",
];

fn has_implementation_extension(basename: &str) -> bool {
    const EXTENSIONS: &[&str] = &[
        "rs", "js", "jsx", "ts", "tsx", "mts", "cts", "mjs", "cjs", "css", "scss", "sass", "less",
        "html", "vue", "svelte",
    ];
    basename
        .rsplit_once('.')
        .is_some_and(|(_, extension)| EXTENSIONS.contains(&extension))
}

#[cfg(test)]
mod tests {
    use okena_core::review::{FactProvenance, ReviewFileStatus, ReviewSubmoduleChange};

    use super::*;

    fn role(path: &str) -> FileRole {
        classify_paths(None, Some(path)).unwrap().role()
    }

    fn file(
        status: ReviewFileStatus,
        old_path: Option<&str>,
        new_path: Option<&str>,
    ) -> ReviewFileFact {
        ReviewFileFact {
            old_path: old_path.map(str::to_string),
            new_path: new_path.map(str::to_string),
            status,
            similarity: None,
            old_mode: Some("100644".to_string()),
            new_mode: Some("100644".to_string()),
            lines_added: Some(1),
            lines_deleted: Some(1),
            binary: false,
            submodule: None::<ReviewSubmoduleChange>,
            classification: FileClassification::from_rule(
                FileRole::Unclassified,
                "builtin.unclassified",
            )
            .unwrap(),
            provenance: FactProvenance::Git,
        }
    }

    #[test]
    fn a_scope_inside_a_file_reads_as_a_test_by_the_same_names() {
        for name in ["tests", "Tests", "test", "spec", "__tests__"] {
            assert!(is_test_scope(name), "{name}");
        }
        for name in ["testing", "attest", "fixtures", "helpers"] {
            assert!(!is_test_scope(name), "{name}");
        }
    }

    #[test]
    fn precedence_protects_generated_vendor_lock_snapshot_and_fixture_roles() {
        assert_eq!(role("generated/tests/widget.test.ts"), FileRole::Generated);
        assert_eq!(role("src/api.generated.test.ts"), FileRole::Generated);
        assert_eq!(role("vendor/tests/widget.test.ts"), FileRole::Vendored);
        assert_eq!(role("tests/fixtures/package-lock.json"), FileRole::Lockfile);
        assert_eq!(role("src/fixtures/widget.test.ts"), FileRole::Fixture);
        assert_eq!(
            role("src/fixtures/__snapshots__/widget.test.ts.snap"),
            FileRole::Snapshot
        );
    }

    #[test]
    fn every_adjacent_precedence_boundary_is_explicit() {
        let cases = [
            ("vendor/generated/widget.ts", FileRole::Generated),
            ("vendor/package-lock.json", FileRole::Vendored),
            ("__snapshots__/package-lock.json", FileRole::Lockfile),
            ("fixtures/__snapshots__/case.snap", FileRole::Snapshot),
            ("tests/fixtures/case.test.ts", FileRole::Fixture),
            ("tests/README.md", FileRole::Test),
            ("docs/examples/demo.ts", FileRole::Documentation),
            ("examples/vite.config.ts", FileRole::Example),
            ("config/worker.ts", FileRole::Configuration),
            ("src/worker.ts", FileRole::Implementation),
            ("assets/logo.png", FileRole::Unclassified),
        ];
        for (path, expected) in cases {
            assert_eq!(role(path), expected, "{path}");
        }
    }

    #[test]
    fn path_pairs_prefer_head_and_fall_back_to_base() {
        let renamed = classify_paths(Some("src/worker.ts"), Some("tests/worker.test.ts")).unwrap();
        assert_eq!(renamed.role(), FileRole::Test);
        assert_eq!(
            classify_paths(Some("docs/removed.md"), None)
                .unwrap()
                .role(),
            FileRole::Documentation
        );
    }

    #[test]
    fn file_fact_status_selects_the_semantically_present_side() {
        let added = file(
            ReviewFileStatus::Added,
            Some("docs/stale.md"),
            Some("src/new.ts"),
        );
        assert_eq!(
            classify_file_fact(&added).unwrap().role(),
            FileRole::Implementation
        );

        let deleted = file(
            ReviewFileStatus::Deleted,
            Some("docs/removed.md"),
            Some("src/stale.ts"),
        );
        assert_eq!(
            classify_file_fact(&deleted).unwrap().role(),
            FileRole::Documentation
        );

        let renamed = file(
            ReviewFileStatus::Renamed,
            Some("src/old.ts"),
            Some("examples/new.ts"),
        );
        assert_eq!(
            classify_file_fact(&renamed).unwrap().role(),
            FileRole::Example
        );

        assert!(classify_file_fact(&file(ReviewFileStatus::Added, None, None)).is_err());
        assert!(
            classify_file_fact(&file(ReviewFileStatus::Deleted, None, Some("src/stale.ts")))
                .is_err()
        );
        assert!(
            classify_file_fact(&file(ReviewFileStatus::Renamed, None, Some("src/new.ts"))).is_err()
        );
    }

    #[test]
    fn recognizes_common_roles_and_stable_rule_ids() {
        let cases = [
            ("pnpm-lock.yaml", FileRole::Lockfile, LOCKFILE_RULE),
            ("README.md", FileRole::Documentation, DOCUMENTATION_RULE),
            ("README.cs.md", FileRole::Documentation, DOCUMENTATION_RULE),
            ("examples/basic.ts", FileRole::Example, EXAMPLE_RULE),
            ("Cargo.toml", FileRole::Configuration, CONFIGURATION_RULE),
            ("src/main.rs", FileRole::Implementation, IMPLEMENTATION_RULE),
            (
                "packages/ui/src/Button.tsx",
                FileRole::Implementation,
                IMPLEMENTATION_RULE,
            ),
            ("assets/logo.png", FileRole::Unclassified, UNCLASSIFIED_RULE),
        ];
        for (path, expected_role, expected_rule) in cases {
            let classification = classify_paths(None, Some(path)).unwrap();
            assert_eq!(classification.role(), expected_role, "{path}");
            assert_eq!(classification.rule_id().as_str(), expected_rule, "{path}");
            assert_eq!(
                classification.provenance(),
                FactProvenance::RuleDerived {
                    rule_id: expected_rule.to_string()
                }
            );
        }
    }

    #[test]
    fn common_test_fixture_example_and_implementation_boundaries_are_conservative() {
        let cases = [
            ("src/tests.rs", FileRole::Test),
            ("test.js", FileRole::Test),
            ("src/foo.test.d.ts", FileRole::Test),
            ("src/__fixtures__/case.json", FileRole::Fixture),
            ("playground/demo.ts", FileRole::Example),
            ("src/component.tsx", FileRole::Implementation),
            ("src/build/mod.rs", FileRole::Implementation),
            ("crates/coverage/src/lib.rs", FileRole::Implementation),
            ("src/license_checker.rs", FileRole::Implementation),
            ("src/readme_generator.ts", FileRole::Implementation),
            ("src/changelog_parser.ts", FileRole::Implementation),
            ("src/readme.generator.ts", FileRole::Implementation),
            ("src/changelog.parser.ts", FileRole::Implementation),
            ("src/license.checker.rs", FileRole::Implementation),
            ("src/notice.service.ts", FileRole::Implementation),
            ("src/.envoy.ts", FileRole::Implementation),
        ];
        for (path, expected) in cases {
            assert_eq!(role(path), expected, "{path}");
        }
    }

    #[test]
    fn dotenv_names_are_configuration_without_using_a_broad_prefix() {
        assert_eq!(role(".env"), FileRole::Configuration);
        assert_eq!(role(".env.local"), FileRole::Configuration);
        assert_eq!(role(".env.production"), FileRole::Configuration);
        assert_eq!(role("src/.envoy.ts"), FileRole::Implementation);
    }

    #[test]
    fn precedence_boundaries_across_human_authored_roles_are_stable() {
        let cases = [
            ("tests/README.md", FileRole::Test),
            ("docs/examples/demo.ts", FileRole::Documentation),
            ("examples/vite.config.ts", FileRole::Example),
            ("config/worker.test.ts", FileRole::Test),
            ("config/worker.ts", FileRole::Configuration),
            ("src/worker.ts", FileRole::Implementation),
        ];
        for (path, expected) in cases {
            assert_eq!(role(path), expected, "{path}");
        }
    }

    #[test]
    fn normalizes_windows_separators_for_matching_only() {
        assert_eq!(
            role(r"packages\worker\__tests__\run.spec.ts"),
            FileRole::Test
        );
        assert_eq!(role(r"src\fixtures\case.json"), FileRole::Fixture);
        assert_eq!(role(r"SRC\FIXTURES\CASE.JSON"), FileRole::Fixture);
    }

    #[test]
    fn classifying_a_fact_does_not_replace_git_provenance() {
        let fact = file(
            ReviewFileStatus::Modified,
            Some("src/old.ts"),
            Some("tests/new.test.ts"),
        );
        let classification = classify_file_fact(&fact).unwrap();
        assert_eq!(classification.role(), FileRole::Test);
        assert_eq!(fact.provenance, FactProvenance::Git);
        assert_eq!(fact.classification.role(), FileRole::Unclassified);
    }

    #[test]
    fn rejects_missing_empty_absolute_and_traversing_paths() {
        assert!(classify_paths(None, None).is_err());
        assert!(classify_paths(None, Some("")).is_err());
        assert!(classify_paths(Some("src/lib.rs"), Some("  ")).is_err());
        assert!(classify_paths(None, Some("/src/lib.rs")).is_err());
        assert!(classify_paths(None, Some("C:\\src\\lib.rs")).is_err());
        assert!(classify_paths(None, Some("C:src\\lib.rs")).is_err());
        assert!(classify_paths(None, Some("src/../lib.rs")).is_err());
        assert!(classify_paths(None, Some("src/\0lib.rs")).is_err());
    }
}
