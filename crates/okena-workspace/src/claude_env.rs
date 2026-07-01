//! GPUI-free Claude config-dir resolution + the per-PTY `CLAUDE_CONFIG_DIR`
//! environment override.
//!
//! Both the desktop GUI (`okena-app`) and the headless daemon
//! (`okena-daemon-core`) need to push the right `CLAUDE_CONFIG_DIR` into the PTYs
//! they spawn so the `claude` CLI inside Okena terminals reads the per-profile
//! account. The GUI used to resolve the dir through the gpui extension registry
//! (`okena-ext-claude::resolve_claude_dir`, which reads the `ExtensionSettingsStore`
//! global). That global is just a thin wrapper over the **gpui-free**
//! [`AppSettings::extension_settings`](crate::settings::AppSettings) map — so the
//! same three-tier resolution can be done without gpui here, against the
//! settings the daemon already owns.
//!
//! Keeping this logic in `okena-workspace` (which both callers depend on, and
//! which builds gpui-free with `default-features = false`) lets the daemon set
//! `CLAUDE_CONFIG_DIR` on its own `PtyManager` without pulling gpui in.

use std::path::{Path, PathBuf};

use crate::settings::AppSettings;

/// Expand a leading `~` / `~/` to the user's home directory. Mirrors the
/// expansion the GUI's `okena-ext-claude::usage::expand_tilde` did.
fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    } else if path == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home;
    }
    PathBuf::from(path)
}

/// Return the expanded path only if it exists on disk (the GUI fell back to the
/// next precedence tier when a configured dir was missing). An empty string is
/// treated as unset.
fn existing_path(path: &str, source: &str) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }
    let expanded = expand_tilde(path);
    if expanded.exists() {
        Some(expanded)
    } else {
        log::warn!("[claude-env] {source} '{path}' does not exist, falling back");
        None
    }
}

/// Resolve the Claude config directory using the same three-tier precedence as
/// the GUI's `okena-ext-claude::resolve_claude_dir`, but gpui-free:
/// 1. `extension_settings."claude-code".config_dir` in settings.json
/// 2. `CLAUDE_CONFIG_DIR` environment variable (Claude CLI convention)
/// 3. `$HOME/.claude` (default)
///
/// The only difference from the gpui version is tier 1: instead of reading the
/// `ExtensionSettingsStore` gpui global, it reads the identical
/// [`AppSettings::extension_settings`] map directly.
pub fn resolve_claude_dir(settings: &AppSettings) -> PathBuf {
    if let Some(blob) = settings.extension_settings.get("claude-code")
        && let Some(dir) = blob.get("config_dir").and_then(|v| v.as_str())
        && let Some(expanded) = existing_path(dir, "settings config_dir")
    {
        return expanded;
    }
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR")
        && let Some(expanded) = existing_path(&dir, "CLAUDE_CONFIG_DIR")
    {
        return expanded;
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
}

/// Whether `claude_dir` resolves to the canonical default `$HOME/.claude`.
fn is_default_claude_dir(claude_dir: &Path) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let default_dir = home.join(".claude");
    let canonical_default = default_dir.canonicalize().unwrap_or(default_dir);
    let canonical_dir = claude_dir
        .canonicalize()
        .unwrap_or_else(|_| claude_dir.to_path_buf());
    canonical_dir == canonical_default
}

/// Compute the per-PTY `CLAUDE_CONFIG_DIR` override list to hand to
/// `PtyManager::set_extra_env`. Pure, gpui-free logic.
///
/// * `claude_dir` — the resolved Claude config dir (see [`resolve_claude_dir`]).
/// * `multi_profile` — whether more than one Okena profile exists.
/// * `parent_has_claude_config_dir` — whether the process that launched Okena
///   already had `CLAUDE_CONFIG_DIR` exported.
pub fn claude_pty_extra_env(
    claude_dir: &Path,
    multi_profile: bool,
    parent_has_claude_config_dir: bool,
) -> Vec<(String, Option<String>)> {
    // Default `~/.claude`: actively remove CLAUDE_CONFIG_DIR from the PTY rather
    // than just leaving it unset. This keeps Claude Code on its canonical Keychain
    // service (an explicit CLAUDE_CONFIG_DIR=~/.claude makes it create a suffixed
    // duplicate) *and* prevents a stale value — e.g. one exported in the shell
    // that launched Okena and inherited by our process — from leaking into the
    // terminal and silently pointing `claude` at the wrong account.
    if is_default_claude_dir(claude_dir) {
        return vec![("CLAUDE_CONFIG_DIR".to_string(), None)];
    }

    // Single-profile user who manages CLAUDE_CONFIG_DIR themselves: there's no
    // profile boundary to enforce, so leave their exported value untouched.
    if !multi_profile && parent_has_claude_config_dir {
        return Vec::new();
    }

    vec![(
        "CLAUDE_CONFIG_DIR".to_string(),
        Some(claude_dir.to_string_lossy().into_owned()),
    )]
}

/// Resolve the Claude dir + profile context and compute the `CLAUDE_CONFIG_DIR`
/// PTY override in one gpui-free call. Shared by the GUI's `sync_claude_pty_env`
/// and the daemon's PTY-manager wiring so both apply identical isolation.
pub fn claude_pty_env_for_settings(settings: &AppSettings) -> Vec<(String, Option<String>)> {
    let multi_profile = okena_core::profiles::all_profiles()
        .map(|p| p.len() > 1)
        .unwrap_or(false);
    let claude_dir = resolve_claude_dir(settings);
    claude_pty_extra_env(
        &claude_dir,
        multi_profile,
        std::env::var("CLAUDE_CONFIG_DIR").is_ok(),
    )
}

#[cfg(test)]
mod tests {
    use super::claude_pty_extra_env;

    #[test]
    fn default_claude_dir_unsets_pty_env() {
        let default_dir = dirs::home_dir().unwrap().join(".claude");

        // The default dir must produce an explicit removal so a stale inherited
        // CLAUDE_CONFIG_DIR can't leak in — regardless of profile count or whether
        // the parent process happened to have the var set.
        for &(multi, parent) in &[(false, false), (true, false), (false, true), (true, true)] {
            let env = claude_pty_extra_env(&default_dir, multi, parent);
            assert_eq!(env.len(), 1, "multi={multi} parent={parent}");
            assert_eq!(env[0].0, "CLAUDE_CONFIG_DIR");
            assert_eq!(env[0].1, None, "default dir must unset, not set");
        }
    }

    #[test]
    fn single_profile_keeps_parent_claude_config_dir() {
        let custom_dir = std::env::temp_dir().join("okena-custom-claude-dir");

        assert!(claude_pty_extra_env(&custom_dir, false, true).is_empty());
    }

    #[test]
    fn custom_claude_dir_is_exported_to_pty() {
        let custom_dir = std::env::temp_dir().join("okena-custom-claude-dir");
        let env = claude_pty_extra_env(&custom_dir, true, true);

        assert_eq!(env.len(), 1);
        assert_eq!(env[0].0, "CLAUDE_CONFIG_DIR");
        assert_eq!(env[0].1.as_deref(), Some(custom_dir.to_string_lossy().as_ref()));
    }
}
