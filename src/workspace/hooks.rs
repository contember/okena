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
    terminals: TerminalsRegistry,
}

impl HookRunner {
    pub fn new(backend: Arc<dyn TerminalBackend>, terminals: TerminalsRegistry) -> Self {
        Self { backend, terminals }
    }
}

impl gpui::Global for HookRunner {}

/// Result of a hook execution via PTY.
#[derive(Clone)]
pub struct HookTerminalResult {
    pub terminal_id: String,
    pub label: String,
    pub hook_type: &'static str,
    pub project_id: String,
    /// The full command with env vars baked in (for rerun).
    pub command: String,
    /// Resolved working directory (for rerun).
    pub cwd: String,
}

impl HookRunner {
    /// Create a PTY-backed terminal for a hook command.
    /// Returns (terminal_id, full_cmd). The terminal is registered in the TerminalsRegistry.
    ///
    /// When `keep_alive` is true, the terminal starts a regular interactive shell and
    /// types the command into it — the shell stays alive after the command finishes.
    /// When false, uses `sh -c` so the PTY exits when the command completes (needed
    /// for sync hooks that block on exit).
    fn create_hook_terminal(
        &self,
        command: &str,
        env_vars: &HashMap<String, String>,
        project_path: &str,
        keep_alive: bool,
    ) -> Result<(String, String), String> {
        // Build the full command with env vars baked in.
        // Filter out any keys that aren't valid shell identifiers to prevent injection.
        let safe_env: Vec<_> = env_vars
            .iter()
            .filter(|(k, _)| {
                if is_valid_env_key(k) {
                    true
                } else {
                    log::warn!("Skipping invalid env var key in hook terminal: {:?}", k);
                    false
                }
            })
            .collect();

        let full_cmd = if cfg!(windows) {
            // Escape all cmd.exe special characters in env var values.
            // ^ must be escaped first since it's the cmd.exe escape character.
            let env_prefix = safe_env
                .iter()
                .map(|(k, v)| {
                    // Escape all cmd.exe special characters.
                    // ^ must be first since it's the escape character itself.
                    let escaped = v
                        .replace('^', "^^")
                        .replace('%', "%%")
                        .replace('"', "\\\"")
                        .replace('&', "^&")
                        .replace('|', "^|")
                        .replace('<', "^<")
                        .replace('>', "^>")
                        .replace('(', "^(")
                        .replace(')', "^)");
                    format!("set \"{}={}\"", k, escaped)
                })
                .collect::<Vec<_>>()
                .join(" && ");
            if env_prefix.is_empty() {
                command.to_string()
            } else {
                format!("{} && {}", env_prefix, command)
            }
        } else {
            // POSIX single-quote escaping: wrap values in '...' and replace each
            // embedded ' with the sequence '\'' (end current single-quoted string,
            // insert an escaped literal quote, re-open single-quoted string).
            // This is the standard POSIX single-quote escape pattern.
            let env_prefix = safe_env
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

        let cwd = if project_path.is_empty() { "." } else { project_path };

        let terminal_id = if keep_alive {
            // Build a shell command that:
            // 1. Exports env vars (available to the hook and the interactive shell)
            // 2. Runs the hook command
            // 3. Execs into the user's default shell so the terminal stays alive
            // This avoids noisy export echoing and zsh session restore issues.
            let mut script = String::new();
            for (k, v) in &safe_env {
                let escaped_v = v.replace('\'', "'\\''");
                script.push_str(&format!("export {}='{}'; ", k, escaped_v));
            }
            script.push_str(command);
            script.push_str("; exec \"${SHELL:-sh}\"");
            let shell = ShellType::for_command(script);
            self.backend.create_terminal(cwd, Some(&shell))
        } else {
            // Use sh -c so the PTY exits when the command completes.
            let shell = ShellType::for_command(full_cmd.clone());
            self.backend.create_terminal(cwd, Some(&shell))
        }.map_err(|e| format!("Failed to create hook terminal: {}", e))?;

        let transport = self.backend.transport();
        let terminal = Arc::new(Terminal::new(
            terminal_id.clone(),
            TerminalSize::default(),
            transport.clone(),
            cwd.to_string(),
        ));
        self.terminals.lock().insert(terminal_id.clone(), terminal);

        Ok((terminal_id, full_cmd))
    }
}

/// Check that an env var key is safe for shell interpolation.
/// Allows `[A-Za-z_][A-Za-z0-9_]*`.
fn is_valid_env_key(key: &str) -> bool {
    let bytes = key.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    if !bytes[0].is_ascii_alphabetic() && bytes[0] != b'_' {
        return false;
    }
    bytes[1..].iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Build a `std::process::Command` for headless hook execution.
/// Handles platform dispatch (sh -c / cmd /C), env vars, and cwd.
fn build_headless_command(command: &str, env_vars: &HashMap<String, String>) -> std::process::Command {
    #[cfg(unix)]
    let mut cmd = crate::process::command("sh");
    #[cfg(unix)]
    cmd.arg("-c").arg(command);

    #[cfg(windows)]
    let mut cmd = crate::process::command("cmd");
    #[cfg(windows)]
    cmd.arg("/C").arg(command);

    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    if let Some(path) = env_vars.get("OKENA_PROJECT_PATH") {
        cmd.current_dir(path);
    }

    cmd
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
    keep_alive: bool,
) -> (Vec<(String, HashMap<String, String>)>, Vec<HookTerminalResult>) {
    let actions = parse_hook_actions(command);
    let mut terminal_actions = Vec::new();
    let mut hook_results = Vec::new();

    for action in actions {
        match action {
            HookAction::Background(cmd) => {
                if let Some(result) = run_hook(cmd, env_vars.clone(), monitor, hook_type, project_name, runner, project_id, keep_alive) {
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

/// Resolve a hook command: project → parent project (if worktree) → global.
fn resolve_hook(
    project_hooks: &HooksConfig,
    global_hooks: &HooksConfig,
    get_field: fn(&HooksConfig) -> &Option<String>,
) -> Option<String> {
    get_field(project_hooks)
        .clone()
        .or_else(|| get_field(global_hooks).clone())
}

/// Resolve a hook command with parent project fallback for worktrees:
/// project → parent project → global.
fn resolve_hook_with_parent(
    project_hooks: &HooksConfig,
    parent_hooks: Option<&HooksConfig>,
    global_hooks: &HooksConfig,
    get_field: fn(&HooksConfig) -> &Option<String>,
) -> Option<String> {
    get_field(project_hooks)
        .clone()
        .or_else(|| parent_hooks.and_then(|h| get_field(h).clone()))
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
///
/// When `keep_alive` is true, the terminal stays interactive after the command finishes.
/// When false, the PTY exits when the command completes (needed for hooks that gate
/// operations like worktree removal).
fn run_hook(
    command: String,
    env_vars: HashMap<String, String>,
    monitor: Option<&HookMonitor>,
    hook_type: &'static str,
    project_name: &str,
    runner: Option<&HookRunner>,
    project_id: &str,
    keep_alive: bool,
) -> Option<HookTerminalResult> {
    // PTY path: create a real terminal so output is visible in the service panel
    if let Some(runner) = runner {
        let project_path = env_vars.get("OKENA_PROJECT_PATH").cloned().unwrap_or_default();
        let label = build_hook_label(hook_type, &env_vars, project_name);
        let resolved_cwd = if project_path.is_empty() { ".".to_string() } else { project_path.clone() };

        match runner.create_hook_terminal(&command, &env_vars, &project_path, keep_alive) {
            Ok((terminal_id, full_cmd)) => {
                // exec_id not needed — PTY hooks are finished via finish_by_terminal_id
                let _ = monitor.map(|m| m.record_start(hook_type, &command, project_name, Some(terminal_id.clone())));
                log::info!("Hook '{}' started in terminal {} (label: {})", hook_type, terminal_id, label);
                return Some(HookTerminalResult {
                    terminal_id,
                    label,
                    hook_type,
                    project_id: project_id.to_string(),
                    command: full_cmd,
                    cwd: resolved_cwd,
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

        let mut cmd = build_headless_command(&command, &env_vars);

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
    // PTY path: requires both runner and monitor (monitor provides the exit waiter channel).
    // If runner exists but monitor is missing, fall through to headless execution.
    if let (Some(runner), Some(monitor)) = (runner, monitor) {
        let project_path = env_vars.get("OKENA_PROJECT_PATH").cloned().unwrap_or_default();
        let label = build_hook_label(hook_type, &env_vars, project_name);
        let resolved_cwd = if project_path.is_empty() { ".".to_string() } else { project_path.clone() };

        let (terminal_id, full_cmd) = runner.create_hook_terminal(command, &env_vars, &project_path, false)?;

        // exec_id not needed — PTY hooks are finished via finish_by_terminal_id
        let _ = monitor.record_start(hook_type, command, project_name, Some(terminal_id.clone()));

        // Register exit waiter and block until the PTY process exits (5 min timeout)
        let rx = monitor.register_exit_waiter(&terminal_id);

        let exit_code = rx.recv_timeout(std::time::Duration::from_secs(300))
            .map_err(|e| match e {
                std::sync::mpsc::RecvTimeoutError::Timeout => {
                    format!("Hook '{}' timed out after 5 minutes — dismiss it from the sidebar to unblock", hook_type)
                }
                std::sync::mpsc::RecvTimeoutError::Disconnected => {
                    "Hook terminal exit channel closed unexpectedly".to_string()
                }
            })?;

        // Do NOT call record_finish here — the main thread's handle_hook_terminal_exits
        // calls finish_by_terminal_id which is the sole authority for PTY hook completion
        // (avoids duplicate toast notifications).
        let success = exit_code == Some(0);

        if success {
            return Ok(Some(HookTerminalResult {
                terminal_id,
                label,
                hook_type,
                project_id: project_id.to_string(),
                command: full_cmd,
                cwd: resolved_cwd,
            }));
        } else {
            let code = exit_code.map(|c| c as i32).unwrap_or(-1);
            return Err(format!("Hook failed (exit {})", code));
        }
    } else if runner.is_some() {
        log::warn!("HookRunner available but no HookMonitor for sync hook '{}'; falling back to headless", hook_type);
    }

    // Fallback: headless execution
    let exec_id = monitor.map(|m| m.record_start(hook_type, command, project_name, None));
    let start = Instant::now();

    let output = build_headless_command(command, &env_vars)
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
    if let Some(cmd) = resolve_hook(project_hooks, &global_hooks, |h| &h.project.on_open) {
        let env = project_env(project_id, project_name, project_path);
        log::info!("Running on_project_open hook for project '{}'", project_name);
        let monitor = try_monitor(cx);
        let runner = try_runner(cx);
        if let Some(result) = run_hook(cmd, env, monitor.as_ref(), "on_project_open", project_name, runner.as_ref(), project_id, true) {
            return vec![result];
        }
    }
    Vec::new()
}

/// Fire the `on_project_close` hook for a project.
/// Runs headlessly (no PTY terminal) since the project is being deleted.
pub fn fire_on_project_close(
    project_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    cx: &App,
) {
    let global_hooks = settings(cx).hooks;
    if let Some(cmd) = resolve_hook(project_hooks, &global_hooks, |h| &h.project.on_close) {
        let env = project_env(project_id, project_name, project_path);
        log::info!("Running on_project_close hook for project '{}'", project_name);
        let monitor = try_monitor(cx);
        run_hook(cmd, env, monitor.as_ref(), "on_project_close", project_name, None, project_id, true);
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
) -> Vec<HookTerminalResult> {
    let global_hooks = settings(cx).hooks;
    if let Some(cmd) = resolve_hook(project_hooks, &global_hooks, |h| &h.worktree.on_create) {
        let mut env = project_env(project_id, project_name, project_path);
        env.insert("OKENA_BRANCH".into(), branch.into());
        log::info!("Running on_worktree_create hook for branch '{}'", branch);
        let monitor = try_monitor(cx);
        let runner = try_runner(cx);
        if let Some(result) = run_hook(cmd, env, monitor.as_ref(), "on_worktree_create", project_name, runner.as_ref(), project_id, true) {
            return vec![result];
        }
    }
    Vec::new()
}

/// Fire the `on_worktree_close` hook after a worktree is successfully removed.
/// Runs headlessly (no PTY terminal) since the worktree project is being deleted.
pub fn fire_on_worktree_close(
    project_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    cx: &App,
) {
    let global_hooks = settings(cx).hooks;
    if let Some(cmd) = resolve_hook(project_hooks, &global_hooks, |h| &h.worktree.on_close) {
        let env = project_env(project_id, project_name, project_path);
        log::info!("Running on_worktree_close hook for project '{}'", project_name);
        let monitor = try_monitor(cx);
        run_hook(cmd, env, monitor.as_ref(), "on_worktree_close", project_name, None, project_id, true);
    }
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
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.worktree.pre_merge) {
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
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.worktree.post_merge) {
        let env = merge_env(project_id, project_name, project_path, branch, target_branch, main_repo_path);
        log::info!("Running post_merge hook for project '{}'", project_name);
        if let Some(result) = run_hook(cmd, env, monitor, "post_merge", project_name, runner, project_id, true) {
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
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.worktree.before_remove) {
        let mut env = project_env(project_id, project_name, project_path);
        env.insert("OKENA_BRANCH".into(), branch.into());
        env.insert("OKENA_MAIN_REPO_PATH".into(), main_repo_path.into());
        log::info!("Running before_worktree_remove hook for project '{}'", project_name);
        return run_hook_sync(&cmd, env, monitor, "before_worktree_remove", project_name, runner, project_id);
    }
    Ok(None)
}

/// Fire the `before_worktree_remove` hook asynchronously (non-blocking).
/// Returns hook terminal results for the caller to register.
/// The caller is responsible for checking the exit code and proceeding with removal.
pub fn fire_before_worktree_remove_async(
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
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.worktree.before_remove) {
        let mut env = project_env(project_id, project_name, project_path);
        env.insert("OKENA_BRANCH".into(), branch.into());
        env.insert("OKENA_MAIN_REPO_PATH".into(), main_repo_path.into());
        log::info!("Running before_worktree_remove hook (async) for project '{}'", project_name);
        if let Some(result) = run_hook(cmd, env, monitor, "before_worktree_remove", project_name, runner, project_id, false) {
            return vec![result];
        }
    }
    Vec::new()
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
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.worktree.on_rebase_conflict) {
        let mut env = merge_env(project_id, project_name, project_path, branch, target_branch, main_repo_path);
        env.insert("OKENA_REBASE_ERROR".into(), rebase_error.into());
        log::info!("Running on_rebase_conflict hook for project '{}'", project_name);
        return run_hook_actions(&cmd, env, monitor, "on_rebase_conflict", project_name, runner, project_id, true);
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
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.worktree.on_dirty_close) {
        let mut env = project_env(project_id, project_name, project_path);
        env.insert("OKENA_BRANCH".into(), branch.into());
        log::info!("Running on_dirty_worktree_close hook for project '{}'", project_name);
        return run_hook_actions(&cmd, env, monitor, "on_dirty_worktree_close", project_name, runner, project_id, true);
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
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.worktree.after_remove) {
        let mut env = project_env(project_id, project_name, project_path);
        env.insert("OKENA_BRANCH".into(), branch.into());
        env.insert("OKENA_MAIN_REPO_PATH".into(), main_repo_path.into());
        log::info!("Running worktree_removed hook for project '{}'", project_name);
        if let Some(result) = run_hook(cmd, env, monitor, "worktree_removed", project_name, runner, project_id, true) {
            return vec![result];
        }
    }
    Vec::new()
}

/// Resolve the `terminal.on_create` hook command.
/// Returns the command string if configured at any level (project/parent/global).
pub fn resolve_terminal_on_create(
    project_hooks: &HooksConfig,
    parent_hooks: Option<&HooksConfig>,
    cx: &App,
) -> Option<String> {
    let global_hooks = settings(cx).hooks;
    resolve_hook_with_parent(project_hooks, parent_hooks, &global_hooks, |h| &h.terminal.on_create)
}

/// Apply the `terminal.on_create` command by wrapping the shell to run
/// the command first, then `exec` into the original shell.
/// Produces: `sh -c '<on_create_cmd>; exec <shell_cmd>'`
pub fn apply_on_create(shell: &ShellType, on_create_cmd: &str) -> ShellType {
    let shell_cmd = shell.to_command_string();
    let script = format!("{}; exec {}", on_create_cmd, shell_cmd);
    ShellType::for_command(script)
}

/// Fire the `terminal.on_close` hook after a terminal PTY exits.
/// Runs headlessly (no PTY runner) since the terminal just exited.
pub fn fire_terminal_on_close(
    project_hooks: &HooksConfig,
    parent_hooks: Option<&HooksConfig>,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    terminal_id: &str,
    exit_code: Option<u32>,
    cx: &App,
) {
    let global_hooks = settings(cx).hooks;
    if let Some(cmd) = resolve_hook_with_parent(project_hooks, parent_hooks, &global_hooks, |h| &h.terminal.on_close) {
        let mut env = project_env(project_id, project_name, project_path);
        env.insert("OKENA_TERMINAL_ID".into(), terminal_id.into());
        if let Some(code) = exit_code {
            env.insert("OKENA_EXIT_CODE".into(), code.to_string());
        }
        log::info!("Running terminal.on_close hook for terminal '{}'", terminal_id);
        let monitor = try_monitor(cx);
        run_hook(cmd, env, monitor.as_ref(), "terminal.on_close", project_name, None, project_id, true);
    }
}

/// Resolve the shell_wrapper for terminal creation.
/// Returns the wrapper command template if configured (project or global level).
pub fn resolve_shell_wrapper(
    project_hooks: &HooksConfig,
    parent_hooks: Option<&HooksConfig>,
    global_hooks: &HooksConfig,
) -> Option<String> {
    resolve_hook_with_parent(project_hooks, parent_hooks, global_hooks, |h| &h.terminal.shell_wrapper)
}

/// Apply shell_wrapper to a ShellType, producing a new ShellType.
/// The wrapper template uses `{shell}` as a placeholder for the resolved shell command.
///
/// The shell is expected to be already resolved (not `ShellType::Default`).
pub fn apply_shell_wrapper(shell: &ShellType, wrapper: &str) -> ShellType {
    let shell_cmd = shell.to_command_string();
    let wrapped = wrapper.replace("{shell}", &shell_cmd);
    ShellType::for_command(wrapped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::settings::WorktreeHooks;

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
            worktree: WorktreeHooks { pre_merge: Some("project-cmd".into()), ..Default::default() },
            ..Default::default()
        };
        let global = HooksConfig {
            worktree: WorktreeHooks { pre_merge: Some("global-cmd".into()), ..Default::default() },
            ..Default::default()
        };
        let resolved = resolve_hook(&project, &global, |h| &h.worktree.pre_merge);
        assert_eq!(resolved, Some("project-cmd".into()));
    }

    #[test]
    fn resolve_hook_falls_back_to_global() {
        let project = HooksConfig::default();
        let global = HooksConfig {
            worktree: WorktreeHooks { pre_merge: Some("global-cmd".into()), ..Default::default() },
            ..Default::default()
        };
        let resolved = resolve_hook(&project, &global, |h| &h.worktree.pre_merge);
        assert_eq!(resolved, Some("global-cmd".into()));
    }

    #[test]
    fn resolve_hook_returns_none_when_both_empty() {
        let project = HooksConfig::default();
        let global = HooksConfig::default();
        let resolved = resolve_hook(&project, &global, |h| &h.worktree.before_remove);
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
        let (terminal_actions, _hook_results) = run_hook_actions("terminal: my-cmd\necho bg", env, None, "test", "proj", None, "proj-id", true);
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

    #[test]
    fn resolve_hook_with_parent_three_tier() {
        use crate::workspace::settings::TerminalHooks;

        let project = HooksConfig::default();
        let parent = HooksConfig {
            terminal: TerminalHooks { on_create: Some("parent-cmd".into()), ..Default::default() },
            ..Default::default()
        };
        let global = HooksConfig {
            terminal: TerminalHooks { on_create: Some("global-cmd".into()), ..Default::default() },
            ..Default::default()
        };

        // Project empty → falls through to parent
        let resolved = resolve_hook_with_parent(&project, Some(&parent), &global, |h| &h.terminal.on_create);
        assert_eq!(resolved, Some("parent-cmd".into()));

        // Project empty, no parent → falls through to global
        let resolved = resolve_hook_with_parent(&project, None, &global, |h| &h.terminal.on_create);
        assert_eq!(resolved, Some("global-cmd".into()));

        // Project set → wins over parent and global
        let project_with_hook = HooksConfig {
            terminal: TerminalHooks { on_create: Some("project-cmd".into()), ..Default::default() },
            ..Default::default()
        };
        let resolved = resolve_hook_with_parent(&project_with_hook, Some(&parent), &global, |h| &h.terminal.on_create);
        assert_eq!(resolved, Some("project-cmd".into()));
    }

    #[test]
    fn valid_env_keys() {
        assert!(is_valid_env_key("OKENA_PROJECT_PATH"));
        assert!(is_valid_env_key("_FOO"));
        assert!(is_valid_env_key("A1"));
        assert!(is_valid_env_key("a"));
    }

    #[test]
    fn invalid_env_keys() {
        assert!(!is_valid_env_key(""));
        assert!(!is_valid_env_key("123ABC"));
        assert!(!is_valid_env_key("FOO BAR"));
        assert!(!is_valid_env_key("FOO;BAR"));
        assert!(!is_valid_env_key("FOO=BAR"));
    }

    #[test]
    fn apply_shell_wrapper_with_default_shell() {
        use super::apply_shell_wrapper;
        let shell = ShellType::Default;
        let wrapper = "docker exec -it mycontainer {shell}";
        let wrapped = apply_shell_wrapper(&shell, wrapper);
        match &wrapped {
            ShellType::Custom { args, .. } => {
                let full_cmd = args.last().expect("args should have command");
                assert!(full_cmd.contains("docker exec -it mycontainer ${SHELL:-sh}"), "got: {}", full_cmd);
            }
            other => panic!("Expected ShellType::Custom, got: {:?}", other),
        }
    }

    #[test]
    fn apply_shell_wrapper_with_custom_shell_and_args() {
        use super::apply_shell_wrapper;
        let shell = ShellType::Custom {
            path: "/bin/zsh".to_string(),
            args: vec!["--login".to_string()],
        };
        let wrapper = "devcontainer exec -- {shell}";
        let wrapped = apply_shell_wrapper(&shell, wrapper);
        match &wrapped {
            ShellType::Custom { args, .. } => {
                let full_cmd = args.last().expect("args should have command");
                assert!(full_cmd.contains("devcontainer exec -- /bin/zsh --login"), "got: {}", full_cmd);
            }
            other => panic!("Expected ShellType::Custom, got: {:?}", other),
        }
    }

    #[test]
    fn shell_to_command_string_custom_no_args() {
        let shell = ShellType::Custom {
            path: "/usr/bin/fish".to_string(),
            args: vec![],
        };
        assert_eq!(shell.to_command_string(), "/usr/bin/fish");
    }
}
