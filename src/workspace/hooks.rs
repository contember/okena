use crate::settings::settings;
use crate::workspace::persistence::HooksConfig;
use gpui::App;
use std::collections::HashMap;

/// A single action parsed from a hook command string.
enum HookAction {
    /// Run command in background (existing behavior)
    Background(String),
    /// Spawn a new terminal pane with this command
    Terminal(String),
}

/// Parse a hook command string into a list of actions.
/// Each line is a separate action. Lines starting with "terminal:" spawn a terminal pane.
fn parse_hook_actions(command: &str) -> Vec<HookAction> {
    command
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .map(|line| {
            if let Some(cmd) = line.strip_prefix("terminal:") {
                HookAction::Terminal(cmd.trim().to_string())
            } else {
                HookAction::Background(line.to_string())
            }
        })
        .collect()
}

/// Process hook actions. Background commands fire immediately.
/// Returns list of (command, env) pairs for terminal actions (caller handles spawning).
fn run_hook_actions(
    command: &str,
    env_vars: HashMap<String, String>,
) -> Vec<(String, HashMap<String, String>)> {
    let actions = parse_hook_actions(command);
    let mut terminal_actions = Vec::new();

    for action in actions {
        match action {
            HookAction::Background(cmd) => {
                run_hook(cmd, env_vars.clone());
            }
            HookAction::Terminal(cmd) => {
                terminal_actions.push((cmd, env_vars.clone()));
            }
        }
    }

    terminal_actions
}

/// Resolve a hook command: per-project override takes priority over global default.
fn resolve_hook(
    project_hooks: &HooksConfig,
    global_hooks: &HooksConfig,
    get_field: fn(&HooksConfig) -> &Option<String>,
) -> Option<String> {
    get_field(project_hooks)
        .clone()
        .or_else(|| get_field(global_hooks).clone())
}

/// Run a hook command asynchronously in a background thread.
/// The command is executed via `sh -c` (or `cmd /C` on Windows).
fn run_hook(command: String, env_vars: HashMap<String, String>) {
    std::thread::spawn(move || {
        #[cfg(unix)]
        let mut cmd = crate::process::command("sh");
        #[cfg(unix)]
        cmd.arg("-c").arg(&command);

        #[cfg(windows)]
        let mut cmd = crate::process::command("cmd");
        #[cfg(windows)]
        cmd.arg("/C").arg(&command);

        for (key, value) in &env_vars {
            cmd.env(key, value);
        }

        match cmd
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output()
        {
            Ok(output) => {
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    log::warn!(
                        "Hook command failed (exit {}): {}",
                        output.status.code().unwrap_or(-1),
                        stderr.trim()
                    );
                }
            }
            Err(e) => {
                log::error!("Failed to execute hook command '{}': {}", command, e);
            }
        }
    });
}

/// Build standard environment variables for a project hook.
fn project_env(project_id: &str, project_name: &str, project_path: &str) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("OKENA_PROJECT_ID".into(), project_id.into());
    env.insert("OKENA_PROJECT_NAME".into(), project_name.into());
    env.insert("OKENA_PROJECT_PATH".into(), project_path.into());
    env
}

/// Fire the `on_project_open` hook for a project.
pub fn fire_on_project_open(
    project_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    cx: &App,
) {
    let global_hooks = settings(cx).hooks;
    if let Some(cmd) = resolve_hook(project_hooks, &global_hooks, |h| &h.on_project_open) {
        let env = project_env(project_id, project_name, project_path);
        log::info!("Running on_project_open hook for project '{}'", project_name);
        run_hook(cmd, env);
    }
}

/// Fire the `on_project_close` hook for a project.
pub fn fire_on_project_close(
    project_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    cx: &App,
) {
    let global_hooks = settings(cx).hooks;
    if let Some(cmd) = resolve_hook(project_hooks, &global_hooks, |h| &h.on_project_close) {
        let env = project_env(project_id, project_name, project_path);
        log::info!("Running on_project_close hook for project '{}'", project_name);
        run_hook(cmd, env);
    }
}

/// Fire the `on_worktree_create` hook after a worktree is successfully created.
pub fn fire_on_worktree_create(
    project_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    branch: &str,
    cx: &App,
) {
    let global_hooks = settings(cx).hooks;
    if let Some(cmd) = resolve_hook(project_hooks, &global_hooks, |h| &h.on_worktree_create) {
        let mut env = project_env(project_id, project_name, project_path);
        env.insert("OKENA_BRANCH".into(), branch.into());
        log::info!("Running on_worktree_create hook for branch '{}'", branch);
        run_hook(cmd, env);
    }
}

/// Fire the `on_worktree_close` hook after a worktree is successfully removed.
pub fn fire_on_worktree_close(
    project_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    cx: &App,
) {
    let global_hooks = settings(cx).hooks;
    if let Some(cmd) = resolve_hook(project_hooks, &global_hooks, |h| &h.on_worktree_close) {
        let env = project_env(project_id, project_name, project_path);
        log::info!("Running on_worktree_close hook for project '{}'", project_name);
        run_hook(cmd, env);
    }
}

/// Run a hook command synchronously, returning Ok(()) on success or Err with stderr on failure.
/// Used for hooks where the caller needs to abort on failure (pre_merge, before_worktree_remove).
fn run_hook_sync(command: &str, env_vars: HashMap<String, String>) -> Result<(), String> {
    #[cfg(unix)]
    let mut cmd = crate::process::command("sh");
    #[cfg(unix)]
    cmd.arg("-c").arg(command);

    #[cfg(windows)]
    let mut cmd = crate::process::command("cmd");
    #[cfg(windows)]
    cmd.arg("/C").arg(command);

    for (key, value) in &env_vars {
        cmd.env(key, value);
    }

    let output = cmd
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| format!("Failed to execute hook '{}': {}", command, e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!(
            "Hook failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            stderr,
        ))
    }
}

/// Build extended environment for merge/worktree-remove hooks.
fn merge_env(
    project_id: &str,
    project_name: &str,
    project_path: &str,
    branch: &str,
    target_branch: &str,
    main_repo_path: &str,
) -> HashMap<String, String> {
    let mut env = project_env(project_id, project_name, project_path);
    env.insert("OKENA_BRANCH".into(), branch.into());
    env.insert("OKENA_TARGET_BRANCH".into(), target_branch.into());
    env.insert("OKENA_MAIN_REPO_PATH".into(), main_repo_path.into());
    env
}

/// Fire the `pre_merge` hook synchronously. Returns Err if hook fails (caller should abort).
pub fn fire_pre_merge(
    project_hooks: &HooksConfig,
    global_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    branch: &str,
    target_branch: &str,
    main_repo_path: &str,
) -> Result<(), String> {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.pre_merge) {
        let env = merge_env(project_id, project_name, project_path, branch, target_branch, main_repo_path);
        log::info!("Running pre_merge hook for project '{}'", project_name);
        run_hook_sync(&cmd, env)?;
    }
    Ok(())
}

/// Fire the `post_merge` hook asynchronously.
pub fn fire_post_merge(
    project_hooks: &HooksConfig,
    global_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    branch: &str,
    target_branch: &str,
    main_repo_path: &str,
) {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.post_merge) {
        let env = merge_env(project_id, project_name, project_path, branch, target_branch, main_repo_path);
        log::info!("Running post_merge hook for project '{}'", project_name);
        run_hook(cmd, env);
    }
}

/// Fire the `before_worktree_remove` hook synchronously. Returns Err if hook fails.
pub fn fire_before_worktree_remove(
    project_hooks: &HooksConfig,
    global_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    branch: &str,
    main_repo_path: &str,
) -> Result<(), String> {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.before_worktree_remove) {
        let mut env = project_env(project_id, project_name, project_path);
        env.insert("OKENA_BRANCH".into(), branch.into());
        env.insert("OKENA_MAIN_REPO_PATH".into(), main_repo_path.into());
        log::info!("Running before_worktree_remove hook for project '{}'", project_name);
        run_hook_sync(&cmd, env)?;
    }
    Ok(())
}

/// Fire the `on_rebase_conflict` hook.
/// Background actions fire immediately. Returns terminal actions for the caller to spawn.
pub fn fire_on_rebase_conflict(
    project_hooks: &HooksConfig,
    global_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    branch: &str,
    target_branch: &str,
    main_repo_path: &str,
    rebase_error: &str,
) -> Vec<(String, HashMap<String, String>)> {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.on_rebase_conflict) {
        let mut env = merge_env(project_id, project_name, project_path, branch, target_branch, main_repo_path);
        env.insert("OKENA_REBASE_ERROR".into(), rebase_error.into());
        log::info!("Running on_rebase_conflict hook for project '{}'", project_name);
        return run_hook_actions(&cmd, env);
    }
    Vec::new()
}

/// Fire the `on_dirty_worktree_close` hook.
/// Background actions fire immediately. Returns terminal actions for the caller to spawn.
pub fn fire_on_dirty_worktree_close(
    project_hooks: &HooksConfig,
    global_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    branch: &str,
) -> Vec<(String, HashMap<String, String>)> {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.on_dirty_worktree_close) {
        let mut env = project_env(project_id, project_name, project_path);
        env.insert("OKENA_BRANCH".into(), branch.into());
        log::info!("Running on_dirty_worktree_close hook for project '{}'", project_name);
        return run_hook_actions(&cmd, env);
    }
    Vec::new()
}

/// Fire the `worktree_removed` hook asynchronously.
pub fn fire_worktree_removed(
    project_hooks: &HooksConfig,
    global_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    branch: &str,
    main_repo_path: &str,
) {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.worktree_removed) {
        let mut env = project_env(project_id, project_name, project_path);
        env.insert("OKENA_BRANCH".into(), branch.into());
        env.insert("OKENA_MAIN_REPO_PATH".into(), main_repo_path.into());
        log::info!("Running worktree_removed hook for project '{}'", project_name);
        run_hook(cmd, env);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_hook_sync_returns_ok_for_true() {
        let result = run_hook_sync("true", HashMap::new());
        assert!(result.is_ok());
    }

    #[test]
    fn run_hook_sync_returns_err_for_false() {
        let result = run_hook_sync("false", HashMap::new());
        assert!(result.is_err());
    }

    #[test]
    fn resolve_hook_prefers_project_over_global() {
        let project = HooksConfig {
            pre_merge: Some("project-cmd".into()),
            ..Default::default()
        };
        let global = HooksConfig {
            pre_merge: Some("global-cmd".into()),
            ..Default::default()
        };
        let resolved = resolve_hook(&project, &global, |h| &h.pre_merge);
        assert_eq!(resolved, Some("project-cmd".into()));
    }

    #[test]
    fn resolve_hook_falls_back_to_global() {
        let project = HooksConfig::default();
        let global = HooksConfig {
            pre_merge: Some("global-cmd".into()),
            ..Default::default()
        };
        let resolved = resolve_hook(&project, &global, |h| &h.pre_merge);
        assert_eq!(resolved, Some("global-cmd".into()));
    }

    #[test]
    fn resolve_hook_returns_none_when_both_empty() {
        let project = HooksConfig::default();
        let global = HooksConfig::default();
        let resolved = resolve_hook(&project, &global, |h| &h.before_worktree_remove);
        assert_eq!(resolved, None);
    }

    #[test]
    fn parse_hook_actions_plain_line() {
        let actions = parse_hook_actions("echo hello");
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], HookAction::Background(cmd) if cmd == "echo hello"));
    }

    #[test]
    fn parse_hook_actions_terminal_prefix() {
        let actions = parse_hook_actions("terminal: claude -p \"fix\"");
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], HookAction::Terminal(cmd) if cmd == "claude -p \"fix\""));
    }

    #[test]
    fn parse_hook_actions_mixed_multiline() {
        let actions = parse_hook_actions("terminal: claude -p \"fix\"\necho logged\n\nterminal: htop");
        assert_eq!(actions.len(), 3);
        assert!(matches!(&actions[0], HookAction::Terminal(cmd) if cmd == "claude -p \"fix\""));
        assert!(matches!(&actions[1], HookAction::Background(cmd) if cmd == "echo logged"));
        assert!(matches!(&actions[2], HookAction::Terminal(cmd) if cmd == "htop"));
    }

    #[test]
    fn parse_hook_actions_trims_whitespace() {
        let actions = parse_hook_actions("  terminal:  spaced  \n  bg cmd  ");
        assert_eq!(actions.len(), 2);
        assert!(matches!(&actions[0], HookAction::Terminal(cmd) if cmd == "spaced"));
        assert!(matches!(&actions[1], HookAction::Background(cmd) if cmd == "bg cmd"));
    }

    #[test]
    fn parse_hook_actions_empty_string() {
        let actions = parse_hook_actions("");
        assert!(actions.is_empty());
    }

    #[test]
    fn run_hook_actions_returns_terminal_actions() {
        let mut env = HashMap::new();
        env.insert("KEY".into(), "val".into());
        let terminal_actions = run_hook_actions("terminal: my-cmd\necho bg", env);
        assert_eq!(terminal_actions.len(), 1);
        assert_eq!(terminal_actions[0].0, "my-cmd");
        assert_eq!(terminal_actions[0].1.get("KEY").unwrap(), "val");
    }
}
