#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

//! Syntax highlighting shared by every viewer that shows code: the file viewer,
//! the diff viewer, and markdown code blocks.
//!
//! It sits below `okena-files` and `okena-markdown` so all three can share one
//! `SyntaxSet` — loading it is expensive and a second copy would be megabytes.

pub mod markdown_highlight;
pub mod styled;
pub mod syntax;
