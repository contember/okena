//! Starting a repository: `git init`, a commit of exactly the given paths, and
//! checking up front that a commit can be made at all.

use std::path::Path;

use okena_core::process::{command, safe_output};

use super::{path_str, require_success};
use crate::error::GitResult;

/// A `.git` directory or file (worktree, submodule) at exactly `path`.
///
/// Check this before probing a folder that is meant to be its own repository:
/// git discovers repositories by walking *up*, so a plain folder nested inside
/// another checkout would answer with that checkout's branch and remote.
pub fn is_repository_at_root(path: &Path) -> bool {
    let dot_git = path.join(".git");
    dot_git.is_dir() || dot_git.is_file()
}

/// Whether `git commit` run in `dir` would find an author and a committer.
///
/// `git var` resolves identity exactly as `commit` does, so a caller can fail
/// before creating anything rather than after.
pub fn has_commit_identity(dir: &Path) -> bool {
    ["GIT_COMMITTER_IDENT", "GIT_AUTHOR_IDENT"]
        .iter()
        .all(|var| {
            safe_output(command("git").arg("var").arg(var).current_dir(dir))
                .is_ok_and(|output| output.status.success())
        })
}

/// `git init` in `path`, which must exist.
pub fn init_repository(path: &Path) -> GitResult<()> {
    let p = path_str(path)?;
    require_success(safe_output(command("git").args(["init", p]))?)
}

/// Stage and commit exactly `pathspecs`.
///
/// Anything else the user had staged stays out of the commit and stays staged.
/// When the commit itself fails (a hook, signing) the paths are unstaged again,
/// so a failure leaves the index as it was.
pub fn commit_paths(path: &Path, message: &str, pathspecs: &[&str]) -> GitResult<()> {
    let p = path_str(path)?;
    require_success(safe_output(
        command("git").args(["-C", p, "add", "--"]).args(pathspecs),
    )?)?;
    let committed = safe_output(
        command("git")
            .args(["-C", p, "commit", "-m", message, "--"])
            .args(pathspecs),
    )
    .map_err(Into::into)
    .and_then(require_success);
    if committed.is_err() {
        // Best effort; on an unborn branch there is nothing to reset to, and
        // the caller removing the repository is the cleanup that matters.
        let _ = safe_output(
            command("git")
                .args(["-C", p, "reset", "-q", "--"])
                .args(pathspecs),
        );
    }
    committed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::test_support::git_in;

    fn git_out(repo: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .expect("git");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// A fresh repository with a local identity, so commits made through the
    /// functions under test don't depend on the machine's global config.
    fn fresh_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        init_repository(dir.path()).expect("init");
        for (key, value) in [
            ("user.name", "test"),
            ("user.email", "test@test"),
            ("commit.gpgsign", "false"),
        ] {
            git_in(dir.path(), &["config", key, value]);
        }
        dir
    }

    #[test]
    fn only_a_folder_with_its_own_dot_git_is_a_repository_root() {
        let dir = fresh_repo();
        assert!(is_repository_at_root(dir.path()));
        std::fs::create_dir_all(dir.path().join("nested")).expect("mkdir");
        assert!(!is_repository_at_root(&dir.path().join("nested")));
        assert!(has_commit_identity(dir.path()));
    }

    #[test]
    fn commit_paths_commits_only_what_it_was_given_and_leaves_other_staging() {
        let dir = fresh_repo();
        let repo = dir.path();
        std::fs::create_dir_all(repo.join("docs")).expect("mkdir");
        std::fs::write(repo.join("docs/a.md"), "a").expect("write");
        std::fs::write(repo.join("staged.txt"), "s").expect("write");
        git_in(repo, &["add", "staged.txt"]);

        commit_paths(repo, "Initialize", &["docs"]).expect("commit");
        assert_eq!(
            git_out(repo, &["show", "--name-only", "--format=", "HEAD"]).trim(),
            "docs/a.md"
        );
        assert_eq!(
            git_out(repo, &["diff", "--cached", "--name-only"]).trim(),
            "staged.txt"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_refused_commit_unstages_its_paths() {
        use std::os::unix::fs::PermissionsExt;
        let dir = fresh_repo();
        let repo = dir.path();
        commit_paths(repo, "base", &[]).ok();
        std::fs::write(repo.join("first.txt"), "1").expect("write");
        commit_paths(repo, "first", &["first.txt"]).expect("first commit");

        let hook = repo.join(".git/hooks/pre-commit");
        std::fs::write(&hook, "#!/bin/sh\nexit 1\n").expect("hook");
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        std::fs::write(repo.join("second.txt"), "2").expect("write");

        assert!(commit_paths(repo, "second", &["second.txt"]).is_err());
        assert_eq!(
            git_out(repo, &["diff", "--cached", "--name-only"]).trim(),
            ""
        );
    }
}
