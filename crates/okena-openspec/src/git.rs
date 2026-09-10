//! The little git a store needs: probes, plus `git init` and one initial
//! commit during setup. Never fetches, pulls or pushes.

use crate::OpenSpecError;
use okena_core::process::{command, safe_output};
use std::path::Path;

/// A `.git` directory or file (worktree, submodule) at exactly `root`.
///
/// Checked before any `git -C root …` probe: git discovers repositories by
/// walking *up*, so probing a plain folder nested inside another repository
/// would report the enclosing repository's origin.
pub fn is_repository_at_root(root: &Path) -> bool {
    let dot_git = root.join(".git");
    dot_git.is_dir() || dot_git.is_file()
}

/// The checkout's `origin` URL — what the registry records as the observed
/// remote.
pub fn origin_url(root: &Path) -> Option<String> {
    if !is_repository_at_root(root) {
        return None;
    }
    let output = safe_output(
        command("git")
            .arg("-C")
            .arg(root)
            .args(["remote", "get-url", "origin"]),
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!url.is_empty()).then_some(url)
}

/// Fail before creating anything when the initial commit could not be made.
/// `git var` resolves identity exactly as `git commit` would.
pub fn assert_commit_identity(probe_dir: &Path) -> Result<(), OpenSpecError> {
    for var in ["GIT_COMMITTER_IDENT", "GIT_AUTHOR_IDENT"] {
        match safe_output(command("git").arg("var").arg(var).current_dir(probe_dir)) {
            Ok(output) if output.status.success() => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(OpenSpecError::new(
                    "store_git_init_failed",
                    "Git is not available, so setup cannot create the initial store commit.",
                )
                .with_fix("Install Git, or set the store up without initializing Git."));
            }
            _ => {
                return Err(OpenSpecError::new(
                    "store_git_identity_missing",
                    "No usable Git commit identity is configured, so setup cannot create the initial store commit.",
                )
                .with_fix(
                    "Run git config --global user.name \"Your Name\" and git config --global user.email \"you@example.com\", or set the store up without initializing Git.",
                ));
            }
        }
    }
    Ok(())
}

/// `git init` unless `root` already is a repository. Returns whether it ran.
pub fn init(root: &Path) -> Result<bool, OpenSpecError> {
    if is_repository_at_root(root) {
        return Ok(false);
    }
    match safe_output(command("git").arg("init").current_dir(root)) {
        Ok(output) if output.status.success() => Ok(true),
        other => Err(OpenSpecError::new(
            "store_git_init_failed",
            format!("Failed to initialize Git repository: {}", failure(other)),
        )
        .with_fix("Install Git, or set the store up without initializing Git.")),
    }
}

/// The initial store commit, scoped to `pathspecs` so anything the user had
/// already staged stays out of it (and stays staged).
pub fn commit(root: &Path, id: &str, pathspecs: &[String]) -> Result<bool, OpenSpecError> {
    if pathspecs.is_empty() {
        return Ok(false);
    }
    let failed = |detail: String| {
        OpenSpecError::new(
            "store_git_commit_failed",
            format!("Failed to create the initial store commit: {detail}"),
        )
    };
    match safe_output(
        command("git")
            .args(["add", "--"])
            .args(pathspecs)
            .current_dir(root),
    ) {
        Ok(output) if output.status.success() => {}
        other => return Err(failed(failure(other))),
    }
    let message = format!("Initialize OpenSpec store {id}");
    match safe_output(
        command("git")
            .args(["commit", "-m", &message, "--"])
            .args(pathspecs)
            .current_dir(root),
    ) {
        Ok(output) if output.status.success() => Ok(true),
        other => {
            // Best effort: a failed commit (signing, hooks) must not leave
            // setup's files staged.
            let _ = safe_output(
                command("git")
                    .args(["reset", "-q", "--"])
                    .args(pathspecs)
                    .current_dir(root),
            );
            Err(failed(failure(other)))
        }
    }
}

fn failure(result: std::io::Result<std::process::Output>) -> String {
    match result {
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if stderr.is_empty() {
                format!("git exited with {}", output.status)
            } else {
                stderr
            }
        }
        Err(e) => e.to_string(),
    }
}
