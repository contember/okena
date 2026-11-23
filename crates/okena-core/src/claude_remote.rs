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
    /// The probe saw the pane's shell itself running no process: whatever ran
    /// there has exited. `false` when unsure (failed or unsupported probe).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shell_idle: bool,
    /// The start-up dialog on the pane's screen, when the screen was read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_dialog: Option<ApiClaudeStartupDialog>,
}

/// A dialog Claude Code shows at start, before the session connects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiClaudeStartupDialog {
    /// "Do you trust this folder": only the user may vouch for a folder.
    Trust,
    /// The development-channels warning `--dangerously-load-development-channels`
    /// causes, which Okena's preload adds for its own channel server.
    Channels,
}

/// Whether `line` is the option `label`, whole or only its start: Claude Code
/// wraps a label too wide for the pane onto the next line, or cuts it with `…`.
/// A start under ten characters is too short to tell the label apart.
fn shows_option(line: &str, label: &str) -> bool {
    let start = line.strip_suffix('…').map_or(line, str::trim_end);
    line == label || (start.len() >= 10 && label.starts_with(start))
}

/// A screen line without its indent and the `❯` selection marker.
fn option_text(line: &str) -> &str {
    line.trim().trim_start_matches('❯').trim_start()
}

impl ApiClaudeStartupDialog {
    /// The dialog Claude shows on `screen` (a pane's visible text), if any.
    pub fn detect(screen: &str) -> Option<Self> {
        [Self::Trust, Self::Channels].into_iter().find(|dialog| {
            (screen.contains(dialog.marker()) && screen.contains(dialog.confirm_label()))
                || dialog.options_on_screen(screen)
        })
    }

    /// Whether each of the dialog's options stands on a line of its own (any
    /// one spelling per option). A short pane scrolls the title off while the
    /// options stay visible.
    fn options_on_screen(self, screen: &str) -> bool {
        let options: &[&[&str]] = match self {
            // Claude Code labels the cancel option by whether the folder's
            // settings ask for permissions.
            Self::Trust => &[
                &["Yes, I trust this folder"],
                &["No, exit", "No, continue without these permissions"],
            ],
            Self::Channels => &[&[Self::Channels.confirm_label()], &["2. Exit"]],
        };
        let lines: Vec<&str> = screen.lines().map(option_text).collect();
        options.iter().all(|spellings| {
            spellings
                .iter()
                .any(|option| lines.iter().any(|line| shows_option(line, option)))
        })
    }

    /// The option that accepts the dialog, as Claude prints it.
    fn confirm_label(self) -> &'static str {
        match self {
            Self::Trust => "Yes, I trust this folder",
            Self::Channels => "1. I am using this for local development",
        }
    }

    /// Whether the screen `line` is the confirming option, selected or not,
    /// whole or cut short by a narrow pane.
    pub fn is_confirm_option(self, line: &str) -> bool {
        shows_option(option_text(line), self.confirm_label())
    }

    /// Whether the confirming option is the selected one (`❯` before it).
    pub fn confirm_selected(self, screen: &str) -> bool {
        screen
            .lines()
            .any(|line| line.trim_start().starts_with('❯') && self.is_confirm_option(line))
    }

    fn marker(self) -> &'static str {
        match self {
            Self::Trust => "Accessing workspace:",
            Self::Channels => "WARNING: Loading development channels",
        }
    }
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
    /// The start-up dialog `claude` waits on before it connects; `null` once
    /// it connected or when none is shown.
    #[serde(default)]
    pub startup_dialog: Option<ApiClaudeStartupDialog>,
    /// The pane's foreground process (`claude`, `vim`, ...), probed only on
    /// request (`?probe_foreground=true`) while `claude` is not connected.
    /// Absent at a shell prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground_command: Option<String>,
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
    /// Refuse (`409`) unless the pane runs this Claude session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect_session_id: Option<String>,
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
    /// Opaque cursor to pass back as `before` to get the entries before this
    /// page. 0 when the page reaches the start of the transcript (and from a
    /// daemon that predates older pages).
    #[serde(default)]
    pub start: u64,
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
        /// When Claude recorded the entry: its transcript line's RFC 3339
        /// `timestamp`.
        #[serde(default)]
        timestamp: Option<String>,
        text: String,
        origin: ApiClaudePromptOrigin,
    },
    Assistant {
        uuid: String,
        /// When Claude recorded the entry: its transcript line's RFC 3339
        /// `timestamp`.
        #[serde(default)]
        timestamp: Option<String>,
        text: String,
    },
    ToolUse {
        uuid: String,
        /// When Claude recorded the entry: its transcript line's RFC 3339
        /// `timestamp`.
        #[serde(default)]
        timestamp: Option<String>,
        tool_use_id: String,
        name: String,
        /// One line describing the call (command, file path, pattern, ...).
        summary: String,
    },
    ToolResult {
        uuid: String,
        /// When Claude recorded the entry: its transcript line's RFC 3339
        /// `timestamp`.
        #[serde(default)]
        timestamp: Option<String>,
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

    const TRUST_SCREEN: &str = "\
 Accessing workspace:

 /var/tmp/project

 Quick safety check: Is this a project you created or one you trust?

 ❯ No, exit
   Yes, I trust this folder

 Enter to confirm · Esc to cancel";

    const CHANNELS_SCREEN: &str = "\
  WARNING: Loading development channels

  Channels: server:okena-remote

  ❯ 1. I am using this for local development
    2. Exit";

    #[test]
    fn startup_dialogs_are_detected_with_their_selection() {
        let trust = ApiClaudeStartupDialog::detect(TRUST_SCREEN);
        assert_eq!(trust, Some(ApiClaudeStartupDialog::Trust));
        assert!(!ApiClaudeStartupDialog::Trust.confirm_selected(TRUST_SCREEN));
        let moved = TRUST_SCREEN
            .replace("❯ No, exit", "  No, exit")
            .replace("  Yes, I trust", "❯ Yes, I trust");
        assert!(ApiClaudeStartupDialog::Trust.confirm_selected(&moved));

        let channels = ApiClaudeStartupDialog::detect(CHANNELS_SCREEN);
        assert_eq!(channels, Some(ApiClaudeStartupDialog::Channels));
        assert!(ApiClaudeStartupDialog::Channels.confirm_selected(CHANNELS_SCREEN));
    }

    #[test]
    fn the_channels_dialog_is_detected_from_its_options_without_the_title() {
        // A short pane: the title and the channel list scrolled off.
        let options_only = "  ❯ 1. I am using this for local development\n    2. Exit";
        assert_eq!(
            ApiClaudeStartupDialog::detect(options_only),
            Some(ApiClaudeStartupDialog::Channels)
        );
        assert!(ApiClaudeStartupDialog::Channels.confirm_selected(options_only));
        // Selection moved to Exit: still the dialog, confirm not selected.
        let on_exit = "    1. I am using this for local development\n  ❯ 2. Exit";
        assert_eq!(
            ApiClaudeStartupDialog::detect(on_exit),
            Some(ApiClaudeStartupDialog::Channels)
        );
        assert!(!ApiClaudeStartupDialog::Channels.confirm_selected(on_exit));
    }

    #[test]
    fn the_trust_dialog_is_detected_from_its_options_without_the_title() {
        // A short pane: the title and the folder path scrolled off.
        let options_only =
            " ❯ No, exit\n   Yes, I trust this folder\n\n Enter to confirm · Esc to cancel";
        assert_eq!(
            ApiClaudeStartupDialog::detect(options_only),
            Some(ApiClaudeStartupDialog::Trust)
        );
        assert!(!ApiClaudeStartupDialog::Trust.confirm_selected(options_only));
        // The cancel label when the folder's settings ask for permissions.
        let with_permissions =
            "   No, continue without these permissions\n ❯ Yes, I trust this folder";
        assert_eq!(
            ApiClaudeStartupDialog::detect(with_permissions),
            Some(ApiClaudeStartupDialog::Trust)
        );
        assert!(ApiClaudeStartupDialog::Trust.confirm_selected(with_permissions));
    }

    #[test]
    fn a_label_truncated_by_a_narrow_pane_still_counts() {
        let narrow = " ❯ No, continue without…\n   Yes, I trust this folder";
        assert_eq!(
            ApiClaudeStartupDialog::detect(narrow),
            Some(ApiClaudeStartupDialog::Trust)
        );
        let channels = "  ❯ 1. I am using this for local dev…\n    2. Exit";
        assert_eq!(
            ApiClaudeStartupDialog::detect(channels),
            Some(ApiClaudeStartupDialog::Channels)
        );
        assert!(ApiClaudeStartupDialog::Channels.confirm_selected(channels));
        // A cut too short to tell the label apart does not.
        assert_eq!(
            ApiClaudeStartupDialog::detect(" ❯ No…\n   Yes, I trust this folder"),
            None
        );
    }

    #[test]
    fn a_label_wrapped_by_a_narrow_pane_counts_by_its_first_line() {
        let channels = "  ❯ 1. I am using this for\n       local development\n    2. Exit";
        assert_eq!(
            ApiClaudeStartupDialog::detect(channels),
            Some(ApiClaudeStartupDialog::Channels)
        );
        assert!(ApiClaudeStartupDialog::Channels.confirm_selected(channels));
        // A first line too short to tell the label apart does not.
        assert_eq!(
            ApiClaudeStartupDialog::detect(" ❯ No, cont\n   inue\n   Yes, I trust this folder"),
            None
        );
    }

    #[test]
    fn the_trust_options_need_both_lines_on_their_own() {
        for screen in [
            " ❯ No, exit",
            "   Yes, I trust this folder",
            "> answer \"Yes, I trust this folder\" or \"No, exit\"",
        ] {
            assert_eq!(ApiClaudeStartupDialog::detect(screen), None, "{screen}");
        }
    }

    #[test]
    fn the_channels_options_need_both_lines_on_their_own() {
        for screen in [
            "  ❯ 1. I am using this for local development",
            "  2. Exit",
            "> pick \"1. I am using this for local development\" or \"2. Exit\"",
        ] {
            assert_eq!(ApiClaudeStartupDialog::detect(screen), None, "{screen}");
        }
    }

    #[test]
    fn a_transcript_quoting_a_dialog_is_not_one() {
        assert_eq!(
            ApiClaudeStartupDialog::detect("> says \"Yes, I trust this folder\" in the docs"),
            None
        );
        assert_eq!(ApiClaudeStartupDialog::detect("$ ls\nsrc"), None);
    }

    #[test]
    fn session_serializes_a_missing_startup_dialog_as_null() {
        let session: ApiClaudeSession = serde_json::from_value(json!({
            "terminal_id": "t1",
            "has_transcript": false,
            "hooks_attached": false,
            "channel_attached": false,
            "queued_prompts": 0,
            "dialogs": [],
        }))
        .expect("session");
        let value = serde_json::to_value(&session).expect("encodes");
        assert_eq!(value["startup_dialog"], serde_json::Value::Null);
    }
}
