#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

//! Pure review comparison and result models.

pub mod call_diff;
pub mod classification;
mod model;
pub mod structure;

pub use model::{
    AnalysisError, AnalysisStage, CallChangeKind, CallDiffChange, CallPairingEvidence,
    CallPairingStrategy, ChangedHunk, ChangedLineRange, FileAnalysisStatus, LanguageCoverage,
    ModelError, OmittedFileGroup, OmittedFileReason, OutlineFact, ReviewStructure, SignatureChange,
    StructuralHotspot, StructuralMetric, StructuredFile, SymbolChange, SymbolChangeKind,
    SymbolReference,
};

pub use okena_core::review::{
    ComparisonSide, ImmutableResolvedComparison, ReviewCoverage, ReviewNavigationTarget,
    ReviewTruncation,
};
