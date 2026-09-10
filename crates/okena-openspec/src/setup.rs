//! Creating a store, and choosing the machine default.
//!
//! `setup_store` is `openspec store setup <id> --path <path> [--remote <url>]
//! [--init-git | --no-init-git]`: the same refusals (a file, a non-empty
//! folder that is not a root, a folder inside another git repository), the
//! same files, one initial commit, and a rollback of everything it created if
//! a later step fails before that commit lands.

use crate::files::{self, Registry, RegistryEntry, STORE_METADATA_DIR, StoreMetadata};
use crate::paths::{canonical, display};
use crate::{OpenSpecDirs, OpenSpecError, git, registry, root, validate_store_id};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupRequest {
    pub id: String,
    pub path: String,
    /// Canonical clone source, written into the store's identity.
    pub remote: Option<String>,
    /// `git init` (when needed) and commit the store files.
    pub init_git: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupOutcome {
    pub id: String,
    pub root: PathBuf,
    /// Created paths relative to the root, directories with a trailing `/`.
    pub created: Vec<String>,
    pub git_initialized: bool,
    pub committed: bool,
    pub already_registered: bool,
}

#[derive(Default)]
struct Progress {
    created: Vec<(String, PathBuf, bool)>,
    git_initialized: bool,
    committed: bool,
}

impl Progress {
    fn dir(&mut self, root: &Path, rel: &str) {
        self.created.push((format!("{rel}/"), root.join(rel), true));
    }

    fn file(&mut self, root: &Path, rel: &str) {
        self.created.push((rel.to_string(), root.join(rel), false));
    }

    fn files(&self) -> Vec<String> {
        self.created
            .iter()
            .filter(|(_, _, dir)| !dir)
            .map(|(rel, _, _)| rel.clone())
            .collect()
    }
}

pub fn setup_store(dirs: &OpenSpecDirs, req: &SetupRequest) -> Result<SetupOutcome, OpenSpecError> {
    let id = validate_store_id(req.id.trim())?.to_string();
    let remote = req
        .remote
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_string);
    let root_path = registry::resolve_input_path(&req.path).map_err(|e| {
        e.with_fix(format!(
            "Choose where the store should live, e.g. ~/openspec/{id}."
        ))
    })?;

    let existed = root_path.exists();
    if existed && !root_path.is_dir() {
        return Err(OpenSpecError::new(
            "store_setup_path_not_directory",
            format!(
                "Store setup path is not a directory: {}",
                display(&root_path)
            ),
        )
        .with_fix("Choose an empty directory or an existing healthy OpenSpec root."));
    }
    // A store is its own repository; creating one inside an implementation
    // repo is almost always an accident.
    if let Some(repo) = containing_git_repository(&root_path) {
        return Err(OpenSpecError::new(
            "store_setup_inside_git_repo",
            format!(
                "Store setup path is inside another Git repository: {}",
                display(&repo)
            ),
        )
        .with_fix("Choose a path outside that Git repository."));
    }

    let mut has_metadata = false;
    if existed {
        registry::assert_not_pointer_root(&root_path)?;
        match StoreMetadata::read(&root_path)? {
            Some(meta) => {
                if meta.id != id {
                    return Err(OpenSpecError::new(
                        "store_metadata_id_mismatch",
                        format!(
                            "Store metadata id '{}' does not match requested id '{id}'.",
                            meta.id
                        ),
                    )
                    .with_fix(format!(
                        "Use id '{}' or choose a different setup path.",
                        meta.id
                    )));
                }
                if remote.is_some() {
                    return Err(OpenSpecError::new(
                        "store_remote_requires_hand_edit",
                        format!(
                            "Store '{id}' already has an identity file; a remote cannot change it."
                        ),
                    )
                    .with_fix(format!(
                        "Edit {} and commit it.",
                        display(&StoreMetadata::path(&root_path))
                    )));
                }
                has_metadata = true;
            }
            None => {
                let fresh = is_empty_dir(&root_path) || is_git_only_dir(&root_path);
                if !root::inspect(&root_path).healthy && !fresh {
                    return Err(OpenSpecError::new(
                        "store_setup_non_empty_directory",
                        "Store setup does not support initializing a non-empty folder that is not a healthy OpenSpec root.",
                    )
                    .with_fix("Choose an empty folder, a Git-only folder, or an existing healthy OpenSpec root."));
                }
            }
        }
    }

    let current = registry::read(dirs)?;
    registry::assert_no_conflict(current.as_ref(), &id, &root_path)?;
    let already_here = registered_at(current.as_ref(), &id, &root_path);
    let repo_existed = git::is_repository_at_root(&root_path);
    if req.init_git {
        let probe = nearest_existing_dir(&root_path).unwrap_or_else(std::env::temp_dir);
        git::assert_commit_identity(&probe)?;
    }

    let mut progress = Progress::default();
    let result = execute(
        dirs,
        &id,
        &root_path,
        remote.as_deref(),
        has_metadata,
        already_here,
        req.init_git,
        repo_existed,
        &mut progress,
    );
    if result.is_err() && !progress.committed {
        // Once the commit has landed the files are durable history; before
        // it, undo exactly what this call created and nothing else.
        for (_, path, is_dir) in progress.created.iter().rev() {
            if *is_dir {
                let _ = std::fs::remove_dir(path);
            } else {
                let _ = std::fs::remove_file(path);
            }
        }
        if progress.git_initialized {
            let _ = std::fs::remove_dir_all(root_path.join(".git"));
        }
        if !existed {
            let _ = std::fs::remove_dir(&root_path);
        }
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn execute(
    dirs: &OpenSpecDirs,
    id: &str,
    root_path: &Path,
    remote: Option<&str>,
    has_metadata: bool,
    already_here: bool,
    init_git: bool,
    repo_existed: bool,
    progress: &mut Progress,
) -> Result<SetupOutcome, OpenSpecError> {
    let io = |what: &str, e: std::io::Error| {
        OpenSpecError::new(
            "store_setup_failed",
            format!("Could not create {what}: {e}"),
        )
    };
    std::fs::create_dir_all(root_path).map_err(|e| io(&display(root_path), e))?;

    for rel in [
        "openspec",
        "openspec/specs",
        "openspec/changes",
        "openspec/changes/archive",
    ] {
        let path = root_path.join(rel);
        if path.is_dir() {
            continue;
        }
        if path.exists() {
            return Err(OpenSpecError::new(
                "store_setup_failed",
                format!("{rel}/ exists but is not a directory."),
            ));
        }
        std::fs::create_dir(&path).map_err(|e| io(rel, e))?;
        progress.dir(root_path, rel);
    }

    if files::config_file(root_path).is_none() {
        std::fs::write(
            root_path.join("openspec/config.yaml"),
            files::default_project_config(),
        )
        .map_err(|e| io("openspec/config.yaml", e))?;
        progress.file(root_path, "openspec/config.yaml");
    }

    // Git cannot track empty directories: anchor them so a teammate's clone
    // is a healthy root. A rerun of a registered store stays a no-op.
    if !already_here {
        for rel in ["openspec/specs", "openspec/changes/archive"] {
            if is_empty_dir(&root_path.join(rel)) {
                let anchor = format!("{rel}/.gitkeep");
                std::fs::write(root_path.join(&anchor), "").map_err(|e| io(&anchor, e))?;
                progress.file(root_path, &anchor);
            }
        }
    }

    if !has_metadata {
        let dir_missing = !root_path.join(STORE_METADATA_DIR).exists();
        StoreMetadata::new(id, remote).write(root_path)?;
        if dir_missing {
            progress.dir(root_path, STORE_METADATA_DIR);
        }
        progress.file(root_path, ".openspec-store/store.yaml");
    }

    if init_git {
        progress.git_initialized = git::init(root_path)?;
        // A repository setup created gets the whole store shape; in the
        // user's own repository only what setup created is committed.
        let pathspecs = if progress.git_initialized {
            vec!["openspec".to_string(), STORE_METADATA_DIR.to_string()]
        } else {
            progress.files()
        };
        if progress.git_initialized || repo_existed {
            progress.committed = git::commit(root_path, id, &pathspecs)?;
        }
    }

    let backend = registry::backend_for(root_path);
    let already_registered = registry::update(dirs, |reg| {
        registry::assert_no_conflict(Some(reg), id, root_path)?;
        let up_to_date = reg.stores.get(id).is_some_and(|e| e.backend == backend);
        if !up_to_date {
            reg.stores.insert(
                id.to_string(),
                RegistryEntry {
                    backend: backend.clone(),
                },
            );
        }
        Ok((already_here, !up_to_date))
    })?;

    Ok(SetupOutcome {
        id: id.to_string(),
        root: canonical(root_path),
        created: progress
            .created
            .iter()
            .map(|(rel, _, _)| rel.clone())
            .collect(),
        git_initialized: progress.git_initialized,
        committed: progress.committed,
        already_registered,
    })
}

fn registered_at(registry: Option<&Registry>, id: &str, root_path: &Path) -> bool {
    registry
        .and_then(|r| r.stores.get(id))
        .is_some_and(|e| canonical(Path::new(&e.backend.local_path)) == canonical(root_path))
}

fn is_empty_dir(path: &Path) -> bool {
    std::fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none())
}

fn is_git_only_dir(path: &Path) -> bool {
    std::fs::read_dir(path).is_ok_and(|entries| {
        let names: Vec<_> = entries.flatten().map(|e| e.file_name()).collect();
        names.len() == 1 && names[0] == ".git"
    }) && git::is_repository_at_root(path)
}

fn nearest_existing_dir(path: &Path) -> Option<PathBuf> {
    path.ancestors().find(|p| p.is_dir()).map(Path::to_path_buf)
}

/// The git repository `path` would be nested in, if any.
fn containing_git_repository(path: &Path) -> Option<PathBuf> {
    let parent = nearest_existing_dir(path.parent()?)?;
    canonical(&parent)
        .ancestors()
        .find(|dir| git::is_repository_at_root(dir))
        .map(Path::to_path_buf)
}

/// Set or clear OpenSpec's machine-wide `defaultStore` — the fallback root for
/// any `openspec` command run outside a repository with its own planning.
pub fn set_default_store(dirs: &OpenSpecDirs, id: Option<&str>) -> Result<(), OpenSpecError> {
    if let Some(id) = id {
        validate_store_id(id)?;
        if !registry::list(dirs)?.iter().any(|s| s.id == id) {
            return Err(
                OpenSpecError::new("unknown_store", format!("Unknown store '{id}'."))
                    .with_fix("Register the store first, then make it the default."),
            );
        }
    }
    files::write_default_store(&dirs.config_path(), id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{Sandbox, healthy_root, write};

    fn request(id: &str, path: &Path, init_git: bool) -> SetupRequest {
        SetupRequest {
            id: id.into(),
            path: display(path),
            remote: Some("git@github.com:acme/team-plans.git".into()),
            init_git,
        }
    }

    fn sorted_files(root: &Path) -> Vec<String> {
        fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                if e.file_name() == ".git" {
                    continue;
                }
                let p = e.path();
                if p.is_dir() {
                    walk(root, &p, out);
                } else {
                    out.push(crate::tree::rel(root, &p));
                }
            }
        }
        let mut out = Vec::new();
        walk(root, root, &mut out);
        out.sort();
        out
    }

    #[test]
    fn setup_leaves_the_same_shape_as_the_cli_and_registers_it() {
        let sb = Sandbox::new();
        let root = sb.path("stores/team-plans");
        let out = setup_store(&sb.dirs, &request("team-plans", &root, false)).unwrap();

        // Exactly the files `openspec store setup` 1.13 created.
        assert_eq!(
            sorted_files(&root),
            [
                ".openspec-store/store.yaml",
                "openspec/changes/archive/.gitkeep",
                "openspec/config.yaml",
                "openspec/specs/.gitkeep",
            ]
        );
        assert_eq!(
            std::fs::read_to_string(root.join(".openspec-store/store.yaml")).unwrap(),
            "version: 1\nid: team-plans\nremote: git@github.com:acme/team-plans.git\n"
        );
        assert!(!out.committed && !out.already_registered);
        assert!(root::inspect(&root).healthy);
        let stores = registry::list(&sb.dirs).unwrap();
        assert_eq!(stores[0].id, "team-plans");
        // The registry records the observed origin, never the metadata remote.
        assert_eq!(stores[0].remote, None);
    }

    #[test]
    fn setup_refuses_what_the_cli_refuses() {
        let sb = Sandbox::new();
        let busy = sb.path("busy");
        write(&busy.join("README.md"), "mine");
        assert_eq!(
            setup_store(&sb.dirs, &request("busy", &busy, false))
                .unwrap_err()
                .code,
            "store_setup_non_empty_directory"
        );

        let file = sb.path("file");
        write(&file, "x");
        assert_eq!(
            setup_store(&sb.dirs, &request("x", &file, false))
                .unwrap_err()
                .code,
            "store_setup_path_not_directory"
        );

        let repo = sb.path("app");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        assert_eq!(
            setup_store(&sb.dirs, &request("x", &repo.join("specs"), false))
                .unwrap_err()
                .code,
            "store_setup_inside_git_repo"
        );

        assert_eq!(
            setup_store(&sb.dirs, &request("Bad Id", &sb.path("y"), false))
                .unwrap_err()
                .code,
            "invalid_store_id"
        );
    }

    #[test]
    fn an_existing_root_is_converted_in_place_without_touching_its_content() {
        let sb = Sandbox::new();
        let root = sb.path("planning");
        healthy_root(&root);
        write(&root.join("openspec/specs/auth/spec.md"), "# Auth");
        let out = setup_store(
            &sb.dirs,
            &SetupRequest {
                remote: None,
                ..request("planning", &root, false)
            },
        )
        .unwrap();
        // Anchors before identity, in the CLI's order.
        assert_eq!(
            out.created,
            [
                "openspec/changes/archive/.gitkeep",
                ".openspec-store/",
                ".openspec-store/store.yaml"
            ]
        );
        assert_eq!(
            std::fs::read_to_string(root.join("openspec/specs/auth/spec.md")).unwrap(),
            "# Auth"
        );
    }

    #[test]
    fn a_failed_registration_rolls_back_everything_setup_created() {
        let sb = Sandbox::new();
        write(&sb.dirs.registry_lock_path(), "1:held-by-the-cli");
        let root = sb.path("stores/new-store");
        let err = setup_store(&sb.dirs, &request("new-store", &root, false)).unwrap_err();
        assert_eq!(err.code, "store_registry_busy");
        assert!(
            !root.exists(),
            "setup must not leave a half-made store behind"
        );
    }

    #[test]
    fn setup_with_git_makes_one_initial_commit() {
        let sb = Sandbox::new();
        // Only where this machine can commit; identity is the user's config.
        if git::assert_commit_identity(sb.dir.path()).is_err() {
            return;
        }
        let root = sb.path("stores/team-plans");
        let out = setup_store(&sb.dirs, &request("team-plans", &root, true)).unwrap();
        assert!(out.git_initialized && out.committed);
        let log = std::process::Command::new("git")
            .args(["log", "--format=%s", "--name-only"])
            .current_dir(&root)
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&log.stdout);
        assert!(
            log.starts_with("Initialize OpenSpec store team-plans"),
            "{log}"
        );
        assert!(log.contains(".openspec-store/store.yaml") && log.contains("openspec/config.yaml"));
    }

    #[test]
    fn the_default_store_must_be_registered() {
        let sb = Sandbox::new();
        assert_eq!(
            set_default_store(&sb.dirs, Some("nope")).unwrap_err().code,
            "unknown_store"
        );
        setup_store(&sb.dirs, &request("team-plans", &sb.path("s"), false)).unwrap();
        set_default_store(&sb.dirs, Some("team-plans")).unwrap();
        assert_eq!(
            files::read_default_store(&sb.dirs.config_path())
                .unwrap()
                .as_deref(),
            Some("team-plans")
        );
        set_default_store(&sb.dirs, None).unwrap();
        assert_eq!(
            files::read_default_store(&sb.dirs.config_path()).unwrap(),
            None
        );
    }
}
