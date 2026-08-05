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
use std::path::{Component, Path};

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
    /// Untrusted in-band data — [`is_valid_transcript_path`] gates it on capture
    /// and again on load, so a harness never opens a traversal or an unbounded
    /// path. Implementors should still confine it to their own session dir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
}

/// The `lbl=` keys the agent-status OSC reserves for session identity. They are
/// read out of the **raw** decoded label map before any display-oriented
/// clamping, so a pane emitting a flood of other labels cannot push its own
/// session identity out of the map (see [`AgentSession::from_labels`]).
pub const RESERVED_LABEL_KEYS: [&str; 3] = ["agent", "session_id", "transcript_path"];

/// Bounds on the two free-form session fields. `session_id` needs none — it is
/// UUID-shaped by [`is_uuid_like`] — but `agent` and `transcript_path` are
/// arbitrary in-band strings that get **persisted to `workspace.json`**, so
/// without a bound one pane can rewrite the user's state file with megabytes of
/// chosen bytes on every autosave. These reject rather than truncate: a cut
/// harness id or path is not a usable value, and rejecting lets the load-time
/// prune drop an already-poisoned file.
pub const MAX_AGENT_ID_LEN: usize = 64;
pub const MAX_TRANSCRIPT_PATH_LEN: usize = 4096;

/// Whether `s` is a usable harness id — bounded and restricted to the shape an
/// extension id has (`"claude-code"`, `"codex"`, …).
pub fn is_valid_agent_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_AGENT_ID_LEN
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Whether a reported transcript path is safe to store and later hand to a
/// harness's parser. The value is fully attacker-chosen, so require it to be
/// bounded, absolute, and free of `..` traversal rather than trusting the
/// harness to confine it.
pub fn is_valid_transcript_path(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_TRANSCRIPT_PATH_LEN
        && Path::new(s).is_absolute()
        && !Path::new(s)
            .components()
            .any(|c| matches!(c, Component::ParentDir))
}

impl AgentSession {
    /// Extract the session identity from a decoded `lbl=` map, or `None` when
    /// the pane didn't report one.
    ///
    /// Requires both `agent` (without it we don't know which harness could
    /// resume) and a [`is_uuid_like`] `session_id`. Takes the map **before**
    /// [`crate::agent_status::AgentStatus::new_clamped`] bounds it: that clamp
    /// keeps only the lowest [`MAX_LABELS`](crate::agent_status::MAX_LABELS)
    /// keys and truncates values, either of which would silently drop or
    /// corrupt a valid session. Reading pre-clamp means this is the only place
    /// the session fields get bounded, hence the checks here — they guard the
    /// one part of an agent status that reaches disk.
    pub fn from_labels(labels: &std::collections::BTreeMap<String, String>) -> Option<Self> {
        let agent = labels.get("agent")?;
        let session_id = labels.get("session_id")?;
        if !is_valid_agent_id(agent) || !is_uuid_like(session_id) {
            return None;
        }
        Some(Self {
            agent: agent.clone(),
            session_id: session_id.clone(),
            // An unusable path is dropped, not a reason to discard the session:
            // resume only needs `agent` + `session_id`.
            transcript_path: labels
                .get("transcript_path")
                .filter(|p| is_valid_transcript_path(p))
                .cloned(),
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
    /// versions, so every invariant [`from_labels`](Self::from_labels) enforces
    /// must be re-checked before a stored id can reach a resume command — and
    /// so that a file already poisoned by an older build gets pruned on load.
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.agent)
            && is_uuid_like(&self.session_id)
            && self
                .transcript_path
                .as_deref()
                .is_none_or(is_valid_transcript_path)
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
        let s =
            AgentSession::from_labels(&labels(&[("agent", "claude-code"), ("session_id", UUID)]))
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

    /// `Path::is_absolute` is platform-dependent, so build a path that is
    /// genuinely absolute on the host running the test.
    fn abs(tail: &str) -> String {
        if cfg!(windows) {
            format!("C:\\{tail}")
        } else {
            format!("/{tail}")
        }
    }

    #[test]
    fn from_labels_bounds_the_fields_that_reach_disk() {
        // Without a bound here, one pane rewrites workspace.json with megabytes
        // of chosen bytes on every autosave — the fields are persisted.
        let huge = "a".repeat(MAX_AGENT_ID_LEN + 1);
        assert!(
            AgentSession::from_labels(&labels(&[("agent", &huge), ("session_id", UUID)])).is_none()
        );
        // A harness id is an extension id, not free-form text.
        assert!(
            AgentSession::from_labels(&labels(&[("agent", "claude code"), ("session_id", UUID)]))
                .is_none()
        );
        assert!(
            AgentSession::from_labels(&labels(&[("agent", "a/../b"), ("session_id", UUID)]))
                .is_none()
        );

        // An unusable transcript path is dropped, but the session survives —
        // resume needs only `agent` + `session_id`.
        let long_path = abs(&"p".repeat(MAX_TRANSCRIPT_PATH_LEN));
        for bad in [
            long_path.as_str(),
            "relative/x.jsonl",
            &abs("a/../../etc/passwd"),
        ] {
            let s = AgentSession::from_labels(&labels(&[
                ("agent", "claude-code"),
                ("session_id", UUID),
                ("transcript_path", bad),
            ]))
            .expect("session survives a bad path");
            assert_eq!(s.transcript_path, None, "should have dropped {bad:?}");
        }

        let good = abs("home/u/.claude/projects/p/s.jsonl");
        let s = AgentSession::from_labels(&labels(&[
            ("agent", "claude-code"),
            ("session_id", UUID),
            ("transcript_path", &good),
        ]))
        .expect("session");
        assert_eq!(s.transcript_path.as_deref(), Some(good.as_str()));
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
                ..good.clone()
            }
            .is_valid()
        );
        // A file poisoned by a build that predated the bounds must be pruned on
        // load, not carried forward.
        assert!(
            !AgentSession {
                agent: "a".repeat(MAX_AGENT_ID_LEN + 1),
                ..good.clone()
            }
            .is_valid()
        );
        assert!(
            !AgentSession {
                transcript_path: Some(abs(&"p".repeat(MAX_TRANSCRIPT_PATH_LEN))),
                ..good.clone()
            }
            .is_valid()
        );
        assert!(
            !AgentSession {
                transcript_path: Some("relative/x.jsonl".to_string()),
                ..good.clone()
            }
            .is_valid()
        );
        assert!(
            AgentSession {
                transcript_path: Some(abs("home/u/s.jsonl")),
                ..good
            }
            .is_valid()
        );
    }
}
