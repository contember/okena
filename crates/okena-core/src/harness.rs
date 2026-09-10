//! Engineering-harness view identifiers.
//!
//! Lives in `okena-core` because both the sidebar (which renders the nav) and
//! the window (which renders the view as a tab) need to name the same sections,
//! and those live in different crates that only share this one.

use serde::{Deserialize, Serialize};

/// The harness views, in nav order.
///
/// `all()` drives the nav, the tab strip and persistence, so the three cannot
/// drift out of sync.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessSection {
    Projects,
    Tasks,
    Specs,
    Knowledge,
}

impl HarnessSection {
    pub const fn all() -> [HarnessSection; 4] {
        [
            HarnessSection::Projects,
            HarnessSection::Tasks,
            HarnessSection::Specs,
            HarnessSection::Knowledge,
        ]
    }

    pub const fn label(self) -> &'static str {
        match self {
            HarnessSection::Projects => "Projects",
            HarnessSection::Tasks => "Tasks",
            HarnessSection::Specs => "Specs",
            HarnessSection::Knowledge => "Knowledge",
        }
    }

    /// Stable id used for element ids and persistence.
    pub const fn slug(self) -> &'static str {
        match self {
            HarnessSection::Projects => "projects",
            HarnessSection::Tasks => "tasks",
            HarnessSection::Specs => "specs",
            HarnessSection::Knowledge => "knowledge",
        }
    }

    /// One-line description of what the view is for. Shown as the section
    /// subtitle while the view itself is a stub.
    pub const fn blurb(self) -> &'static str {
        match self {
            HarnessSection::Projects => {
                "Agents in flight per project, what each is waiting on, and PR / pipeline status."
            }
            HarnessSection::Tasks => {
                "Epics, features and stories from your task manager — launch an agent on one."
            }
            HarnessSection::Specs => {
                "Spec documents broken down into epics, features and stories. Git-backed."
            }
            HarnessSection::Knowledge => {
                "Skills, technical designs and feature docs. Git-backed collections."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_unique_and_stable() {
        // Slugs key element ids and persisted tab state, so a duplicate would
        // silently collapse two tabs into one.
        let mut seen = std::collections::HashSet::new();
        for s in HarnessSection::all() {
            assert!(seen.insert(s.slug()), "duplicate slug: {}", s.slug());
        }
    }

    #[test]
    fn every_section_has_label_and_blurb() {
        for s in HarnessSection::all() {
            assert!(!s.label().is_empty());
            assert!(!s.blurb().is_empty());
        }
    }

    #[test]
    fn round_trips_through_serde() {
        for s in HarnessSection::all() {
            let json = serde_json::to_string(&s).expect("serialize");
            let back: HarnessSection = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, s);
        }
    }
}

// ─── Agent sessions ──────────────────────────────────────────────────────────

use serde::{Deserialize as De, Serialize as Ser};

/// What an agent produced.
///
/// Open-ended on purpose: agents report what they made, and the harness should
/// display a kind it doesn't model rather than dropping it.
#[derive(Clone, Debug, PartialEq, Eq, Ser, De, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentAssetKind {
    PullRequest,
    Branch,
    Document,
    #[default]
    #[serde(other)]
    Other,
}

impl AgentAssetKind {
    pub const fn label(&self) -> &'static str {
        match self {
            AgentAssetKind::PullRequest => "PR",
            AgentAssetKind::Branch => "branch",
            AgentAssetKind::Document => "doc",
            AgentAssetKind::Other => "asset",
        }
    }
}

/// One thing an agent produced — the sketch's "agent#1#asset#1 (PR on Proj1)".
#[derive(Clone, Debug, PartialEq, Eq, Ser, De)]
pub struct AgentAsset {
    pub kind: AgentAssetKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Which repo it landed in. An agent spanning several projects produces
    /// assets in more than one, so the asset carries its own project rather
    /// than inheriting the session's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Unix millis, stamped by the daemon on receipt. Agents have no reliable
    /// clock agreement with the host, so their timestamps are not trusted.
    #[serde(default)]
    pub created_at: u64,
}

/// Agent-reported state for a session project.
#[derive(Clone, Debug, PartialEq, Eq, Ser, De, Default)]
pub struct AgentSessionState {
    /// Free-text status the agent last reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<AgentAsset>,
}

#[cfg(test)]
mod agent_tests {
    use super::{AgentAsset, AgentAssetKind, AgentSessionState};

    #[test]
    fn unknown_asset_kind_decodes_as_other() {
        // A newer agent reporting a kind this build doesn't model must still
        // show up, not fail the whole payload.
        let k: AgentAssetKind = serde_json::from_str("\"deployment\"").expect("decode");
        assert_eq!(k, AgentAssetKind::Other);
    }

    #[test]
    fn known_kinds_round_trip() {
        for k in [
            AgentAssetKind::PullRequest,
            AgentAssetKind::Branch,
            AgentAssetKind::Document,
        ] {
            let j = serde_json::to_string(&k).expect("encode");
            let back: AgentAssetKind = serde_json::from_str(&j).expect("decode");
            assert_eq!(back, k);
        }
    }

    #[test]
    fn empty_session_state_serializes_compactly() {
        // Every project carries this field; an empty one must not bloat
        // workspace.json with nulls and empty arrays.
        let j = serde_json::to_string(&AgentSessionState::default()).expect("encode");
        assert_eq!(j, "{}");
    }

    #[test]
    fn asset_keeps_its_own_project() {
        let a = AgentAsset {
            kind: AgentAssetKind::PullRequest,
            title: "Add harness".into(),
            url: Some("https://github.com/x/y/pull/12".into()),
            project: Some("okena".into()),
            created_at: 42,
        };
        let back: AgentAsset =
            serde_json::from_str(&serde_json::to_string(&a).expect("encode")).expect("decode");
        assert_eq!(back, a);
        assert_eq!(back.kind.label(), "PR");
    }
}
