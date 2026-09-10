//! Rust module gating across files.
//!
//! A file is not only what it contains. `#[cfg(test)] mod fixtures;` in one
//! file makes the whole of `fixtures.rs` test code, and nothing inside that
//! file says so. Resolving it means opening the declaring file — which the
//! comparison may not even have changed.

use std::cell::RefCell;
use std::collections::HashMap;

use okena_syntax::{AnalysisLimits, DocumentSymbols, SymbolFact, SymbolKind, SyntaxLanguage};

use crate::composition::SourceLoader;

/// How far up the module chain to walk before giving up. Deeper than any real
/// crate; the cap only stops a cycle a symlinked path could create.
const MAX_DEPTH: usize = 32;

/// Answers "is this file reached only through a `#[cfg(test)]` module
/// declaration", reading each declaring file at most once.
pub(crate) struct TestGate<'a> {
    loader: &'a dyn SourceLoader,
    limits: AnalysisLimits,
    documents: RefCell<HashMap<(bool, String), Option<DocumentSymbols>>>,
}

impl<'a> TestGate<'a> {
    pub(crate) fn new(loader: &'a dyn SourceLoader, limits: AnalysisLimits) -> Self {
        Self {
            loader,
            limits,
            documents: RefCell::new(HashMap::new()),
        }
    }

    /// Whether an ancestor declares this file's module behind a test gate.
    ///
    /// One side only: a file that moved in or out of a gate mid-branch is
    /// judged by the side it ends on.
    pub(crate) fn is_gated(&self, path: &str, head_side: bool) -> bool {
        self.gated(path, head_side, 0)
    }

    fn gated(&self, path: &str, head_side: bool, depth: usize) -> bool {
        if depth >= MAX_DEPTH {
            return false;
        }
        let Some(module) = ModulePath::of(path) else {
            return false;
        };
        for parent in module.parents {
            let Some(document) = self.document(&parent, head_side) else {
                continue;
            };
            let Some(declaration) = document.symbols.iter().find(|symbol| {
                symbol.kind == SymbolKind::Module && !symbol.has_body && symbol.name == module.name
            }) else {
                continue;
            };
            // The declaring file is found; no other candidate can declare it.
            return declares_tests(declaration) || self.gated(&parent, head_side, depth + 1);
        }
        false
    }

    fn document(&self, path: &str, head_side: bool) -> Option<DocumentSymbols> {
        let key = (head_side, path.to_string());
        if let Some(cached) = self.documents.borrow().get(&key) {
            return cached.clone();
        }
        let source = if head_side {
            self.loader.head(path)
        } else {
            self.loader.base(path)
        };
        let document =
            source.map(|source| okena_syntax::analyze(SyntaxLanguage::Rust, &source, self.limits));
        self.documents.borrow_mut().insert(key, document.clone());
        document
    }
}

/// Whether a module declaration is written behind a test gate.
fn declares_tests(declaration: &SymbolFact) -> bool {
    declaration.has_attribute("test")
        || declaration
            .attributes
            .iter()
            .any(|attribute| crate::composition::cfg_names_test(attribute))
}

/// A Rust file's module name and the files that could declare it.
struct ModulePath {
    name: String,
    parents: Vec<String>,
}

impl ModulePath {
    /// `None` for a crate root, which nothing declares, and for anything that
    /// is not a Rust source file.
    fn of(path: &str) -> Option<Self> {
        let (directory, file) = path.rsplit_once('/').unwrap_or(("", path));
        let stem = file.strip_suffix(".rs")?;
        if matches!(stem, "lib" | "main") {
            return None;
        }
        // `a/b/mod.rs` is module `b`, declared one level further up than
        // `a/b/c.rs`, which is module `c` declared in `a/b`.
        let (name, scope) = if stem == "mod" {
            let (above, directory_name) = directory.rsplit_once('/').unwrap_or(("", directory));
            if directory_name.is_empty() {
                return None;
            }
            (directory_name.to_string(), above.to_string())
        } else {
            (stem.to_string(), directory.to_string())
        };
        Some(Self {
            name,
            parents: parent_candidates(&scope),
        })
    }
}

/// The files that can declare a module living in `scope`, in resolution order.
fn parent_candidates(scope: &str) -> Vec<String> {
    if scope.is_empty() {
        return vec!["lib.rs".to_string(), "main.rs".to_string()];
    }
    vec![
        format!("{scope}/mod.rs"),
        format!("{scope}.rs"),
        format!("{scope}/lib.rs"),
        format!("{scope}/main.rs"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Sources(HashMap<String, String>);

    impl Sources {
        fn with(mut self, path: &str, source: &str) -> Self {
            self.0.insert(path.to_string(), source.to_string());
            self
        }
        fn gate(&self) -> TestGate<'_> {
            TestGate::new(self, AnalysisLimits::default())
        }
    }

    impl SourceLoader for Sources {
        fn head(&self, path: &str) -> Option<String> {
            self.0.get(path).cloned()
        }
        fn base(&self, path: &str) -> Option<String> {
            self.head(path)
        }
    }

    fn names(path: &str) -> (String, Vec<String>) {
        let module = ModulePath::of(path).expect("a module path");
        (module.name, module.parents)
    }

    #[test]
    fn a_plain_file_is_declared_by_its_directory_module() {
        let (name, parents) = names("src/views/panel.rs");
        assert_eq!(name, "panel");
        assert_eq!(parents[0], "src/views/mod.rs");
        assert_eq!(parents[1], "src/views.rs");
    }

    #[test]
    fn a_mod_file_is_declared_one_level_further_up() {
        let (name, parents) = names("src/views/mod.rs");
        assert_eq!(name, "views");
        assert_eq!(parents[0], "src/mod.rs");
        assert_eq!(parents[1], "src.rs");
        assert!(parents.contains(&"src/lib.rs".to_string()));
    }

    #[test]
    fn a_crate_root_has_no_declaring_file() {
        assert!(ModulePath::of("src/lib.rs").is_none());
        assert!(ModulePath::of("src/main.rs").is_none());
        assert!(ModulePath::of("README.md").is_none());
    }

    #[test]
    fn a_gated_declaration_makes_the_whole_file_tests() {
        let sources = Sources::default().with(
            "src/ui/mod.rs",
            "mod panel;\n#[cfg(test)]\npub mod fixtures;\n",
        );
        let gate = sources.gate();
        assert!(gate.is_gated("src/ui/fixtures.rs", true));
        assert!(!gate.is_gated("src/ui/panel.rs", true));
    }

    #[test]
    fn the_gate_is_inherited_from_a_gated_ancestor() {
        let sources = Sources::default()
            .with("src/lib.rs", "#[cfg(test)]\nmod harness;\n")
            .with("src/harness/mod.rs", "mod builders;\n")
            .with("src/harness.rs", "mod builders;\n");
        assert!(sources.gate().is_gated("src/harness/builders.rs", true));
    }

    #[test]
    fn an_ungated_chain_is_not_gated_at_any_depth() {
        let sources = Sources::default()
            .with("src/lib.rs", "mod harness;\n")
            .with("src/harness.rs", "mod builders;\n");
        assert!(!sources.gate().is_gated("src/harness/builders.rs", true));
    }

    #[test]
    fn an_inline_module_with_a_body_is_not_a_declaration() {
        let sources = Sources::default().with(
            "src/ui/mod.rs",
            "#[cfg(test)]\nmod fixtures { pub fn seed() {} }\n",
        );
        // The gate applies to a file the declaration points at; an inline
        // module has no other file to gate.
        assert!(!sources.gate().is_gated("src/ui/fixtures.rs", true));
    }

    #[test]
    fn a_missing_parent_leaves_the_file_ungated() {
        assert!(
            !Sources::default()
                .gate()
                .is_gated("src/ui/fixtures.rs", true)
        );
    }

    #[test]
    fn a_feature_gate_named_after_testing_is_not_a_test_gate() {
        let sources = Sources::default().with(
            "src/ui/mod.rs",
            "#[cfg(feature = \"testing\")]\npub mod fixtures;\n",
        );
        assert!(!sources.gate().is_gated("src/ui/fixtures.rs", true));
    }

    #[test]
    fn every_path_is_read_at_most_once() {
        let sources = Sources::default().with(
            "src/ui/mod.rs",
            "#[cfg(test)]\npub mod fixtures;\nmod panel;\n",
        );
        let gate = sources.gate();
        assert!(gate.is_gated("src/ui/fixtures.rs", true));
        assert!(!gate.is_gated("src/ui/panel.rs", true));
        let reads = gate.documents.borrow().len();

        assert!(gate.is_gated("src/ui/fixtures.rs", true));
        assert!(!gate.is_gated("src/ui/panel.rs", true));
        assert_eq!(gate.documents.borrow().len(), reads);
        // Misses are cached too, so a directory with no declaring file is not
        // re-probed for every file under it.
        assert!(reads > 1);
    }
}
