//! Knowledge stores on disk.
//!
//! A knowledge store is a git repository of an organisation's engineering
//! knowledge — docs, skills, agents and prompt templates in kind folders — and a
//! project repo can carry its own and follow stores through
//! `.okena/knowledge.yaml`. The format is okena's own (ADR-0003,
//! `docs/reference/knowledge.md`); the wire shapes live in
//! `okena_core::knowledge`.
//!
//! - [`identity`]: a store's committed `.okena-knowledge/store.yaml`;
//! - [`project`]: a project's `.okena/knowledge.yaml`;
//! - [`registry`]: okena's per-profile list of store checkouts;
//! - [`tree`]: the entries inside one root;
//! - [`discover`]: every root on this machine, with health;
//! - [`git`]: sync state, clone, fetch and fast-forward pull;
//! - [`setup`]: creating a new store.
//!
//! Every problem that doesn't stop a listing becomes a diagnostic on the thing
//! it concerns, so one broken checkout never hides the rest.

pub mod discover;
pub mod frontmatter;
pub mod git;
pub mod identity;
pub mod project;
pub mod registry;
pub mod setup;
pub mod tree;

use okena_core::knowledge::Diagnostic;

/// A failed knowledge operation, with a stable code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeError {
    pub code: &'static str,
    pub message: String,
    /// A concrete next step.
    pub fix: Option<String>,
}

impl KnowledgeError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            fix: None,
        }
    }

    pub(crate) fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }

    pub fn to_diagnostic(&self) -> Diagnostic {
        let d = Diagnostic::error(self.code, self.message.clone());
        match &self.fix {
            Some(fix) => d.with_fix(fix.clone()),
            None => d,
        }
    }
}

impl std::fmt::Display for KnowledgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.fix {
            Some(fix) => write!(f, "{} — {}", self.message, fix),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for KnowledgeError {}

/// A path for messages and the wire.
pub(crate) fn display(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(crate) use okena_core::fs::canonical;

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::Path;

    pub fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, content).expect("write");
    }

    /// A store root with identity `id` and one doc.
    pub fn store(root: &Path, id: &str) {
        write(
            &root.join(".okena-knowledge/store.yaml"),
            &format!("version: 1\nid: {id}\n"),
        );
        write(&root.join("docs/readme.md"), "# Readme\n");
    }
}
