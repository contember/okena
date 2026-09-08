//! Deterministic, path-only file classification.
//!
//! One rule matches, and it says which. Nothing here reads file content — a
//! role must be available for every changed file, in every language, the moment
//! the diff lands.

use okena_core::review::{FileClassification, FileRole};

const GENERATED: &str = "builtin.path.generated.v1";
const VENDORED: &str = "builtin.path.vendored.v1";
const LOCKFILE: &str = "builtin.path.lockfile.v1";
const SNAPSHOT: &str = "builtin.path.snapshot.v1";
const FIXTURE: &str = "builtin.path.fixture.v1";
const TEST: &str = "builtin.path.test.v1";
const DOCUMENTATION: &str = "builtin.path.documentation.v1";
const EXAMPLE: &str = "builtin.path.example.v1";
const CONFIGURATION: &str = "builtin.path.configuration.v1";
const IMPLEMENTATION: &str = "builtin.path.implementation.v1";
const UNCLASSIFIED: &str = "builtin.path.unclassified.v1";
/// Applied by `composition`, not by `classify`: it needs the declaring file's
/// source, which a path alone cannot give.
pub const TEST_MODULE_RULE: &str = "builtin.module.cfg-test.v1";

/// Classify one repository-relative path.
///
/// Rules are tried in this order, first match wins: generated, vendored,
/// lockfile, snapshot, fixture, test, documentation, example, configuration,
/// implementation, unclassified.
pub fn classify(path: &str) -> FileClassification {
    let lower = path.trim_start_matches("./").to_ascii_lowercase();
    let segments: Vec<&str> = lower.split('/').filter(|part| !part.is_empty()).collect();
    let basename = segments.last().copied().unwrap_or_default();
    let extension = basename.rsplit_once('.').map_or("", |(_, tail)| tail);

    let (role, rule_id) = if has_segment(&segments, GENERATED_DIRS) {
        (FileRole::Generated, GENERATED)
    } else if has_segment(&segments, VENDORED_DIRS) {
        (FileRole::Vendored, VENDORED)
    } else if LOCKFILES.contains(&basename) {
        (FileRole::Lockfile, LOCKFILE)
    } else if has_segment(&segments, SNAPSHOT_DIRS) || extension == "snap" {
        (FileRole::Snapshot, SNAPSHOT)
    } else if has_segment(&segments, FIXTURE_DIRS) {
        (FileRole::Fixture, FIXTURE)
    } else if has_segment(&segments, TEST_DIRS) || is_test_basename(basename) {
        (FileRole::Test, TEST)
    } else if has_segment(&segments, DOCUMENTATION_DIRS)
        || DOCUMENTATION_EXTENSIONS.contains(&extension)
        || DOCUMENTATION_NAMES.contains(&stem(basename))
    {
        (FileRole::Documentation, DOCUMENTATION)
    } else if has_segment(&segments, EXAMPLE_DIRS) {
        (FileRole::Example, EXAMPLE)
    } else if is_configuration(&segments, basename, extension) {
        (FileRole::Configuration, CONFIGURATION)
    } else if CODE_EXTENSIONS.contains(&extension) {
        (FileRole::Implementation, IMPLEMENTATION)
    } else {
        (FileRole::Unclassified, UNCLASSIFIED)
    };

    FileClassification {
        role,
        rule_id: rule_id.to_string(),
    }
}

/// Whether a directory name means "tests live here". Shared with the scope
/// vocabulary in `composition`, so "what counts as a test" has one answer.
pub fn is_test_directory(name: &str) -> bool {
    TEST_DIRS.contains(&name.to_ascii_lowercase().as_str())
}

/// The rule id in words, for "why this role".
pub fn rule_label(rule_id: &str) -> &'static str {
    match rule_id {
        GENERATED => "a generated-output directory",
        VENDORED => "a vendored-dependency directory",
        LOCKFILE => "a dependency lockfile",
        SNAPSHOT => "a snapshot directory or a .snap file",
        FIXTURE => "a fixture or test-data directory",
        TEST => "a test directory or a test filename",
        DOCUMENTATION => "a docs directory, a prose extension, or a README-like name",
        EXAMPLE => "an examples directory",
        CONFIGURATION => "a config directory, extension, or well-known config filename",
        IMPLEMENTATION => "a source-code extension",
        TEST_MODULE_RULE => "declared behind #[cfg(test)] by the file that owns it",
        _ => "no rule matched",
    }
}

fn has_segment(segments: &[&str], names: &[&str]) -> bool {
    segments.iter().any(|segment| names.contains(segment))
}

/// `engine.test.ts`, `test_engine.py`, `engine_test.go`, a bare `test.rs`.
fn is_test_basename(basename: &str) -> bool {
    if basename.contains(".test.") || basename.contains(".spec.") {
        return true;
    }
    let stem = stem(basename);
    stem.starts_with("test_")
        || matches!(stem, "test" | "spec")
        || ["_test", "-test", ".test", "_spec", "-spec", ".spec"]
            .iter()
            .any(|suffix| stem.ends_with(suffix))
}

fn is_configuration(segments: &[&str], basename: &str, extension: &str) -> bool {
    has_segment(segments, CONFIGURATION_DIRS)
        || CONFIGURATION_EXTENSIONS.contains(&extension)
        || CONFIGURATION_NAMES.contains(&basename)
        || basename.starts_with('.')
}

/// The basename without its final extension.
fn stem(basename: &str) -> &str {
    basename.rsplit_once('.').map_or(basename, |(head, _)| head)
}

const GENERATED_DIRS: &[&str] = &[
    "generated",
    "__generated__",
    "dist",
    "target",
    ".next",
    ".nuxt",
    ".svelte-kit",
];
const VENDORED_DIRS: &[&str] = &[
    "vendor",
    "vendored",
    "third_party",
    "third-party",
    "node_modules",
];
const SNAPSHOT_DIRS: &[&str] = &["snapshots", "__snapshots__"];
const FIXTURE_DIRS: &[&str] = &[
    "fixture",
    "fixtures",
    "__fixtures__",
    "__mocks__",
    "testdata",
    "test-data",
];
const TEST_DIRS: &[&str] = &["test", "tests", "__tests__", "spec", "specs", "e2e"];
const DOCUMENTATION_DIRS: &[&str] = &["doc", "docs", "documentation"];
const EXAMPLE_DIRS: &[&str] = &["example", "examples", "demo", "demos", "samples"];
const CONFIGURATION_DIRS: &[&str] = &[".github", ".vscode", ".idea", ".config"];

const LOCKFILES: &[&str] = &[
    "cargo.lock",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    "composer.lock",
    "gemfile.lock",
    "poetry.lock",
    "uv.lock",
    "go.sum",
    "flake.lock",
];
const DOCUMENTATION_EXTENSIONS: &[&str] = &["md", "mdx", "rst", "adoc"];
const DOCUMENTATION_NAMES: &[&str] = &[
    "readme",
    "changelog",
    "license",
    "licence",
    "contributing",
    "authors",
    "notice",
];
const CONFIGURATION_EXTENSIONS: &[&str] = &[
    "toml",
    "yaml",
    "yml",
    "json",
    "jsonc",
    "json5",
    "ini",
    "cfg",
    "conf",
    "env",
    "properties",
    "plist",
    "nix",
    "tf",
    "tfvars",
];
const CONFIGURATION_NAMES: &[&str] = &[
    "dockerfile",
    "makefile",
    "justfile",
    "rakefile",
    "procfile",
    "gemfile",
    "brewfile",
];
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts", "go", "py", "java", "kt", "kts",
    "swift", "c", "h", "cc", "cpp", "cxx", "hpp", "hh", "cs", "rb", "php", "scala", "sh", "bash",
    "zsh", "fish", "ps1", "sql", "css", "scss", "sass", "less", "html", "htm", "vue", "svelte",
    "lua", "ex", "exs", "erl", "dart", "m", "mm", "zig", "hs", "ml", "mli", "clj", "cljs", "pl",
    "r", "jl", "nim", "v", "proto", "graphql", "gql", "wgsl", "glsl",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn role(path: &str) -> FileRole {
        classify(path).role
    }

    #[test]
    fn source_files_are_implementation() {
        assert_eq!(
            role("crates/okena-git/src/diff.rs"),
            FileRole::Implementation
        );
        assert_eq!(role("web/src/app.tsx"), FileRole::Implementation);
        assert_eq!(role("scripts/release.sh"), FileRole::Implementation);
    }

    #[test]
    fn test_directories_and_test_filenames_are_tests() {
        assert_eq!(role("tests/integration.rs"), FileRole::Test);
        assert_eq!(role("web/src/engine.test.ts"), FileRole::Test);
        assert_eq!(role("web/src/engine.spec.ts"), FileRole::Test);
        assert_eq!(role("pkg/engine_test.go"), FileRole::Test);
        assert_eq!(role("api/test_engine.py"), FileRole::Test);
    }

    #[test]
    fn a_test_word_inside_a_longer_name_is_not_a_test_file() {
        assert_eq!(role("src/latest.rs"), FileRole::Implementation);
        assert_eq!(role("src/contest.ts"), FileRole::Implementation);
        assert_eq!(role("src/testing_helpers.rs"), FileRole::Implementation);
    }

    #[test]
    fn support_roles_win_over_the_code_extension() {
        assert_eq!(
            role("crates/okena-syntax/src/fixtures/review.ts"),
            FileRole::Fixture
        );
        assert_eq!(role("src/__snapshots__/view.ts.snap"), FileRole::Snapshot);
        assert_eq!(role("examples/basic/main.rs"), FileRole::Example);
        assert_eq!(role("node_modules/left-pad/index.js"), FileRole::Vendored);
        assert_eq!(role("target/debug/build.rs"), FileRole::Generated);
    }

    #[test]
    fn configuration_covers_extensions_dotfiles_and_well_known_names() {
        assert_eq!(role("Cargo.toml"), FileRole::Configuration);
        assert_eq!(role(".github/workflows/ci.yml"), FileRole::Configuration);
        assert_eq!(role(".gitignore"), FileRole::Configuration);
        assert_eq!(role("Dockerfile"), FileRole::Configuration);
        assert_eq!(role("web/package.json"), FileRole::Configuration);
    }

    #[test]
    fn lockfiles_outrank_the_configuration_extension() {
        assert_eq!(role("Cargo.lock"), FileRole::Lockfile);
        assert_eq!(role("web/pnpm-lock.yaml"), FileRole::Lockfile);
        assert_eq!(role("package-lock.json"), FileRole::Lockfile);
    }

    #[test]
    fn prose_is_documentation_wherever_it_sits() {
        assert_eq!(role("docs/CLAUDE.md"), FileRole::Documentation);
        assert_eq!(role("README.md"), FileRole::Documentation);
        assert_eq!(role("crates/okena-git/CLAUDE.md"), FileRole::Documentation);
        assert_eq!(role("LICENSE"), FileRole::Documentation);
    }

    #[test]
    fn an_unknown_extension_stays_unclassified() {
        assert_eq!(role("assets/icons/logo.svg"), FileRole::Unclassified);
        assert_eq!(role("assets/fonts/Inter.ttf"), FileRole::Unclassified);
    }

    #[test]
    fn classification_is_case_insensitive_and_leading_dot_slash_tolerant() {
        assert_eq!(role("./Tests/Integration.RS"), FileRole::Test);
        assert_eq!(role("CARGO.LOCK"), FileRole::Lockfile);
    }

    #[test]
    fn every_rule_id_has_a_label() {
        for path in [
            "target/x.rs",
            "node_modules/x.js",
            "Cargo.lock",
            "a.snap",
            "fixtures/a.ts",
            "tests/a.rs",
            "README.md",
            "examples/a.rs",
            "Cargo.toml",
            "src/a.rs",
        ] {
            let classification = classify(path);
            assert_ne!(
                rule_label(&classification.rule_id),
                "no rule matched",
                "{path} produced an unlabelled rule",
            );
        }
    }

    #[test]
    fn an_unmatched_path_says_so_rather_than_naming_a_rule() {
        let classification = classify("assets/icons/logo.svg");
        assert_eq!(classification.role, FileRole::Unclassified);
        assert_eq!(rule_label(&classification.rule_id), "no rule matched");
    }

    #[test]
    fn test_directory_vocabulary_matches_the_path_rule() {
        assert!(is_test_directory("tests"));
        assert!(is_test_directory("Spec"));
        assert!(!is_test_directory("src"));
    }
}
