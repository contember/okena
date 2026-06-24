//! Durable agent session identity captured from the agent-status OSC's `lbl=`.
//!
//! Unlike [`crate::agent_status::AgentStatus`] (ephemeral, runtime-only — it
//! drives the indicator and is dropped on `st=clear` / restart), this is the
//! **sticky** identity of the AI session running in a pane: its `session_id`
//! (and transcript path). It is captured from a `session_id` label, survives
//! `st=clear`, and is meant to be *persisted* so a pane can offer to resume its
//! session (`claude --resume <id>`) after a restart and surface transcript
//! stats. Kept deliberately separate from the ephemeral status so that status
//! can stay runtime-only.
//!
//! The values arrive in-band from an **untrusted** byte stream (any process in
//! the pane can emit the OSC), so [`is_uuid_like`] gates the `session_id` before
//! it is ever stored or handed to a resume command.

use serde::{Deserialize, Serialize};

/// The agent session running (or last run) in a pane, captured from the
/// agent-status OSC `lbl=` `agent` / `session_id` / `transcript_path` keys.
///
/// Deliberately harness-agnostic: the [`agent`](Self::agent) id selects which
/// harness knows how to *resume* it and *parse* its transcript (Claude Code,
/// Codex, …) via the harness registry. An unknown agent id is still stored and
/// displayed — it just has no resume/stats until a harness for it is
/// registered, so new harnesses are additive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSession {
    /// Harness id that produced this session — matches the extension id
    /// (`"claude-code"`, `"codex"`, …). Selects the per-harness resume command
    /// and transcript parser. Free-form on the wire so a new harness needs no
    /// core change.
    pub agent: String,
    /// The agent's own session id (e.g. Claude Code / Codex `session_id`).
    /// Always [`is_uuid_like`]-validated before construction here, since it is
    /// untrusted in-band data that may later be passed to a resume command.
    pub session_id: String,
    /// Absolute path to the session transcript, when the agent reported one.
    /// Format/location is the harness's concern; here it is just an opaque path
    /// handed to that harness's transcript parser. Drives the stats view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
}

/// Conservative check that `s` is a canonical UUID (`8-4-4-4-12` lowercase- or
/// uppercase-hex groups joined by hyphens). Guards the in-band, untrusted
/// `session_id` before it is stored or used to build a resume command, so a
/// hostile pane can't plant an arbitrary string there.
pub fn is_uuid_like(s: &str) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let mut parts = s.split('-');
    for len in GROUPS {
        match parts.next() {
            Some(p) if p.len() == len && p.bytes().all(|b| b.is_ascii_hexdigit()) => {}
            _ => return false,
        }
    }
    // Reject trailing junk after the final group ("…-12345-extra").
    parts.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_canonical_uuid() {
        assert!(is_uuid_like("3b9c1f2a-4d5e-6f70-8a9b-0c1d2e3f4a5b"));
        assert!(is_uuid_like("3B9C1F2A-4D5E-6F70-8A9B-0C1D2E3F4A5B"));
    }

    #[test]
    fn rejects_non_uuid() {
        assert!(!is_uuid_like(""));
        assert!(!is_uuid_like("not-a-uuid"));
        assert!(!is_uuid_like("3b9c1f2a4d5e6f708a9b0c1d2e3f4a5b")); // no hyphens
        assert!(!is_uuid_like("3b9c1f2a-4d5e-6f70-8a9b-0c1d2e3f4a5b-extra")); // trailing
        assert!(!is_uuid_like("zzzzzzzz-4d5e-6f70-8a9b-0c1d2e3f4a5b")); // non-hex
        assert!(!is_uuid_like("3b9c1f2a-4d5e-6f70-8a9b-0c1d2e3f4a5")); // short group
        // Defends against an injection attempt smuggled as a session id.
        assert!(!is_uuid_like("$(rm -rf ~)"));
    }
}
