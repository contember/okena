use serde::{Deserialize, Serialize};

/// What a declaration is. Language-neutral; adapters map their own node kinds
/// onto it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Module,
    Function,
    Method,
    Struct,
    Enum,
    Union,
    Trait,
    Impl,
    Class,
    Interface,
    TypeAlias,
    Constant,
    /// A `describe` / `suite` block — a scope that groups tests.
    TestGroup,
}

/// How far a declaration reaches. `Crate` covers Rust's `pub(crate)` and
/// `pub(super)`; nothing downstream distinguishes them yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolVisibility {
    Public,
    Crate,
    Private,
}

/// One declaration, with the scopes it sits in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolFact {
    pub kind: SymbolKind,
    pub name: String,
    /// Enclosing declaration names, outermost first.
    pub scope: Vec<String>,
    pub visibility: SymbolVisibility,
    /// One-based inclusive line span of the whole declaration.
    pub start_line: u32,
    pub end_line: u32,
    /// Whether the declaration carries a body. False for a Rust `mod foo;`
    /// that names a module living in another file, and for a trait's
    /// signature-only methods.
    pub has_body: bool,
    /// Attributes written on the declaration, normalized without `#[]` —
    /// `cfg(test)`, `test`, `tokio::test`. Empty for languages without them.
    pub attributes: Vec<String>,
}

impl SymbolFact {
    /// `module::Type::method`.
    pub fn qualified(&self) -> String {
        let mut parts = self.scope.clone();
        parts.push(self.name.clone());
        parts.join("::")
    }

    /// Whether any attribute is `name` or `name(...)` — `cfg(test)` matches
    /// `cfg`, and `tokio::test` matches `test` on its last segment.
    pub fn has_attribute(&self, name: &str) -> bool {
        self.attributes.iter().any(|attribute| {
            let head = attribute
                .split_once('(')
                .map_or(attribute.as_str(), |(head, _)| head);
            head.rsplit("::").next().unwrap_or(head).trim() == name
        })
    }
}

/// Why an analysis stopped short of the whole document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Truncation {
    FileTooLarge {
        bytes: usize,
        limit: usize,
    },
    TooManySymbols {
        limit: usize,
    },
    /// tree-sitter could not build a tree at all.
    ParseFailed,
}

/// What one document yielded.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentSymbols {
    pub symbols: Vec<SymbolFact>,
    /// `None` when the whole document was read.
    pub truncation: Option<Truncation>,
}

impl DocumentSymbols {
    pub(crate) fn truncated(truncation: Truncation) -> Self {
        Self {
            symbols: Vec::new(),
            truncation: Some(truncation),
        }
    }

    /// Whether the extraction reached the end of the document.
    pub fn is_complete(&self) -> bool {
        self.truncation.is_none()
    }
}

/// Ceilings that keep one document's analysis bounded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnalysisLimits {
    pub max_bytes: usize,
    pub max_symbols: usize,
}

impl Default for AnalysisLimits {
    fn default() -> Self {
        Self {
            max_bytes: 1 << 20,
            max_symbols: 5_000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(name: &str, scope: &[&str], attributes: &[&str]) -> SymbolFact {
        SymbolFact {
            kind: SymbolKind::Function,
            name: name.to_string(),
            scope: scope.iter().map(|part| part.to_string()).collect(),
            visibility: SymbolVisibility::Private,
            start_line: 1,
            end_line: 2,
            has_body: true,
            attributes: attributes.iter().map(|part| part.to_string()).collect(),
        }
    }

    #[test]
    fn qualified_joins_scope_and_name() {
        assert_eq!(
            fact("run", &["engine", "Runner"], &[]).qualified(),
            "engine::Runner::run"
        );
        assert_eq!(fact("run", &[], &[]).qualified(), "run");
    }

    #[test]
    fn attribute_matching_ignores_arguments_and_paths() {
        let symbol = fact("case", &[], &["cfg(test)", "tokio::test"]);
        assert!(symbol.has_attribute("cfg"));
        assert!(symbol.has_attribute("test"));
        assert!(!symbol.has_attribute("cfg(test)"));
        assert!(!symbol.has_attribute("ignore"));
    }

    #[test]
    fn a_document_with_no_truncation_is_complete() {
        assert!(DocumentSymbols::default().is_complete());
        assert!(!DocumentSymbols::truncated(Truncation::ParseFailed).is_complete());
    }
}
