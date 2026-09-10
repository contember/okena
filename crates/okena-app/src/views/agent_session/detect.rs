//! Recognizing which terminal sessions are running a coding agent.
//!
//! A heuristic over the session's launch command and OSC title, so it is
//! labelled as such wherever it surfaces rather than presented as
//! authoritative. An agent that renames its own title, or one launched through
//! a wrapper script, will be missed; nothing here should be read as "these are
//! all the agents".

use okena_core::shell::ShellType;

/// Commands that identify an AI coding agent.
///
/// Matched against the session's custom-shell command and its terminal title.
/// Deliberately a short, explicit list: a broad pattern would sweep in ordinary
/// shells and make the view untrustworthy.
pub const AGENT_COMMANDS: &[&str] = &["claude", "copilot"];

/// Identify the agent a session is running, if any.
///
/// Returns the matched command name so the UI can label the row ("claude")
/// rather than asserting a vendor.
pub fn detect_agent(shell: &ShellType, title: Option<&str>) -> Option<String> {
    // A custom shell records the command okena launched, which is the strongest
    // signal available — it is what okena itself ran, not what the process
    // later claimed via an escape sequence.
    if let ShellType::Custom { path, .. } = shell {
        let base = path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(path.as_str())
            .to_ascii_lowercase();
        if let Some(cmd) = AGENT_COMMANDS.iter().find(|c| base == **c) {
            return Some((*cmd).to_string());
        }
    }
    // Fall back to the OSC title, which most agents set. Weaker: a shell
    // sitting in a directory named "claude" would match, so require the title
    // to start with the command.
    let title = title?.trim().to_ascii_lowercase();
    AGENT_COMMANDS
        .iter()
        .find(|c| title == **c || title.starts_with(&format!("{c} ")))
        .map(|c| (*c).to_string())
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: that pulls in the `gpui::*` glob, whose `test`
    // attribute macro shadows the built-in one and recurses forever.
    use super::{AGENT_COMMANDS, detect_agent};
    use okena_core::shell::ShellType;

    fn custom(path: &str) -> ShellType {
        ShellType::Custom {
            path: path.to_string(),
            args: Vec::new(),
        }
    }

    #[test]
    fn detects_agent_from_launch_command() {
        assert_eq!(
            detect_agent(&custom("claude"), None).as_deref(),
            Some("claude")
        );
        // An absolute path still resolves to its basename.
        assert_eq!(
            detect_agent(&custom("/opt/homebrew/bin/copilot"), None).as_deref(),
            Some("copilot")
        );
    }

    #[test]
    fn plain_shell_is_not_an_agent() {
        assert_eq!(detect_agent(&ShellType::Default, None), None);
        assert_eq!(detect_agent(&custom("/bin/zsh"), None), None);
    }

    #[test]
    fn detects_agent_from_title() {
        assert_eq!(
            detect_agent(&ShellType::Default, Some("claude")).as_deref(),
            Some("claude")
        );
        assert_eq!(
            detect_agent(&ShellType::Default, Some("Copilot working…")).as_deref(),
            Some("copilot")
        );
    }

    #[test]
    fn title_must_start_with_the_command() {
        // A shell sitting in a directory named after an agent must not match —
        // that would fill the view with things that aren't agents.
        assert_eq!(
            detect_agent(&ShellType::Default, Some("~/src/claude")),
            None
        );
        assert_eq!(
            detect_agent(&ShellType::Default, Some("vim claude.rs")),
            None
        );
    }

    #[test]
    fn a_pane_inheriting_the_projects_shell_is_detected() {
        // What okena actually produces: the pane is `Default` and the agent
        // command sits on the project. Resolving only the pane finds nothing.
        let node = ShellType::Default;
        let project_default = Some(custom("claude"));
        let resolved = match node {
            ShellType::Default => project_default.clone().unwrap_or_default(),
            explicit => explicit,
        };
        assert_eq!(detect_agent(&resolved, None).as_deref(), Some("claude"));
    }

    #[test]
    fn an_explicit_pane_shell_wins_over_the_projects() {
        // A pane the user pointed at a plain shell must not be reported as an
        // agent just because the project defaults to one.
        let node = custom("/bin/zsh");
        let project_default = Some(custom("claude"));
        let resolved = match node {
            ShellType::Default => project_default.clone().unwrap_or_default(),
            explicit => explicit,
        };
        assert_eq!(detect_agent(&resolved, None), None);
    }

    #[test]
    fn command_match_is_exact_not_substring() {
        // `claudius` is not `claude`.
        assert_eq!(detect_agent(&custom("claudius"), None), None);
    }

    #[test]
    fn agent_command_list_is_lowercase() {
        // Detection lowercases its input, so an uppercase entry could never match.
        for c in AGENT_COMMANDS {
            assert_eq!(*c, &c.to_ascii_lowercase(), "{c} must be lowercase");
        }
    }
}
