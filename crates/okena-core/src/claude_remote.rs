//! Wire types for "Claude remote": following and steering a `claude` session
//! running in a pane from another client (phone, web).
//!
//! Three parties speak these types:
//! - the `okena claude-remote` CLI, run by `claude` itself as a hook or as an
//!   MCP channel server inside the pane ([`ClaudeHookRequest`],
//!   [`ClaudeChannelPollResponse`]);
//! - the daemon's remote server, which owns pending dialogs and prompt queues;
//! - remote clients, which read the transcript and answer dialogs.

use crate::agent_session::AgentSession;
use crate::agent_status::AgentLifecycle;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A Claude Code hook invocation forwarded by `okena claude-remote hook`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClaudeHookRequest {
    /// `OKENA_TERMINAL_ID` of the pane the hook process runs in.
    pub terminal_id: String,
    /// The hook's stdin JSON, unchanged (`hook_event_name`, `session_id`,
    /// `tool_name`, `tool_input`, `permission_suggestions`, ...).
    pub input: serde_json::Value,
}

/// Prompts handed to the MCP channel server by one long-poll round. Empty when
/// the poll timed out without a prompt.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClaudeChannelPollResponse {
    pub prompts: Vec<String>,
}

/// Daemon → remote server: what the daemon knows about the agent in one pane
/// (from the lifecycle plugin's agent-status OSC and the pane's process tree).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClaudeTerminalInfo {
    pub session: Option<AgentSession>,
    pub lifecycle: Option<AgentLifecycle>,
    /// Name of the process the pane's shell runs in the foreground (`comm` on
    /// Linux). `None` when the shell is at its prompt or the probe failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground_command: Option<String>,
}

/// What a remote client sees of the Claude session in one pane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiClaudeSession {
    pub terminal_id: String,
    /// Session id reported by the agent (lifecycle plugin or our hooks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Whether a transcript can be read for this pane.
    pub has_transcript: bool,
    /// Whether our hooks have reported from this pane, i.e. dialogs can be
    /// answered remotely.
    pub hooks_attached: bool,
    /// Whether the MCP channel server is connected, i.e. prompts are delivered
    /// through the channel instead of the PTY.
    pub channel_attached: bool,
    /// Whether Claude is in the middle of a turn. `None` when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy: Option<bool>,
    /// Prompts accepted from remote clients but not yet delivered.
    pub queued_prompts: u32,
    /// Open dialogs, oldest first.
    pub dialogs: Vec<ApiClaudeDialog>,
    /// Subagents between their `SubagentStart` and `SubagentStop` hooks,
    /// oldest first. Empty when hooks are not attached.
    #[serde(default)]
    pub running_subagents: Vec<ApiClaudeRunningSubagent>,
    /// Pull requests the session created or touched, newest first.
    #[serde(default)]
    pub pr_links: Vec<ApiClaudePrLink>,
}

/// A pull request Claude Code recorded in the session transcript (a `pr-link`
/// record), deduplicated by `url`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiClaudePrLink {
    pub number: u64,
    /// Always `https://`.
    pub url: String,
    /// `owner/repo`.
    pub repository: String,
    /// The newest record's timestamp, RFC 3339 as Claude Code wrote it.
    pub last_seen_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiClaudeRunningSubagent {
    pub agent_id: String,
    /// `general-purpose`, `Explore`, a custom agent's name, ...
    pub agent_type: String,
}

/// The subagents of the pane's current session, from Claude's metadata files.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiClaudeSubagentList {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Oldest first.
    pub subagents: Vec<ApiClaudeSubagent>,
}

/// A subagent started in the session. Its transcript is read with the
/// transcript route's `agent_id`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiClaudeSubagent {
    pub agent_id: String,
    /// The `Agent` tool call that started it, when Claude recorded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Listed in [`ApiClaudeSession::running_subagents`].
    pub running: bool,
}

/// A permission dialog Claude is showing in the terminal, mirrored from a
/// `PermissionRequest` hook. The client picks the UI from `tool_name`:
/// `AskUserQuestion` → questions, `ExitPlanMode` → plan approval, anything else
/// → allow / deny.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiClaudeDialog {
    pub id: String,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    /// `permission_suggestions` from the hook input, as Claude sent them.
    #[serde(default)]
    pub permission_suggestions: Vec<serde_json::Value>,
    pub created_at_ms: u64,
}

/// A remote client's answer to an [`ApiClaudeDialog`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApiClaudeDecision {
    /// Allow once, or also for the rest of the session (`always`), which applies
    /// the dialog's `permission_suggestions` with `destination: "session"`.
    Allow {
        #[serde(default)]
        always: bool,
    },
    Deny {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// Answers for `AskUserQuestion`: question text → chosen label (or free
    /// text). Multi-select answers are comma-separated labels.
    Answer { answers: BTreeMap<String, String> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiClaudePromptRequest {
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiClaudePromptDelivery {
    /// Pushed to the MCP channel; Claude picks it up after the current turn.
    Channel,
    /// Typed into the idle terminal.
    Pty,
    /// Held until Claude finishes its turn, then typed into the terminal.
    Queued,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiClaudePromptResponse {
    pub delivery: ApiClaudePromptDelivery,
}

/// One page of the session transcript.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiClaudeTranscriptPage {
    /// Session the entries belong to. A client that holds entries of another
    /// session must drop them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub entries: Vec<ApiClaudeEntry>,
    /// Opaque cursor to pass back to get only newer entries.
    pub cursor: u64,
    /// True when the entries replace everything the client had (first page,
    /// another session, or a rewritten transcript).
    pub reset: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiClaudePromptOrigin {
    /// Typed in the terminal (or by Okena's PTY fallback).
    Terminal,
    /// Delivered through the MCP channel.
    Channel,
}

/// A transcript entry, reduced to what a chat view renders.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApiClaudeEntry {
    User {
        uuid: String,
        text: String,
        origin: ApiClaudePromptOrigin,
    },
    Assistant {
        uuid: String,
        text: String,
    },
    ToolUse {
        uuid: String,
        tool_use_id: String,
        name: String,
        /// One line describing the call (command, file path, pattern, ...).
        summary: String,
    },
    ToolResult {
        uuid: String,
        tool_use_id: String,
        /// Result text, truncated.
        text: String,
        is_error: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn session_from_a_server_without_pr_links_has_none() {
        let session: ApiClaudeSession = serde_json::from_value(json!({
            "terminal_id": "t1",
            "has_transcript": true,
            "hooks_attached": true,
            "channel_attached": false,
            "queued_prompts": 0,
            "dialogs": [],
        }))
        .expect("session");
        assert_eq!(session.pr_links, Vec::new());
    }
}
