#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

//! What a git comparison is made of.
//!
//! Two steps, both GPUI-free and both deterministic: classify every changed
//! file by path, then measure how much of the implementation volume is tests
//! written inside implementation files.

pub mod classification;
pub mod composition;
mod modules;

pub use classification::{classify, is_test_directory, rule_label};
pub use composition::{ChangedFile, CompositionLimits, SourceLoader, compose};
