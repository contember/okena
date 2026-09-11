//! okena's knowledge store registry: `<profile config dir>/knowledge/stores.yaml`.
//!
//! ```yaml
//! version: 1
//! stores:
//!   acme-eng:
//!     path: /Users/me/knowledge/eng-knowledge
//!     remote: git@github.com:acme/eng-knowledge.git   # observed origin
//! ```
//!
//! Checkout paths are machine state, not preferences, so they live here rather
//! than in `settings.json` (ADR-0003). The daemon is the only writer — it holds
//! the profile's instance lock — so an in-process mutex is all the locking
//! writes need. One checkout per id, one id per checkout.

use crate::identity::{self, StoreIdentity};
use crate::{KnowledgeError, canonical, display, tree};
use okena_core::fs::{expand_home, write_atomically};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const REGISTRY_VERSION: u32 = 1;

/// Where a profile's registry lives.
pub fn registry_path(config_dir: &Path) -> PathBuf {
    config_dir.join("knowledge").join("stores.yaml")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    pub version: u32,
    #[serde(default)]
    pub stores: BTreeMap<String, RegistryEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryEntry {
    pub path: String,
    /// The checkout's `origin` when it was registered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

impl Registry {
    fn empty() -> Self {
        Self {
            version: REGISTRY_VERSION,
            stores: BTreeMap::new(),
        }
    }

    /// `Ok(None)` when the file doesn't exist yet.
    pub fn read(path: &Path) -> Result<Option<Self>, KnowledgeError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(KnowledgeError::new(
                    "registry_unreadable",
                    format!("Could not read {}: {e}", display(path)),
                ));
            }
        };
        let registry: Self = serde_yaml_ng::from_str(&text).map_err(|e| {
            KnowledgeError::new(
                "registry_invalid",
                format!("{} is not a valid knowledge registry: {e}", display(path)),
            )
            .with_fix("Fix or remove the file; okena will not overwrite it.")
        })?;
        if registry.version > REGISTRY_VERSION {
            return Err(KnowledgeError::new(
                "registry_version",
                format!(
                    "{} was written by a newer okena (version {}).",
                    display(path),
                    registry.version
                ),
            )
            .with_fix("Update okena."));
        }
        Ok(Some(registry))
    }

    fn to_yaml(&self) -> String {
        serde_yaml_ng::to_string(self).unwrap_or_default()
    }
}

/// A registered checkout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredStore {
    pub id: String,
    pub root: PathBuf,
    pub remote: Option<String>,
}

pub fn list(registry: &Path) -> Result<Vec<RegisteredStore>, KnowledgeError> {
    Ok(Registry::read(registry)?
        .map(|r| {
            r.stores
                .into_iter()
                .map(|(id, entry)| RegisteredStore {
                    id,
                    root: PathBuf::from(entry.path),
                    remote: entry.remote,
                })
                .collect()
        })
        .unwrap_or_default())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisterOutcome {
    pub id: String,
    pub root: PathBuf,
    /// This checkout was already registered under this id.
    pub already_registered: bool,
    /// The checkout has no committed identity; its id came from the folder.
    pub identity_missing: bool,
}

/// Register the checkout at `checkout`, a path typed by a person.
///
/// The id is the checkout's committed identity, else its folder name. A folder
/// with neither an identity nor any kind folder is refused: registering it
/// would only produce an empty, broken root.
///
/// `remote` is the checkout's observed origin, recorded for display.
pub fn register(
    registry: &Path,
    checkout: &str,
    remote: Option<String>,
) -> Result<RegisterOutcome, KnowledgeError> {
    let root = resolve_checkout(checkout)?;
    let identity = StoreIdentity::read(&root)?;
    let identity_missing = identity.is_none();
    let id = match identity {
        Some(identity) => identity.id,
        None => {
            if !tree::has_kind_folder(&root) {
                return Err(KnowledgeError::new(
                    "not_a_knowledge_root",
                    format!(
                        "{} has no .okena-knowledge/store.yaml and none of docs/, skills/, agents/ or templates/.",
                        display(&root)
                    ),
                )
                .with_fix("Pick the top of the knowledge repository, or create a new store there."));
            }
            identity::id_from_folder(&root).ok_or_else(|| {
                KnowledgeError::new(
                    "invalid_store_id",
                    format!(
                        "No store id can be made from the folder name of {}.",
                        display(&root)
                    ),
                )
                .with_fix("Commit a .okena-knowledge/store.yaml with `version: 1` and `id: <kebab-case-id>`.")
            })?
        }
    };
    let path = display(&root);
    let already_registered = update(registry, |reg| {
        if let Some((other, _)) = reg
            .stores
            .iter()
            .find(|(other, e)| **other != id && canonical(Path::new(&e.path)) == root)
        {
            return Err(KnowledgeError::new(
                "store_path_taken",
                format!("{path} is already registered as `{other}`."),
            )
            .with_fix(format!("Unregister `{other}` first.")));
        }
        match reg.stores.get_mut(&id) {
            Some(entry) if canonical(Path::new(&entry.path)) != root => Err(KnowledgeError::new(
                "store_id_taken",
                format!("A store `{id}` is already registered at {}.", entry.path),
            )
            .with_fix(format!(
                "Use that checkout, or unregister `{id}` before adding this one."
            ))),
            Some(entry) => {
                let changed = entry.remote != remote && remote.is_some();
                if changed {
                    entry.remote = remote.clone();
                }
                Ok((true, changed))
            }
            None => {
                reg.stores.insert(
                    id.clone(),
                    RegistryEntry {
                        path: path.clone(),
                        remote: remote.clone(),
                    },
                );
                Ok((false, true))
            }
        }
    })?;
    Ok(RegisterOutcome {
        id,
        root,
        already_registered,
        identity_missing,
    })
}

/// Forget store `id`. Returns the checkout path, which stays on disk.
pub fn unregister(registry: &Path, id: &str) -> Result<PathBuf, KnowledgeError> {
    update(registry, |reg| match reg.stores.remove(id) {
        Some(entry) => Ok((PathBuf::from(entry.path), true)),
        None => Err(KnowledgeError::new(
            "unknown_store",
            format!("No store `{id}` is registered."),
        )),
    })
}

/// A path typed by a person: trimmed, `~` expanded, and required to be
/// absolute — the daemon's working directory means nothing to them.
pub fn absolute_input(input: &str) -> Result<PathBuf, KnowledgeError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(KnowledgeError::new(
            "store_path_required",
            "Pass the folder for the store.",
        ));
    }
    let path = expand_home(trimmed);
    if !path.is_absolute() {
        return Err(KnowledgeError::new(
            "store_path_not_absolute",
            format!("Store path must be absolute: {trimmed}"),
        )
        .with_fix("Use a full path such as ~/knowledge/acme-eng."));
    }
    Ok(path)
}

/// A checkout path typed by a person: [`absolute_input`], an existing
/// directory, canonical.
pub fn resolve_checkout(input: &str) -> Result<PathBuf, KnowledgeError> {
    let path = absolute_input(input)?;
    if !path.is_dir() {
        return Err(KnowledgeError::new(
            "store_path_missing",
            format!("{} is not a folder.", display(&path)),
        ));
    }
    Ok(canonical(&path))
}

/// Serialises every registry write in this process.
static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Read, change, and write only when something changed.
///
/// An unreadable registry fails here instead of being replaced: it may list
/// every store this machine knows about.
fn update<T>(
    path: &Path,
    change: impl FnOnce(&mut Registry) -> Result<(T, bool), KnowledgeError>,
) -> Result<T, KnowledgeError> {
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut registry = Registry::read(path)?.unwrap_or_else(Registry::empty);
    let (out, changed) = change(&mut registry)?;
    if changed {
        write_atomically(path, &registry.to_yaml(), false).map_err(|e| {
            KnowledgeError::new(
                "registry_write_failed",
                format!("Could not write {}: {e}", display(path)),
            )
        })?;
    }
    Ok(out)
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
        fn registry(&self) -> PathBuf {
            registry_path(&self.dir.path().join("config"))
        }
        fn path(&self, rel: &str) -> PathBuf {
            self.dir.path().join(rel)
        }
        fn arg(&self, rel: &str) -> String {
            display(&self.path(rel))
        }
    }

    #[test]
    fn register_list_and_unregister_leave_the_checkout_on_disk() {
        let s = Sandbox::new();
        store(&s.path("eng"), "acme-eng");
        let out = register(
            &s.registry(),
            &s.arg("eng"),
            Some("git@x:acme/eng.git".into()),
        )
        .expect("register");
        assert_eq!(out.id, "acme-eng");
        assert!(!out.already_registered && !out.identity_missing);

        let listed = list(&s.registry()).expect("list");
        assert_eq!(
            listed,
            [RegisteredStore {
                id: "acme-eng".into(),
                root: canonical(&s.path("eng")),
                remote: Some("git@x:acme/eng.git".into()),
            }]
        );

        let again = register(&s.registry(), &s.arg("eng"), None).expect("again");
        assert!(again.already_registered);
        assert_eq!(list(&s.registry()).expect("list").len(), 1);
        assert_eq!(
            list(&s.registry()).expect("list")[0].remote.as_deref(),
            Some("git@x:acme/eng.git"),
            "re-registering without a remote keeps the recorded one"
        );

        let left = unregister(&s.registry(), "acme-eng").expect("unregister");
        assert!(left.join(".okena-knowledge/store.yaml").is_file());
        assert!(list(&s.registry()).expect("list").is_empty());
        assert_eq!(
            unregister(&s.registry(), "acme-eng")
                .expect_err("gone")
                .code,
            "unknown_store"
        );
    }

    #[test]
    fn a_checkout_without_identity_takes_its_folder_name() {
        let s = Sandbox::new();
        write(&s.path("Team_Docs/docs/a.md"), "x");
        let out = register(&s.registry(), &s.arg("Team_Docs"), None).expect("register");
        assert_eq!(out.id, "team-docs");
        assert!(out.identity_missing);
    }

    #[test]
    fn folders_that_are_not_knowledge_roots_or_not_folders_are_refused() {
        let s = Sandbox::new();
        write(&s.path("plain/README.md"), "x");
        let code = |input: &str| {
            register(&s.registry(), input, None)
                .expect_err("refused")
                .code
        };
        assert_eq!(code(&s.arg("plain")), "not_a_knowledge_root");
        assert_eq!(code(&s.arg("missing")), "store_path_missing");
        assert_eq!(code("relative/path"), "store_path_not_absolute");
        assert_eq!(code("  "), "store_path_required");
        assert!(!s.registry().exists(), "nothing was written");
    }

    #[test]
    fn one_checkout_per_id_and_one_id_per_checkout() {
        let s = Sandbox::new();
        store(&s.path("a"), "acme-eng");
        store(&s.path("b"), "acme-eng");
        register(&s.registry(), &s.arg("a"), None).expect("first");
        assert_eq!(
            register(&s.registry(), &s.arg("b"), None)
                .expect_err("second checkout of the same id")
                .code,
            "store_id_taken"
        );

        write(&s.path("c/docs/x.md"), "x");
        register(&s.registry(), &s.arg("c"), None).expect("as `c`");
        // The same folder later gains a committed identity with another id.
        write(
            &StoreIdentity::path(&s.path("c")),
            "version: 1\nid: renamed\n",
        );
        assert_eq!(
            register(&s.registry(), &s.arg("c"), None)
                .expect_err("same checkout, new id")
                .code,
            "store_path_taken"
        );
    }

    #[test]
    fn a_corrupt_registry_is_never_overwritten() {
        let s = Sandbox::new();
        store(&s.path("eng"), "acme-eng");
        write(&s.registry(), "stores: [not, a, map\n");
        assert_eq!(
            register(&s.registry(), &s.arg("eng"), None)
                .expect_err("refused")
                .code,
            "registry_invalid"
        );
        assert_eq!(
            std::fs::read_to_string(s.registry()).expect("read"),
            "stores: [not, a, map\n"
        );
    }
}
