//! OpenSpec wire types.
//!
//! Models OpenSpec (<https://github.com/Fission-AI/OpenSpec>) the way okena
//! shows it: a set of *roots* — each one an `openspec/` planning tree — found
//! where OpenSpec itself looks, and the tree inside one root.
//!
//! ```text
//! <root>/
//! ├── .openspec-store/store.yaml          only when the root is a store
//! └── openspec/
//!     ├── config.yaml                     schema, `store:` pointer, `references:`
//!     ├── specs/<capability>/spec.md      capabilities may nest: <area>/<id>
//!     └── changes/
//!         ├── <change>/
//!         │   ├── .openspec.yaml          schema + created date
//!         │   ├── proposal.md             why, and what changes
//!         │   ├── design.md               technical approach
//!         │   ├── tasks.md                implementation checklist
//!         │   └── specs/<capability>/spec.md   the change's delta specs
//!         └── archive/                    completed changes
//! ```
//!
//! Reading and writing live in `okena-openspec`. These are only the shapes that
//! cross the wire, so a client that cannot see the filesystem can render them.

use serde::{Deserialize, Serialize};

/// The artifact files a change directory conventionally holds.
///
/// Ordered as a reader wants them: why, then how, then the checklist.
pub const CHANGE_ARTIFACTS: &[&str] = &["proposal.md", "design.md", "tasks.md"];

/// One document in an OpenSpec root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecDoc {
    /// Path relative to the root, e.g. `openspec/changes/add-login/proposal.md`.
    /// Relative so it is meaningful to a client that cannot see the filesystem.
    pub path: String,
    /// Display name: the file name for an artifact, the capability id (e.g.
    /// `auth` or `platform/session`) for a spec.
    pub name: String,
}

/// An in-flight or archived change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecChange {
    /// Directory name, which is the change's identity in OpenSpec.
    pub name: String,
    pub path: String,
    /// Markdown artifacts that actually exist, conventional ones first. OpenSpec
    /// is explicitly "fluid not rigid" and custom schemas define their own
    /// artifacts, so a change holding only `.openspec.yaml` is normal.
    pub artifacts: Vec<SpecDoc>,
    /// Delta specs under the change's own `specs/`.
    pub specs: Vec<SpecDoc>,
    pub archived: bool,
    /// Workflow schema from `.openspec.yaml`, e.g. `spec-driven`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Creation date from `.openspec.yaml`, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
}

/// The planning tree of one root.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecTree {
    /// The [`SpecRoot::key`] this tree was read from.
    #[serde(default)]
    pub root_key: String,
    /// Absolute path of the root, for display.
    #[serde(default)]
    pub root: String,
    /// Set when the root is a store, so hints can carry `--store <id>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_id: Option<String>,
    /// Whether `openspec/` exists there yet.
    #[serde(default)]
    pub initialized: bool,
    /// Capabilities under `openspec/specs/`, sorted by id.
    #[serde(default)]
    pub specs: Vec<SpecDoc>,
    /// Active changes, most recently touched first.
    #[serde(default)]
    pub changes: Vec<SpecChange>,
    /// Completed changes under `openspec/changes/archive/`, newest first.
    #[serde(default)]
    pub archived: Vec<SpecChange>,
}

/// Where a root was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpecRootKind {
    /// Registered in OpenSpec's machine store registry — what
    /// `openspec store list` shows and `--store <id>` selects.
    Store,
    /// An okena project whose repository holds its own planning tree.
    Project,
    /// A folder added in okena's settings. Not known to the `openspec` CLI.
    Folder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpecSeverity {
    Error,
    Warning,
}

/// A problem with a root, a reference or the registry.
///
/// `code` reuses OpenSpec's own diagnostic codes where one exists
/// (`unknown_store`, `reference_unresolved`, …), so what okena reports matches
/// what `openspec doctor` says about the same setup.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecDiagnostic {
    pub severity: SpecSeverity,
    pub code: String,
    pub message: String,
    /// A concrete next step, often a pasteable command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl SpecDiagnostic {
    pub fn error(code: &str, message: impl Into<String>) -> Self {
        Self {
            severity: SpecSeverity::Error,
            code: code.to_string(),
            message: message.into(),
            fix: None,
        }
    }

    pub fn warning(code: &str, message: impl Into<String>) -> Self {
        Self {
            severity: SpecSeverity::Warning,
            code: code.to_string(),
            message: message.into(),
            fix: None,
        }
    }

    pub fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }
}

/// A store a root's `openspec/config.yaml` declares under `references:` —
/// read-only upstream context for the root's work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecReference {
    pub id: String,
    /// Clone source the declaration names, used for the onboarding hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// The registered checkout, when the reference resolves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<SpecDiagnostic>,
}

/// One OpenSpec root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecRoot {
    /// Stable identity a client sends back to name this root: `store:<id>` for
    /// a registered store, `path:<absolute path>` otherwise. The daemon only
    /// accepts keys it discovered itself, so a key cannot name an arbitrary
    /// directory.
    pub key: String,
    pub kind: SpecRootKind,
    /// Store id, project name, or folder name.
    pub name: String,
    pub path: String,
    /// The store id from `.openspec-store/store.yaml`. Also set on a folder
    /// that is a store checkout nobody has registered yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_id: Option<String>,
    /// Canonical clone source: the store's own metadata, else the registry's
    /// observed git origin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// Workflow schema from `openspec/config.yaml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Usable as a root. Problems that don't stop reading are warnings in
    /// `status` on a healthy root.
    pub healthy: bool,
    /// This store is OpenSpec's machine-wide `defaultStore`.
    #[serde(default)]
    pub is_default: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<SpecReference>,
    /// okena projects that resolve to this root through a `store:` pointer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub used_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<SpecDiagnostic>,
}

/// An okena project whose `openspec/config.yaml` points at a store with
/// `store:` instead of holding a planning tree of its own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecPointer {
    pub project: String,
    pub path: String,
    pub store_id: String,
    /// The root the pointer resolves to, when it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_key: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<SpecDiagnostic>,
}

/// Everything okena discovered about OpenSpec on this machine.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecStores {
    /// OpenSpec's store registry file, whether or not it exists yet.
    #[serde(default)]
    pub registry_path: String,
    /// OpenSpec's global config file, which holds `defaultStore`.
    #[serde(default)]
    pub config_path: String,
    /// OpenSpec's machine-wide `defaultStore`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_store: Option<String>,
    #[serde(default)]
    pub roots: Vec<SpecRoot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pointers: Vec<SpecPointer>,
    /// Problems not tied to one root: an unreadable registry, a stale
    /// `defaultStore`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<SpecDiagnostic>,
}

impl SpecStores {
    pub fn root(&self, key: &str) -> Option<&SpecRoot> {
        self.roots.iter().find(|r| r.key == key)
    }

    /// The root to open when nobody has picked one.
    ///
    /// Follows OpenSpec's own fallback — the machine `defaultStore` — and then
    /// prefers a store over a project checkout, because a store is where
    /// cross-repo planning is meant to live.
    pub fn default_root(&self) -> Option<&SpecRoot> {
        self.roots
            .iter()
            .find(|r| r.is_default && r.healthy)
            .or_else(|| {
                self.roots
                    .iter()
                    .find(|r| r.kind == SpecRootKind::Store && r.healthy)
            })
            .or_else(|| self.roots.iter().find(|r| r.healthy))
            .or_else(|| self.roots.first())
    }
}

/// Serde default for store setup's `init_git`: on, as `openspec store setup`
/// defaults to.
pub(crate) fn default_init_git() -> bool {
    true
}

/// Root key for a registered store.
pub fn store_root_key(id: &str) -> String {
    format!("store:{id}")
}

/// Root key for a root known by its path.
pub fn path_root_key(path: &str) -> String {
    format!("path:{path}")
}

/// OpenSpec's id grammar for stores and changes: lowercase letters and digits
/// in runs separated by single hyphens.
pub fn is_kebab_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && !id.ends_with('-')
        && !id.contains("--")
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Turn a free-text idea into an OpenSpec change directory name.
///
/// OpenSpec identifies a change by its directory, so the name has to be
/// filesystem-safe and stable. Kept short because it becomes a path segment
/// that appears in every artifact reference. The result always satisfies
/// [`is_kebab_id`] (or is empty).
pub fn change_slug(idea: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in idea.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out.truncate(48);
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_is_filesystem_safe() {
        assert_eq!(
            change_slug("Add login with Google/Apple!"),
            "add-login-with-google-apple"
        );
    }

    #[test]
    fn slug_collapses_runs_and_trims() {
        assert_eq!(change_slug("  a   b  "), "a-b");
    }

    #[test]
    fn slug_truncates_without_a_trailing_separator() {
        let s = change_slug(&"word ".repeat(30));
        assert!(s.len() <= 48, "got {}", s.len());
        assert!(!s.ends_with('-'));
    }

    #[test]
    fn slug_of_nothing_usable_is_empty() {
        // The caller must notice and ask for a better idea rather than
        // creating a directory named `-`.
        assert_eq!(change_slug("!!!"), "");
        assert_eq!(change_slug(""), "");
    }

    #[test]
    fn slugs_are_valid_openspec_ids() {
        // `openspec validate` rejects a change whose directory is not kebab
        // case, so everything okena scaffolds must pass the same grammar.
        for idea in [
            "Add login",
            "  x__y  ",
            "v2 API: rate limits",
            "ÄÖ résumé 42",
        ] {
            let s = change_slug(idea);
            assert!(is_kebab_id(&s), "{idea:?} -> {s:?}");
        }
    }

    #[test]
    fn kebab_ids_follow_openspec_grammar() {
        for ok in ["team-plans", "api-v2", "a", "0"] {
            assert!(is_kebab_id(ok), "{ok}");
        }
        for bad in [
            "", "My_Store", "MyStore", "my store", "-a", "a-", "a--b", "a/b", "..",
        ] {
            assert!(!is_kebab_id(bad), "{bad}");
        }
    }

    #[test]
    fn artifacts_are_ordered_why_then_how_then_work() {
        assert_eq!(CHANGE_ARTIFACTS, ["proposal.md", "design.md", "tasks.md"]);
    }

    fn root(key: &str, kind: SpecRootKind, healthy: bool, is_default: bool) -> SpecRoot {
        SpecRoot {
            key: key.into(),
            kind,
            name: key.into(),
            path: format!("/{key}"),
            store_id: None,
            remote: None,
            schema: None,
            healthy,
            is_default,
            references: Vec::new(),
            used_by: Vec::new(),
            status: Vec::new(),
        }
    }

    #[test]
    fn the_default_store_wins_when_it_is_usable() {
        let stores = SpecStores {
            roots: vec![
                root("path:/p", SpecRootKind::Project, true, false),
                root("store:a", SpecRootKind::Store, true, false),
                root("store:b", SpecRootKind::Store, true, true),
            ],
            ..Default::default()
        };
        assert_eq!(stores.default_root().unwrap().key, "store:b");
    }

    #[test]
    fn a_broken_default_falls_back_to_a_healthy_store_then_anything_healthy() {
        // A stale `defaultStore` must not strand the view on a root it cannot
        // read when a working one is right there.
        let mut stores = SpecStores {
            roots: vec![
                root("path:/p", SpecRootKind::Project, true, false),
                root("store:a", SpecRootKind::Store, true, false),
                root("store:b", SpecRootKind::Store, false, true),
            ],
            ..Default::default()
        };
        assert_eq!(stores.default_root().unwrap().key, "store:a");
        stores.roots.remove(1);
        assert_eq!(stores.default_root().unwrap().key, "path:/p");
    }

    #[test]
    fn a_tree_from_an_older_daemon_still_decodes() {
        // Older daemons sent no root key or store id.
        let t: SpecTree =
            serde_json::from_str(r#"{"root":"/r","initialized":true}"#).expect("decode");
        assert_eq!(t.root, "/r");
        assert!(t.root_key.is_empty() && t.store_id.is_none());
    }
}
