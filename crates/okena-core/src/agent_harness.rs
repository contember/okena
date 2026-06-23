//! Per-harness agent capabilities — *resume a session* and *parse a transcript*
//! — keyed by agent id, kept **gpui-free** so they work in the desktop app AND
//! in headless/remote (where resume/stats may be driven from a mobile client).
//!
//! The agent-status protocol is harness-agnostic: a pane reports its `agent` id
//! (`"claude-code"`, `"codex"`, …) plus a `session_id` via `OSC 9001` (see
//! [`crate::agent_session`]). This registry is how Okena turns that id into
//! actions, without baking any single agent's specifics into the core or app.
//! Each harness implementation lives in its matching `okena-ext-*` crate and is
//! registered at startup via [`init`]; an unknown id simply has no entry, so its
//! session is stored/displayed but not resumable until a harness for it exists.
//! New harnesses are therefore purely additive.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// Best-effort stats parsed from a session transcript, for the session-info
/// view. Each harness fills what its format exposes; absent fields stay `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TranscriptStats {
    pub user_messages: usize,
    pub assistant_messages: usize,
    pub tool_calls: usize,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

/// What one agent harness (Claude Code, Codex, …) knows how to do with its own
/// sessions. Implemented in the matching `okena-ext-*` crate. Must stay
/// gpui-free so headless/remote can use it too.
pub trait AgentHarness: Send + Sync {
    /// Harness id; must equal the `agent` id agents report on the wire
    /// (e.g. `"claude-code"`, `"codex"`).
    fn id(&self) -> &str;

    /// Build the argv that resumes `session_id` from working dir `cwd`, or
    /// `None` if this harness can't / shouldn't resume. `session_id` is already
    /// [`is_uuid_like`](crate::agent_session::is_uuid_like)-validated by the
    /// caller. The argv is launched as a command in the pane and is **never**
    /// passed through a shell, so no quoting/escaping is needed here.
    fn resume_command(&self, session_id: &str, cwd: &Path) -> Option<Vec<String>>;

    /// Parse a transcript file into [`TranscriptStats`], or `None` when the path
    /// is unreadable / unsupported. Best-effort — partial stats are fine.
    fn transcript_stats(&self, transcript_path: &Path) -> Option<TranscriptStats> {
        let _ = transcript_path;
        None
    }
}

/// Registry of harnesses keyed by [`AgentHarness::id`]. Built once at startup
/// (desktop + headless) and installed via [`init`].
#[derive(Default)]
pub struct AgentHarnessRegistry {
    by_id: HashMap<String, Arc<dyn AgentHarness>>,
}

impl AgentHarnessRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a harness (last registration for a given id wins).
    pub fn register(&mut self, harness: Arc<dyn AgentHarness>) {
        self.by_id.insert(harness.id().to_string(), harness);
    }

    pub fn get(&self, agent_id: &str) -> Option<&Arc<dyn AgentHarness>> {
        self.by_id.get(agent_id)
    }
}

static REGISTRY: OnceLock<AgentHarnessRegistry> = OnceLock::new();

/// Install the process-wide harness registry. Call once at startup; later calls
/// are ignored (first wins), so desktop and headless each build their own
/// before serving requests.
pub fn init(registry: AgentHarnessRegistry) {
    let _ = REGISTRY.set(registry);
}

/// Look up the harness for an agent id, if one is registered. Returns `None`
/// before [`init`] or for an unknown id (caller treats that as "no resume/stats
/// for this agent").
pub fn for_agent(agent_id: &str) -> Option<&'static Arc<dyn AgentHarness>> {
    REGISTRY.get()?.get(agent_id)
}
