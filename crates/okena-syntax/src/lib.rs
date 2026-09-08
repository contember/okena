#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

//! Structural facts from source code, extracted with tree-sitter.
//!
//! Declarations only — name, kind, visibility, line span, attributes. Sibling
//! of `okena-highlight`, which colours source; this one describes its shape.

mod language;
mod model;
mod rust;
mod typescript;

pub use language::SyntaxLanguage;
pub use model::{
    AnalysisLimits, DocumentSymbols, SymbolFact, SymbolKind, SymbolVisibility, Truncation,
};

/// Extract declarations from one document.
///
/// Never fails: a source tree-sitter cannot parse yields whatever it did reach,
/// with the shortfall recorded in [`DocumentSymbols::truncation`].
pub fn analyze(language: SyntaxLanguage, source: &str, limits: AnalysisLimits) -> DocumentSymbols {
    if source.len() > limits.max_bytes {
        return DocumentSymbols::truncated(Truncation::FileTooLarge {
            bytes: source.len(),
            limit: limits.max_bytes,
        });
    }
    match language {
        SyntaxLanguage::Rust => rust::analyze(source, limits),
        SyntaxLanguage::TypeScript | SyntaxLanguage::Tsx => {
            typescript::analyze(language, source, limits)
        }
    }
}
