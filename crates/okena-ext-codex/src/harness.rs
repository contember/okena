//! The Codex agent harness. The resume command is a placeholder until the exact
//! `codex` resume invocation is confirmed — the point here is the **seam**: a
//! second harness plugging into [`okena_core::agent_harness`] purely by
//! registering, no core/app change. Codex session *capture* (its own hook glue)
//! and transcript stats land alongside this. `okena-ext-codex` already knows how
//! to read `~/.codex/sessions/**.jsonl` (see `usage.rs`), so `transcript_stats`
//! is a natural follow-up here.

use okena_core::agent_harness::AgentHarness;
use std::path::Path;
use std::sync::Arc;

/// Codex (`codex` CLI). Agent id `"codex"` — matches the extension id.
pub struct CodexHarness;

impl AgentHarness for CodexHarness {
    fn id(&self) -> &str {
        "codex"
    }

    fn resume_command(&self, _session_id: &str, _cwd: &Path) -> Option<Vec<String>> {
        // TODO: confirm Codex's resume CLI invocation before enabling
        // auto-resume for Codex. `None` = no auto-resume yet (graceful — the
        // session is still captured, persisted, and shown).
        None
    }
}

/// Build the Codex harness for registration in `okena_core::agent_harness`.
pub fn register_harness() -> Arc<dyn AgentHarness> {
    Arc::new(CodexHarness)
}
