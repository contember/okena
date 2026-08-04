//! Per-harness agent capabilities — *resume a session* and *parse a transcript*
//! — keyed by agent id, kept **gpui-free** so they work in the desktop app AND
//! in headless/remote (where resume/stats may be driven from a mobile client).
//!
//! The agent-status protocol is harness-agnostic: a pane reports its `agent` id
//! (`"claude-code"`, `"codex"`, …) plus a `session_id` via `OSC 9001` (see
//! [`crate::agent_session`]). This registry is how Okena turns that id into
//! actions, without baking any single agent's specifics into the core or app.
//! The implementations live in the gpui-free `okena-agent-harnesses` crate and
//! are registered at startup via [`init`]; an unknown id simply has no entry, so
//! its session is stored/displayed but not resumable until a harness for it
//! exists. New harnesses are therefore purely additive.
//!
//! They are kept out of the `okena-ext-*` crates on purpose: those depend on
//! `gpui`, and the standalone `okena-daemon` binary — which owns restore — is
//! CI-gated GPUI-free, so it could not link them and every lookup here returned
//! `None`.

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
/// sessions. Implemented in `okena-agent-harnesses`, which must stay gpui-free
/// so the headless daemon links it too.
pub trait AgentHarness: Send + Sync {
    /// Harness id; must equal the `agent` id agents report on the wire
    /// (e.g. `"claude-code"`, `"codex"`).
    fn id(&self) -> &str;

    /// Build the argv that resumes `session_id` from working dir `cwd`, or
    /// `None` if this harness can't / shouldn't resume. `session_id` is already
    /// [`is_uuid_like`](crate::agent_session::is_uuid_like)-validated by the
    /// caller.
    ///
    /// The argv becomes the pane's startup command, which Okena composes into a
    /// shell line so the pane keeps a shell after the agent exits. Rather than
    /// quote per dialect (POSIX sh, cmd, PowerShell all differ), Okena requires
    /// every element to be a shell-neutral literal and **refuses to resume**
    /// otherwise — see [`resume_command_line`]. Return plain flags and
    /// validated ids; if a harness ever needs spaces or metacharacters, it
    /// should ship its own launcher script instead.
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
/// before serving requests. A second call is logged rather than swallowed —
/// silently keeping an earlier registry is the kind of thing that turns into a
/// no-op nobody can see.
pub fn init(registry: AgentHarnessRegistry) {
    if REGISTRY.set(registry).is_err() {
        log::warn!(
            "agent-harness: registry already installed — ignoring this one (first install wins)"
        );
    }
}

/// Look up the harness for an agent id, if one is registered. Returns `None`
/// before [`init`] or for an unknown id (caller treats that as "no resume/stats
/// for this agent").
pub fn for_agent(agent_id: &str) -> Option<&'static Arc<dyn AgentHarness>> {
    REGISTRY.get()?.get(agent_id)
}

/// Characters allowed in a resume argv element.
///
/// The line is composed into whichever shell the pane launches, so an element
/// is only accepted when it is a literal in *every* dialect Okena targets: no
/// whitespace, quotes, or metacharacters. This is what lets the composition be
/// a plain join with no dialect-specific quoting.
///
/// The composition itself lives in `okena_hooks::terminal_launch_parts` — that
/// is the crate that knows the dialects. If a new dialect ever makes one of the
/// bytes allowed here special, this is the function to narrow.
fn is_shell_neutral(arg: &str) -> bool {
    !arg.is_empty()
        && arg
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./=:@+,".contains(&b))
}

/// Build the shell line that resumes `session`, or `None` when it can't be
/// resumed safely.
///
/// Returns `None` when the session is malformed, no harness is registered for
/// its `agent` id, the harness declines, or any argv element is not
/// [shell-neutral](is_shell_neutral). Refusing beats quoting: the caller splices
/// this into a shell command line, and a resume that silently doesn't happen is
/// strictly better than one that mis-parses.
pub fn resume_command_line(session: &crate::agent_session::AgentSession, cwd: &Path) -> Option<String> {
    if !session.is_valid() {
        log::warn!(
            "agent-resume: refusing malformed session for agent {:?}",
            session.agent
        );
        return None;
    }
    let Some(harness) = for_agent(&session.agent) else {
        // Either no harness exists for this id, or nothing called `init` in this
        // process. Both are silent no-ops without a log, and the second one is a
        // wiring bug, so say so rather than returning like the branches below.
        log::warn!(
            "agent-resume: no harness registered for agent {:?} — not resuming",
            session.agent
        );
        return None;
    };
    let argv = harness.resume_command(&session.session_id, cwd)?;
    if argv.is_empty() || !argv.iter().all(|a| is_shell_neutral(a)) {
        log::warn!(
            "agent-resume: harness {:?} returned an argv that is not shell-neutral — not resuming",
            session.agent
        );
        return None;
    }
    Some(argv.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_session::AgentSession;

    const UUID: &str = "3b9c1f2a-4d5e-6f70-8a9b-0c1d2e3f4a5b";

    struct StubHarness {
        argv: Option<Vec<String>>,
    }

    impl AgentHarness for StubHarness {
        fn id(&self) -> &str {
            "stub"
        }
        fn resume_command(&self, _session_id: &str, _cwd: &Path) -> Option<Vec<String>> {
            self.argv.clone()
        }
    }

    fn session(agent: &str) -> AgentSession {
        AgentSession {
            agent: agent.to_string(),
            session_id: UUID.to_string(),
            transcript_path: None,
        }
    }

    fn line(argv: Option<Vec<&str>>) -> Option<String> {
        // Exercise the composition without touching the process-wide registry.
        let harness = StubHarness {
            argv: argv.map(|a| a.into_iter().map(str::to_string).collect()),
        };
        let s = session("stub");
        if !s.is_valid() {
            return None;
        }
        let argv = harness.resume_command(&s.session_id, Path::new("/proj"))?;
        (!argv.is_empty() && argv.iter().all(|a| is_shell_neutral(a))).then(|| argv.join(" "))
    }

    #[test]
    fn accepts_a_plain_resume_argv() {
        assert_eq!(
            line(Some(vec!["claude", "--resume", UUID])),
            Some(format!("claude --resume {UUID}"))
        );
    }

    #[test]
    fn refuses_anything_a_shell_would_reinterpret() {
        for argv in [
            vec!["claude", "--resume", "a b"],
            vec!["claude", "--resume", "$(id)"],
            vec!["claude", "; rm -rf ~"],
            vec!["claude", "--flag", "x|y"],
            vec!["claude", ""],
            vec![],
        ] {
            assert_eq!(line(Some(argv.clone())), None, "accepted {argv:?}");
        }
    }

    #[test]
    fn a_declining_harness_yields_no_line() {
        assert_eq!(line(None), None);
    }

    #[test]
    fn malformed_session_is_refused_before_the_harness_runs() {
        let bad = AgentSession {
            agent: "stub".to_string(),
            session_id: "$(rm -rf ~)".to_string(),
            transcript_path: None,
        };
        assert!(!bad.is_valid());
        assert_eq!(resume_command_line(&bad, Path::new("/proj")), None);
    }
}
