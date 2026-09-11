//! Knowledge store wire types.
//!
//! A knowledge store is a git repository of engineering knowledge that sits
//! above any one repo — principles, processes, architecture notes — plus the
//! skills, subagents and prompt templates an organisation shares. Layout
//! (ADR-0003, `docs/reference/knowledge.md`):
//!
//! ```text
//! <store>/
//! ├── .okena-knowledge/store.yaml   version: 1, id, name?, description?, remote?
//! ├── docs/**/*.md                  frontmatter: title? description? tags?
//! ├── skills/**/SKILL.md            Agent Skills format, plus supporting files
//! ├── agents/**/*.md                Claude Code subagent format
//! └── templates/**/*.md             frontmatter `for:` flows; `{placeholder}` body
//! ```
//!
//! A project repo joins through `.okena/knowledge.yaml`: `stores:` names the
//! stores it follows, `root:` where its own kind folders live.
//!
//! Reading, discovery and git live in `okena-knowledge`. These are only the
//! shapes that cross the wire, so a client that cannot see the filesystem can
//! render them. Root keys use the same shapes as OpenSpec's
//! ([`crate::specs::store_root_key`], [`crate::specs::path_root_key`]).

use serde::{Deserialize, Serialize};

pub use crate::diagnostic::{Diagnostic, Severity};

/// What an entry is, which is where it lives in a root.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeKind {
    Doc,
    Skill,
    Agent,
    Template,
}

impl KnowledgeKind {
    /// Every kind, in the order a reader wants them: prose first, then what an
    /// agent is handed.
    pub const fn all() -> [KnowledgeKind; 4] {
        [
            KnowledgeKind::Doc,
            KnowledgeKind::Skill,
            KnowledgeKind::Agent,
            KnowledgeKind::Template,
        ]
    }

    /// The folder under a root that holds this kind.
    pub const fn folder(self) -> &'static str {
        match self {
            KnowledgeKind::Doc => "docs",
            KnowledgeKind::Skill => "skills",
            KnowledgeKind::Agent => "agents",
            KnowledgeKind::Template => "templates",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            KnowledgeKind::Doc => "Docs",
            KnowledgeKind::Skill => "Skills",
            KnowledgeKind::Agent => "Agents",
            KnowledgeKind::Template => "Templates",
        }
    }
}

/// One document, skill, agent or template in a root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    pub kind: KnowledgeKind,
    /// Path of the entry's file relative to the root, e.g.
    /// `docs/ci/pipeline.md` or `skills/release/SKILL.md`. Together with the
    /// store id this addresses the entry — see [`KnowledgeRef`].
    pub path: String,
    /// Identity within its kind: a skill's or agent's frontmatter `name`, else
    /// its path under the kind folder without the extension (`ci/pipeline`,
    /// `release`).
    pub name: String,
    /// What to show: frontmatter `title`, else the first `#` heading, else the
    /// name.
    pub title: String,
    /// Frontmatter `description` — what a person or an agent picks an entry by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// For a skill: the other files in its directory, relative to the root.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// For a template: the launch flows its frontmatter `for:` names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flows: Vec<String>,
    /// For a template: the `{placeholder}` names its body uses, sorted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<String>,
    /// Problems that still let the entry be listed, e.g. unparseable
    /// frontmatter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<Diagnostic>,
}

/// Every entry in one root.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeTree {
    /// The [`KnowledgeRoot::key`] this tree was read from.
    #[serde(default)]
    pub root_key: String,
    /// Absolute path of the root, for display.
    #[serde(default)]
    pub root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_id: Option<String>,
    /// Sorted by kind, then path.
    #[serde(default)]
    pub entries: Vec<KnowledgeEntry>,
    /// Problems with the tree as a whole, e.g. the entry cap was reached.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<Diagnostic>,
}

impl KnowledgeTree {
    pub fn entry(&self, path: &str) -> Option<&KnowledgeEntry> {
        self.entries.iter().find(|e| e.path == path)
    }
}

/// Where a root was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeRootKind {
    /// A checkout registered in okena's knowledge registry.
    Store,
    /// An okena project's own kind folders (`.okena/knowledge.yaml` `root:`).
    Project,
}

/// A checkout's sync state, from local git only — nothing here touches the
/// network, so it is only as fresh as the last fetch ([`Self::fetched_at`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeGitStatus {
    /// Checked-out branch; `None` when HEAD is detached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The branch's upstream, e.g. `origin/main`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    #[serde(default)]
    pub ahead: u32,
    #[serde(default)]
    pub behind: u32,
    /// Uncommitted changes, staged or not, including untracked files.
    #[serde(default)]
    pub dirty: bool,
    /// When the checkout last fetched, in Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<u64>,
}

impl KnowledgeGitStatus {
    /// Whether a fast-forward pull has anything to do and can do it: there is
    /// an upstream, it has commits we lack, and we have none it lacks. Pull
    /// refuses anything else, so the view disables it rather than offering a
    /// button that can only fail.
    pub fn can_fast_forward(&self) -> bool {
        self.upstream.is_some() && self.behind > 0 && self.ahead == 0
    }
}

/// How many entries of each kind a root holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeCounts {
    #[serde(default)]
    pub docs: u32,
    #[serde(default)]
    pub skills: u32,
    #[serde(default)]
    pub agents: u32,
    #[serde(default)]
    pub templates: u32,
}

impl KnowledgeCounts {
    pub fn of(&self, kind: KnowledgeKind) -> u32 {
        match kind {
            KnowledgeKind::Doc => self.docs,
            KnowledgeKind::Skill => self.skills,
            KnowledgeKind::Agent => self.agents,
            KnowledgeKind::Template => self.templates,
        }
    }

    pub fn total(&self) -> u32 {
        self.docs + self.skills + self.agents + self.templates
    }
}

/// One knowledge root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeRoot {
    /// Stable identity a client sends back to name this root: `store:<id>` for
    /// a registered store, `path:<absolute path>` for a project root. The
    /// daemon only accepts keys it discovered itself, so a key cannot name an
    /// arbitrary directory.
    pub key: String,
    pub kind: KnowledgeRootKind,
    /// Store name (else id), or project name.
    pub name: String,
    pub path: String,
    /// The store id. For a checkout without `.okena-knowledge/store.yaml` this
    /// is derived from its folder name, with a warning in `status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Canonical clone source: the store's own metadata, else the registry's
    /// observed git origin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// Usable as a root. Problems that don't stop reading are warnings in
    /// `status` on a healthy root.
    pub healthy: bool,
    /// Sync state, when the root is the top of a git checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<KnowledgeGitStatus>,
    #[serde(default)]
    pub counts: KnowledgeCounts,
    /// okena projects whose `.okena/knowledge.yaml` follows this store.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub used_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<Diagnostic>,
}

/// An okena project that follows a store through `.okena/knowledge.yaml`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgePointer {
    pub project: String,
    pub path: String,
    pub store_id: String,
    /// The root the pointer resolves to, when the store is registered here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_key: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<Diagnostic>,
}

/// Everything okena discovered about knowledge on this machine.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeStores {
    /// okena's knowledge registry file, whether or not it exists yet.
    #[serde(default)]
    pub registry_path: String,
    #[serde(default)]
    pub roots: Vec<KnowledgeRoot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pointers: Vec<KnowledgePointer>,
    /// Problems not tied to one root, e.g. an unreadable registry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<Diagnostic>,
}

impl KnowledgeStores {
    pub fn root(&self, key: &str) -> Option<&KnowledgeRoot> {
        self.roots.iter().find(|r| r.key == key)
    }

    /// The root to open when nobody has picked one: a healthy store, because
    /// shared knowledge is what the view is for, then any healthy root, then
    /// whatever exists so its problems are on screen.
    pub fn default_root(&self) -> Option<&KnowledgeRoot> {
        self.roots
            .iter()
            .find(|r| r.kind == KnowledgeRootKind::Store && r.healthy)
            .or_else(|| self.roots.iter().find(|r| r.healthy))
            .or_else(|| self.roots.first())
    }
}

/// A document read from a root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeDocument {
    pub root_key: String,
    /// Relative to the root, as sent.
    pub path: String,
    pub content: String,
}

/// Where an entry lives, independent of any one machine's checkout path: the
/// store id plus the path relative to the store root. What a launch flow will
/// name when it hands an agent knowledge.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KnowledgeRef {
    pub store: String,
    pub path: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(key: &str, kind: KnowledgeRootKind, healthy: bool) -> KnowledgeRoot {
        KnowledgeRoot {
            key: key.into(),
            kind,
            name: key.into(),
            path: format!("/{key}"),
            store_id: None,
            description: None,
            remote: None,
            healthy,
            git: None,
            counts: KnowledgeCounts::default(),
            used_by: Vec::new(),
            status: Vec::new(),
        }
    }

    fn entry(kind: KnowledgeKind, path: &str) -> KnowledgeEntry {
        KnowledgeEntry {
            kind,
            path: path.into(),
            name: path.into(),
            title: path.into(),
            description: None,
            tags: Vec::new(),
            files: Vec::new(),
            flows: Vec::new(),
            variables: Vec::new(),
            status: Vec::new(),
        }
    }

    #[test]
    fn kind_folders_are_the_adr_0003_layout() {
        // These names are the on-disk contract store authors write against.
        let folders: Vec<_> = KnowledgeKind::all().iter().map(|k| k.folder()).collect();
        assert_eq!(folders, ["docs", "skills", "agents", "templates"]);
    }

    #[test]
    fn default_root_prefers_a_healthy_store_then_anything_healthy() {
        let mut stores = KnowledgeStores {
            roots: vec![
                root("store:broken", KnowledgeRootKind::Store, false),
                root("path:/p", KnowledgeRootKind::Project, true),
                root("store:org", KnowledgeRootKind::Store, true),
            ],
            ..Default::default()
        };
        assert_eq!(stores.default_root().unwrap().key, "store:org");
        // A broken store is not preferred over a working project root.
        stores.roots.pop();
        assert_eq!(stores.default_root().unwrap().key, "path:/p");
        // Nothing healthy still opens something, so its problems show.
        stores.roots.pop();
        assert_eq!(stores.default_root().unwrap().key, "store:broken");
        stores.roots.clear();
        assert!(stores.default_root().is_none());
    }

    #[test]
    fn fast_forward_needs_an_upstream_and_only_incoming_commits() {
        let status = |upstream: bool, ahead, behind| KnowledgeGitStatus {
            upstream: upstream.then(|| "origin/main".to_string()),
            ahead,
            behind,
            ..Default::default()
        };
        assert!(status(true, 0, 3).can_fast_forward());
        assert!(!status(true, 0, 0).can_fast_forward(), "nothing to pull");
        assert!(!status(true, 1, 3).can_fast_forward(), "diverged");
        assert!(!status(false, 0, 3).can_fast_forward(), "no upstream");
    }

    #[test]
    fn stores_and_trees_round_trip() {
        let mut store = root("store:acme-eng", KnowledgeRootKind::Store, true);
        store.store_id = Some("acme-eng".into());
        store.remote = Some("git@github.com:acme/eng-knowledge.git".into());
        store.git = Some(KnowledgeGitStatus {
            branch: Some("main".into()),
            upstream: Some("origin/main".into()),
            ahead: 0,
            behind: 3,
            dirty: true,
            fetched_at: Some(1_757_500_000),
        });
        store.counts = KnowledgeCounts {
            docs: 12,
            skills: 2,
            agents: 1,
            templates: 3,
        };
        store.used_by = vec!["okena".into()];
        store.status = vec![Diagnostic::warning("store_identity_missing", "m").with_fix("f")];
        let stores = KnowledgeStores {
            registry_path: "/cfg/knowledge/stores.yaml".into(),
            roots: vec![store, root("path:/p", KnowledgeRootKind::Project, true)],
            pointers: vec![KnowledgePointer {
                project: "okena".into(),
                path: "/p/okena".into(),
                store_id: "acme-eng".into(),
                root_key: Some("store:acme-eng".into()),
                status: Vec::new(),
            }],
            status: vec![Diagnostic::error("registry_unreadable", "m")],
        };
        let json = serde_json::to_string(&stores).expect("encode");
        assert_eq!(
            serde_json::from_str::<KnowledgeStores>(&json).expect("decode"),
            stores
        );

        let mut skill = entry(KnowledgeKind::Skill, "skills/release/SKILL.md");
        skill.files = vec!["skills/release/checklist.md".into()];
        let mut template = entry(KnowledgeKind::Template, "templates/task-start.md");
        template.flows = vec!["task-start".into()];
        template.variables = vec!["key".into(), "title".into()];
        let mut doc = entry(KnowledgeKind::Doc, "docs/ci/pipeline.md");
        doc.description = Some("How CI runs".into());
        doc.tags = vec!["ci".into()];
        doc.status = vec![Diagnostic::warning("frontmatter_invalid", "m")];
        let tree = KnowledgeTree {
            root_key: "store:acme-eng".into(),
            root: "/k/eng".into(),
            store_id: Some("acme-eng".into()),
            entries: vec![
                doc,
                skill,
                entry(KnowledgeKind::Agent, "agents/reviewer.md"),
                template,
            ],
            status: Vec::new(),
        };
        let json = serde_json::to_string(&tree).expect("encode");
        let back: KnowledgeTree = serde_json::from_str(&json).expect("decode");
        assert_eq!(back, tree);
        assert_eq!(
            back.entry("agents/reviewer.md").map(|e| e.kind),
            Some(KnowledgeKind::Agent)
        );
    }

    #[test]
    fn a_minimal_entry_decodes() {
        // Optional fields are skipped when empty, so the smallest entry on the
        // wire must still decode.
        let e: KnowledgeEntry =
            serde_json::from_str(r#"{"kind":"doc","path":"docs/a.md","name":"a","title":"A"}"#)
                .expect("decode");
        assert_eq!(e, {
            let mut want = entry(KnowledgeKind::Doc, "docs/a.md");
            want.name = "a".into();
            want.title = "A".into();
            want
        });
    }
}
