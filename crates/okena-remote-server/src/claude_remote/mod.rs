//! Claude remote: dialogs, prompt delivery and transcript reading for `claude`
//! sessions in panes, served to remote clients under `/v1/claude/...`.

mod channel;
mod decision;
mod hook_input;
mod hub;
mod transcript;

pub use hub::ClaudeRemoteHub;

/// Why a `/v1/claude` request could not be served. The routes map each case to
/// an HTTP status.
#[derive(Debug)]
pub(crate) enum ClaudeRemoteError {
    /// The hook's stdin JSON lacks a field the request needs.
    InvalidHookInput(String),
    EmptyPrompt,
    PromptTooLong,
    /// Claude is in a turn and no `Stop` hook will tell us when it ends.
    Busy,
    /// `claude` is not the pane's foreground process, or neither hooks nor the
    /// agent-status OSC show it.
    NoClaudeSession,
    QueueFull,
    /// The prompt named a session the pane no longer runs.
    SessionMismatch {
        expected: String,
        current: Option<String>,
    },
    /// No pane runs a session with this id.
    SessionNotFound,
    TerminalNotFound(String),
    DialogNotFound,
    NoTranscript,
    /// The daemon refused a command (unknown terminal, closed PTY, ...).
    Daemon(String),
    BridgeUnavailable,
    Io(std::io::Error),
}
