/// Structured error type for service operations.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// Docker/compose CLI command failed to start.
    #[error("failed to execute command")]
    CommandFailed(#[from] std::io::Error),

    /// Docker/compose command exited with a non-zero status.
    #[error("{context}: {stderr}")]
    CommandExitError { context: String, stderr: String },

    /// Failed to parse JSON or YAML output.
    #[error("{context}: {detail}")]
    ParseError {
        context: String,
        detail: String,
    },

    /// Failed to read a config file.
    #[error("failed to read {path}: {source}")]
    ReadError {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// Convenience alias for `Result<T, ServiceError>`.
pub type ServiceResult<T> = Result<T, ServiceError>;
