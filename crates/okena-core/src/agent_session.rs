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

/// The `lbl=` keys the agent-status OSC reserves for session identity. They are
/// read out of the **raw** decoded label map before any display-oriented
/// clamping, so a pane emitting a flood of other labels cannot push its own
/// session identity out of the map (see [`AgentSession::from_labels`]).
pub const RESERVED_LABEL_KEYS: [&str; 3] = ["agent", "session_id", "transcript_path"];

impl AgentSession {
    /// Extract the session identity from a decoded `lbl=` map, or `None` when
    /// the pane didn't report one.
    ///
    /// Requires both `agent` (without it we don't know which harness could
    /// resume) and a [`is_uuid_like`] `session_id`. Takes the map **before**
    /// [`crate::agent_status::AgentStatus::new_clamped`] bounds it: that clamp
    /// keeps only the lowest [`MAX_LABELS`](crate::agent_status::MAX_LABELS)
    /// keys and truncates values, either of which would silently drop or
    /// corrupt a valid session.
    pub fn from_labels(labels: &std::collections::BTreeMap<String, String>) -> Option<Self> {
        let agent = labels.get("agent")?;
        let session_id = labels.get("session_id")?;
        if agent.is_empty() || !is_uuid_like(session_id) {
            return None;
        }
        Some(Self {
            agent: agent.clone(),
            session_id: session_id.clone(),
            transcript_path: labels.get("transcript_path").cloned(),
        })
    }

    /// Whether `self` and `other` identify the same run of the same harness.
    /// Used to tell a *partial update* of the current session (merge) from a
    /// genuinely new one (replace).
    pub fn is_same_session(&self, other: &Self) -> bool {
        self.agent == other.agent && self.session_id == other.session_id
    }

    /// Re-validate a session that came from persisted state rather than from
    /// the live parser. `workspace.json` is user-editable and survives across
    /// versions, so the [`is_uuid_like`] invariant the parser enforces must be
    /// re-checked before a stored id can reach a resume command.
    pub fn is_valid(&self) -> bool {
        !self.agent.is_empty() && is_uuid_like(&self.session_id)
    }
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

    const UUID: &str = "3b9c1f2a-4d5e-6f70-8a9b-0c1d2e3f4a5b";

    fn labels(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn from_labels_requires_agent_and_valid_uuid() {
        let s = AgentSession::from_labels(&labels(&[("agent", "claude-code"), ("session_id", UUID)]))
            .expect("session");
        assert_eq!(s.agent, "claude-code");
        assert_eq!(s.session_id, UUID);
        assert_eq!(s.transcript_path, None);

        assert!(AgentSession::from_labels(&labels(&[("session_id", UUID)])).is_none());
        assert!(AgentSession::from_labels(&labels(&[("agent", "claude-code")])).is_none());
        assert!(
            AgentSession::from_labels(&labels(&[("agent", ""), ("session_id", UUID)])).is_none()
        );
        assert!(
            AgentSession::from_labels(&labels(&[("agent", "x"), ("session_id", "nope")])).is_none()
        );
    }

    #[test]
    fn from_labels_survives_a_flood_of_other_labels() {
        // The regression this guards: reserved keys used to be read from the
        // display-clamped map, so 32 lexicographically earlier keys hid them.
        let mut l = labels(&[("agent", "claude-code"), ("session_id", UUID)]);
        for i in 0..200 {
            l.insert(format!("aaa{i:04}"), "filler".to_string());
        }
        let s = AgentSession::from_labels(&l).expect("session survives the flood");
        assert_eq!(s.session_id, UUID);
    }

    #[test]
    fn is_valid_rejects_tampered_persisted_state() {
        let good = AgentSession {
            agent: "claude-code".to_string(),
            session_id: UUID.to_string(),
            transcript_path: None,
        };
        assert!(good.is_valid());
        assert!(
            !AgentSession {
                session_id: "; rm -rf ~".to_string(),
                ..good.clone()
            }
            .is_valid()
        );
        assert!(
            !AgentSession {
                agent: String::new(),
                ..good
            }
            .is_valid()
        );
    }
}
