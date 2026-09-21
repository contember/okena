//! The built-in [`AgentHarness`] implementations, and the one place they are
//! installed into the process-wide registry.
//!
//! # Why these don't live in the `okena-ext-*` crates
//!
//! They used to. A harness is dispatched by the `agent` id a pane reports over
//! `OSC 9001`, and the natural home looked like the matching extension crate —
//! `okena-ext-claude` for `"claude-code"`, and so on. But those crates depend on
//! `gpui` (they also ship status-bar widgets and a settings view), and the
//! standalone `okena-daemon` binary is CI-gated to be **GPUI-free**. It
//! therefore could not link them, could not install the registry, and its
//! `for_agent()` lookups all returned `None` — auto-resume was a silent no-op in
//! exactly the deployment that owns restore.
//!
//! A harness is pure data — an id and a fixed argv — so it costs nothing to keep
//! it out of the UI crates. This crate is that home: gpui-free, linked by both
//! the desktop binary and the daemon binary, so both resolve the same harnesses.
//!
//! Adding a harness stays additive: implement [`AgentHarness`] here and register
//! it in [`build_registry`].
//!
//! # Relationship to the extension toggle
//!
//! Registration here is **not** gated on the `okena-extensions` enable/disable
//! toggle that `okena-ext-claude` / `okena-ext-codex` carry. That toggle governs
//! their GPUI status-bar widgets and settings view, and it lives behind a GPUI
//! global the daemon cannot read. Resume has its own opt-in — the
//! `auto_resume_agent_sessions` setting, read daemon-side — which is the gate
//! that actually applies to it.

use okena_core::agent_harness::{AgentHarness, AgentHarnessRegistry};
use std::path::Path;
use std::sync::Arc;

/// Claude Code (`claude` CLI). Agent id `"claude-code"` — matches the extension
/// id and the `OKENA_AGENT` the bundled lifecycle plugin sets.
pub struct ClaudeHarness;

impl AgentHarness for ClaudeHarness {
    fn id(&self) -> &str {
        "claude-code"
    }

    fn resume_command(&self, session_id: &str, _cwd: &Path) -> Option<Vec<String>> {
        // `claude --resume <id>` resumes a specific conversation. Claude scopes
        // session lookup to the cwd it runs in, which is exactly the pane's
        // restored working directory — so cwd needs no special handling here.
        // `session_id` is already UUID-validated upstream and is passed as a
        // distinct argv element (never shell-interpolated).
        Some(vec![
            "claude".to_string(),
            "--resume".to_string(),
            session_id.to_string(),
        ])
    }
}

/// Codex (`codex` CLI). Agent id `"codex"` — matches the extension id.
///
/// Registration-only for now: the resume invocation is unconfirmed, so this
/// declines rather than guessing. `okena-ext-codex` already parses
/// `~/.codex/sessions/**.jsonl`, which is where `transcript_stats` would come
/// from once there is a view that consumes it.
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

/// Build the registry of built-in harnesses.
pub fn build_registry() -> AgentHarnessRegistry {
    let mut registry = AgentHarnessRegistry::new();
    registry.register(Arc::new(ClaudeHarness));
    registry.register(Arc::new(CodexHarness));
    registry
}

/// Install the built-in harnesses process-wide.
///
/// Call once during startup, from every binary that can reach a restore path:
/// the desktop app, `okena --headless`, and the standalone `okena-daemon`.
/// Missing the call is not a compile error — it just makes every resume lookup
/// return `None` — which is why [`okena_core::agent_harness::init`] logs when a
/// second install is ignored.
pub fn install() {
    okena_core::agent_harness::init(build_registry());
}

#[cfg(test)]
mod tests {
    use super::*;

    const UUID: &str = "3b9c1f2a-4d5e-6f70-8a9b-0c1d2e3f4a5b";

    #[test]
    fn claude_resumes_with_a_fixed_argv() {
        assert_eq!(ClaudeHarness.id(), "claude-code");
        assert_eq!(
            ClaudeHarness.resume_command(UUID, Path::new("/proj")),
            Some(vec![
                "claude".to_string(),
                "--resume".to_string(),
                UUID.to_string(),
            ])
        );
    }

    #[test]
    fn codex_declines_until_its_invocation_is_confirmed() {
        assert_eq!(CodexHarness.id(), "codex");
        assert_eq!(CodexHarness.resume_command(UUID, Path::new("/proj")), None);
    }

    #[test]
    fn the_registry_resolves_every_built_in_id() {
        let registry = build_registry();
        assert!(registry.get("claude-code").is_some());
        assert!(registry.get("codex").is_some());
        assert!(registry.get("nope").is_none());
    }
}
