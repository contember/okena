//! Concrete task providers.
//!
//! Each provider translates one task-manager platform into the neutral types in
//! [`crate::task`]. Adding Jira or Azure DevOps means adding a module here and
//! implementing [`crate::provider::TaskProvider`] — nothing outside this
//! directory needs to change.

pub mod linear;

pub use linear::LinearProvider;
