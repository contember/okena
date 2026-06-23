//! The Claude Code agent harness — how Okena resumes a captured Claude session
//! (and, later, reads its transcript). Registered into the gpui-free
//! [`okena_core::agent_harness`] registry so resume works on desktop *and*
//! headless/remote.

use okena_core::agent_harness::AgentHarness;
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

/// Build the Claude Code harness for registration in `okena_core::agent_harness`.
pub fn register_harness() -> Arc<dyn AgentHarness> {
    Arc::new(ClaudeHarness)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_command_shape() {
        let h = ClaudeHarness;
        assert_eq!(h.id(), "claude-code");
        assert_eq!(
            h.resume_command("3b9c1f2a-4d5e-6f70-8a9b-0c1d2e3f4a5b", Path::new("/proj")),
            Some(vec![
                "claude".to_string(),
                "--resume".to_string(),
                "3b9c1f2a-4d5e-6f70-8a9b-0c1d2e3f4a5b".to_string(),
            ])
        );
    }
}
