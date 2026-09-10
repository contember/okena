//! OpenSpec (<https://github.com/Fission-AI/OpenSpec>) on disk.
//!
//! Reads and writes the files the `openspec` CLI does, in the shapes it does,
//! so okena and the CLI can work on one machine's stores side by side:
//!
//! - the machine store registry, `<data>/stores/registry.yaml`, guarded by the
//!   CLI's `registry.yaml.lock` protocol (see [`lock`]);
//! - a store's identity, `.openspec-store/store.yaml`;
//! - a root's `openspec/config.yaml` — schema, `store:` pointer, `references:`;
//! - the global `<config>/config.json`, for `defaultStore`;
//! - the planning tree itself.
//!
//! Behaviour follows OpenSpec 1.13 (`src/core/store`, `root-selection.ts`,
//! `references.ts`), including its diagnostic codes, so a problem okena reports
//! reads the same as what `openspec doctor` says. Nothing here needs the CLI.

pub mod discover;
pub mod files;
mod git;
pub mod lock;
pub mod paths;
pub mod registry;
pub mod root;
pub mod setup;
pub mod tree;

pub use paths::OpenSpecDirs;

use okena_core::specs::SpecDiagnostic;

/// A failed OpenSpec operation, carrying OpenSpec's diagnostic code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenSpecError {
    pub code: &'static str,
    pub message: String,
    /// A concrete next step, often a pasteable `openspec` command.
    pub fix: Option<String>,
}

impl OpenSpecError {
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

    pub fn to_diagnostic(&self) -> SpecDiagnostic {
        let d = SpecDiagnostic::error(self.code, self.message.clone());
        match &self.fix {
            Some(fix) => d.with_fix(fix.clone()),
            None => d,
        }
    }
}

impl std::fmt::Display for OpenSpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.fix {
            Some(fix) => write!(f, "{} — {}", self.message, fix),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for OpenSpecError {}

/// OpenSpec's wording for the store id grammar, used in errors and fixes.
pub(crate) const KEBAB_ID_DESCRIPTION: &str =
    "must be kebab-case with lowercase letters, numbers, and single hyphen separators";

/// Validate a store id the way OpenSpec's `validateStoreId` does.
pub fn validate_store_id(id: &str) -> Result<&str, OpenSpecError> {
    if okena_core::specs::is_kebab_id(id) {
        Ok(id)
    } else {
        Err(OpenSpecError::new(
            "invalid_store_id",
            format!("Store id {KEBAB_ID_DESCRIPTION}."),
        )
        .with_fix("Use a kebab-case id such as team-plans."))
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use crate::OpenSpecDirs;
    use std::path::{Path, PathBuf};

    /// A sandbox with its own OpenSpec data and config directories, so tests
    /// never touch the real registry.
    pub struct Sandbox {
        pub dir: tempfile::TempDir,
        pub dirs: OpenSpecDirs,
    }

    impl Sandbox {
        pub fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let dirs = OpenSpecDirs {
                data_dir: dir.path().join("data/openspec"),
                config_dir: dir.path().join("config/openspec"),
            };
            Self { dir, dirs }
        }

        pub fn path(&self, rel: &str) -> PathBuf {
            self.dir.path().join(rel)
        }
    }

    pub fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, content).expect("write");
    }

    /// A healthy OpenSpec root, shaped like `openspec store setup` leaves it.
    pub fn healthy_root(root: &Path) {
        std::fs::create_dir_all(root.join("openspec/specs")).expect("specs");
        std::fs::create_dir_all(root.join("openspec/changes/archive")).expect("archive");
        write(&root.join("openspec/config.yaml"), "schema: spec-driven\n");
    }

    /// A healthy store root with identity metadata.
    pub fn store_root(root: &Path, id: &str) {
        healthy_root(root);
        write(
            &root.join(".openspec-store/store.yaml"),
            &format!("version: 1\nid: {id}\n"),
        );
    }
}
