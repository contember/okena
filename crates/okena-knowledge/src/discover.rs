//! Every knowledge root okena can see on this machine.
//!
//! Two sources, merged by canonical path:
//!
//! 1. **Stores** in okena's registry, each checked for a checkout, an identity
//!    that agrees with the registry, and at least something to list.
//! 2. **Projects**: each okena project's `.okena/knowledge.yaml` names the
//!    stores it follows (shown as `used_by`, or a pointer diagnostic when the
//!    store isn't registered here), and its own kind folders become a project
//!    root. A project root that is also a registered store is the store.
//!
//! Git sync state is filled in by the caller; nothing here runs `git`.

use crate::identity::{StoreIdentity, validate_store_id};
use crate::registry::{self, RegisteredStore};
use crate::{canonical, display, project, tree};
use okena_core::knowledge::{
    Diagnostic, KnowledgePointer, KnowledgeRoot, KnowledgeRootKind, KnowledgeStores,
};
use okena_core::specs::{path_root_key, store_root_key};
use std::path::{Path, PathBuf};

/// An okena project to look for knowledge in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectSource {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sources {
    pub registry_path: PathBuf,
    /// Empty when project discovery is turned off.
    pub projects: Vec<ProjectSource>,
}

pub fn discover(sources: &Sources) -> KnowledgeStores {
    let mut out = KnowledgeStores {
        registry_path: display(&sources.registry_path),
        ..Default::default()
    };
    // Canonical checkout paths already claimed by a root.
    let mut seen: Vec<PathBuf> = Vec::new();

    match registry::list(&sources.registry_path) {
        Ok(stores) => {
            for store in stores {
                let real = canonical(&store.root);
                let mut root = inspect_store(&store);
                if seen.contains(&real) {
                    root.healthy = false;
                    root.status.push(
                        Diagnostic::error(
                            "store_path_duplicate",
                            format!(
                                "{} is registered under more than one id.",
                                display(&store.root)
                            ),
                        )
                        .with_fix(format!("Unregister `{}`.", store.id)),
                    );
                } else {
                    seen.push(real);
                }
                out.roots.push(root);
            }
        }
        Err(e) => out.status.push(e.to_diagnostic()),
    }

    let mut projects: Vec<&ProjectSource> = sources.projects.iter().collect();
    projects.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.path.cmp(&b.path)));
    let mut project_roots = Vec::new();
    for source in projects {
        let config = match project::read_config(&source.path) {
            Ok(c) => c,
            Err(e) => {
                out.status.push(in_project(source, e.to_diagnostic()));
                None
            }
        };
        for id in config.iter().flat_map(|c| c.stores.iter()) {
            out.pointers.push(follow(&mut out.roots, source, id));
        }
        match project::project_root(&source.path, config.as_ref()) {
            Ok(Some(dir)) => {
                let real = canonical(&dir);
                if !seen.contains(&real) {
                    seen.push(real);
                    project_roots.push(project_root(source, &dir));
                }
            }
            Ok(None) => {}
            Err(e) => out.status.push(in_project(source, e.to_diagnostic())),
        }
    }
    out.roots.extend(project_roots);
    out
}

fn inspect_store(store: &RegisteredStore) -> KnowledgeRoot {
    let mut root = KnowledgeRoot {
        key: store_root_key(&store.id),
        kind: KnowledgeRootKind::Store,
        name: store.id.clone(),
        path: display(&store.root),
        store_id: Some(store.id.clone()),
        description: None,
        remote: store.remote.clone(),
        healthy: false,
        git: None,
        counts: Default::default(),
        used_by: Vec::new(),
        status: Vec::new(),
    };
    if !store.root.is_dir() {
        root.status.push(
            Diagnostic::error(
                "store_checkout_missing",
                format!("The checkout of `{}` is gone: {}", store.id, root.path),
            )
            .with_fix("Clone it again, or unregister it in Settings → Knowledge."),
        );
        return root;
    }
    match StoreIdentity::read(&store.root) {
        Err(e) => {
            root.status.push(e.to_diagnostic());
            return root;
        }
        Ok(Some(identity)) if identity.id != store.id => {
            root.status.push(
                Diagnostic::error(
                    "store_identity_mismatch",
                    format!(
                        "{} is registered as `{}` but its store.yaml says `{}`.",
                        root.path, store.id, identity.id
                    ),
                )
                .with_fix("Unregister it and add the folder again."),
            );
            return root;
        }
        Ok(Some(identity)) => {
            if let Some(name) = identity.name {
                root.name = name;
            }
            root.description = identity.description;
            root.remote = identity.remote.or(root.remote);
        }
        Ok(None) => {
            if !tree::has_kind_folder(&store.root) {
                root.status.push(
                    Diagnostic::error(
                        "not_a_knowledge_root",
                        format!(
                            "{} has no store identity and none of docs/, skills/, agents/ or templates/.",
                            root.path
                        ),
                    )
                    .with_fix("Unregister it, or add the folders."),
                );
                return root;
            }
            root.status.push(
                Diagnostic::warning(
                    "store_identity_missing",
                    format!(
                        "{} has no .okena-knowledge/store.yaml, so `{}` is only this machine's name for it.",
                        root.path, store.id
                    ),
                )
                .with_fix(format!(
                    "Commit a .okena-knowledge/store.yaml with `version: 1` and `id: {}` so every clone agrees.",
                    store.id
                )),
            );
        }
    }
    root.healthy = true;
    root.counts = tree::count_entries(&store.root);
    root
}

/// Resolve one `stores:` line of a project against the registered stores.
fn follow(roots: &mut [KnowledgeRoot], source: &ProjectSource, id: &str) -> KnowledgePointer {
    let mut pointer = KnowledgePointer {
        project: source.name.clone(),
        path: display(&source.path),
        store_id: id.to_string(),
        root_key: None,
        status: Vec::new(),
    };
    if let Err(e) = validate_store_id(id) {
        pointer.status.push(e.to_diagnostic());
        return pointer;
    }
    let store = roots
        .iter_mut()
        .find(|r| r.kind == KnowledgeRootKind::Store && r.store_id.as_deref() == Some(id));
    match store {
        Some(root) => {
            if !root.used_by.contains(&source.name) {
                root.used_by.push(source.name.clone());
            }
            pointer.root_key = Some(root.key.clone());
        }
        None => pointer.status.push(
            Diagnostic::error(
                "unknown_store",
                format!(
                    "`{}` follows the store `{id}`, which is not on this machine.",
                    source.name
                ),
            )
            .with_fix("Clone it in Settings → Knowledge."),
        ),
    }
    pointer
}

fn project_root(source: &ProjectSource, dir: &Path) -> KnowledgeRoot {
    let path = display(dir);
    let mut status = Vec::new();
    if !tree::has_kind_folder(dir) {
        status.push(
            Diagnostic::warning(
                "no_kind_folders",
                format!("{path} has none of docs/, skills/, agents/ or templates/ yet."),
            )
            .with_fix("Add a docs/ folder with a Markdown file."),
        );
    }
    KnowledgeRoot {
        key: path_root_key(&path),
        kind: KnowledgeRootKind::Project,
        name: source.name.clone(),
        path,
        store_id: None,
        description: None,
        remote: None,
        healthy: true,
        git: None,
        counts: tree::count_entries(dir),
        used_by: Vec::new(),
        status,
    }
}

/// Name the project in a diagnostic that isn't attached to a root.
fn in_project(source: &ProjectSource, mut d: Diagnostic) -> Diagnostic {
    d.message = format!("{}: {}", source.name, d.message);
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{store, write};

    struct Sandbox {
        dir: tempfile::TempDir,
    }

    impl Sandbox {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().expect("tempdir"),
            }
        }
        fn path(&self, rel: &str) -> PathBuf {
            self.dir.path().join(rel)
        }
        fn registry(&self) -> PathBuf {
            registry::registry_path(&self.path("config"))
        }
        fn register(&self, rel: &str) {
            registry::register(&self.registry(), &display(&self.path(rel)), None)
                .expect("register");
        }
        fn project(&self, name: &str, rel: &str) -> ProjectSource {
            ProjectSource {
                name: name.into(),
                path: self.path(rel),
            }
        }
        fn discover(&self, projects: Vec<ProjectSource>) -> KnowledgeStores {
            discover(&Sources {
                registry_path: self.registry(),
                projects,
            })
        }
    }

    fn codes(status: &[Diagnostic]) -> Vec<&str> {
        status.iter().map(|d| d.code.as_str()).collect()
    }

    #[test]
    fn a_registered_store_is_a_healthy_root_with_its_identity_and_counts() {
        let s = Sandbox::new();
        store(&s.path("eng"), "acme-eng");
        write(
            &StoreIdentity::path(&s.path("eng")),
            "version: 1\nid: acme-eng\nname: Acme Engineering\ndescription: How we build\nremote: git@x:acme/eng.git\n",
        );
        write(&s.path("eng/skills/release/SKILL.md"), "x");
        s.register("eng");

        let found = s.discover(Vec::new());
        assert!(found.status.is_empty(), "{:?}", found.status);
        let root = found.root("store:acme-eng").expect("root");
        assert!(root.healthy && root.status.is_empty());
        assert_eq!(root.name, "Acme Engineering");
        assert_eq!(root.description.as_deref(), Some("How we build"));
        assert_eq!(root.remote.as_deref(), Some("git@x:acme/eng.git"));
        assert_eq!((root.counts.docs, root.counts.skills), (1, 1));
        assert_eq!(
            found.default_root().map(|r| r.key.as_str()),
            Some("store:acme-eng")
        );
    }

    #[test]
    fn store_problems_become_diagnostics_on_that_store_only() {
        let s = Sandbox::new();
        store(&s.path("good"), "good");
        store(&s.path("gone"), "gone");
        write(&s.path("loose/docs/a.md"), "x");
        store(&s.path("renamed"), "renamed");
        for rel in ["good", "gone", "loose", "renamed"] {
            s.register(rel);
        }
        std::fs::remove_dir_all(s.path("gone")).expect("rm");
        write(
            &StoreIdentity::path(&s.path("renamed")),
            "version: 1\nid: something-else\n",
        );
        // A registered folder that later loses everything it had.
        std::fs::remove_dir_all(s.path("loose/docs")).expect("rm");

        let found = s.discover(Vec::new());
        let root = |key| found.root(key).expect(key);
        assert!(root("store:good").healthy);
        assert!(!root("store:gone").healthy);
        assert_eq!(
            codes(&root("store:gone").status),
            ["store_checkout_missing"]
        );
        assert!(!root("store:renamed").healthy);
        assert_eq!(
            codes(&root("store:renamed").status),
            ["store_identity_mismatch"]
        );
        assert!(!root("store:loose").healthy);
        assert_eq!(codes(&root("store:loose").status), ["not_a_knowledge_root"]);
    }

    #[test]
    fn a_store_without_identity_is_healthy_with_a_warning() {
        let s = Sandbox::new();
        write(&s.path("team-docs/docs/a.md"), "x");
        s.register("team-docs");
        let found = s.discover(Vec::new());
        let root = found.root("store:team-docs").expect("root");
        assert!(root.healthy);
        assert_eq!(codes(&root.status), ["store_identity_missing"]);
    }

    #[test]
    fn a_corrupt_registry_is_reported_and_projects_still_list() {
        let s = Sandbox::new();
        write(&s.registry(), "version: [\n");
        write(&s.path("repo/.okena/knowledge/docs/a.md"), "x");
        let found = s.discover(vec![s.project("repo", "repo")]);
        assert_eq!(codes(&found.status), ["registry_invalid"]);
        assert_eq!(found.roots.len(), 1);
        assert_eq!(found.roots[0].kind, KnowledgeRootKind::Project);
    }

    #[test]
    fn projects_follow_stores_and_unknown_ones_are_diagnosed() {
        let s = Sandbox::new();
        store(&s.path("eng"), "acme-eng");
        s.register("eng");
        write(
            &s.path("api/.okena/knowledge.yaml"),
            "stores: [acme-eng, platform, Bad_Id]\n",
        );
        write(&s.path("web/.okena/knowledge.yaml"), "stores: [acme-eng]\n");

        let found = s.discover(vec![s.project("web", "web"), s.project("api", "api")]);
        assert_eq!(
            found.root("store:acme-eng").expect("store").used_by,
            ["api", "web"],
            "in project name order"
        );
        let pointer = |project: &str, id: &str| {
            found
                .pointers
                .iter()
                .find(|p| p.project == project && p.store_id == id)
                .expect("pointer")
        };
        assert_eq!(
            pointer("api", "acme-eng").root_key.as_deref(),
            Some("store:acme-eng")
        );
        assert_eq!(codes(&pointer("api", "platform").status), ["unknown_store"]);
        assert_eq!(
            codes(&pointer("api", "Bad_Id").status),
            ["invalid_store_id"]
        );
        assert!(
            found
                .roots
                .iter()
                .all(|r| r.kind == KnowledgeRootKind::Store)
        );
    }

    #[test]
    fn project_roots_come_from_the_default_or_configured_folder() {
        let s = Sandbox::new();
        write(&s.path("a/.okena/knowledge/docs/x.md"), "x");
        write(&s.path("b/.okena/knowledge.yaml"), "root: docs/knowledge\n");
        write(&s.path("b/docs/knowledge/agents/r.md"), "x");
        write(&s.path("c/.okena/knowledge.yaml"), "root: ../a\n");
        write(&s.path("d/.okena/knowledge.yaml"), "stores: {broken\n");
        std::fs::create_dir_all(s.path("e/.okena/knowledge")).expect("mkdir");

        let found = s.discover(
            ["a", "b", "c", "d", "e"]
                .into_iter()
                .map(|n| s.project(n, n))
                .collect(),
        );
        let names: Vec<_> = found.roots.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "e"]);
        let b = &found.roots[1];
        assert_eq!(b.key, path_root_key(&display(&s.path("b/docs/knowledge"))));
        assert_eq!(b.counts.agents, 1);
        assert_eq!(codes(&found.roots[2].status), ["no_kind_folders"]);
        assert_eq!(
            codes(&found.status),
            ["project_root_outside", "project_config_invalid"]
        );
        assert!(found.status[0].message.starts_with("c: "));
    }

    #[test]
    fn a_project_root_that_is_a_registered_store_is_listed_once_as_the_store() {
        let s = Sandbox::new();
        store(&s.path("eng"), "acme-eng");
        s.register("eng");
        write(&s.path("eng/.okena/knowledge.yaml"), "root: .\n");
        let found = s.discover(vec![s.project("eng", "eng")]);
        assert_eq!(found.roots.len(), 1);
        assert_eq!(found.roots[0].kind, KnowledgeRootKind::Store);
    }
}
