//! OpenSpec tree types.
//!
//! Models the conventions of the OpenSpec framework
//! (<https://github.com/Fission-AI/OpenSpec>), which okena reads straight off
//! disk:
//!
//! ```text
//! openspec/
//! ├── specs/                  stable requirements
//! └── changes/                in-flight work, one directory per change
//!     ├── <change>/
//!     │   ├── proposal.md     why, and what changes
//!     │   ├── design.md       technical approach
//!     │   ├── tasks.md        implementation checklist
//!     │   └── specs/          the requirements this change introduces
//!     └── archive/            completed changes
//! ```
//!
//! Deliberately convention-based rather than shelling out to the `openspec`
//! CLI: the layout is plain Markdown in a git repo, so okena can read it
//! whether or not the CLI is installed, and an agent can still use the CLI
//! itself when authoring.

use serde::{Deserialize, Serialize};

/// The artifact files a change directory conventionally holds.
///
/// Ordered as a reader wants them: why, then how, then the checklist.
pub const CHANGE_ARTIFACTS: &[&str] = &["proposal.md", "design.md", "tasks.md"];

/// One document in the spec repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecDoc {
    /// Path relative to the repository root, e.g.
    /// `openspec/changes/add-login/proposal.md`. Relative so it is meaningful
    /// to a client that cannot see the filesystem.
    pub path: String,
    /// File name for display, e.g. `proposal.md`.
    pub name: String,
}

/// An in-flight or archived change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecChange {
    /// Directory name, which is the change's identity in OpenSpec.
    pub name: String,
    pub path: String,
    /// `proposal.md` / `design.md` / `tasks.md` that actually exist. OpenSpec
    /// is explicitly "fluid not rigid", so a change with only a proposal is
    /// normal and must not read as broken.
    pub artifacts: Vec<SpecDoc>,
    /// Requirement documents under the change's own `specs/`.
    pub specs: Vec<SpecDoc>,
    pub archived: bool,
}

/// The spec repository as okena sees it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecTree {
    /// Absolute path of the configured repository, for display.
    #[serde(default)]
    pub root: String,
    /// Whether `openspec/` exists there yet.
    #[serde(default)]
    pub initialized: bool,
    /// Stable requirements under `openspec/specs/`.
    #[serde(default)]
    pub specs: Vec<SpecDoc>,
    /// Active changes, newest directory first.
    #[serde(default)]
    pub changes: Vec<SpecChange>,
    /// Completed changes under `openspec/changes/archive/`.
    #[serde(default)]
    pub archived: Vec<SpecChange>,
}

/// Turn a free-text idea into an OpenSpec change directory name.
///
/// OpenSpec identifies a change by its directory, so the name has to be
/// filesystem-safe and stable. Kept short because it becomes a path segment
/// that appears in every artifact reference.
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
    use super::{CHANGE_ARTIFACTS, change_slug};

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
    fn artifacts_are_ordered_why_then_how_then_work() {
        assert_eq!(CHANGE_ARTIFACTS, ["proposal.md", "design.md", "tasks.md"]);
    }
}
