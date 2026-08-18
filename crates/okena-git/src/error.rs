use std::fmt;
use std::path::PathBuf;

/// Byte budget that an exact review source request exceeded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ReviewSourceBudgetKind {
    /// One source side exceeded the maximum blob size.
    PerFileSourceBytes,
    /// The combined source sides exceeded the request's remaining byte budget.
    AggregateSourceBytes,
}

impl fmt::Display for ReviewSourceBudgetKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PerFileSourceBytes => formatter.write_str("per-file byte"),
            Self::AggregateSourceBytes => formatter.write_str("aggregate byte"),
        }
    }
}

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

    /// Failed to parse structured output (JSON, etc.).
    #[error("parse error: {0}")]
    ParseError(String),

    /// An exact review source request exceeded a caller-owned byte budget.
    #[error(
        "exact review source {kind} budget exceeded: observed {observed} bytes, limit {limit} bytes"
    )]
    ReviewSourceBudgetExceeded {
        kind: ReviewSourceBudgetKind,
        observed: u64,
        limit: u64,
    },
}

/// Convenience alias for `Result<T, GitError>`.
pub type GitResult<T> = Result<T, GitError>;
