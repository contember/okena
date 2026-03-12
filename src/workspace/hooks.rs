use crate::settings::settings;
use crate::terminal::backend::TerminalBackend;
use crate::terminal::shell_config::ShellType;
use crate::terminal::terminal::{Terminal, TerminalSize};
use crate::views::root::TerminalsRegistry;
use crate::workspace::hook_monitor::{HookMonitor, HookStatus};
use crate::workspace::persistence::HooksConfig;
use gpui::App;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

/// Bundles the dependencies needed to run hooks through PTY terminals.
/// Stored as a GPUI Global. All fields are Clone + Send + Sync.
#[derive(Clone)]
pub struct HookRunner {
    pub backend: Arc<dyn TerminalBackend>,
    pub terminals: TerminalsRegistry,
}

impl gpui::Global for HookRunner {}

/// Result of a hook execution via PTY.
#[derive(Clone)]
pub struct HookTerminalResult {
    pub terminal_id: String,
    pub label: String,
    pub hook_type: String,
    pub project_id: String,
}

impl HookRunner {
    /// Create a PTY-backed terminal for a hook command.
    /// Returns the terminal_id. The terminal is registered in the TerminalsRegistry.
    fn create_hook_terminal(
        &self,
        command: &str,
        env_vars: &HashMap<String, String>,
        project_path: &str,
    ) -> Result<String, String> {
        // Build the full command with env vars baked in
        let full_cmd = if cfg!(windows) {
            let env_prefix = env_vars
                .iter()
                .map(|(k, v)| format!("set {}={}", k, v))
                .collect::<Vec<_>>()
                .join(" && ");
            if env_prefix.is_empty() {
                command.to_string()
            } else {
                format!("{} && {}", env_prefix, command)
            }
        } else {
            let env_prefix = env_vars
                .iter()
                .map(|(k, v)| format!("{}='{}'", k, v.replace('\'', "'\\''")))
                .collect::<Vec<_>>()
                .join(" ");
            if env_prefix.is_empty() {
                command.to_string()
            } else {
                format!("{} {}", env_prefix, command)
            }
        };

        let shell = if cfg!(windows) {
            ShellType::Custom {
                path: "cmd".to_string(),
                args: vec!["/C".to_string(), full_cmd],
            }
        } else {
            ShellType::Custom {
                path: "sh".to_string(),
                args: vec!["-c".to_string(), full_cmd],
            }
        };

        let cwd = if project_path.is_empty() { "." } else { project_path };

        let terminal_id = self.backend.create_terminal(cwd, Some(&shell))
            .map_err(|e| format!("Failed to create hook terminal: {}", e))?;

        let terminal = Arc::new(Terminal::new(
            terminal_id.clone(),
            TerminalSize::default(),
            self.backend.transport(),
            cwd.to_string(),
        ));
        self.terminals.lock().insert(terminal_id.clone(), terminal);

        Ok(terminal_id)
    }
}

/// Build a display label for a hook terminal tab.
fn build_hook_label(hook_type: &str, env_vars: &HashMap<String, String>, project_name: &str) -> String {
    let context = env_vars.get("OKENA_BRANCH")
        .map(|s| s.as_str())
        .unwrap_or(project_name);
    format!("{} ({})", hook_type, context)
}

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
/// Returns list of (command, env) pairs for terminal actions (caller handles spawning),
/// and any HookTerminalResult values from PTY-backed background commands.
fn run_hook_actions(
    command: &str,
    env_vars: HashMap<String, String>,
    monitor: Option<&HookMonitor>,
    hook_type: &'static str,
    project_name: &str,
    runner: Option<&HookRunner>,
    project_id: &str,
) -> (Vec<(String, HashMap<String, String>)>, Vec<HookTerminalResult>) {
    let actions = parse_hook_actions(command);
    let mut terminal_actions = Vec::new();
    let mut hook_results = Vec::new();

    for action in actions {
        match action {
            HookAction::Background(cmd) => {
                if let Some(result) = run_hook(cmd, env_vars.clone(), monitor, hook_type, project_name, runner, project_id) {
                    hook_results.push(result);
                }
            }
            HookAction::Terminal(cmd) => {
                terminal_actions.push((cmd, env_vars.clone()));
            }
        }
    }

    (terminal_actions, hook_results)
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

/// Try to get the global HookMonitor from GPUI context.
pub fn try_monitor(cx: &App) -> Option<HookMonitor> {
    cx.try_global::<HookMonitor>().cloned()
}

/// Try to get the global HookRunner from GPUI context.
pub fn try_runner(cx: &App) -> Option<HookRunner> {
    cx.try_global::<HookRunner>().cloned()
}

/// Run a hook command asynchronously in a background thread.
/// When a HookRunner is available, creates a PTY-backed terminal and returns a HookTerminalResult.
/// Otherwise falls back to headless execution via `sh -c` (or `cmd /C` on Windows).
fn run_hook(
    command: String,
    env_vars: HashMap<String, String>,
    monitor: Option<&HookMonitor>,
    hook_type: &'static str,
    project_name: &str,
    runner: Option<&HookRunner>,
    project_id: &str,
) -> Option<HookTerminalResult> {
    // PTY path: create a real terminal so output is visible in the service panel
    if let Some(runner) = runner {
        let project_path = env_vars.get("OKENA_PROJECT_PATH").cloned().unwrap_or_default();
        let label = build_hook_label(hook_type, &env_vars, project_name);

        match runner.create_hook_terminal(&command, &env_vars, &project_path) {
            Ok(terminal_id) => {
                let _exec_id = monitor.map(|m| m.record_start(hook_type, &command, project_name, Some(terminal_id.clone())));
                log::info!("Hook '{}' started in terminal {} (label: {})", hook_type, terminal_id, label);
                return Some(HookTerminalResult {
                    terminal_id,
                    label,
                    hook_type: hook_type.to_string(),
                    project_id: project_id.to_string(),
                });
            }
            Err(e) => {
                log::error!("Failed to create hook terminal for '{}': {}", hook_type, e);
                if let Some(m) = monitor {
                    let id = m.record_start(hook_type, &command, project_name, None);
                    m.record_finish(id, HookStatus::SpawnError { message: e });
                }
                return None;
            }
        }
    }

    // Fallback: headless execution (no HookRunner, e.g. in tests)
    let monitor_clone = monitor.cloned();
    let exec_id = monitor.map(|m| m.record_start(hook_type, &command, project_name, None));

    std::thread::spawn(move || {
        let start = Instant::now();

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

        if let Some(path) = env_vars.get("OKENA_PROJECT_PATH") {
            cmd.current_dir(path);
        }

        match cmd
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output()
        {
            Ok(output) => {
                let duration = start.elapsed();
                if output.status.success() {
                    if let (Some(monitor), Some(id)) = (&monitor_clone, exec_id) {
                        monitor.record_finish(id, HookStatus::Succeeded { duration });
                    }
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                    let exit_code = output.status.code().unwrap_or(-1);
                    log::warn!(
                        "Hook command failed (exit {}): {}",
                        exit_code,
                        stderr,
                    );
                    if let (Some(monitor), Some(id)) = (&monitor_clone, exec_id) {
                        monitor.record_finish(id, HookStatus::Failed {
                            duration,
                            exit_code,
                            stderr,
                        });
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to execute hook command '{}': {}", command, e);
                if let (Some(monitor), Some(id)) = (&monitor_clone, exec_id) {
                    monitor.record_finish(id, HookStatus::SpawnError {
                        message: e.to_string(),
                    });
                }
            }
        }
    });

    None
}

/// Run a hook command synchronously, blocking until completion.
/// When a HookRunner is available, creates a PTY terminal and waits for exit via the monitor's
/// exit waiter channel. Otherwise falls back to headless execution.
/// Returns Ok(Some(result)) on PTY success, Ok(None) on headless success, Err on failure.
fn run_hook_sync(
    command: &str,
    env_vars: HashMap<String, String>,
    monitor: Option<&HookMonitor>,
    hook_type: &'static str,
    project_name: &str,
    runner: Option<&HookRunner>,
    project_id: &str,
) -> Result<Option<HookTerminalResult>, String> {
    // PTY path
    if let Some(runner) = runner {
        let project_path = env_vars.get("OKENA_PROJECT_PATH").cloned().unwrap_or_default();
        let label = build_hook_label(hook_type, &env_vars, project_name);
        let start = Instant::now();

        let terminal_id = runner.create_hook_terminal(command, &env_vars, &project_path)?;

        let exec_id = monitor.map(|m| m.record_start(hook_type, command, project_name, Some(terminal_id.clone())));

        // Register exit waiter and block until the PTY process exits
        let rx = monitor
            .map(|m| m.register_exit_waiter(&terminal_id))
            .ok_or_else(|| "HookMonitor required for sync PTY hooks".to_string())?;

        let exit_code = rx.recv().map_err(|_| "Hook terminal exit channel closed unexpectedly".to_string())?;
        let duration = start.elapsed();

        let success = exit_code == Some(0);

        if success {
            if let (Some(m), Some(id)) = (monitor, exec_id) {
                m.record_finish(id, HookStatus::Succeeded { duration });
            }
            return Ok(Some(HookTerminalResult {
                terminal_id,
                label,
                hook_type: hook_type.to_string(),
                project_id: project_id.to_string(),
            }));
        } else {
            let code = exit_code.map(|c| c as i32).unwrap_or(-1);
            if let (Some(m), Some(id)) = (monitor, exec_id) {
                m.record_finish(id, HookStatus::Failed {
                    duration,
                    exit_code: code,
                    stderr: String::new(),
                });
            }
            return Err(format!("Hook failed (exit {})", code));
        }
    }

    // Fallback: headless execution
    let exec_id = monitor.map(|m| m.record_start(hook_type, command, project_name, None));
    let start = Instant::now();

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

    if let Some(path) = env_vars.get("OKENA_PROJECT_PATH") {
        cmd.current_dir(path);
    }

    let output = cmd
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| {
            let msg = format!("Failed to execute hook '{}': {}", command, e);
            if let (Some(monitor), Some(id)) = (monitor, exec_id) {
                monitor.record_finish(id, HookStatus::SpawnError { message: e.to_string() });
            }
            msg
        })?;

    let duration = start.elapsed();
    if output.status.success() {
        if let (Some(monitor), Some(id)) = (monitor, exec_id) {
            monitor.record_finish(id, HookStatus::Succeeded { duration });
        }
        Ok(None)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let exit_code = output.status.code().unwrap_or(-1);
        if let (Some(monitor), Some(id)) = (monitor, exec_id) {
            monitor.record_finish(id, HookStatus::Failed { duration, exit_code, stderr: stderr.clone() });
        }
        Err(format!(
            "Hook failed (exit {}): {}",
            exit_code,
            stderr,
        ))
    }
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
) -> Vec<HookTerminalResult> {
    let global_hooks = settings(cx).hooks;
    if let Some(cmd) = resolve_hook(project_hooks, &global_hooks, |h| &h.on_project_open) {
        let env = project_env(project_id, project_name, project_path);
        log::info!("Running on_project_open hook for project '{}'", project_name);
        let monitor = try_monitor(cx);
        let runner = try_runner(cx);
        if let Some(result) = run_hook(cmd, env, monitor.as_ref(), "on_project_open", project_name, runner.as_ref(), project_id) {
            return vec![result];
        }
    }
    Vec::new()
}

/// Fire the `on_project_close` hook for a project.
pub fn fire_on_project_close(
    project_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    cx: &App,
) -> Vec<HookTerminalResult> {
    let global_hooks = settings(cx).hooks;
    if let Some(cmd) = resolve_hook(project_hooks, &global_hooks, |h| &h.on_project_close) {
        let env = project_env(project_id, project_name, project_path);
        log::info!("Running on_project_close hook for project '{}'", project_name);
        let monitor = try_monitor(cx);
        let runner = try_runner(cx);
        if let Some(result) = run_hook(cmd, env, monitor.as_ref(), "on_project_close", project_name, runner.as_ref(), project_id) {
            return vec![result];
        }
    }
    Vec::new()
}

/// Fire the `on_worktree_create` hook after a worktree is successfully created.
pub fn fire_on_worktree_create(
    project_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    branch: &str,
    cx: &App,
) -> Vec<HookTerminalResult> {
    let global_hooks = settings(cx).hooks;
    if let Some(cmd) = resolve_hook(project_hooks, &global_hooks, |h| &h.on_worktree_create) {
        let mut env = project_env(project_id, project_name, project_path);
        env.insert("OKENA_BRANCH".into(), branch.into());
        log::info!("Running on_worktree_create hook for branch '{}'", branch);
        let monitor = try_monitor(cx);
        let runner = try_runner(cx);
        if let Some(result) = run_hook(cmd, env, monitor.as_ref(), "on_worktree_create", project_name, runner.as_ref(), project_id) {
            return vec![result];
        }
    }
    Vec::new()
}

/// Fire the `on_worktree_close` hook after a worktree is successfully removed.
pub fn fire_on_worktree_close(
    project_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    cx: &App,
) -> Vec<HookTerminalResult> {
    let global_hooks = settings(cx).hooks;
    if let Some(cmd) = resolve_hook(project_hooks, &global_hooks, |h| &h.on_worktree_close) {
        let env = project_env(project_id, project_name, project_path);
        log::info!("Running on_worktree_close hook for project '{}'", project_name);
        let monitor = try_monitor(cx);
        let runner = try_runner(cx);
        if let Some(result) = run_hook(cmd, env, monitor.as_ref(), "on_worktree_close", project_name, runner.as_ref(), project_id) {
            return vec![result];
        }
    }
    Vec::new()
}

/// Bare sync hook runner for tests (no monitor, no runner).
#[cfg(test)]
fn run_hook_sync_bare(command: &str, env_vars: HashMap<String, String>) -> Result<Option<HookTerminalResult>, String> {
    run_hook_sync(command, env_vars, None, "", "", None, "")
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
    monitor: Option<&HookMonitor>,
    runner: Option<&HookRunner>,
) -> Result<Option<HookTerminalResult>, String> {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.pre_merge) {
        let env = merge_env(project_id, project_name, project_path, branch, target_branch, main_repo_path);
        log::info!("Running pre_merge hook for project '{}'", project_name);
        return run_hook_sync(&cmd, env, monitor, "pre_merge", project_name, runner, project_id);
    }
    Ok(None)
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
    monitor: Option<&HookMonitor>,
    runner: Option<&HookRunner>,
) -> Vec<HookTerminalResult> {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.post_merge) {
        let env = merge_env(project_id, project_name, project_path, branch, target_branch, main_repo_path);
        log::info!("Running post_merge hook for project '{}'", project_name);
        if let Some(result) = run_hook(cmd, env, monitor, "post_merge", project_name, runner, project_id) {
            return vec![result];
        }
    }
    Vec::new()
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
    monitor: Option<&HookMonitor>,
    runner: Option<&HookRunner>,
) -> Result<Option<HookTerminalResult>, String> {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.before_worktree_remove) {
        let mut env = project_env(project_id, project_name, project_path);
        env.insert("OKENA_BRANCH".into(), branch.into());
        env.insert("OKENA_MAIN_REPO_PATH".into(), main_repo_path.into());
        log::info!("Running before_worktree_remove hook for project '{}'", project_name);
        return run_hook_sync(&cmd, env, monitor, "before_worktree_remove", project_name, runner, project_id);
    }
    Ok(None)
}

/// Fire the `on_rebase_conflict` hook.
/// Background actions fire immediately. Returns terminal actions for the caller to spawn,
/// and any HookTerminalResult values from PTY-backed background commands.
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
    monitor: Option<&HookMonitor>,
    runner: Option<&HookRunner>,
) -> (Vec<(String, HashMap<String, String>)>, Vec<HookTerminalResult>) {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.on_rebase_conflict) {
        let mut env = merge_env(project_id, project_name, project_path, branch, target_branch, main_repo_path);
        env.insert("OKENA_REBASE_ERROR".into(), rebase_error.into());
        log::info!("Running on_rebase_conflict hook for project '{}'", project_name);
        return run_hook_actions(&cmd, env, monitor, "on_rebase_conflict", project_name, runner, project_id);
    }
    (Vec::new(), Vec::new())
}

/// Fire the `on_dirty_worktree_close` hook.
/// Background actions fire immediately. Returns terminal actions for the caller to spawn,
/// and any HookTerminalResult values from PTY-backed background commands.
pub fn fire_on_dirty_worktree_close(
    project_hooks: &HooksConfig,
    global_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    branch: &str,
    monitor: Option<&HookMonitor>,
    runner: Option<&HookRunner>,
) -> (Vec<(String, HashMap<String, String>)>, Vec<HookTerminalResult>) {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.on_dirty_worktree_close) {
        let mut env = project_env(project_id, project_name, project_path);
        env.insert("OKENA_BRANCH".into(), branch.into());
        log::info!("Running on_dirty_worktree_close hook for project '{}'", project_name);
        return run_hook_actions(&cmd, env, monitor, "on_dirty_worktree_close", project_name, runner, project_id);
    }
    (Vec::new(), Vec::new())
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
    monitor: Option<&HookMonitor>,
    runner: Option<&HookRunner>,
) -> Vec<HookTerminalResult> {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.worktree_removed) {
        let mut env = project_env(project_id, project_name, project_path);
        env.insert("OKENA_BRANCH".into(), branch.into());
        env.insert("OKENA_MAIN_REPO_PATH".into(), main_repo_path.into());
        log::info!("Running worktree_removed hook for project '{}'", project_name);
        if let Some(result) = run_hook(cmd, env, monitor, "worktree_removed", project_name, runner, project_id) {
            return vec![result];
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_hook_sync_returns_ok_for_true() {
        let result = run_hook_sync_bare("true", HashMap::new());
        assert!(result.is_ok());
    }

    #[test]
    fn run_hook_sync_returns_err_for_false() {
        let result = run_hook_sync_bare("false", HashMap::new());
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
        let (terminal_actions, _hook_results) = run_hook_actions("terminal: my-cmd\necho bg", env, None, "test", "proj", None, "proj-id");
        assert_eq!(terminal_actions.len(), 1);
        assert_eq!(terminal_actions[0].0, "my-cmd");
        assert_eq!(terminal_actions[0].1.get("KEY").unwrap(), "val");
    }

    #[test]
    fn build_hook_label_uses_branch() {
        let mut env = HashMap::new();
        env.insert("OKENA_BRANCH".into(), "feature/foo".into());
        assert_eq!(build_hook_label("on_project_open", &env, "my-project"), "on_project_open (feature/foo)");
    }

    #[test]
    fn build_hook_label_falls_back_to_project_name() {
        let env = HashMap::new();
        assert_eq!(build_hook_label("on_project_open", &env, "my-project"), "on_project_open (my-project)");
    }
}
