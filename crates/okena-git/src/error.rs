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

    /// Failed to remove a directory.
    #[error("failed to remove directory '{path}'")]
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

/// Convenience alias for `Result<T, GitError>`.
pub type GitResult<T> = Result<T, GitError>;
