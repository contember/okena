//! The agents okena offers to start, and how each looks on a launcher.
//!
//! One list for every place that starts an agent. There were three copies of
//! it — the Tasks view, the Specs view and the New agent dialog each had their
//! own — which is how a new agent ends up startable from one place and missing
//! from the next.

use super::{AGENT_COMMANDS, AgentSessionInfo};
use crate::theme::ThemeColors;
use okena_ui::agent_launcher::{LaunchOption, LauncherSession};

/// An agent okena knows how to start and recognize.
struct KnownAgent {
    command: &'static str,
    label: &'static str,
    icon: &'static str,
    /// The agent's own colour rather than a theme colour, so it is recognized
    /// by it. Drawn over a faint wash of itself, which reads on light and dark
    /// themes alike.
    accent: u32,
}

const KNOWN_AGENTS: &[KnownAgent] = &[
    KnownAgent {
        command: "claude",
        label: "Claude",
        icon: "icons/agent-claude.svg",
        accent: 0xD97757,
    },
    KnownAgent {
        command: "copilot",
        label: "Copilot",
        icon: "icons/agent-copilot.svg",
        accent: 0x8B7CF6,
    },
];

/// The option for `command`: the known agent's look, or a terminal for a
/// command okena does not know.
pub fn launch_option(command: &str, t: &ThemeColors) -> LaunchOption {
    match KNOWN_AGENTS.iter().find(|a| a.command == command) {
        Some(agent) => LaunchOption {
            command: agent.command.into(),
            label: agent.label.into(),
            icon: agent.icon.into(),
            accent: agent.accent,
        },
        None => LaunchOption {
            command: command.to_string().into(),
            label: command.to_string().into(),
            icon: "icons/terminal.svg".into(),
            accent: t.text_secondary,
        },
    }
}

/// Starting without an agent, named for what that means where it is offered —
/// "Worktrees only", "Scaffold only", "Plain shell".
pub fn no_agent_option(label: &str, t: &ThemeColors) -> LaunchOption {
    LaunchOption {
        command: "".into(),
        label: label.to_string().into(),
        icon: "icons/terminal.svg".into(),
        accent: t.text_secondary,
    }
}

/// Commands offered for a launch, in order.
///
/// The configured agent leads, so the default sits where a hand goes first,
/// and is offered even when okena does not know it: a custom command in
/// settings must stay startable from everywhere a known one is.
pub fn offered_commands(configured: Option<&str>) -> Vec<String> {
    let configured = configured.map(str::trim).filter(|c| !c.is_empty());
    let mut commands: Vec<String> = configured.map(str::to_string).into_iter().collect();
    for command in AGENT_COMMANDS {
        if configured != Some(*command) {
            commands.push((*command).to_string());
        }
    }
    commands
}

/// Every agent option for a launch. See [`offered_commands`].
pub fn launch_options(configured: Option<&str>, t: &ThemeColors) -> Vec<LaunchOption> {
    offered_commands(configured)
        .iter()
        .map(|command| launch_option(command, t))
        .collect()
}

/// A running session as a launcher shows it.
pub fn launcher_session(info: &AgentSessionInfo, t: &ThemeColors) -> LauncherSession {
    let activity = info.activity();
    LauncherSession {
        id: info.project_id.clone().into(),
        name: info.name.clone().into(),
        activity: activity.label().into(),
        activity_color: activity.color(t),
        agent: info
            .agent
            .as_deref()
            .map(|command| launch_option(command, t)),
        status: info.status.clone().map(Into::into),
        // Only worth saying of a running agent: it explains why nothing it
        // does shows up here.
        warning: (info.running && !info.mcp).then(|| "no okena mcp".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::{AGENT_COMMANDS, KNOWN_AGENTS, offered_commands};

    #[test]
    fn every_detected_agent_has_a_look_and_every_look_is_detected() {
        // Otherwise okena would start an agent it then fails to recognize, or
        // recognize one it never offers.
        let known: Vec<&str> = KNOWN_AGENTS.iter().map(|a| a.command).collect();
        assert_eq!(known, AGENT_COMMANDS);
    }

    #[test]
    fn the_configured_agent_leads() {
        assert_eq!(offered_commands(Some("copilot")), ["copilot", "claude"]);
    }

    #[test]
    fn an_unknown_configured_agent_is_still_offered() {
        assert_eq!(
            offered_commands(Some("codex")),
            ["codex", "claude", "copilot"]
        );
    }

    #[test]
    fn a_blank_configured_agent_offers_the_known_ones() {
        assert_eq!(offered_commands(Some("  ")), ["claude", "copilot"]);
        assert_eq!(offered_commands(None), ["claude", "copilot"]);
    }
}
