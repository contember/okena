//! Creating a new knowledge store: its identity, empty kind folders, one
//! initial commit, and registration — with a rollback of everything created if
//! a step fails before the commit lands.

use crate::identity::{STORE_METADATA_DIR, StoreIdentity, validate_store_id};
use crate::{KnowledgeError, canonical, display, registry};
use okena_core::knowledge::KnowledgeKind;
use okena_git::repository as git;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SetupRequest {
    pub id: String,
    /// Where the store should live, as a person typed it.
    pub path: String,
    pub name: Option<String>,
    pub description: Option<String>,
    /// Canonical clone source, written into the identity.
    pub remote: Option<String>,
    /// `git init` when needed, and commit the new files.
    pub init_git: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupOutcome {
    pub id: String,
    pub root: PathBuf,
    pub git_initialized: bool,
    pub committed: bool,
}

pub fn setup_store(
    registry_path: &Path,
    req: &SetupRequest,
) -> Result<SetupOutcome, KnowledgeError> {
    let id = validate_store_id(req.id.trim())?.to_string();
    let root = registry::absolute_input(&req.path)?;
    let at = display(&root);

    let existed = root.exists();
    if existed && !root.is_dir() {
        return Err(KnowledgeError::new(
            "store_setup_not_directory",
            format!("{at} is a file."),
        ));
    }
    if existed && !is_empty_or_git_only(&root) {
        return Err(KnowledgeError::new(
            "store_setup_not_empty",
            format!("{at} is not empty."),
        )
        .with_fix("Choose an empty folder. To add a repository that already holds knowledge, add it as an existing folder."));
    }
    // A store is its own repository; one inside an implementation repo is
    // almost always an accident, and its history would mix with that repo's.
    if let Some(repo) = containing_repository(&root) {
        return Err(KnowledgeError::new(
            "store_setup_inside_git_repo",
            format!("{at} is inside the git repository {}.", display(&repo)),
        )
        .with_fix("Choose a folder outside it."));
    }
    if let Some(taken) = registry::list(registry_path)?
        .into_iter()
        .find(|s| s.id == id)
    {
        return Err(KnowledgeError::new(
            "store_id_taken",
            format!(
                "A store `{id}` is already registered at {}.",
                display(&taken.root)
            ),
        )
        .with_fix("Pick another id."));
    }
    let repo_existed = git::is_repository_at_root(&root);
    if req.init_git {
        let probe = root
            .ancestors()
            .find(|p| p.is_dir())
            .map(Path::to_path_buf)
            .unwrap_or_else(std::env::temp_dir);
        if !git::has_commit_identity(&probe) {
            return Err(KnowledgeError::new(
                "store_git_identity_missing",
                "git has no author name and email configured, so the initial commit can't be made.",
            )
            .with_fix("Run `git config --global user.name \"Your Name\"` and `git config --global user.email you@example.com`, or create the store without git."));
        }
    }

    let mut progress = Progress::default();
    let result = create(
        registry_path,
        &id,
        &root,
        req,
        existed,
        repo_existed,
        &mut progress,
    );
    if result.is_err() && !progress.committed {
        // Before the commit, undo exactly what this call created; after it,
        // the files are history and stay.
        if progress.git_initialized {
            let _ = std::fs::remove_dir_all(root.join(".git"));
        }
        for (path, is_dir) in progress.created.iter().rev() {
            let _ = if *is_dir {
                std::fs::remove_dir(path)
            } else {
                std::fs::remove_file(path)
            };
        }
    }
    result
}

#[derive(Default)]
struct Progress {
    created: Vec<(PathBuf, bool)>,
    git_initialized: bool,
    committed: bool,
}

fn create(
    registry_path: &Path,
    id: &str,
    root: &Path,
    req: &SetupRequest,
    existed: bool,
    repo_existed: bool,
    progress: &mut Progress,
) -> Result<SetupOutcome, KnowledgeError> {
    let io = |what: &Path, e: std::io::Error| {
        KnowledgeError::new(
            "store_setup_failed",
            format!("Could not create {}: {e}", display(what)),
        )
    };
    let mkdir = |path: PathBuf, progress: &mut Progress| -> Result<(), KnowledgeError> {
        std::fs::create_dir(&path).map_err(|e| io(&path, e))?;
        progress.created.push((path, true));
        Ok(())
    };
    let write = |path: PathBuf, content: &str, progress: &mut Progress| {
        std::fs::write(&path, content).map_err(|e| io(&path, e))?;
        progress.created.push((path, false));
        Ok::<(), KnowledgeError>(())
    };

    if !existed {
        std::fs::create_dir_all(root.parent().unwrap_or(root)).map_err(|e| io(root, e))?;
        mkdir(root.to_path_buf(), progress)?;
    }
    mkdir(root.join(STORE_METADATA_DIR), progress)?;
    let identity = StoreIdentity {
        name: req.name.clone().filter(|n| !n.trim().is_empty()),
        description: req.description.clone().filter(|d| !d.trim().is_empty()),
        remote: req.remote.clone().filter(|r| !r.trim().is_empty()),
        ..StoreIdentity::new(id)
    };
    write(StoreIdentity::path(root), &identity.to_yaml(), progress)?;
    // Git cannot track empty folders; anchor them so every clone has the shape.
    for kind in KnowledgeKind::all() {
        let dir = root.join(kind.folder());
        mkdir(dir.clone(), progress)?;
        write(dir.join(".gitkeep"), "", progress)?;
    }

    if req.init_git {
        if !repo_existed {
            git::init_repository(root)
                .map_err(|e| KnowledgeError::new("store_git_init_failed", e.user_detail()))?;
            progress.git_initialized = true;
        }
        let mut paths = vec![STORE_METADATA_DIR];
        paths.extend(KnowledgeKind::all().map(KnowledgeKind::folder));
        git::commit_paths(root, &format!("Initialize knowledge store {id}"), &paths).map_err(
            |e| {
                KnowledgeError::new(
                    "store_git_commit_failed",
                    format!("The initial commit failed: {}", e.user_detail()),
                )
            },
        )?;
        progress.committed = true;
    }

    let registered = registry::register(registry_path, &display(root), None)?;
    Ok(SetupOutcome {
        id: registered.id,
        root: registered.root,
        git_initialized: progress.git_initialized,
        committed: progress.committed,
    })
}

fn is_empty_or_git_only(path: &Path) -> bool {
    std::fs::read_dir(path).is_ok_and(|entries| entries.flatten().all(|e| e.file_name() == ".git"))
}

/// The git repository `path` would be nested in, if any.
fn containing_repository(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?.ancestors().find(|p| p.is_dir())?;
    canonical(parent)
        .ancestors()
        .find(|dir| git::is_repository_at_root(dir))
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testgit::git;
    use crate::registry::registry_path;
    use crate::testutil::{store, write};

    fn request(id: &str, path: &Path, init_git: bool) -> SetupRequest {
        SetupRequest {
            id: id.into(),
            path: display(path),
            description: Some("How we build".into()),
            init_git,
            ..Default::default()
        }
    }

    #[test]
    fn creates_the_layout_commits_it_and_registers_the_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = registry_path(&dir.path().join("config"));
        // A git-only folder with its own identity, so the commit doesn't depend
        // on this machine's global git config.
        let root = dir.path().join("eng");
        std::fs::create_dir_all(&root).expect("mkdir");
        git(&root, &["init", "-b", "main"]);
        for (k, v) in [
            ("user.name", "t"),
            ("user.email", "t@t"),
            ("commit.gpgsign", "false"),
        ] {
            git(&root, &["config", k, v]);
        }

        let out = setup_store(&registry, &request("acme-eng", &root, true)).expect("setup");
        assert_eq!(out.id, "acme-eng");
        assert!(out.committed && !out.git_initialized);
        for folder in ["docs", "skills", "agents", "templates"] {
            assert!(root.join(folder).join(".gitkeep").is_file(), "{folder}");
        }
        let identity = StoreIdentity::read(&root).expect("read").expect("identity");
        assert_eq!(identity.description.as_deref(), Some("How we build"));
        assert_eq!(git(&root, &["rev-list", "--count", "HEAD"]).trim(), "1");
        assert_eq!(git(&root, &["status", "--porcelain"]).trim(), "");
        assert_eq!(registry::list(&registry).expect("list")[0].id, "acme-eng");
    }

    #[test]
    fn without_git_it_only_creates_and_registers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = registry_path(&dir.path().join("config"));
        let root = dir.path().join("new/place/team-docs");
        let out = setup_store(&registry, &request("team-docs", &root, false)).expect("setup");
        assert!(!out.committed);
        assert!(!root.join(".git").exists());
        assert!(root.join("docs/.gitkeep").is_file());
        assert_eq!(registry::list(&registry).expect("list").len(), 1);
    }

    #[test]
    fn refusals_create_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = registry_path(&dir.path().join("config"));

        write(&dir.path().join("full/notes.txt"), "x");
        assert_eq!(
            setup_store(&registry, &request("full", &dir.path().join("full"), false))
                .expect_err("not empty")
                .code,
            "store_setup_not_empty"
        );

        let outer = dir.path().join("outer");
        std::fs::create_dir_all(&outer).expect("mkdir");
        git(&outer, &["init"]);
        assert_eq!(
            setup_store(&registry, &request("inner", &outer.join("inner"), false))
                .expect_err("nested")
                .code,
            "store_setup_inside_git_repo"
        );
        assert!(!outer.join("inner").exists());

        store(&dir.path().join("existing"), "taken");
        registry::register(&registry, &display(&dir.path().join("existing")), None)
            .expect("register");
        assert_eq!(
            setup_store(
                &registry,
                &request("taken", &dir.path().join("fresh"), false)
            )
            .expect_err("id taken")
            .code,
            "store_id_taken"
        );
        assert!(!dir.path().join("fresh").exists());

        assert_eq!(
            setup_store(&registry, &request("Bad Id", &dir.path().join("x"), false))
                .expect_err("id")
                .code,
            "invalid_store_id"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_commit_rolls_back_everything_setup_created() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = registry_path(&dir.path().join("config"));
        let root = dir.path().join("eng");
        std::fs::create_dir_all(&root).expect("mkdir");
        git(&root, &["init", "-b", "main"]);
        for (k, v) in [
            ("user.name", "t"),
            ("user.email", "t@t"),
            ("commit.gpgsign", "false"),
        ] {
            git(&root, &["config", k, v]);
        }
        let hook = root.join(".git/hooks/pre-commit");
        std::fs::write(&hook, "#!/bin/sh\nexit 1\n").expect("hook");
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        assert_eq!(
            setup_store(&registry, &request("eng", &root, true))
                .expect_err("hook refuses")
                .code,
            "store_git_commit_failed"
        );
        let left: Vec<_> = std::fs::read_dir(&root)
            .expect("dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            left,
            [".git"],
            "the user's repository stays, setup's files go"
        );
        assert!(registry::list(&registry).expect("list").is_empty());
    }
}
