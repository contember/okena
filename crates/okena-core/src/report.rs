//! Report-back: a terminal started on behalf of another one (its *parent*)
//! reports what happened in it, and the daemon delivers that report into the
//! parent's input or to a parent blocked in `okena wait`.
//!
//! Wire types of the `/v1/report/...` routes and of the daemon probe behind
//! them. The behaviour is described in `docs/reference/report-back.md`.

use serde::{Deserialize, Serialize};

use crate::agent_status::AgentLifecycle;

/// What happened in a child terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportEventKind {
    /// An agent's turn ended (agent status `done` or `idle`), or the child
    /// said so itself.
    Done,
    /// An agent's turn ended in an error (agent status `error`).
    Error,
    /// An agent waits for permission or input (agent status `blocked`).
    Blocked,
    /// The command started by `okena spawn` exited.
    Exit,
    /// The child sent a report with `okena report`.
    Report,
    /// The child asked its parent for help with `okena report --ask`.
    Ask,
    /// The child terminal was closed.
    Closed,
}

impl ReportEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Error => "error",
            Self::Blocked => "blocked",
            Self::Exit => "exit",
            Self::Report => "report",
            Self::Ask => "ask",
            Self::Closed => "closed",
        }
    }
}

/// Where a report went.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportDelivery {
    /// Through the parent's Claude channel.
    Channel,
    /// Pasted into the parent's Claude prompt.
    Pty,
    /// Queued until the parent's Claude turn ends.
    Queued,
    /// Pasted into another agent running in the parent (its foreground is not
    /// a shell).
    Typed,
    /// Returned to the parent's `okena wait`.
    Wait,
    /// Nothing in the parent could take it (a shell prompt, an editor): kept
    /// for `okena wait` and `okena children`.
    Kept,
}

/// One report of a child.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiReportEvent {
    /// Increases with every report the daemon records.
    pub seq: u64,
    pub child_terminal_id: String,
    /// The child's terminal name, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_name: Option<String>,
    pub event: ReportEventKind,
    /// Of an `exit` event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// The report's text: what the child sent, the agent's last message, or
    /// the last lines of its screen. Untrusted, sanitized.
    pub body: String,
    /// Milliseconds since the Unix epoch.
    pub at_ms: u64,
    /// The `<okena-report …>` element the parent receives.
    pub message: String,
    /// `None` until delivered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<ReportDelivery>,
}

/// `POST /v1/report/link`: report `child`'s events to `parent`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiReportLinkRequest {
    pub child_terminal_id: String,
    pub parent_terminal_id: String,
    /// Close the child when the parent terminal goes away.
    #[serde(default)]
    pub owned: bool,
}

/// `POST /v1/report/unlink`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiReportUnlinkRequest {
    pub child_terminal_id: String,
}

/// `POST /v1/report`: a child reports to its parent.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiReportRequest {
    /// The reporting child (`$OKENA_TERMINAL_ID`).
    pub terminal_id: String,
    /// `report`, `ask`, `done`, `error` or `exit`.
    pub event: ReportEventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub text: String,
}

/// `POST /v1/report/wait`: block until children of `parent` report.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiReportWaitRequest {
    pub parent_terminal_id: String,
    /// Empty: every child of the parent.
    #[serde(default)]
    pub children: Vec<String>,
    /// Wait for every child instead of the first one.
    #[serde(default)]
    pub all: bool,
    /// How long this request may block; the daemon caps it.
    #[serde(default)]
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ApiReportWaitResponse {
    /// Reports not returned by an earlier wait, oldest first. With `all`,
    /// empty until every awaited child has one.
    pub events: Vec<ApiReportEvent>,
    /// Awaited children without a report yet.
    pub waiting: Vec<String>,
}

/// A child as `GET /v1/report/children` lists it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiReportChild {
    pub terminal_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub parent_terminal_id: String,
    pub owned: bool,
    /// Steered or started since its last report.
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event: Option<ApiReportEvent>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ApiReportChildrenResponse {
    pub children: Vec<ApiReportChild>,
}

/// The daemon's view of one terminal, for the report hub's poll.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReportProbeEntry {
    pub terminal_id: String,
    pub project_id: String,
    /// The user-set terminal name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<AgentLifecycle>,
    /// The agent status's free-form text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
    /// `Terminal::agent_lifecycle_changes`.
    #[serde(default)]
    pub lifecycle_changes: u64,
    /// The visible screen, when asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
}
