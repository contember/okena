//! Clone a remote repository into a fresh directory.

use std::path::Path;

use okena_core::process::{CommandBus, CommandHandle, CommandSpec, Lane};

use super::{network_command, path_str, require_success};
use crate::error::{GitError, GitResult};

/// Validate that a clone URL cannot be read by git as an option.
///
/// `git clone` is invoked with a `--` separator, so a leading `-` is already
/// harmless there; this rejects it anyway so the bad input surfaces as a clear
/// error instead of a confusing git failure. Empty URLs are rejected too.
pub fn validate_clone_url(url: &str) -> GitResult<&str> {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed.starts_with('-') {
        return Err(GitError::InvalidUrl(url.to_string()));
    }
    Ok(trimmed)
}

/// Derive the directory name `git clone <url>` would create.
///
/// Mirrors git's own rule: take the last non-empty path segment and drop a
/// trailing `.git`. Handles both URL forms (`https://host/a/b.git`) and the
/// scp-like form (`git@host:a/b.git`). Returns `None` when nothing usable is
/// left, so callers can ask the user for a name instead of guessing.
pub fn clone_dir_name(url: &str) -> Option<String> {
    let url = url.trim();
    // Strip a fragment/query before splitting — `?ref=x` is not part of the name.
    let url = url.split(['?', '#']).next().unwrap_or(url);
    // For a `scheme://host/path` URL the name comes from the path, so drop the
    // scheme and host first — otherwise a hostless `https://` would yield the
    // scheme itself. Everything else (scp-like `git@host:a/b`, a local path) is
    // already just a path.
    let path = match url.split_once("://") {
        Some((_scheme, rest)) => rest.split_once('/')?.1,
        None => url,
    };
    // Both `/` and `:` separate the repo from its host in the forms git accepts.
    let name = path
        .trim_end_matches('/')
        .rsplit(['/', ':', '\\'])
        .find(|segment| !segment.is_empty())?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    Some(name.to_string())
}

/// Reject a clone target that already holds something.
///
/// `git clone` refuses a non-empty directory itself, but checking up-front
/// keeps the failure fast and the message ours.
fn require_absent_clone_target(target_path: &Path) -> GitResult<()> {
    let occupied = match std::fs::read_dir(target_path) {
        Ok(mut entries) => entries.next().is_some(),
        // Not a directory (a file sits there) still counts as occupied.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => target_path.exists(),
    };
    if occupied {
        return Err(GitError::CloneTargetExists {
            path: target_path.to_path_buf(),
        });
    }
    Ok(())
}

/// Submit `git clone <url> <target_path>` to the process bus.
///
/// Runs on [`Lane::Long`]: a clone is network-bound and unbounded in duration,
/// so it must never occupy an interactive or poller slot.
pub fn start_clone_repository(url: &str, target_path: &Path) -> GitResult<CommandHandle> {
    let url = validate_clone_url(url)?;
    require_absent_clone_target(target_path)?;

    let target_str = path_str(target_path)?;
    if let Some(parent) = target_path.parent() {
        std::fs::create_dir_all(parent).map_err(GitError::CommandFailed)?;
    }

    let mut cmd = network_command();
    cmd.args(["clone", "--", url, target_str]);
    Ok(CommandBus::global().submit(
        CommandSpec::from_command(&cmd)
            .lane(Lane::Long)
            .label("git clone"),
    ))
}

/// Wait for a clone submitted by [`start_clone_repository`].
pub fn finish_clone_repository(handle: CommandHandle) -> GitResult<()> {
    require_success(handle.wait()?)
}

/// Whether a clone into `path` ran all the way to a checked-out commit.
///
/// `git clone` is not atomic: killed mid-fetch it leaves the target directory
/// behind holding a `.git` whose HEAD points at a branch that does not exist
/// yet. That wreckage is indistinguishable from a finished clone by existence
/// alone, so startup recovery asks this instead — otherwise it promotes a
/// repo with no files in it to a normal-looking project.
///
/// Resolving HEAD is the line between the two: git writes it only once the
/// fetch has landed the branch it is about to check out.
pub fn is_complete_checkout(path: &Path) -> bool {
    gix::open(path).is_ok_and(|repo| repo.head_id().is_ok())
}

/// Submit and synchronously wait for `git clone <url> <target_path>`.
pub fn clone_repository(url: &str, target_path: &Path) -> GitResult<()> {
    finish_clone_repository(start_clone_repository(url, target_path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_the_directory_name_git_would_use() {
        let cases = [
            ("https://github.com/user/okena.git", "okena"),
            ("https://github.com/user/okena", "okena"),
            ("https://github.com/user/okena/", "okena"),
            ("git@github.com:user/okena.git", "okena"),
            ("ssh://git@host:2222/user/okena.git", "okena"),
            ("/srv/repos/okena.git", "okena"),
            ("https://host/user/okena.git?ref=main", "okena"),
            ("  https://host/user/okena.git  ", "okena"),
        ];
        for (url, expected) in cases {
            assert_eq!(clone_dir_name(url).as_deref(), Some(expected), "url: {url}");
        }
    }

    #[test]
    fn rejects_urls_with_no_usable_name() {
        for url in ["", "   ", "/", "https://", "https://host/", "../"] {
            assert_eq!(clone_dir_name(url), None, "url: {url}");
        }
    }

    #[test]
    fn rejects_option_like_and_empty_urls() {
        assert!(validate_clone_url("--upload-pack=evil").is_err());
        assert!(validate_clone_url("-x").is_err());
        assert!(validate_clone_url("   ").is_err());
        assert_eq!(
            validate_clone_url("  https://host/a.git ").ok(),
            Some("https://host/a.git")
        );
    }

    #[test]
    fn an_interrupted_clone_is_not_mistaken_for_a_finished_one() {
        use crate::repository::test_support::{git_in, init_temp_repo};

        // A finished checkout: HEAD resolves.
        let (_tmp, repo) = init_temp_repo();
        std::fs::write(repo.join("file.txt"), "a\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(
            &repo,
            &["-c", "commit.gpgsign=false", "commit", "-m", "seed"],
        );
        assert!(is_complete_checkout(&repo));

        // What a clone killed mid-fetch leaves: a `.git` with an unborn HEAD.
        let dir = std::env::temp_dir().join(format!("okena-partial-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        git_in(&dir, &["init"]);
        assert!(
            !is_complete_checkout(&dir),
            "a repo with no commit is not a finished clone"
        );

        // Not a repo at all.
        let plain = dir.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert!(!is_complete_checkout(&plain));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_non_empty_target_is_rejected_before_git_runs() {
        let dir = std::env::temp_dir().join(format!("okena-clone-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // Missing target is fine.
        assert!(require_absent_clone_target(&dir).is_ok());

        // Existing-but-empty is fine (git accepts it too).
        std::fs::create_dir_all(&dir).unwrap();
        assert!(require_absent_clone_target(&dir).is_ok());

        // Anything inside makes it occupied.
        std::fs::write(dir.join("file"), b"x").unwrap();
        assert!(matches!(
            require_absent_clone_target(&dir),
            Err(GitError::CloneTargetExists { .. })
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
