//! Pane environment for Claude remote: the variables that make Bun load
//! Okena's preload into `claude` (see `integrations/claude-code/okena-remote`).

use std::path::Path;

const BUN_OPTIONS: &str = "BUN_OPTIONS";
const OKENA_BIN: &str = "OKENA_BIN";
const CHANNEL: &str = "OKENA_CLAUDE_REMOTE_CHANNEL";

/// Extra PTY environment for panes created from now on.
///
/// When the feature is off or cannot work, the variables are removed (and
/// `BUN_OPTIONS` reset to the inherited value) rather than left out: a session
/// backend server such as tmux keeps the environment of the previous spawn.
///
/// * `preload` — the written `preload.js`.
/// * `okena_bin` — the `okena` binary the hooks and the channel server run.
/// * `inherited_bun_options` — `BUN_OPTIONS` of the daemon's own environment,
///   kept after our preload.
pub fn claude_remote_pty_env(
    enabled: bool,
    channel: bool,
    preload: Option<&Path>,
    okena_bin: Option<&Path>,
    inherited_bun_options: Option<&str>,
) -> Vec<(String, Option<String>)> {
    let inherited = inherited_bun_options.filter(|options| !options.trim().is_empty());
    let env = match (enabled, preload, okena_bin) {
        (true, Some(preload), Some(okena_bin)) => {
            enabled_env(channel, preload, okena_bin, inherited)
        }
        _ => None,
    };
    env.unwrap_or_else(|| {
        vec![
            (BUN_OPTIONS.to_string(), inherited.map(str::to_string)),
            (OKENA_BIN.to_string(), None),
            (CHANNEL.to_string(), None),
        ]
    })
}

fn enabled_env(
    channel: bool,
    preload: &Path,
    okena_bin: &Path,
    inherited: Option<&str>,
) -> Option<Vec<(String, Option<String>)>> {
    let (Some(preload), Some(okena_bin)) = (preload.to_str(), okena_bin.to_str()) else {
        log::warn!("[claude-remote] disabled: preload or okena binary path is not UTF-8");
        return None;
    };
    // Bun splits BUN_OPTIONS on whitespace and has no quoting.
    if preload.chars().any(char::is_whitespace) {
        log::warn!("[claude-remote] disabled: preload path {preload:?} contains whitespace");
        return None;
    }

    let bun_options = match inherited {
        Some(inherited) => format!("--preload {preload} {inherited}"),
        None => format!("--preload {preload}"),
    };
    let channel = if channel { "1" } else { "0" };
    Some(vec![
        (BUN_OPTIONS.to_string(), Some(bun_options)),
        (OKENA_BIN.to_string(), Some(okena_bin.to_string())),
        (CHANNEL.to_string(), Some(channel.to_string())),
    ])
}

#[cfg(test)]
mod tests {
    use super::claude_remote_pty_env;
    use std::path::Path;

    const PRELOAD: &str = "/run/user/1000/okena/default/claude-remote/preload.js";
    const OKENA_BIN: &str = "/opt/okena/okena";

    fn env_for(
        enabled: bool,
        channel: bool,
        preload: Option<&str>,
        okena_bin: Option<&str>,
        inherited: Option<&str>,
    ) -> Vec<(String, Option<String>)> {
        claude_remote_pty_env(
            enabled,
            channel,
            preload.map(Path::new),
            okena_bin.map(Path::new),
            inherited,
        )
    }

    fn set(key: &str, value: &str) -> (String, Option<String>) {
        (key.to_string(), Some(value.to_string()))
    }

    fn removed(key: &str) -> (String, Option<String>) {
        (key.to_string(), None)
    }

    fn removals() -> Vec<(String, Option<String>)> {
        vec![
            removed("BUN_OPTIONS"),
            removed("OKENA_BIN"),
            removed("OKENA_CLAUDE_REMOTE_CHANNEL"),
        ]
    }

    #[test]
    fn enabled_sets_preload_okena_bin_and_channel() {
        assert_eq!(
            env_for(true, true, Some(PRELOAD), Some(OKENA_BIN), None),
            vec![
                set("BUN_OPTIONS", &format!("--preload {PRELOAD}")),
                set("OKENA_BIN", OKENA_BIN),
                set("OKENA_CLAUDE_REMOTE_CHANNEL", "1"),
            ]
        );
    }

    #[test]
    fn channel_off_is_exported_as_zero() {
        let env = env_for(true, false, Some(PRELOAD), Some(OKENA_BIN), None);

        assert_eq!(env[2], set("OKENA_CLAUDE_REMOTE_CHANNEL", "0"));
    }

    #[test]
    fn inherited_bun_options_follow_the_preload() {
        let env = env_for(
            true,
            true,
            Some(PRELOAD),
            Some(OKENA_BIN),
            Some("--smol --preload /x.js"),
        );

        assert_eq!(
            env[0],
            set(
                "BUN_OPTIONS",
                &format!("--preload {PRELOAD} --smol --preload /x.js")
            )
        );
    }

    #[test]
    fn blank_inherited_bun_options_are_dropped() {
        let env = env_for(true, true, Some(PRELOAD), Some(OKENA_BIN), Some("  "));

        assert_eq!(env[0], set("BUN_OPTIONS", &format!("--preload {PRELOAD}")));
    }

    #[test]
    fn disabled_removes_the_variables() {
        assert_eq!(
            env_for(false, true, Some(PRELOAD), Some(OKENA_BIN), None),
            removals()
        );
    }

    #[test]
    fn disabled_restores_inherited_bun_options() {
        assert_eq!(
            env_for(false, true, Some(PRELOAD), Some(OKENA_BIN), Some("--smol")),
            vec![
                set("BUN_OPTIONS", "--smol"),
                removed("OKENA_BIN"),
                removed("OKENA_CLAUDE_REMOTE_CHANNEL"),
            ]
        );
    }

    #[test]
    fn missing_preload_or_okena_bin_removes_the_variables() {
        assert_eq!(env_for(true, true, None, Some(OKENA_BIN), None), removals());
        assert_eq!(env_for(true, true, Some(PRELOAD), None, None), removals());
    }

    #[test]
    fn preload_path_with_whitespace_removes_the_variables() {
        let preload = "/home/a user/okena/claude-remote/preload.js";

        assert_eq!(
            env_for(true, true, Some(preload), Some(OKENA_BIN), None),
            removals()
        );
    }

    #[test]
    fn okena_bin_with_whitespace_is_fine() {
        let okena_bin = "/Applications/Okena App/okena";
        let env = env_for(true, true, Some(PRELOAD), Some(okena_bin), None);

        assert_eq!(env[1], set("OKENA_BIN", okena_bin));
    }
}
