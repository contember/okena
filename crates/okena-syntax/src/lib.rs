#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

//! GPUI-free syntax facts shared by review and navigation features.

mod language;
mod model;

// Wave 1 adapters own separate directories so they can be implemented independently.
pub mod rust;
pub mod typescript;

pub use language::SyntaxLanguage;
pub use model::{
    AnalysisBudget, AnalysisControl, AnalysisInput, CallFact, CaptureByteTracker, ControlContext,
    DiagnosticSeverity, DocumentStatus, DocumentStructure, ModelError, SourceRange, SymbolFact,
    SymbolKey, SymbolKind, SymbolVisibility, SyntaxAdapter, SyntaxDiagnostic, SyntaxProvenance,
    SyntaxTruncation, SyntaxTruncationReason,
};
