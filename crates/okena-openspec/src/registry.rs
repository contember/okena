//! Reading and changing OpenSpec's machine store registry.
//!
//! `register` is `openspec store register <path> [--id <id>] --yes` and
//! `unregister` is `openspec store unregister <id>`, with the same checks, the
//! same conflict rules (one checkout per store id, one id per checkout) and the
//! same lock.

use crate::files::{Backend, Registry, RegistryEntry, StoreMetadata, StorePointer};
use crate::lock::RegistryLock;
use crate::paths::{canonical, display, expand_home};
use crate::{OpenSpecDirs, OpenSpecError, git, root, validate_store_id};
use std::path::{Path, PathBuf};

/// One registered store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredStore {
    pub id: String,
    pub root: PathBuf,
    /// The git origin observed when the store was registered.
    pub remote: Option<String>,
    pub branch: Option<String>,
}

pub fn read(dirs: &OpenSpecDirs) -> Result<Option<Registry>, OpenSpecError> {
    Registry::read(&dirs.registry_path())
}

pub fn entries(registry: &Registry) -> Vec<RegisteredStore> {
    registry
        .stores
        .iter()
        .map(|(id, entry)| RegisteredStore {
            id: id.clone(),
            root: PathBuf::from(&entry.backend.local_path),
            remote: entry.backend.remote.clone(),
            branch: entry.backend.branch.clone(),
        })
        .collect()
}

/// Registered stores, sorted by id. Empty when there is no registry yet.
pub fn list(dirs: &OpenSpecDirs) -> Result<Vec<RegisteredStore>, OpenSpecError> {
    Ok(read(dirs)?.as_ref().map(entries).unwrap_or_default())
}

fn same_path(a: &Path, b: &Path) -> bool {
    canonical(a) == canonical(b)
}

pub(crate) fn assert_no_conflict(
    registry: Option<&Registry>,
    id: &str,
    local_path: &Path,
) -> Result<(), OpenSpecError> {
    let Some(registry) = registry else {
        return Ok(());
    };
    for (entry_id, entry) in &registry.stores {
        let same = same_path(Path::new(&entry.backend.local_path), local_path);
        if entry_id == id && same {
            continue;
        }
        if entry_id == id {
            return Err(OpenSpecError::new(
                "store_id_conflict",
                format!(
                    "Store '{id}' is already registered at {}. One checkout per store id is supported on this machine.",
                    entry.backend.local_path
                ),
            )
            .with_fix(format!(
                "Use the existing registration, or run openspec store unregister {id} first to switch this id to a different checkout."
            )));
        }
        if same {
            return Err(OpenSpecError::new(
                "store_path_conflict",
                format!("Store path is already registered as '{entry_id}'."),
            )
            .with_fix(format!(
                "Use the existing '{entry_id}' registration or choose a different path."
            )));
        }
    }
    Ok(())
}

/// Lock, read, change, and write only when something changed.
///
/// An unreadable registry fails here instead of being replaced: it may hold
/// every store this machine knows about.
pub(crate) fn update<T>(
    dirs: &OpenSpecDirs,
    change: impl FnOnce(&mut Registry) -> Result<(T, bool), OpenSpecError>,
) -> Result<T, OpenSpecError> {
    let _lock = RegistryLock::acquire(&dirs.registry_lock_path())?;
    let path = dirs.registry_path();
    let mut registry = Registry::read(&path)?.unwrap_or_else(Registry::empty);
    let (out, changed) = change(&mut registry)?;
    if changed {
        crate::files::write_atomically(&path, &registry.to_yaml(), true).map_err(|e| {
            OpenSpecError::new(
                "store_registry_write_failed",
                format!("Could not write {}: {e}", display(&path)),
            )
        })?;
    }
    Ok(out)
}

/// The registry entry for a checkout, with its observed git origin.
pub(crate) fn backend_for(root: &Path) -> Backend {
    Backend {
        kind: "git".into(),
        local_path: display(&canonical(root)),
        remote: git::origin_url(root),
        branch: None,
    }
}

/// A path typed by a person: trimmed, `~` expanded, and required to be
/// absolute — the daemon's working directory means nothing to them.
pub(crate) fn resolve_input_path(input: &str) -> Result<PathBuf, OpenSpecError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(
            OpenSpecError::new("store_path_required", "Pass a store path.")
                .with_fix("Choose the folder that holds the store checkout."),
        );
    }
    let path = expand_home(trimmed);
    if !path.is_absolute() {
        return Err(OpenSpecError::new(
            "store_path_not_absolute",
            format!("Store path must be absolute: {trimmed}"),
        )
        .with_fix("Use a full path such as ~/openspec/team-plans."));
    }
    Ok(path)
}

/// Refuse a config-only repo that points at a store: its planning lives
/// elsewhere, so it is not itself a store root.
pub(crate) fn assert_not_pointer_root(path: &Path) -> Result<(), OpenSpecError> {
    let c = root::classify(path);
    let Some(config) = c.config else {
        return Ok(());
    };
    if c.has_planning_shape {
        return Ok(());
    }
    let file = display(&config.path);
    match config.store {
        StorePointer::Absent => Ok(()),
        StorePointer::Malformed(problem) => Err(OpenSpecError::new(
            "invalid_store_pointer",
            format!("The store declaration in {file} is invalid ({}).", problem.describe()),
        )
        .with_fix(format!(
            "Fix or remove the store: line in {file} before registering this path as a store."
        ))),
        StorePointer::Value(value) => Err(OpenSpecError::new(
            "store_root_pointer_declared",
            format!(
                "This repo's planning is externalized to store '{value}' ({file}); it is not itself a store root."
            ),
        )
        .with_fix(
            "Register the checkout for the declared store, or remove the store: line first to convert this repo into a local store root.",
        )),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registration {
    pub id: String,
    pub root: PathBuf,
    /// `.openspec-store/store.yaml` was created — the root was converted into
    /// a store and the user should commit the new file.
    pub metadata_created: bool,
    pub already_registered: bool,
}

/// Register an existing store checkout (`openspec store register --yes`).
///
/// The id comes from the store's committed identity when it has one; `id` may
/// only confirm it. Otherwise `id`, else the folder name, becomes the identity
/// and `.openspec-store/store.yaml` is written.
pub fn register(
    dirs: &OpenSpecDirs,
    path: &str,
    id: Option<&str>,
) -> Result<Registration, OpenSpecError> {
    let root_path = resolve_input_path(path)?;
    if !root_path.exists() {
        return Err(OpenSpecError::new(
            "store_path_missing",
            format!("Store path does not exist: {}", display(&root_path)),
        )
        .with_fix("Clone or create the store folder before registering it."));
    }
    if !root_path.is_dir() {
        return Err(OpenSpecError::new(
            "store_path_not_directory",
            format!("Store path is not a directory: {}", display(&root_path)),
        )
        .with_fix("Pass an existing store directory."));
    }
    let root_path = canonical(&root_path);

    assert_not_pointer_root(&root_path)?;
    let health = root::inspect(&root_path);
    if !health.healthy {
        return Err(OpenSpecError::new(
            "store_register_root_unhealthy",
            format!(
                "Store register requires an existing healthy OpenSpec root. {}",
                health.problems()
            ),
        )
        .with_fix(
            "Set up a new store instead, or point at a checkout whose openspec/ files are present.",
        ));
    }

    let metadata = StoreMetadata::read(&root_path)?;
    let explicit = match id.map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => Some(validate_store_id(id)?.to_string()),
        None => None,
    };
    if let (Some(meta), Some(explicit)) = (&metadata, &explicit)
        && &meta.id != explicit
    {
        return Err(OpenSpecError::new(
            "store_metadata_id_mismatch",
            format!(
                "Store metadata id '{}' does not match id '{explicit}'. The id comes from the store's committed .openspec-store/store.yaml.",
                meta.id
            ),
        )
        .with_fix(format!("Use id {} or register a different folder.", meta.id)));
    }
    let id = match (&metadata, explicit) {
        (Some(meta), _) => meta.id.clone(),
        (None, Some(explicit)) => explicit,
        (None, None) => {
            let name = root_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            validate_store_id(&name)
                .map_err(|e| {
                    e.with_fix(format!(
                        "The folder name '{name}' is not a valid store id; give the store an id."
                    ))
                })?
                .to_string()
        }
    };

    let backend = backend_for(&root_path);
    assert_no_conflict(read(dirs)?.as_ref(), &id, &root_path)?;

    // Identity first, so a committed registry entry never points at a store
    // without one.
    let metadata_created = metadata.is_none();
    if metadata_created {
        StoreMetadata::new(&id, None).write(&root_path)?;
    }

    let result = update(dirs, |registry| {
        assert_no_conflict(Some(registry), &id, &root_path)?;
        let existing = registry.stores.get(&id).map(|e| &e.backend);
        let already = existing.is_some_and(|b| {
            same_path(Path::new(&b.local_path), &root_path) && b.branch == backend.branch
        });
        let up_to_date = already && existing.is_some_and(|b| b.remote == backend.remote);
        if !up_to_date {
            registry.stores.insert(
                id.clone(),
                RegistryEntry {
                    backend: backend.clone(),
                },
            );
        }
        Ok((already, !up_to_date))
    });

    match result {
        Ok(already_registered) => Ok(Registration {
            id,
            root: root_path,
            metadata_created,
            already_registered,
        }),
        Err(e) => {
            // Roll back the identity we wrote — unless a concurrent
            // registration has since committed against it.
            if metadata_created
                && !read(dirs)
                    .ok()
                    .flatten()
                    .is_some_and(|r| r.stores.contains_key(&id))
            {
                let _ = std::fs::remove_file(StoreMetadata::path(&root_path));
                let _ = std::fs::remove_dir(root_path.join(crate::files::STORE_METADATA_DIR));
            }
            Err(e)
        }
    }
}

/// Forget a registration (`openspec store unregister <id>`). The checkout
/// stays on disk; its path is returned so the caller can say where.
pub fn unregister(dirs: &OpenSpecDirs, id: &str) -> Result<PathBuf, OpenSpecError> {
    validate_store_id(id)?;
    update(dirs, |registry| match registry.stores.remove(id) {
        Some(entry) => Ok((PathBuf::from(entry.backend.local_path), true)),
        None => Err(
            OpenSpecError::new("store_not_found", format!("Unknown store '{id}'"))
                .with_fix("Run openspec store list to see registered stores."),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Sandbox, healthy_root, store_root, write};

    fn p(path: &Path) -> String {
        display(path)
    }

    #[test]
    fn registering_a_store_checkout_records_it_with_its_committed_id() {
        let sb = Sandbox::new();
        let checkout = sb.path("clones/whatever-folder");
        store_root(&checkout, "team-plans");

        let r = register(&sb.dirs, &p(&checkout), None).unwrap();
        assert_eq!(r.id, "team-plans");
        assert!(!r.metadata_created && !r.already_registered);

        let list = list(&sb.dirs).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "team-plans");
        assert_eq!(list[0].root, canonical(&checkout));
        assert!(!sb.dirs.registry_lock_path().exists(), "lock released");

        let again = register(&sb.dirs, &p(&checkout), Some("team-plans")).unwrap();
        assert!(again.already_registered);
    }

    #[test]
    fn registering_a_plain_root_converts_it_into_a_store() {
        let sb = Sandbox::new();
        let checkout = sb.path("design-system");
        healthy_root(&checkout);

        let r = register(&sb.dirs, &p(&checkout), None).unwrap();
        assert_eq!(r.id, "design-system", "folder name becomes the id");
        assert!(r.metadata_created);
        assert_eq!(
            std::fs::read_to_string(StoreMetadata::path(&checkout)).unwrap(),
            "version: 1\nid: design-system\n"
        );
    }

    #[test]
    fn an_explicit_id_cannot_contradict_committed_identity() {
        let sb = Sandbox::new();
        let checkout = sb.path("s");
        store_root(&checkout, "team-plans");
        let err = register(&sb.dirs, &p(&checkout), Some("other")).unwrap_err();
        assert_eq!(err.code, "store_metadata_id_mismatch");
    }

    #[test]
    fn a_folder_name_that_is_not_an_id_asks_for_one() {
        let sb = Sandbox::new();
        let checkout = sb.path("My Specs");
        healthy_root(&checkout);
        assert_eq!(
            register(&sb.dirs, &p(&checkout), None).unwrap_err().code,
            "invalid_store_id"
        );
        assert!(register(&sb.dirs, &p(&checkout), Some("my-specs")).is_ok());
    }

    #[test]
    fn one_checkout_per_id_and_one_id_per_checkout() {
        let sb = Sandbox::new();
        let first = sb.path("a/team-plans");
        let second = sb.path("b/team-plans");
        store_root(&first, "team-plans");
        store_root(&second, "team-plans");
        register(&sb.dirs, &p(&first), None).unwrap();

        let err = register(&sb.dirs, &p(&second), None).unwrap_err();
        assert_eq!(err.code, "store_id_conflict");

        let plain = sb.path("plain");
        healthy_root(&plain);
        register(&sb.dirs, &p(&plain), Some("plain")).unwrap();
        // Same checkout, a different id: the identity file now says `plain`.
        assert_eq!(
            register(&sb.dirs, &p(&plain), Some("again"))
                .unwrap_err()
                .code,
            "store_metadata_id_mismatch"
        );
    }

    #[test]
    fn unhealthy_and_pointer_roots_are_refused() {
        let sb = Sandbox::new();
        let empty = sb.path("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(
            register(&sb.dirs, &p(&empty), Some("x")).unwrap_err().code,
            "store_register_root_unhealthy"
        );

        let pointer = sb.path("web");
        write(
            &pointer.join("openspec/config.yaml"),
            "schema: spec-driven\nstore: team-plans\n",
        );
        assert_eq!(
            register(&sb.dirs, &p(&pointer), Some("web"))
                .unwrap_err()
                .code,
            "store_root_pointer_declared"
        );

        assert_eq!(
            register(&sb.dirs, "relative/path", None).unwrap_err().code,
            "store_path_not_absolute"
        );
        assert_eq!(
            register(&sb.dirs, &p(&sb.path("missing")), None)
                .unwrap_err()
                .code,
            "store_path_missing"
        );
    }

    #[test]
    fn a_corrupt_registry_is_never_overwritten() {
        let sb = Sandbox::new();
        write(&sb.dirs.registry_path(), "version: 1\nstores: [broken\n");
        let checkout = sb.path("s");
        store_root(&checkout, "s");
        assert_eq!(
            register(&sb.dirs, &p(&checkout), None).unwrap_err().code,
            "invalid_store_registry"
        );
        assert_eq!(
            std::fs::read_to_string(sb.dirs.registry_path()).unwrap(),
            "version: 1\nstores: [broken\n"
        );
    }

    #[test]
    fn a_busy_registry_leaves_no_half_made_identity_behind() {
        let sb = Sandbox::new();
        let checkout = sb.path("plain");
        healthy_root(&checkout);
        write(&sb.dirs.registry_lock_path(), "1:held-by-the-cli");
        let err = register(&sb.dirs, &p(&checkout), Some("plain")).unwrap_err();
        assert_eq!(err.code, "store_registry_busy");
        assert!(!StoreMetadata::path(&checkout).exists());
    }

    #[test]
    fn unregister_forgets_the_store_and_keeps_its_files() {
        let sb = Sandbox::new();
        let checkout = sb.path("team-plans");
        store_root(&checkout, "team-plans");
        register(&sb.dirs, &p(&checkout), None).unwrap();

        let left = unregister(&sb.dirs, "team-plans").unwrap();
        assert_eq!(left, canonical(&checkout));
        assert!(checkout.join("openspec").is_dir());
        assert!(list(&sb.dirs).unwrap().is_empty());
        assert_eq!(
            std::fs::read_to_string(sb.dirs.registry_path()).unwrap(),
            "version: 1\nstores: {}\n"
        );
        assert_eq!(
            unregister(&sb.dirs, "team-plans").unwrap_err().code,
            "store_not_found"
        );
    }
}
