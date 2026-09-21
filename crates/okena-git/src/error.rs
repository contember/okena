use std::path::PathBuf;

/// Structured error type for git operations.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// Path could not be converted to a UTF-8 string.
    #[error("path is not valid UTF-8: {0}")]
    InvalidPath(PathBuf),

    /// Git subprocess failed to start (I/O error).
    #[error("failed to execute git command")]
    CommandFailed(#[from] std::io::Error),

    /// Git process exited with a non-zero status.
    #[error("git exited with status {status}: {stderr}")]
    GitExitError { status: i32, stderr: String },

    /// Target directory is already an active worktree.
    #[error("directory '{path}' is already an active worktree")]
    WorktreeExists { path: PathBuf },

    /// Failed to remove a directory. The cause belongs in the message: this
    /// error reaches the user as a toast and the log as `{e}`, and neither
    /// walks the source chain, so without it a failed worktree close reports
    /// only the path it already named.
    #[error("failed to remove directory '{path}': {source}")]
    RemoveFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A destructive worktree operation failed its ownership check.
    #[error("unsafe worktree operation for '{path}': {reason}")]
    UnsafeWorktree { path: PathBuf, reason: String },

    /// A git ref (branch name, commit hash) looks like a CLI flag.
    #[error("invalid git ref: {0}")]
    InvalidRef(String),

    /// A clone URL is empty or looks like a CLI flag.
    #[error("invalid repository URL: {0}")]
    InvalidUrl(String),

    /// Clone target directory already exists and is not empty.
    #[error("directory '{path}' already exists and is not empty")]
    CloneTargetExists { path: PathBuf },

    /// Failed to parse structured output (JSON, etc.).
    #[error("parse error: {0}")]
    ParseError(String),
}

impl GitError {
    /// The one line worth putting in front of a user.
    ///
    /// Git writes progress to stderr alongside errors, so a failed command's
    /// full message opens with chatter (`Cloning into '...'`) and buries the
    /// cause several lines down. Pick out git's own error line instead; the
    /// untouched message still goes to the log.
    pub fn user_detail(&self) -> String {
        match self {
            GitError::GitExitError { status, stderr } => git_failure_line(stderr)
                .unwrap_or_else(|| format!("git exited with status {status}")),
            other => other.to_string(),
        }
    }
}

/// The last line git marked as the failure, without its prefix.
///
/// Last rather than first: when git reports several, the final one is the
/// operation's actual verdict.
fn git_failure_line(stderr: &str) -> Option<String> {
    const PREFIXES: [&str; 4] = ["fatal: ", "error: ", "remote: error: ", "warning: "];
    stderr.lines().rev().find_map(|line| {
        let line = line.trim();
        PREFIXES
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
            .filter(|rest| !rest.is_empty())
            .map(str::to_string)
    })
}

/// Convenience alias for `Result<T, GitError>`.
pub type GitResult<T> = Result<T, GitError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_sees_gits_error_line_not_its_progress_chatter() {
        let err = GitError::GitExitError {
            status: 128,
            stderr: "Cloning into '/tmp/repo'...\n                     fatal: could not read Username for 'https://github.com': terminal prompts disabled"
                .to_string(),
        };
        assert_eq!(
            err.user_detail(),
            "could not read Username for 'https://github.com': terminal prompts disabled"
        );
    }

    /// The io cause is what says whether the close failed on a busy directory,
    /// a permission, or a vanished path. Dropping it leaves the user with a
    /// sentence that only repeats the path.
    #[test]
    fn a_failed_removal_names_its_cause() {
        let err = GitError::RemoveFailed {
            path: PathBuf::from("/tmp/wt"),
            source: std::io::Error::from(std::io::ErrorKind::DirectoryNotEmpty),
        };
        let message = err.user_detail();
        assert!(message.contains("/tmp/wt"), "{message}");
        assert!(message.contains("not empty"), "{message}");
    }

    #[test]
    fn the_last_failure_line_wins() {
        let err = GitError::GitExitError {
            status: 1,
            stderr: "error: failed to push some refs\nfatal: the remote end hung up".to_string(),
        };
        assert_eq!(err.user_detail(), "the remote end hung up");
    }

    #[test]
    fn stderr_with_no_recognisable_line_falls_back_to_the_status() {
        let err = GitError::GitExitError {
            status: 129,
            stderr: "usage: git clone [<options>] [--] <repo>".to_string(),
        };
        assert_eq!(err.user_detail(), "git exited with status 129");
    }

    #[test]
    fn other_error_kinds_keep_their_own_message() {
        let err = GitError::InvalidUrl("-x".to_string());
        assert_eq!(err.user_detail(), "invalid repository URL: -x");
    }
}
