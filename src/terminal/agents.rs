//! AI agent process detection and session ID resolution.
//!
//! Detects AI coding agents (Claude Code, Copilot CLI) running inside terminal
//! processes and reads their session IDs from agent-specific storage locations:
//! - Claude: `~/.claude/sessions/<pid>.json`
//! - Copilot: `~/.copilot/session-state/<uuid>/inuse.<pid>.lock`

use crate::workspace::state::AgentType;

/// Detected agent info including the agent's PID (for session file lookup).
pub struct DetectedAgent {
    pub agent_type: AgentType,
    pub pid: u32,
}

/// Detect AI coding agent among all descendant processes of the given PID.
/// Returns the AgentType and PID if a known agent is found.
#[cfg(unix)]
pub fn detect_agent_process(shell_pid: u32) -> Option<DetectedAgent> {
    let descendants = collect_descendant_pids(shell_pid);

    descendants.iter()
        .filter_map(|&pid| {
            let cmdline = std::fs::read(format!("/proc/{}/cmdline", pid)).ok()?;
            let exe = cmdline.split(|&b| b == 0).next()?;
            let basename = String::from_utf8_lossy(exe);
            let basename = basename.rsplit('/').next()?;
            match basename {
                "claude" => Some(DetectedAgent { agent_type: AgentType::Claude, pid }),
                "copilot" => Some(DetectedAgent { agent_type: AgentType::Copilot, pid }),
                _ => None,
            }
        })
        .next()
}

#[cfg(not(unix))]
pub fn detect_agent_process(_shell_pid: u32) -> Option<DetectedAgent> {
    None
}

/// Read the agent session ID using the appropriate method for the agent type.
#[cfg(unix)]
pub fn read_agent_session_id(agent: &DetectedAgent) -> Option<String> {
    match agent.agent_type {
        AgentType::Claude => read_claude_session_id(agent.pid),
        AgentType::Copilot => read_copilot_session_id(agent.pid),
    }
}

#[cfg(not(unix))]
pub fn read_agent_session_id(_agent: &DetectedAgent) -> Option<String> {
    None
}

/// Read a Claude Code session ID from `~/.claude/sessions/<pid>.json`.
#[cfg(unix)]
fn read_claude_session_id(pid: u32) -> Option<String> {
    let path = dirs::home_dir()?.join(format!(".claude/sessions/{}.json", pid));
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<serde_json::Value>(&content)
        .ok()?
        .get("sessionId")?
        .as_str()
        .map(str::to_string)
}

/// Read a Copilot CLI session ID from `~/.copilot/session-state/`.
/// Copilot creates `inuse.<child_pid>.lock` files inside session directories.
/// We search for lock files matching the copilot PID or any of its children.
#[cfg(unix)]
fn read_copilot_session_id(pid: u32) -> Option<String> {
    let session_dir = dirs::home_dir()?.join(".copilot/session-state");
    if !session_dir.is_dir() {
        return None;
    }

    // Collect the copilot PID + its direct children (copilot spawns a backend process)
    let mut pids_to_match: Vec<u32> = vec![pid];
    if let Ok(output) = std::process::Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
    {
        pids_to_match.extend(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|l| l.trim().parse::<u32>().ok()),
        );
    }

    // Search session directories for inuse.<pid>.lock files
    std::fs::read_dir(&session_dir).ok()?.find_map(|entry| {
        let entry = entry.ok()?;
        let session_id = entry.file_name().to_str()?.to_string();
        let has_lock = pids_to_match.iter().any(|p| {
            entry.path().join(format!("inuse.{}.lock", p)).exists()
        });
        has_lock.then_some(session_id)
    })
}

/// Recursively collect all descendant PIDs of a given process.
#[cfg(unix)]
fn collect_descendant_pids(root_pid: u32) -> Vec<u32> {
    let mut all = Vec::new();
    let mut queue = vec![root_pid];

    while let Some(pid) = queue.pop() {
        let output = std::process::Command::new("pgrep")
            .args(["-P", &pid.to_string()])
            .output();

        if let Ok(output) = output {
            let children: Vec<u32> = String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| line.trim().parse::<u32>().ok())
                .collect();
            all.extend(&children);
            queue.extend(children);
        }
    }
    all
}
