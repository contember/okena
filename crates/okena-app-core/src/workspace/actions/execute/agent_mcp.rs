//! Wiring okena's MCP server into the agents okena launches.
//!
//! The point is that the user never configures this. When okena starts an
//! agent, it also hands that agent a config file pointing at `okena mcp`, so
//! the agent can call `okena_whoami`, report status and register assets without
//! anyone editing a settings file.
//!
//! The config is written **into okena's profile directory**, never into the
//! worktree. Writing `.mcp.json` into a checkout would either clobber a
//! repository's own checked-in config or leave an untracked file dirtying every
//! `git status` — both worse than an extra command-line flag.

use okena_workspace::settings::AppSettings;
use std::path::PathBuf;

/// Placeholder replaced with the generated config's path.
const CONFIG_PLACEHOLDER: &str = "{config}";

/// Default flags per agent for pointing it at an MCP config file.
///
/// Only agents whose flag is known are listed; anything else gets no injection
/// rather than a guessed flag that would make the agent fail to start.
/// `settings.harness.agent_mcp_args` overrides this per deployment, so a
/// changed CLI can be fixed in settings instead of waiting for a release.
fn default_mcp_args(agent: &str) -> Option<Vec<String>> {
    match agent {
        // Verified against `claude --help`: takes JSON files or strings.
        "claude" => Some(vec!["--mcp-config".into(), CONFIG_PLACEHOLDER.into()]),
        // Verified against `copilot --help`: takes a JSON string, or a file
        // path when prefixed with `@`. It augments ~/.copilot/mcp-config.json
        // rather than replacing it, so the user's own servers survive.
        "copilot" => Some(vec![
            "--additional-mcp-config".into(),
            format!("@{CONFIG_PLACEHOLDER}"),
        ]),
        _ => None,
    }
}

/// Path of the generated MCP config for the active profile.
fn config_path() -> Option<PathBuf> {
    okena_core::profiles::try_current().map(|p| p.root.join("agent-mcp.json"))
}

/// Write (or refresh) the MCP config pointing at this okena binary.
///
/// Rewritten on every launch rather than cached: the binary path changes across
/// upgrades and profile switches, and a stale path yields an agent whose MCP
/// tools silently fail to start.
fn write_config() -> Option<PathBuf> {
    let path = config_path()?;
    let exe = std::env::current_exe().ok()?;

    let body = serde_json::json!({
        "mcpServers": {
            "okena": {
                "command": exe.to_string_lossy(),
                "args": ["mcp"],
            }
        }
    });

    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return None;
    }
    match std::fs::write(&path, serde_json::to_string_pretty(&body).ok()?) {
        Ok(()) => Some(path),
        Err(e) => {
            log::warn!("[tasks] could not write agent MCP config: {e}");
            None
        }
    }
}

/// Extra arguments that point `agent` at okena's MCP server.
///
/// Empty when injection is disabled, the agent's flag is unknown, or the config
/// could not be written — in every one of those cases the agent still launches,
/// just without okena's tools.
pub(super) fn injection_args(agent: &str, settings: &AppSettings) -> Vec<String> {
    if !settings.harness.agent_mcp_injection {
        return Vec::new();
    }
    let template = settings
        .harness
        .agent_mcp_args
        .clone()
        .filter(|t| !t.is_empty())
        .or_else(|| default_mcp_args(agent));
    let Some(template) = template else {
        return Vec::new();
    };
    let Some(path) = write_config() else {
        return Vec::new();
    };
    let path = path.to_string_lossy().into_owned();
    template
        .into_iter()
        .map(|arg| arg.replace(CONFIG_PLACEHOLDER, &path))
        .collect()
}

/// Whether `args` show okena's MCP config was injected.
///
/// Used by the Agents view to report whether a running session actually has
/// okena's tools, rather than assuming every agent it launched does.
pub fn args_have_mcp(args: &[String]) -> bool {
    args.iter().any(|a| a.ends_with("agent-mcp.json"))
}

#[cfg(test)]
mod tests {
    use super::{args_have_mcp, default_mcp_args};
    use okena_workspace::settings::AppSettings;

    #[test]
    fn claude_has_a_known_flag() {
        let args = default_mcp_args("claude").expect("claude is supported");
        assert_eq!(args[0], "--mcp-config");
        assert_eq!(args[1], "{config}");
    }

    #[test]
    fn copilot_takes_a_file_path_prefixed_with_at() {
        let args = default_mcp_args("copilot").expect("copilot is supported");
        assert_eq!(args[0], "--additional-mcp-config");
        assert_eq!(args[1], "@{config}");
    }

    #[test]
    fn an_unknown_agent_gets_no_injection() {
        // Guessing a flag would make the agent fail to start, which is worse
        // than launching it without okena's tools.
        assert!(default_mcp_args("some-new-agent").is_none());
        assert!(default_mcp_args("").is_none());
    }

    #[test]
    fn injection_is_on_by_default() {
        assert!(AppSettings::default().harness.agent_mcp_injection);
    }

    #[test]
    fn disabling_injection_yields_no_args() {
        let mut s = AppSettings::default();
        s.harness.agent_mcp_injection = false;
        assert!(super::injection_args("claude", &s).is_empty());
    }

    #[test]
    fn detects_an_injected_config_in_args() {
        assert!(args_have_mcp(&[
            "--mcp-config".into(),
            "/x/profiles/dev/agent-mcp.json".into()
        ]));
        // copilot's `@`-prefixed form must be recognized too, or its sessions
        // would wrongly report "no okena mcp".
        assert!(args_have_mcp(&[
            "--additional-mcp-config".into(),
            "@/x/profiles/dev/agent-mcp.json".into()
        ]));
    }

    #[test]
    fn plain_args_are_not_mistaken_for_injection() {
        assert!(!args_have_mcp(&["Work on LIN-1".into()]));
        assert!(!args_have_mcp(&[]));
    }
}
