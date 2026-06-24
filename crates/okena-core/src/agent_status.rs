//! Agent status — what an AI coding agent reports about itself in a pane.
//!
//! An agent (e.g. Claude Code) pushes its state in-band via the agent-status
//! OSC sequence (parsed in `okena-terminal`); it then surfaces in the tab, the
//! sidebar "Agents" section, and the remote API. The model is intentionally
//! **open**: a small fixed [`AgentLifecycle`] drives color / sort / notification,
//! while the free-form [`AgentStatus::custom`] string and [`AgentStatus::labels`]
//! carry whatever the agent wants and are rendered verbatim. This is the
//! deliberate inversion of a closed status enum — canonical states stay fixed,
//! agents stay free to attach arbitrary text.
//!
//! Runtime-only: agent status is ephemeral and never persisted.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Caps on the agent-supplied free-form fields, mirroring the OSC 99 caps in
/// `okena-terminal`. The terminal byte stream is untrusted — any process in a
/// pane can push an `OSC 9001` with an arbitrarily large `msg`/`lbl`. Unlike a
/// one-shot notification body, an agent status is held on the `Terminal` *and*
/// re-serialized into every remote `/v1/state` response, so an unbounded value
/// would be pinned in memory and amplified to every connected client. Clamp at
/// the point the status is built ([`AgentStatus::new_clamped`]).
pub const MAX_CUSTOM_LEN: usize = 4096;
/// Max number of `labels` entries kept (lowest keys first; extras dropped).
pub const MAX_LABELS: usize = 32;
/// Max byte length of a single label key (over-long keys are truncated).
pub const MAX_LABEL_KEY_LEN: usize = 128;
/// Max byte length of a single label value (over-long values are truncated).
pub const MAX_LABEL_VALUE_LEN: usize = 1024;

/// Lifecycle state an agent reports about itself.
///
/// The token names match the `st=` values of the agent-status OSC (`clear` is
/// not a lifecycle — it removes the status, handled by the parser).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentLifecycle {
    /// The agent is actively working.
    Working,
    /// The agent is blocked waiting on the user (permission, input, …).
    Blocked,
    /// The agent finished its turn.
    Done,
    /// The agent is idle / at rest.
    Idle,
}

impl AgentLifecycle {
    /// Parse the `st=` token from the agent-status OSC. Returns `None` for
    /// unknown tokens (callers treat that as "ignore"). `clear` is handled by
    /// the caller as "remove status", not a lifecycle, so it is not parsed here.
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "working" => Some(Self::Working),
            "blocked" => Some(Self::Blocked),
            "done" => Some(Self::Done),
            "idle" => Some(Self::Idle),
            _ => None,
        }
    }

    /// Sort / attention priority, highest first: a blocked agent needs you now,
    /// a done agent wants acknowledging, working is in flight, idle is at rest.
    pub fn priority(self) -> u8 {
        match self {
            Self::Blocked => 3,
            Self::Done => 2,
            Self::Working => 1,
            Self::Idle => 0,
        }
    }

    /// Whether a transition *into* this state should raise a desktop
    /// notification. `blocked` and `done` are the actionable edges; `working`
    /// and `idle` are silent. (Visibility gating — e.g. not pinging for the
    /// pane you're watching — is the GPUI layer's job, not this enum's.)
    pub fn notifies(self) -> bool {
        matches!(self, Self::Blocked | Self::Done)
    }

    /// Short human-readable label for this lifecycle (e.g. a tab tooltip).
    /// Distinct from the `st=` wire token ([`from_token`]) even though they
    /// currently coincide — one is for display, the other for the protocol.
    pub fn label(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Idle => "idle",
        }
    }

    /// The theme palette slot this lifecycle maps to: blocked needs attention
    /// (error), working is in flight (warning yellow), done is finished
    /// (success), idle is at rest (muted). The single source of truth for agent
    /// coloring, shared by the tab indicator and the sidebar Agents list.
    /// Returns the raw `u32` color so `okena-core` needn't depend on gpui;
    /// callers wrap it with `rgb(..)`.
    pub fn theme_color(self, theme: &crate::theme::ThemeColors) -> u32 {
        match self {
            Self::Blocked => theme.error,
            Self::Working => theme.term_yellow,
            Self::Done => theme.success,
            Self::Idle => theme.text_muted,
        }
    }
}

/// A status an agent reports about itself in a terminal pane.
///
/// Pushed in-band via the agent-status OSC and surfaced in the tab, the sidebar
/// "Agents" section, and the remote API. Runtime-only — never persisted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStatus {
    pub lifecycle: AgentLifecycle,
    /// Free-form, human-readable status set by the agent, e.g.
    /// `"running tests 3/5"`. Rendered verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
    /// Optional structured key/value extras the agent attaches.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

impl AgentStatus {
    /// A status carrying only a lifecycle, no custom text or labels.
    pub fn new(lifecycle: AgentLifecycle) -> Self {
        Self {
            lifecycle,
            custom: None,
            labels: BTreeMap::new(),
        }
    }

    /// Build a status from raw decoded OSC fields, clamping `custom` and
    /// `labels` to bounded sizes (see the `MAX_*` consts) so a hostile pane
    /// can't pin unbounded memory or amplify it to every remote client. An
    /// empty `custom` (e.g. a `msg=` that decodes to `""`) collapses to `None`,
    /// so a notifying state still falls back to its default body instead of an
    /// empty string. This is the only constructor the OSC parser uses.
    pub fn new_clamped(
        lifecycle: AgentLifecycle,
        custom: Option<String>,
        labels: BTreeMap<String, String>,
    ) -> Self {
        let custom = custom.and_then(|mut c| {
            truncate_to_bytes(&mut c, MAX_CUSTOM_LEN);
            (!c.is_empty()).then_some(c)
        });
        Self {
            lifecycle,
            custom,
            labels: clamp_labels(labels),
        }
    }
}

/// Truncate `s` in place to at most `max` bytes, never splitting a UTF-8 char
/// (drops the straddling char whole). Used to bound agent-supplied text.
fn truncate_to_bytes(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

/// Bound a labels map against hostile input: keep at most [`MAX_LABELS`]
/// entries (lowest keys first) and truncate over-long keys/values. Truncated
/// keys may collide; the map dedups them, which is fine — this is a memory
/// bound, not a fidelity guarantee.
fn clamp_labels(labels: BTreeMap<String, String>) -> BTreeMap<String, String> {
    labels
        .into_iter()
        .take(MAX_LABELS)
        .map(|(mut k, mut v)| {
            truncate_to_bytes(&mut k, MAX_LABEL_KEY_LEN);
            truncate_to_bytes(&mut v, MAX_LABEL_VALUE_LEN);
            (k, v)
        })
        .collect()
}

/// Parse a flat `{"k":"v"}` JSON object into labels for the agent-status OSC's
/// `lbl=` field. Anything that isn't a string→string object yields an empty
/// map, so a malformed field is simply ignored. Lives here (next to the type)
/// rather than in `okena-terminal` so the parser crate needn't depend on
/// `serde_json`.
pub fn parse_labels_json(json: &str) -> BTreeMap<String, String> {
    serde_json::from_str(json).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_token_roundtrips() {
        for (tok, life) in [
            ("working", AgentLifecycle::Working),
            ("blocked", AgentLifecycle::Blocked),
            ("done", AgentLifecycle::Done),
            ("idle", AgentLifecycle::Idle),
        ] {
            assert_eq!(AgentLifecycle::from_token(tok), Some(life));
        }
        assert_eq!(AgentLifecycle::from_token("clear"), None);
        assert_eq!(AgentLifecycle::from_token("bogus"), None);
    }

    #[test]
    fn priority_orders_blocked_first() {
        assert!(AgentLifecycle::Blocked.priority() > AgentLifecycle::Done.priority());
        assert!(AgentLifecycle::Done.priority() > AgentLifecycle::Working.priority());
        assert!(AgentLifecycle::Working.priority() > AgentLifecycle::Idle.priority());
    }

    #[test]
    fn only_blocked_and_done_notify() {
        assert!(AgentLifecycle::Blocked.notifies());
        assert!(AgentLifecycle::Done.notifies());
        assert!(!AgentLifecycle::Working.notifies());
        assert!(!AgentLifecycle::Idle.notifies());
    }

    #[test]
    fn parse_labels_json_handles_valid_and_malformed() {
        let m = parse_labels_json(r#"{"stage":"verify","eta":"5m"}"#);
        assert_eq!(m.get("stage").map(String::as_str), Some("verify"));
        assert_eq!(m.len(), 2);
        // Anything that isn't a flat string->string object yields an empty map
        // (the documented `unwrap_or_default` branch) rather than an error.
        assert!(parse_labels_json("[1,2]").is_empty());
        assert!(parse_labels_json(r#""just a string""#).is_empty());
        assert!(parse_labels_json(r#"{"k":5}"#).is_empty());
        assert!(parse_labels_json("not json").is_empty());
        assert!(parse_labels_json("").is_empty());
    }

    #[test]
    fn new_clamped_bounds_hostile_input() {
        // Oversized custom is truncated to the byte cap...
        let huge = "x".repeat(MAX_CUSTOM_LEN * 4);
        let s = AgentStatus::new_clamped(AgentLifecycle::Working, Some(huge), BTreeMap::new());
        assert_eq!(s.custom.as_ref().map(|c| c.len()), Some(MAX_CUSTOM_LEN));

        // ...empty custom collapses to None (so the default body still applies)...
        let s = AgentStatus::new_clamped(AgentLifecycle::Blocked, Some(String::new()), BTreeMap::new());
        assert_eq!(s.custom, None);

        // ...and labels are bounded in count and per-key/value length.
        let mut labels = BTreeMap::new();
        for i in 0..(MAX_LABELS * 2) {
            labels.insert(format!("k{i:04}"), "v".to_string());
        }
        labels.insert("k".repeat(500), "v".repeat(5000));
        let s = AgentStatus::new_clamped(AgentLifecycle::Working, None, labels);
        assert!(s.labels.len() <= MAX_LABELS);
        for (k, v) in &s.labels {
            assert!(k.len() <= MAX_LABEL_KEY_LEN);
            assert!(v.len() <= MAX_LABEL_VALUE_LEN);
        }
    }

    #[test]
    fn truncate_to_bytes_respects_char_boundary() {
        // A multi-byte char straddling the cap is dropped whole, not split.
        let mut s = "€€€".to_string(); // 3 bytes each = 9 bytes total
        truncate_to_bytes(&mut s, 4); // cap lands inside the 2nd char
        assert_eq!(s, "€"); // only the first whole char survives
        assert!(s.is_char_boundary(s.len()));
    }

    #[test]
    fn serde_omits_empty_optional_fields() {
        let s = AgentStatus::new(AgentLifecycle::Working);
        let json = serde_json::to_string(&s).expect("serialize");
        assert_eq!(json, r#"{"lifecycle":"working"}"#);

        let parsed: AgentStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, s);
    }
}
