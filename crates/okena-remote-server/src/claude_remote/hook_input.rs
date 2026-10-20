//! The fields of a Claude Code hook's stdin JSON that the hub uses.

use okena_core::agent_session::is_valid_transcript_path;
use serde::Deserialize;

use super::ClaudeRemoteError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HookEvent {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PermissionRequest,
    PostToolUse,
    PostToolUseFailure,
    Stop,
    SessionEnd,
    /// An event the hub does not act on beyond noting that hooks are attached.
    Other,
}

impl HookEvent {
    fn from_name(name: &str) -> Self {
        match name {
            "SessionStart" => Self::SessionStart,
            "UserPromptSubmit" => Self::UserPromptSubmit,
            "PreToolUse" => Self::PreToolUse,
            "PermissionRequest" => Self::PermissionRequest,
            "PostToolUse" => Self::PostToolUse,
            "PostToolUseFailure" => Self::PostToolUseFailure,
            "Stop" => Self::Stop,
            "SessionEnd" => Self::SessionEnd,
            _ => Self::Other,
        }
    }
}

#[derive(Deserialize)]
struct RawHookInput {
    hook_event_name: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    transcript_path: Option<String>,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    tool_input: serde_json::Value,
    #[serde(default)]
    permission_suggestions: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    source: Option<String>,
}

#[derive(Debug)]
pub(super) struct HookInput {
    pub event: HookEvent,
    pub session_id: Option<String>,
    /// Only a path that passes [`is_valid_transcript_path`]; hook input is
    /// written by whatever runs in the pane.
    pub transcript_path: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: serde_json::Value,
    pub permission_suggestions: Vec<serde_json::Value>,
    /// `SessionStart` only: `startup`, `resume`, `clear` or `compact`.
    pub source: Option<String>,
}

impl HookInput {
    pub fn parse(input: &serde_json::Value) -> Result<Self, ClaudeRemoteError> {
        let raw = RawHookInput::deserialize(input)
            .map_err(|error| ClaudeRemoteError::InvalidHookInput(error.to_string()))?;
        Ok(Self {
            event: HookEvent::from_name(&raw.hook_event_name),
            session_id: raw.session_id,
            transcript_path: raw
                .transcript_path
                .filter(|path| is_valid_transcript_path(path)),
            tool_name: raw.tool_name,
            tool_input: raw.tool_input,
            permission_suggestions: raw.permission_suggestions.unwrap_or_default(),
            source: raw.source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_permission_request() {
        let input = HookInput::parse(&json!({
            "hook_event_name": "PermissionRequest",
            "session_id": "s1",
            "transcript_path": "/home/u/.claude/projects/p/s1.jsonl",
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "permission_suggestions": [{"type": "addRules"}],
            "cwd": "/tmp"
        }))
        .expect("valid input");

        assert_eq!(input.event, HookEvent::PermissionRequest);
        assert_eq!(input.session_id.as_deref(), Some("s1"));
        assert_eq!(
            input.transcript_path.as_deref(),
            Some("/home/u/.claude/projects/p/s1.jsonl")
        );
        assert_eq!(input.tool_name.as_deref(), Some("Bash"));
        assert_eq!(input.tool_input, json!({"command": "ls"}));
        assert_eq!(input.permission_suggestions.len(), 1);
    }

    #[test]
    fn drops_unsafe_transcript_path_and_tolerates_null_suggestions() {
        let input = HookInput::parse(&json!({
            "hook_event_name": "Stop",
            "transcript_path": "/home/u/../etc/passwd",
            "permission_suggestions": null
        }))
        .expect("valid input");

        assert_eq!(input.event, HookEvent::Stop);
        assert_eq!(input.transcript_path, None);
        assert!(input.permission_suggestions.is_empty());
    }

    #[test]
    fn unknown_event_is_other_and_missing_event_is_rejected() {
        let input =
            HookInput::parse(&json!({"hook_event_name": "Notification"})).expect("valid input");
        assert_eq!(input.event, HookEvent::Other);

        assert!(matches!(
            HookInput::parse(&json!({"session_id": "s1"})),
            Err(ClaudeRemoteError::InvalidHookInput(_))
        ));
    }
}
