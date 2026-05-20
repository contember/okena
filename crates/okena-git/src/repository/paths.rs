//! Path utilities: repo-root resolution and worktree/project path computation.
//!
//! These are pure (no subprocesses) apart from `get_repo_root`, which opens
//! the repo via gix to find the work directory.

use std::path::{Component, Path, PathBuf};

/// Get the root directory of the git repository containing the given path.
/// Returns None if the path is not inside a git repository.
pub fn get_repo_root(path: &Path) -> Option<PathBuf> {
    let repo = crate::gix_helpers::open(path)?;
    repo.workdir().map(|p| p.to_path_buf())
}

/// Normalize a path by resolving `.` and `..` components without filesystem access.
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => { result.pop(); }
            Component::CurDir => {}
            other => result.push(other),
        }
    }
    result
}

/// Resolve the git repository root and the project subdirectory within it.
///
/// For a monorepo project at `/repo/packages/app`, returns
/// `(/repo, packages/app)`. For a root-level project, subdir is empty.
/// Both paths are normalized before `strip_prefix` to handle symlinks,
/// trailing slashes, and `..` components.
pub fn resolve_git_root_and_subdir(project_path: &Path) -> (PathBuf, PathBuf) {
    let git_root = get_repo_root(project_path)
        .unwrap_or_else(|| project_path.to_path_buf());
    let norm_project = normalize_path(project_path);
    let norm_root = normalize_path(&git_root);
    let subdir = norm_project.strip_prefix(&norm_root)
        .unwrap_or(Path::new(""))
        .to_path_buf();
    (git_root, subdir)
}

/// Given a worktree checkout path and a subdir, return the project path.
/// If subdir is empty, returns the worktree path as-is.
pub fn project_path_in_worktree(worktree_path: &str, subdir: &Path) -> String {
    if subdir.as_os_str().is_empty() {
        worktree_path.to_string()
    } else {
        PathBuf::from(worktree_path)
            .join(subdir)
            .to_string_lossy()
            .to_string()
    }
}

/// Compute worktree and project paths from template, git root, and subdir.
/// Returns (worktree_path, project_path).
pub fn compute_target_paths(
    git_root: &Path,
    subdir: &Path,
    template: &str,
    branch: &str,
) -> (String, String) {
    let repo_name = git_root.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");
    let safe_branch = branch.replace('/', "-");

    let expanded = template
        .replace("{repo}", repo_name)
        .replace("{branch}", &safe_branch);

    let worktree_path = {
        let path = PathBuf::from(&expanded);
        if path.is_relative() {
            normalize_path(&git_root.join(&expanded))
                .to_string_lossy()
                .to_string()
        } else {
            expanded
        }
    };

    let project_path = project_path_in_worktree(&worktree_path, subdir);

    (worktree_path, project_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::test_support::{git_in, init_temp_repo};

    #[test]
    fn get_repo_root_returns_none_for_invalid_path() {
        let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(get_repo_root(&path).is_none());
    }

    /// Compare computed paths as `Path` objects for cross-platform correctness
    fn assert_paths_eq(actual: &str, expected: &Path) {
        assert_eq!(Path::new(actual), expected);
    }

    #[test]
    fn target_path_simple_repo() {
        let git_root = PathBuf::from("/projects/myrepo");
        let subdir = Path::new("");
        let (wt, proj) = compute_target_paths(&git_root, subdir, "../{repo}-wt/{branch}", "feature");
        let expected = PathBuf::from("/projects").join("myrepo-wt").join("feature");
        assert_paths_eq(&wt, &expected);
        assert_paths_eq(&proj, &expected);
    }

    #[test]
    fn target_path_monorepo() {
        let git_root = PathBuf::from("/projects/monorepo");
        let subdir = Path::new("app-in-monorepo");
        let (wt, proj) = compute_target_paths(&git_root, subdir, "../{repo}-wt/{branch}", "feature");
        let expected_wt = PathBuf::from("/projects").join("monorepo-wt").join("feature");
        assert_paths_eq(&wt, &expected_wt);
        assert_paths_eq(&proj, &expected_wt.join("app-in-monorepo"));
    }

    #[test]
    fn target_path_nested_monorepo_subdir() {
        let git_root = PathBuf::from("/projects/monorepo");
        let subdir = Path::new("packages/app");
        let (wt, proj) = compute_target_paths(&git_root, subdir, "../{repo}-wt/{branch}", "fix-bug");
        let expected_wt = PathBuf::from("/projects").join("monorepo-wt").join("fix-bug");
        assert_paths_eq(&wt, &expected_wt);
        assert_paths_eq(&proj, &expected_wt.join("packages").join("app"));
    }

    #[test]
    fn target_path_absolute_template() {
        let git_root = PathBuf::from("/projects/monorepo");
        let subdir = Path::new("app");
        let (wt, proj) = compute_target_paths(&git_root, subdir, "/tmp/worktrees/{repo}/{branch}", "main");
        let expected_wt = PathBuf::from("/tmp").join("worktrees").join("monorepo").join("main");
        assert_paths_eq(&wt, &expected_wt);
        assert_paths_eq(&proj, &expected_wt.join("app"));
    }

    #[test]
    fn target_path_branch_with_slashes() {
        let git_root = PathBuf::from("/projects/repo");
        let subdir = Path::new("");
        let (wt, proj) = compute_target_paths(&git_root, subdir, "../{repo}-wt/{branch}", "feature/my-branch");
        let expected = PathBuf::from("/projects").join("repo-wt").join("feature-my-branch");
        assert_paths_eq(&wt, &expected);
        assert_paths_eq(&proj, &expected);
    }

    // ─── get_repo_root worktree / monorepo tests ───────────────────────

    #[test]
    fn get_repo_root_returns_toplevel_for_subdirectory() {
        let (_tmp, repo) = init_temp_repo();
        let sub = repo.join("packages").join("app");
        std::fs::create_dir_all(&sub).unwrap();

        let root = get_repo_root(&sub).expect("should resolve repo root");
        assert_eq!(root.canonicalize().unwrap(), repo.canonicalize().unwrap());
    }

    #[test]
    fn get_repo_root_resolves_worktree_root_not_subdir() {
        let (_tmp, repo) = init_temp_repo();
        // Worktree lives in its own tempdir so parallel runs don't collide on
        // a shared /tmp path that survives between runs.
        let wt_tmp = tempfile::tempdir().expect("create worktree tempdir");
        let wt_path = wt_tmp.path().join("my-worktree");
        git_in(
            &repo,
            &["worktree", "add", wt_path.to_str().unwrap(), "-b", "wt-branch"],
        );

        // Create a nested subdirectory inside the worktree (monorepo subproject)
        let nested = wt_path.join("packages").join("app");
        std::fs::create_dir_all(&nested).unwrap();

        // get_repo_root from the nested subdir should return the worktree root,
        // NOT the main repo — this is the path `git worktree remove` needs.
        let root = get_repo_root(&nested).expect("should resolve worktree root");
        assert_eq!(root.canonicalize().unwrap(), wt_path.canonicalize().unwrap());
    }
}
