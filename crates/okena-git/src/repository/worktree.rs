//! Worktree operations: create / remove / list.

use std::path::{Path, PathBuf};

use okena_core::process::{command, safe_output};

use super::branch::get_default_branch;
use super::{head_branch_short, path_str, require_success};
use crate::error::{GitError, GitResult};

#[derive(Clone, Debug, PartialEq, Eq)]
enum FilesystemObjectIdentity {
    #[cfg(unix)]
    Unix { device: u64, inode: u64 },
    #[cfg(windows)]
    Windows { volume: u32, file: u64 },
    #[cfg(not(any(unix, windows)))]
    Canonical(PathBuf),
}

fn filesystem_object_identity(path: &Path) -> Option<FilesystemObjectIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;

        let metadata = std::fs::metadata(path).ok()?;
        Some(FilesystemObjectIdentity::Unix {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;

        let metadata = std::fs::metadata(path).ok()?;
        match (metadata.volume_serial_number(), metadata.file_index()) {
            (Some(volume), Some(file)) => Some(FilesystemObjectIdentity::Windows { volume, file }),
            _ => None,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        std::fs::canonicalize(path)
            .ok()
            .map(FilesystemObjectIdentity::Canonical)
    }
}

/// Freshly verified ownership of one linked worktree checkout.
///
/// This token is intentionally produced without the status-path repository
/// cache and carries the checkout directory's filesystem identity. Destructive
/// operations revalidate both before touching the path.
#[derive(Clone, Debug)]
pub struct VerifiedWorktree {
    parent_path: PathBuf,
    checkout_path: PathBuf,
    identity: FilesystemObjectIdentity,
}

impl VerifiedWorktree {
    pub fn checkout_path(&self) -> &Path {
        &self.checkout_path
    }

    pub fn parent_path(&self) -> &Path {
        &self.parent_path
    }
}

fn unsafe_worktree(path: &Path, reason: impl Into<String>) -> GitError {
    GitError::UnsafeWorktree {
        path: path.to_path_buf(),
        reason: reason.into(),
    }
}

fn fresh_repo(path: &Path) -> GitResult<gix::Repository> {
    gix::ThreadSafeRepository::discover(path)
        .map(|repository| repository.to_thread_local())
        .map_err(|error| unsafe_worktree(path, format!("repository discovery failed: {error}")))
}

/// Verify from fresh filesystem and Git metadata that `checkout_path` is a
/// linked worktree registered by `parent_path`.
pub fn verify_linked_worktree_fresh(
    parent_path: &Path,
    checkout_path: &Path,
) -> GitResult<VerifiedWorktree> {
    let parent_repo = fresh_repo(parent_path)?;
    let checkout_repo = fresh_repo(checkout_path)?;
    let checkout_root = checkout_repo
        .workdir()
        .ok_or_else(|| unsafe_worktree(checkout_path, "checkout repository has no work directory"))?
        .to_path_buf();
    let identity = filesystem_object_identity(&checkout_root).ok_or_else(|| {
        unsafe_worktree(
            &checkout_root,
            "checkout filesystem identity is unavailable",
        )
    })?;
    let parent_common = filesystem_object_identity(parent_repo.common_dir()).ok_or_else(|| {
        unsafe_worktree(parent_path, "parent Git directory identity is unavailable")
    })?;
    let checkout_common =
        filesystem_object_identity(checkout_repo.common_dir()).ok_or_else(|| {
            unsafe_worktree(
                checkout_path,
                "checkout Git directory identity is unavailable",
            )
        })?;
    if parent_common != checkout_common {
        return Err(unsafe_worktree(
            checkout_path,
            "checkout does not belong to the parent repository",
        ));
    }

    let registered = parent_repo
        .worktrees()
        .map_err(|error| {
            unsafe_worktree(parent_path, format!("linked worktree list failed: {error}"))
        })?
        .into_iter()
        .filter_map(|proxy| proxy.base().ok())
        .filter_map(|path| filesystem_object_identity(&path))
        .any(|registered| registered == identity);
    if !registered {
        return Err(unsafe_worktree(
            checkout_path,
            "checkout is not registered as a linked worktree",
        ));
    }

    Ok(VerifiedWorktree {
        parent_path: parent_path.to_path_buf(),
        checkout_path: checkout_root,
        identity,
    })
}

fn revalidate_verified_worktree(verified: &VerifiedWorktree) -> GitResult<()> {
    let current = verify_linked_worktree_fresh(&verified.parent_path, &verified.checkout_path)?;
    if current.identity != verified.identity {
        return Err(unsafe_worktree(
            &verified.checkout_path,
            "checkout directory identity changed",
        ));
    }
    Ok(())
}

/// Remove only a directory that is absent, empty, or contains regular
/// `.DS_Store` files. This handles Finder metadata recreated after the verified
/// checkout was quarantined without ever deleting a replacement directory.
fn remove_benign_residual(path: &Path) -> std::io::Result<bool> {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    };

    let mut ds_store_files = Vec::new();
    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if entry.file_name() != ".DS_Store" || !file_type.is_file() || file_type.is_symlink() {
            return Ok(false);
        }
        ds_store_files.push(entry.path());
    }
    for ds_store in ds_store_files {
        match std::fs::remove_file(ds_store) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    match std::fs::remove_dir(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        // A concurrent Finder write is harmless only if a subsequent inspection
        // again proves the residual is exclusively benign metadata.
        Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => Ok(false),
        Err(error) => Err(error),
    }
}

fn cleanup_benign_residual(path: &Path) -> GitResult<()> {
    match remove_benign_residual(path) {
        Ok(true) => Ok(()),
        Ok(false) => Err(unsafe_worktree(
            path,
            "checkout path was recreated with non-benign content; preserved it",
        )),
        Err(source) => Err(GitError::RemoveFailed {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Refuse every existing target. An unregistered directory is not proof that
/// Okena owns its contents, so create must never remove it speculatively.
fn require_absent_worktree_target(target_path: &Path) -> GitResult<()> {
    if target_path.exists() {
        return Err(GitError::WorktreeExists {
            path: target_path.to_path_buf(),
        });
    }
    Ok(())
}

/// Create a new worktree.
pub fn create_worktree(
    repo_path: &Path,
    branch: &str,
    target_path: &Path,
    create_branch: bool,
) -> GitResult<()> {
    crate::validate_git_ref(branch)?;
    require_absent_worktree_target(target_path)?;

    let repo_str = path_str(repo_path)?;
    let target_str = path_str(target_path)?;

    let mut args = vec!["-C", repo_str, "worktree", "add"];

    // When creating a new branch, fetch the remote default branch first,
    // then base the worktree on origin/{default} so it starts from the
    // latest remote state instead of a potentially stale local ref.
    let start_point;
    if create_branch {
        args.push("-b");
        args.push(branch);
        args.push(target_str);
        if let Some(default_branch) = get_default_branch(repo_path) {
            let _ = safe_output(command("git").args([
                "-C",
                repo_str,
                "fetch",
                "origin",
                &default_branch,
            ]));
            start_point = format!("origin/{}", default_branch);
            args.push(&start_point);
        }
    } else {
        args.push(target_str);
        args.push(branch);
    }

    let output = safe_output(command("git").args(&args))?;
    require_success(output)
}

/// Create a new worktree with an optional pre-fetched start point.
/// If `start_branch` is Some, creates `-b <branch> <target> origin/<start_branch>`
/// without re-fetching (caller is expected to have fetched already).
pub fn create_worktree_with_start_point(
    repo_path: &Path,
    branch: &str,
    target_path: &Path,
    start_branch: Option<&str>,
) -> GitResult<()> {
    crate::validate_git_ref(branch)?;
    if let Some(sb) = start_branch {
        crate::validate_git_ref(sb)?;
    }
    require_absent_worktree_target(target_path)?;

    let repo_str = path_str(repo_path)?;
    let target_str = path_str(target_path)?;

    let mut args = vec!["-C", repo_str, "worktree", "add", "-b", branch, target_str];

    let start_point;
    if let Some(sb) = start_branch {
        start_point = format!("origin/{}", sb);
        args.push(&start_point);
    }

    let output = safe_output(command("git").args(&args))?;
    require_success(output)
}

/// Best-effort freshen a just-created worktree to the latest remote default:
/// `git fetch origin <default_branch>`, then fast-forward the worktree's branch
/// to `origin/<default_branch>` with `merge --ff-only`.
///
/// This lets the worktree window appear immediately (created from the LOCAL
/// `origin/<default>` with no blocking fetch) and then catch up to the true
/// remote tip in the background. `--ff-only` NEVER rewrites local work: if the
/// branch has diverged (a commit was made) or the tree is dirty in a conflicting
/// way, git declines and this is a safe no-op. All failures are non-fatal (the
/// worktree simply stays on the local base) — logged, not returned.
pub fn fetch_and_fast_forward(repo_path: &Path, worktree_path: &Path, default_branch: &str) {
    let (Ok(repo_str), Ok(wt_str)) = (path_str(repo_path), path_str(worktree_path)) else {
        return;
    };
    match safe_output(command("git").args(["-C", repo_str, "fetch", "origin", default_branch])) {
        Ok(out) if out.status.success() => {}
        Ok(out) => {
            log::warn!(
                "worktree freshen: fetch origin {default_branch} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            return;
        }
        Err(e) => {
            log::warn!("worktree freshen: fetch origin {default_branch} failed: {e}");
            return;
        }
    }
    let start = format!("origin/{}", default_branch);
    match safe_output(command("git").args(["-C", wt_str, "merge", "--ff-only", &start])) {
        Ok(out) if out.status.success() => {}
        Ok(out) => log::info!(
            "worktree freshen: fast-forward to {start} skipped (branch diverged or dirty): {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(e) => log::warn!("worktree freshen: fast-forward merge failed: {e}"),
    }
}

/// Remove a worktree.
pub fn remove_worktree(verified: &VerifiedWorktree, force: bool) -> GitResult<()> {
    revalidate_verified_worktree(verified)?;
    let wt_str = path_str(&verified.checkout_path)?;

    let mut args = vec!["-C", wt_str, "worktree", "remove"];

    if force {
        args.push("--force");
    }

    args.push(wt_str);

    let output = safe_output(command("git").args(&args))?;
    require_success(output)
}

/// Fast worktree removal: quarantine the verified directory, delete it, and
/// prune stale worktree metadata.
/// Much faster than `git worktree remove` which does expensive status checks.
/// Only safe when the caller has already handled dirty state (stash/discard).
///
/// Note: `git worktree prune` removes ALL stale entries (not just the one we deleted).
/// This is safe because prune only acts on entries whose directories no longer exist,
/// and we only delete the single target directory before pruning.
pub fn remove_worktree_fast(verified: &VerifiedWorktree) -> GitResult<()> {
    remove_worktree_fast_with(verified, |path| std::fs::remove_dir_all(path))
}

fn remove_worktree_fast_with(
    verified: &VerifiedWorktree,
    remove_dir_all: impl FnOnce(&Path) -> std::io::Result<()>,
) -> GitResult<()> {
    revalidate_verified_worktree(verified)?;
    let worktree_path = &verified.checkout_path;
    let parent = worktree_path
        .parent()
        .ok_or_else(|| unsafe_worktree(worktree_path, "checkout directory has no parent"))?;
    let quarantine = parent.join(format!(".okena-removing-{}", uuid::Uuid::new_v4()));
    std::fs::rename(worktree_path, &quarantine).map_err(|source| GitError::RemoveFailed {
        path: worktree_path.clone(),
        source,
    })?;

    let quarantined_identity = filesystem_object_identity(&quarantine);
    if quarantined_identity.as_ref() != Some(&verified.identity) {
        let restore = if !worktree_path.exists() {
            std::fs::rename(&quarantine, worktree_path)
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "original checkout path was claimed during quarantine",
            ))
        };
        let reason = match restore {
            Ok(()) => "quarantined directory identity changed; original path restored".to_string(),
            Err(error) => format!(
                "quarantined directory identity changed; data preserved at '{}': {error}",
                quarantine.display()
            ),
        };
        return Err(unsafe_worktree(worktree_path, reason));
    }

    match remove_dir_all(&quarantine) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            // `remove_dir_all` can have already removed the checkout and leave
            // only Finder metadata behind. Delete that narrow, verified class of
            // debris; otherwise restore the still-owned quarantine and fail closed.
            if let Err(cleanup_error) = cleanup_benign_residual(&quarantine) {
                let source = match std::fs::rename(&quarantine, worktree_path) {
                    Ok(()) => std::io::Error::other(format!(
                        "{error}; residual cleanup refused: {cleanup_error}"
                    )),
                    Err(restore_error) => std::io::Error::new(
                        error.kind(),
                        format!(
                            "{error}; residual cleanup refused: {cleanup_error}; remaining checkout preserved at '{}'; restore failed: {restore_error}",
                            quarantine.display()
                        ),
                    ),
                };
                return Err(GitError::RemoveFailed {
                    path: worktree_path.to_path_buf(),
                    source,
                });
            }
        }
    }

    // A process such as Finder can recreate the old path after the atomic
    // quarantine. It is safe to delete only an empty directory or `.DS_Store`;
    // any other replacement is foreign data and must survive without pruning.
    cleanup_benign_residual(worktree_path)?;

    // Prune stale worktree entries from the main repo
    let main_str = path_str(&verified.parent_path)?;
    let output = safe_output(command("git").args(["-C", main_str, "worktree", "prune"]))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::warn!("git worktree prune warning: {}", stderr.trim());
    }

    Ok(())
}

/// Move a verified linked worktree and return a fresh token for its new root.
pub fn move_worktree(verified: &VerifiedWorktree, new_path: &Path) -> GitResult<VerifiedWorktree> {
    revalidate_verified_worktree(verified)?;
    require_absent_worktree_target(new_path)?;
    let parent = path_str(&verified.parent_path)?;
    let old = path_str(&verified.checkout_path)?;
    let new = path_str(new_path)?;
    let output = safe_output(command("git").args(["-C", parent, "worktree", "move", old, new]))?;
    require_success(output)?;

    match verify_linked_worktree_fresh(&verified.parent_path, new_path) {
        Ok(moved) => Ok(moved),
        Err(error) => {
            let rollback =
                safe_output(command("git").args(["-C", parent, "worktree", "move", new, old]))
                    .map_err(GitError::from)
                    .and_then(require_success);
            match rollback {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(unsafe_worktree(
                    new_path,
                    format!(
                        "post-move verification failed: {error}; rollback failed: {rollback_error}"
                    ),
                )),
            }
        }
    }
}

/// List all worktrees in a repository (main + linked). Returns vec of
/// (path, branch_name) pairs; detached worktrees are omitted.
pub fn list_git_worktrees(repo_path: &Path) -> Vec<(String, String)> {
    let Some(repo) = crate::gix_helpers::open(repo_path) else {
        return vec![];
    };

    let mut result = Vec::new();

    // Main worktree: open via common_dir, which always resolves to the main
    // repository even when `repo_path` lives in a linked worktree.
    if let Ok(main_repo) = gix::open(repo.common_dir())
        && let (Some(workdir), Some(branch)) = (main_repo.workdir(), head_branch_short(&main_repo))
    {
        result.push((workdir.to_string_lossy().into_owned(), branch));
    }

    // Linked worktrees from .git/worktrees/*.
    if let Ok(worktrees) = repo.worktrees() {
        for proxy in worktrees {
            let Some(workdir) = proxy.base().ok() else {
                continue;
            };
            let Ok(wt_repo) = proxy.into_repo_with_possibly_inaccessible_worktree() else {
                continue;
            };
            if let Some(branch) = head_branch_short(&wt_repo) {
                result.push((workdir.to_string_lossy().into_owned(), branch));
            }
        }
    }

    result
}

/// List the paths Git registers as linked worktrees for a repository.
/// The main worktree is intentionally excluded.
pub fn list_linked_worktree_paths(repo_path: &Path) -> Vec<PathBuf> {
    let Some(repo) = crate::gix_helpers::open(repo_path) else {
        return Vec::new();
    };
    // macOS exposes `/var` through `/private/var`. gix may report either spelling
    // for the main worktree, so compare existing paths by canonical filesystem
    // identity instead of lexical components. Missing paths retain the portable
    // lexical fallback used elsewhere in this module.
    let main_worktree = repo.workdir().map(path_identity);
    let Ok(worktrees) = repo.worktrees() else {
        return Vec::new();
    };
    worktrees
        .into_iter()
        .filter_map(|proxy| proxy.base().ok())
        .filter(|path| main_worktree.as_ref() != Some(&path_identity(path)))
        .collect()
}

fn path_identity(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| crate::repository::normalize_path(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::test_support::{git_in, init_temp_repo};
    use std::path::PathBuf;

    #[test]
    fn list_git_worktrees_returns_empty_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(list_git_worktrees(&path).is_empty());
    }

    #[test]
    fn list_git_worktrees_returns_main_plus_linked() {
        let (_tmp, repo) = init_temp_repo();
        let wt_tmp = tempfile::tempdir().expect("create worktree tempdir");
        let wt_path = wt_tmp.path().join("wt-feat");
        git_in(
            &repo,
            &["worktree", "add", wt_path.to_str().unwrap(), "-b", "feat"],
        );

        let mut entries = list_git_worktrees(&repo);
        entries.sort_by(|a, b| a.1.cmp(&b.1));
        let branches: Vec<&str> = entries.iter().map(|(_, b)| b.as_str()).collect();
        assert_eq!(branches, vec!["feat", "main"]);
    }

    #[test]
    fn path_identity_prefers_canonical_filesystem_path() {
        let directory = tempfile::tempdir().expect("create identity directory");
        let dotted = directory.path().join(".");
        assert_eq!(
            path_identity(&dotted),
            directory.path().canonicalize().unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn path_identity_matches_a_directory_symlink_alias() {
        let parent = tempfile::tempdir().expect("create identity parent");
        let actual = parent.path().join("actual");
        let alias = parent.path().join("alias");
        std::fs::create_dir(&actual).expect("create actual directory");
        std::os::unix::fs::symlink(&actual, &alias).expect("create directory alias");

        assert_eq!(path_identity(&actual), path_identity(&alias));
    }

    #[test]
    fn list_linked_worktree_paths_excludes_main_worktree() {
        let (_tmp, repo) = init_temp_repo();
        let wt_tmp = tempfile::tempdir().expect("create worktree tempdir");
        let wt_path = wt_tmp.path().join("wt-feat");
        git_in(
            &repo,
            &["worktree", "add", wt_path.to_str().unwrap(), "-b", "feat"],
        );

        assert_eq!(
            list_linked_worktree_paths(&repo)
                .iter()
                .map(|path| path_identity(path))
                .collect::<Vec<_>>(),
            vec![path_identity(&wt_path)]
        );
    }

    #[test]
    fn fresh_verification_rejects_a_path_replaced_after_cached_discovery() {
        let (_tmp, repo) = init_temp_repo();
        let wt_tmp = tempfile::tempdir().expect("create worktree tempdir");
        let wt_path = wt_tmp.path().join("wt-feat");
        let moved_path = wt_tmp.path().join("moved-feat");
        git_in(
            &repo,
            &["worktree", "add", wt_path.to_str().unwrap(), "-b", "feat"],
        );

        assert!(crate::repository::get_repo_root(&wt_path).is_some());
        assert!(crate::repository::get_repo_common_dir(&wt_path).is_some());
        std::fs::rename(&wt_path, &moved_path).expect("move original checkout");
        std::fs::create_dir(&wt_path).expect("create replacement directory");
        let sentinel = wt_path.join("must-survive.txt");
        std::fs::write(&sentinel, "independent data").expect("write sentinel");

        assert!(verify_linked_worktree_fresh(&repo, &wt_path).is_err());
        assert_eq!(
            std::fs::read_to_string(sentinel).expect("replacement survives"),
            "independent data"
        );
    }

    #[test]
    fn benign_residual_cleanup_accepts_ds_store_and_absence() {
        let parent = tempfile::tempdir().expect("create residual parent");
        let residual = parent.path().join("worktree");
        std::fs::create_dir(&residual).expect("create residual");
        std::fs::write(residual.join(".DS_Store"), "finder metadata").expect("write metadata");

        assert!(remove_benign_residual(&residual).expect("remove benign metadata"));
        assert!(!residual.exists());
        assert!(remove_benign_residual(&residual).expect("already absent is benign"));
    }

    #[test]
    fn benign_residual_cleanup_preserves_foreign_replacement() {
        let parent = tempfile::tempdir().expect("create residual parent");
        let residual = parent.path().join("worktree");
        std::fs::create_dir(&residual).expect("create residual");
        let sentinel = residual.join("must-survive.txt");
        std::fs::write(&sentinel, "foreign data").expect("write sentinel");

        assert!(!remove_benign_residual(&residual).expect("inspect foreign residual"));
        assert_eq!(
            std::fs::read_to_string(sentinel).expect("sentinel survives"),
            "foreign data"
        );
    }

    #[test]
    fn fast_removal_cleans_partial_ds_store_residual_and_preserves_old_path_replacement() {
        let (_tmp, repo) = init_temp_repo();
        let wt_tmp = tempfile::tempdir().expect("create worktree tempdir");
        let wt_path = wt_tmp.path().join("wt-feat");
        git_in(
            &repo,
            &["worktree", "add", wt_path.to_str().unwrap(), "-b", "feat"],
        );
        let verified = verify_linked_worktree_fresh(&repo, &wt_path).expect("verify worktree");
        let replacement_path = wt_path.clone();

        let result = remove_worktree_fast_with(&verified, |quarantine| {
            std::fs::remove_dir_all(quarantine).expect("remove quarantined checkout contents");
            std::fs::create_dir(quarantine).expect("recreate partial quarantine residual");
            std::fs::write(quarantine.join(".DS_Store"), "finder metadata")
                .expect("write partial residual");
            std::fs::create_dir(&replacement_path).expect("recreate old checkout path");
            std::fs::write(replacement_path.join("must-survive.txt"), "foreign data")
                .expect("write replacement sentinel");
            Err(std::io::Error::other(
                "simulated partial remove_dir_all failure",
            ))
        });

        assert!(
            result.is_err(),
            "foreign old-path replacement must stop pruning"
        );
        assert!(
            !wt_tmp
                .path()
                .read_dir()
                .expect("inspect worktree parent")
                .filter_map(Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".okena-removing-")),
            "benign .DS_Store quarantine residual must be removed"
        );
        assert_eq!(
            std::fs::read_to_string(wt_path.join("must-survive.txt"))
                .expect("foreign replacement survives"),
            "foreign data"
        );
    }

    #[test]
    fn guarded_fast_removal_rejects_a_replaced_checkout() {
        let (_tmp, repo) = init_temp_repo();
        let wt_tmp = tempfile::tempdir().expect("create worktree tempdir");
        let wt_path = wt_tmp.path().join("wt-feat");
        let moved_path = wt_tmp.path().join("moved-feat");
        git_in(
            &repo,
            &["worktree", "add", wt_path.to_str().unwrap(), "-b", "feat"],
        );
        let verified = verify_linked_worktree_fresh(&repo, &wt_path).unwrap();
        std::fs::rename(&wt_path, &moved_path).expect("move original checkout");
        std::fs::create_dir(&wt_path).expect("create replacement directory");
        let sentinel = wt_path.join("must-survive.txt");
        std::fs::write(&sentinel, "independent data").expect("write sentinel");

        assert!(remove_worktree_fast(&verified).is_err());
        assert!(moved_path.exists());
        assert_eq!(
            std::fs::read_to_string(sentinel).expect("replacement survives"),
            "independent data"
        );
    }

    #[test]
    fn worktree_move_returns_fresh_ownership() {
        let (_tmp, repo) = init_temp_repo();
        let wt_tmp = tempfile::tempdir().expect("create worktree tempdir");
        let old_path = wt_tmp.path().join("wt-feat");
        let new_path = wt_tmp.path().join("renamed-feat");
        git_in(
            &repo,
            &["worktree", "add", old_path.to_str().unwrap(), "-b", "feat"],
        );
        let verified = verify_linked_worktree_fresh(&repo, &old_path).unwrap();

        let moved = move_worktree(&verified, &new_path).expect("move linked worktree");

        assert_eq!(moved.checkout_path(), new_path);
        assert!(!old_path.exists());
        assert!(verify_linked_worktree_fresh(&repo, &new_path).is_ok());
    }

    #[test]
    fn get_worktree_branches_returns_branch_names() {
        let (_tmp, repo) = init_temp_repo();
        let wt_tmp = tempfile::tempdir().expect("create worktree tempdir");
        let wt_path = wt_tmp.path().join("wt-feat");
        git_in(
            &repo,
            &["worktree", "add", wt_path.to_str().unwrap(), "-b", "feat"],
        );

        let mut branches = crate::repository::get_worktree_branches(&repo);
        branches.sort();
        assert_eq!(branches, vec!["feat", "main"]);
    }

    #[test]
    fn create_refuses_existing_unregistered_directory_without_deleting_it() {
        let (_tmp, repo) = init_temp_repo();
        let target_parent = tempfile::tempdir().expect("create target parent");
        let target = target_parent.path().join("existing-directory");
        std::fs::create_dir(&target).expect("create existing target");
        let sentinel = target.join("keep-me.txt");
        std::fs::write(&sentinel, "user data").expect("write sentinel");

        let result = create_worktree(&repo, "feature", &target, true);

        assert!(matches!(
            result,
            Err(GitError::WorktreeExists { ref path }) if path == &target
        ));
        assert_eq!(
            std::fs::read_to_string(sentinel).expect("existing data survives"),
            "user data"
        );
    }
}
