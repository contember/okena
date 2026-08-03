// The hook-firing functions thread project metadata, env vars, the monitor,
// the runner and hook config through a family of related signatures; grouping
// them into a context struct would obscure more than it clarifies here.
#![allow(clippy::too_many_arguments)]

use crate::hook_monitor::{HookMonitor, HookStatus};
#[cfg(feature = "gpui")]
use gpui::App;
use okena_state::HooksConfig;
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::{TerminalBackend, TerminalLaunchCommand, TerminalLaunchPlan};
use okena_terminal::shell_config::ShellType;
use okena_terminal::terminal::{Terminal, TerminalSize};
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

#[cfg(feature = "gpui")]
impl gpui::Global for HookRunner {}

/// Pending terminal-backed hook actions paired with their env vars, returned
/// alongside the `HookTerminalResult`s produced by background PTY commands.
pub type HookActionOutcome = (
    Vec<(String, HashMap<String, String>)>,
    Vec<HookTerminalResult>,
);

/// Hook actions resolved off-reactor and deferred until their owner can
/// register any spawned PTYs before yielding back to the event loop.
#[derive(Clone)]
pub struct HookActionPlan {
    command: String,
    env_vars: HashMap<String, String>,
    hook_type: &'static str,
    project_name: String,
    project_id: String,
    keep_alive: bool,
}

/// Execute a previously resolved hook plan with the caller's PTY services.
pub fn execute_hook_action_plan(
    plan: HookActionPlan,
    monitor: Option<&HookMonitor>,
    runner: Option<&HookRunner>,
) -> HookActionOutcome {
    run_hook_actions(
        &plan.command,
        plan.env_vars,
        monitor,
        plan.hook_type,
        &plan.project_name,
        runner,
        &plan.project_id,
        plan.keep_alive,
    )
}

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

/// Immutable terminal-backed hook launch with caller-reserved ownership.
#[derive(Clone)]
pub struct PreparedHookTerminal {
    result: HookTerminalResult,
    launch_plan: TerminalLaunchPlan,
    monitor_command: String,
    project_name: String,
}

impl PreparedHookTerminal {
    pub fn result(&self) -> &HookTerminalResult {
        &self.result
    }

    /// Publish monitor ownership before a fast PTY can emit its exit event.
    pub fn publish_monitor(&self, monitor: Option<&HookMonitor>) {
        if let Some(monitor) = monitor {
            let _ = monitor.record_start(
                self.result.hook_type,
                &self.monitor_command,
                &self.project_name,
                Some(self.result.terminal_id.clone()),
            );
        }
    }

    pub fn finish_failed_monitor(&self, monitor: Option<&HookMonitor>) {
        if let Some(monitor) = monitor {
            monitor.finish_by_terminal_id(&self.result.terminal_id, None);
        }
    }
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
        // Store an independently rerunnable command, including persistent env setup.
        let full_cmd = rerunnable_hook_command(command, env_vars);

        let cwd = if project_path.is_empty() {
            "."
        } else {
            project_path
        };

        let shell = if keep_alive {
            keep_alive_hook_shell(&full_cmd)
        } else {
            // Use sh -c so the PTY exits when the command completes.
            ShellType::for_command(full_cmd.clone())
        };
        let plan =
            TerminalLaunchPlan::for_shell(shell).with_environment(safe_hook_environment(env_vars));
        let terminal_id = self
            .backend
            .create_terminal_with_plan(cwd, &plan)
            .map_err(|e| format!("Failed to create hook terminal: {}", e))?;

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

    /// Publish the UI terminal before launching a prepared hook PTY.
    pub fn publish_prepared_terminal(&self, prepared: &PreparedHookTerminal) {
        let terminal = Arc::new(Terminal::new(
            prepared.result.terminal_id.clone(),
            TerminalSize::default(),
            self.backend.transport(),
            prepared.result.cwd.clone(),
        ));
        self.terminals
            .lock()
            .insert(prepared.result.terminal_id.clone(), terminal);
    }

    /// Launch a prepared hook using its already-published logical id.
    pub fn launch_prepared_terminal(&self, prepared: &PreparedHookTerminal) -> Result<(), String> {
        let terminal_id = self
            .backend
            .reconnect_terminal_with_plan(
                &prepared.result.terminal_id,
                &prepared.result.cwd,
                &prepared.launch_plan,
            )
            .map_err(|error| format!("Failed to create hook terminal: {error}"))?;
        if terminal_id != prepared.result.terminal_id {
            self.backend.kill(&terminal_id);
            return Err(format!(
                "hook backend returned unexpected terminal id {terminal_id}"
            ));
        }
        Ok(())
    }

    pub fn remove_prepared_terminal(&self, prepared: &PreparedHookTerminal) {
        self.terminals.lock().remove(&prepared.result.terminal_id);
    }
}

/// Resolve one project-open hook without launching its PTY.
pub fn prepare_project_open_hook(
    terminal_id: String,
    project_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    folder_id: Option<&str>,
    folder_name: Option<&str>,
    global_hooks: &HooksConfig,
) -> Option<PreparedHookTerminal> {
    let command = resolve_hook(project_hooks, global_hooks, |hooks| &hooks.project.on_open)?;
    let env_vars = project_env(
        project_id,
        project_name,
        project_path,
        folder_id,
        folder_name,
    );
    let full_command = rerunnable_hook_command(&command, &env_vars);
    let cwd = if project_path.is_empty() {
        ".".to_string()
    } else {
        project_path.to_string()
    };
    let launch_plan = TerminalLaunchPlan::for_shell(keep_alive_hook_shell(&full_command))
        .with_environment(safe_hook_environment(&env_vars));
    Some(PreparedHookTerminal {
        result: HookTerminalResult {
            terminal_id,
            label: build_hook_label("on_project_open", &env_vars, project_name),
            hook_type: "on_project_open",
            project_id: project_id.to_string(),
            command: full_command,
            cwd,
        },
        launch_plan,
        monitor_command: command,
        project_name: project_name.to_string(),
    })
}

/// Wrap a rerunnable hook command so it reports completion and stays interactive.
pub fn keep_alive_hook_shell(command: &str) -> ShellType {
    build_keep_alive_hook_shell(command, cfg!(windows))
}

fn build_keep_alive_hook_shell(command: &str, windows: bool) -> ShellType {
    if windows {
        let script = format!(
            "{} & set \"__okena_rc=!ERRORLEVEL!\" & <nul set /p \"=\x1b]0;__okena_hook_exit:!__okena_rc!\x07\" & cmd /K",
            command
        );
        ShellType::Custom {
            path: "cmd".to_string(),
            args: vec!["/V:ON".to_string(), "/C".to_string(), script],
        }
    } else {
        ShellType::for_command(format!(
            "{}; __okena_rc=$?; printf '\\033]0;__okena_hook_exit:%d\\007' \"$__okena_rc\"; exec \"${{SHELL:-sh}}\"",
            command
        ))
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
    bytes[1..]
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Build POSIX shell export statements from a HashMap of env vars.
#[cfg(not(windows))]
fn build_export_prefix(env_vars: &HashMap<String, String>) -> String {
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

    if safe_env.is_empty() {
        return String::new();
    }

    if cfg!(windows) {
        let parts: Vec<_> = safe_env
            .iter()
            .map(|(k, v)| {
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
            .collect();
        format!("{} && ", parts.join(" && "))
    } else {
        let parts: Vec<_> = safe_env
            .iter()
            .map(|(k, v)| format!("export {}='{}'; ", k, v.replace('\'', "'\\''")))
            .collect();
        parts.join("")
    }
}

#[cfg(windows)]
fn build_export_prefix(_env_vars: &HashMap<String, String>) -> String {
    // Windows callers that need environment propagation must use TerminalLaunchPlan.
    String::new()
}

#[cfg(windows)]
fn windows_encoded_hook_command(command: &str, env_vars: &HashMap<String, String>) -> String {
    use base64::Engine;

    let mut script = String::new();
    for (key, value) in safe_hook_environment(env_vars) {
        let value = base64::engine::general_purpose::STANDARD.encode(value.as_bytes());
        script.push_str(&format!(
            "$env:{key}=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{value}'));"
        ));
    }
    let command = base64::engine::general_purpose::STANDARD.encode(command.as_bytes());
    script.push_str(&format!(
        "$okenaCommand=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{command}'));"
    ));
    script.push_str("& $env:ComSpec /D /S /C $okenaCommand;exit $LASTEXITCODE");

    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
    format!("powershell.exe -NoLogo -NoProfile -EncodedCommand {encoded}")
}

fn safe_hook_environment(env_vars: &HashMap<String, String>) -> Vec<(String, String)> {
    let mut environment: Vec<_> = env_vars
        .iter()
        .filter(|(key, _)| is_valid_env_key(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    environment.sort_by(|a, b| a.0.cmp(&b.0));
    environment
}

fn rerunnable_hook_command(command: &str, env_vars: &HashMap<String, String>) -> String {
    #[cfg(windows)]
    {
        return windows_encoded_hook_command(command, env_vars);
    }
    #[cfg(not(windows))]
    {
        format!("{}{}", build_export_prefix(env_vars), command)
    }
}

/// Build environment variables for terminal hooks.
/// Includes base project vars and, for worktree projects, OKENA_BRANCH.
pub fn terminal_hook_env(
    project_id: &str,
    project_name: &str,
    project_path: &str,
    is_worktree: bool,
    folder_id: Option<&str>,
    folder_name: Option<&str>,
) -> HashMap<String, String> {
    let mut env = project_env(
        project_id,
        project_name,
        project_path,
        folder_id,
        folder_name,
    );
    if is_worktree {
        let path = std::path::Path::new(project_path);
        let branch = okena_git::get_git_status(path)
            .and_then(|s| s.branch)
            .or_else(|| okena_git::get_current_branch(path));
        if let Some(branch) = branch {
            env.insert("OKENA_BRANCH".into(), branch);
        }
    }
    env
}

/// Build a `std::process::Command` for headless hook execution.
/// Handles platform dispatch (sh -c / cmd /C), env vars, and cwd.
fn build_headless_command(
    command: &str,
    env_vars: &HashMap<String, String>,
) -> std::process::Command {
    #[cfg(unix)]
    let mut cmd = okena_core::process::command("sh");
    #[cfg(unix)]
    cmd.arg("-c").arg(command);

    #[cfg(windows)]
    let mut cmd = okena_core::process::command("cmd");
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
fn build_hook_label(
    hook_type: &str,
    env_vars: &HashMap<String, String>,
    project_name: &str,
) -> String {
    let context = env_vars
        .get("OKENA_BRANCH")
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
) -> HookActionOutcome {
    let actions = parse_hook_actions(command);
    let mut terminal_actions = Vec::new();
    let mut hook_results = Vec::new();

    for action in actions {
        match action {
            HookAction::Background(cmd) => {
                if let Some(result) = run_hook(
                    cmd,
                    env_vars.clone(),
                    monitor,
                    hook_type,
                    project_name,
                    runner,
                    project_id,
                    keep_alive,
                ) {
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
#[cfg(feature = "gpui")]
pub fn try_monitor(cx: &App) -> Option<HookMonitor> {
    cx.try_global::<HookMonitor>().cloned()
}

/// Try to get the global HookRunner from GPUI context.
#[cfg(feature = "gpui")]
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
        let project_path = env_vars
            .get("OKENA_PROJECT_PATH")
            .cloned()
            .unwrap_or_default();
        let label = build_hook_label(hook_type, &env_vars, project_name);
        let resolved_cwd = if project_path.is_empty() {
            ".".to_string()
        } else {
            project_path.clone()
        };

        match runner.create_hook_terminal(&command, &env_vars, &project_path, keep_alive) {
            Ok((terminal_id, full_cmd)) => {
                // exec_id not needed — PTY hooks are finished via finish_by_terminal_id
                let _ = monitor.map(|m| {
                    m.record_start(hook_type, &command, project_name, Some(terminal_id.clone()))
                });
                log::info!(
                    "Hook '{}' started in terminal {} (label: {})",
                    hook_type,
                    terminal_id,
                    label
                );
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

        let cmd = build_headless_command(&command, &env_vars);
        // Long lane: a hook can run for minutes, so it must not contend for the
        // bus permits the git/services pollers need.
        let spec = okena_core::process::CommandSpec::from_command(&cmd)
            .lane(okena_core::process::Lane::Long)
            .label("hook")
            .timeout(std::time::Duration::from_secs(300));

        match okena_core::process::run(spec) {
            Ok(output) => {
                let duration = start.elapsed();
                if output.status.success() {
                    if let (Some(monitor), Some(id)) = (&monitor_clone, exec_id) {
                        monitor.record_finish(id, HookStatus::Succeeded { duration });
                    }
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                    let exit_code = output.status.code().unwrap_or(-1);
                    log::warn!("Hook command failed (exit {}): {}", exit_code, stderr,);
                    if let (Some(monitor), Some(id)) = (&monitor_clone, exec_id) {
                        monitor.record_finish(
                            id,
                            HookStatus::Failed {
                                duration,
                                exit_code,
                                stderr,
                            },
                        );
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to execute hook command '{}': {}", command, e);
                if let (Some(monitor), Some(id)) = (&monitor_clone, exec_id) {
                    monitor.record_finish(
                        id,
                        HookStatus::SpawnError {
                            message: e.to_string(),
                        },
                    );
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
        let project_path = env_vars
            .get("OKENA_PROJECT_PATH")
            .cloned()
            .unwrap_or_default();
        let label = build_hook_label(hook_type, &env_vars, project_name);
        let resolved_cwd = if project_path.is_empty() {
            ".".to_string()
        } else {
            project_path.clone()
        };

        let exit_reservation = monitor.reserve_exit_waiter();
        let (terminal_id, full_cmd) =
            runner.create_hook_terminal(command, &env_vars, &project_path, false)?;

        // exec_id not needed — PTY hooks are finished via finish_by_terminal_id
        let _ = monitor.record_start(hook_type, command, project_name, Some(terminal_id.clone()));

        // Register exit waiter and block until the PTY process exits (5 min timeout)
        let rx = exit_reservation.bind(&terminal_id);

        let exit_code = match rx.recv_timeout(std::time::Duration::from_secs(300)) {
            Ok(exit_code) => exit_code,
            Err(error) => {
                monitor.cancel_exit_waiter(&terminal_id);
                runner.backend.kill(&terminal_id);
                runner.terminals.lock().remove(&terminal_id);
                monitor.finish_by_terminal_id(&terminal_id, None);
                return Err(match error {
                    std::sync::mpsc::RecvTimeoutError::Timeout => {
                        format!("Hook '{}' timed out after 5 minutes", hook_type)
                    }
                    std::sync::mpsc::RecvTimeoutError::Disconnected => {
                        "Hook terminal exit channel closed unexpectedly".to_string()
                    }
                });
            }
        };

        // The PTY loop normally finishes this first. The idempotent fallback covers
        // a very fast exit that arrived before the execution and waiter were registered.
        monitor.finish_by_terminal_id(&terminal_id, exit_code);
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
            runner.backend.kill(&terminal_id);
            runner.terminals.lock().remove(&terminal_id);
            return Err(format!("Hook failed (exit {})", code));
        }
    } else if runner.is_some() {
        log::warn!(
            "HookRunner available but no HookMonitor for sync hook '{}'; falling back to headless",
            hook_type
        );
    }

    // Fallback: headless execution
    let exec_id = monitor.map(|m| m.record_start(hook_type, command, project_name, None));
    let start = Instant::now();

    let cmd = build_headless_command(command, &env_vars);
    let spec = okena_core::process::CommandSpec::from_command(&cmd)
        .lane(okena_core::process::Lane::Long)
        .label("hook")
        .timeout(std::time::Duration::from_secs(300));
    let output = okena_core::process::run(spec).map_err(|e| {
        let msg = format!("Failed to execute hook '{}': {}", command, e);
        if let (Some(monitor), Some(id)) = (monitor, exec_id) {
            monitor.record_finish(
                id,
                HookStatus::SpawnError {
                    message: e.to_string(),
                },
            );
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
            monitor.record_finish(
                id,
                HookStatus::Failed {
                    duration,
                    exit_code,
                    stderr: stderr.clone(),
                },
            );
        }
        Err(format!("Hook failed (exit {}): {}", exit_code, stderr,))
    }
}

/// Build standard environment variables for a project hook.
fn project_env(
    project_id: &str,
    project_name: &str,
    project_path: &str,
    folder_id: Option<&str>,
    folder_name: Option<&str>,
) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("OKENA_PROJECT_ID".into(), project_id.into());
    env.insert("OKENA_PROJECT_NAME".into(), project_name.into());
    env.insert("OKENA_PROJECT_PATH".into(), project_path.into());
    if let Some(id) = folder_id {
        env.insert("OKENA_FOLDER_ID".into(), id.into());
    }
    if let Some(name) = folder_name {
        env.insert("OKENA_FOLDER_NAME".into(), name.into());
    }
    env
}

/// Fire the `on_project_open` hook for a project.
///
/// GPUI-free: takes the `HookRunner`/`HookMonitor` services explicitly so the
/// daemon reactor can drive it without an `&App`. Callers in scope of GPUI pass
/// `try_runner(cx)`/`try_monitor(cx)`.
pub fn fire_on_project_open(
    project_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    folder_id: Option<&str>,
    folder_name: Option<&str>,
    global_hooks: &HooksConfig,
    runner: Option<&HookRunner>,
    monitor: Option<&HookMonitor>,
) -> Vec<HookTerminalResult> {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.project.on_open) {
        let env = project_env(
            project_id,
            project_name,
            project_path,
            folder_id,
            folder_name,
        );
        log::info!(
            "Running on_project_open hook for project '{}'",
            project_name
        );
        if let Some(result) = run_hook(
            cmd,
            env,
            monitor,
            "on_project_open",
            project_name,
            runner,
            project_id,
            true,
        ) {
            return vec![result];
        }
    }
    Vec::new()
}

/// Fire the `on_project_close` hook for a project.
/// Runs headlessly (no PTY terminal) since the project is being deleted.
///
/// GPUI-free: takes the `HookMonitor` service explicitly (no runner — close
/// hooks never spawn a PTY terminal).
pub fn fire_on_project_close(
    project_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    folder_id: Option<&str>,
    folder_name: Option<&str>,
    global_hooks: &HooksConfig,
    monitor: Option<&HookMonitor>,
) {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.project.on_close) {
        let env = project_env(
            project_id,
            project_name,
            project_path,
            folder_id,
            folder_name,
        );
        log::info!(
            "Running on_project_close hook for project '{}'",
            project_name
        );
        run_hook(
            cmd,
            env,
            monitor,
            "on_project_close",
            project_name,
            None,
            project_id,
            true,
        );
    }
}

/// Run `on_project_close` to completion without creating a project-owned PTY.
pub fn fire_on_project_close_headless_sync(
    project_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    folder_id: Option<&str>,
    folder_name: Option<&str>,
    global_hooks: &HooksConfig,
    monitor: Option<&HookMonitor>,
) -> Result<(), String> {
    let Some(command) = resolve_hook(project_hooks, global_hooks, |h| &h.project.on_close) else {
        return Ok(());
    };
    let env = project_env(
        project_id,
        project_name,
        project_path,
        folder_id,
        folder_name,
    );
    log::info!(
        "Running on_project_close hook for project '{}'",
        project_name
    );
    run_hook_sync(
        &command,
        env,
        monitor,
        "on_project_close",
        project_name,
        None,
        project_id,
    )?;
    Ok(())
}

/// Fire the `on_worktree_create` hook after a worktree is successfully created.
///
/// GPUI-free: takes the `HookRunner`/`HookMonitor` services explicitly.
pub fn fire_on_worktree_create(
    project_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    branch: &str,
    folder_id: Option<&str>,
    folder_name: Option<&str>,
    global_hooks: &HooksConfig,
    runner: Option<&HookRunner>,
    monitor: Option<&HookMonitor>,
) -> Vec<HookTerminalResult> {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.worktree.on_create) {
        let mut env = project_env(
            project_id,
            project_name,
            project_path,
            folder_id,
            folder_name,
        );
        env.insert("OKENA_BRANCH".into(), branch.into());
        log::info!("Running on_worktree_create hook for branch '{}'", branch);
        if let Some(result) = run_hook(
            cmd,
            env,
            monitor,
            "on_worktree_create",
            project_name,
            runner,
            project_id,
            true,
        ) {
            return vec![result];
        }
    }
    Vec::new()
}

/// Fire the `on_worktree_close` hook after a worktree is successfully removed.
/// Runs headlessly (no PTY terminal) since the worktree project is being deleted.
///
/// GPUI-free: takes the `HookMonitor` service explicitly (no runner — close
/// hooks never spawn a PTY terminal).
pub fn fire_on_worktree_close_with_services(
    project_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    branch: &str,
    folder_id: Option<&str>,
    folder_name: Option<&str>,
    global_hooks: &HooksConfig,
    monitor: Option<&HookMonitor>,
) {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.worktree.on_close) {
        let mut env = project_env(
            project_id,
            project_name,
            project_path,
            folder_id,
            folder_name,
        );
        env.insert("OKENA_BRANCH".into(), branch.into());
        log::info!(
            "Running on_worktree_close hook for project '{}' (branch: {})",
            project_name,
            branch
        );
        run_hook(
            cmd,
            env,
            monitor,
            "on_worktree_close",
            project_name,
            None,
            project_id,
            true,
        );
    }
}

/// Run `on_worktree_close` to completion while its checkout still exists.
#[allow(clippy::too_many_arguments)]
pub fn fire_on_worktree_close_headless_sync(
    project_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    branch: &str,
    folder_id: Option<&str>,
    folder_name: Option<&str>,
    global_hooks: &HooksConfig,
    monitor: Option<&HookMonitor>,
) -> Result<(), String> {
    let Some(command) = resolve_hook(project_hooks, global_hooks, |h| &h.worktree.on_close) else {
        return Ok(());
    };
    let mut env = project_env(
        project_id,
        project_name,
        project_path,
        folder_id,
        folder_name,
    );
    env.insert("OKENA_BRANCH".into(), branch.into());
    log::info!(
        "Running on_worktree_close hook for project '{}' (branch: {})",
        project_name,
        branch
    );
    run_hook_sync(
        &command,
        env,
        monitor,
        "on_worktree_close",
        project_name,
        None,
        project_id,
    )?;
    Ok(())
}

/// GPUI wrapper around [`fire_on_worktree_close_with_services`]: reads the
/// `HookMonitor` global from `&App` and delegates. Kept so existing `&App`
/// callers (e.g. okena-app's pending-worktree-close path) compile unchanged.
#[cfg(feature = "gpui")]
pub fn fire_on_worktree_close(
    project_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    branch: &str,
    folder_id: Option<&str>,
    folder_name: Option<&str>,
    global_hooks: &HooksConfig,
    cx: &App,
) {
    let monitor = try_monitor(cx);
    fire_on_worktree_close_with_services(
        project_hooks,
        project_id,
        project_name,
        project_path,
        branch,
        folder_id,
        folder_name,
        global_hooks,
        monitor.as_ref(),
    );
}

/// Bare sync hook runner for tests (no monitor, no runner).
#[cfg(test)]
fn run_hook_sync_bare(
    command: &str,
    env_vars: HashMap<String, String>,
) -> Result<Option<HookTerminalResult>, String> {
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
    folder_id: Option<&str>,
    folder_name: Option<&str>,
) -> HashMap<String, String> {
    let mut env = project_env(
        project_id,
        project_name,
        project_path,
        folder_id,
        folder_name,
    );
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
    folder_id: Option<&str>,
    folder_name: Option<&str>,
    monitor: Option<&HookMonitor>,
    runner: Option<&HookRunner>,
) -> Result<Option<HookTerminalResult>, String> {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.worktree.pre_merge) {
        let env = merge_env(
            project_id,
            project_name,
            project_path,
            branch,
            target_branch,
            main_repo_path,
            folder_id,
            folder_name,
        );
        log::info!("Running pre_merge hook for project '{}'", project_name);
        return run_hook_sync(
            &cmd,
            env,
            monitor,
            "pre_merge",
            project_name,
            runner,
            project_id,
        );
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
    folder_id: Option<&str>,
    folder_name: Option<&str>,
    monitor: Option<&HookMonitor>,
    runner: Option<&HookRunner>,
) -> Vec<HookTerminalResult> {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.worktree.post_merge) {
        let env = merge_env(
            project_id,
            project_name,
            project_path,
            branch,
            target_branch,
            main_repo_path,
            folder_id,
            folder_name,
        );
        log::info!("Running post_merge hook for project '{}'", project_name);
        if let Some(result) = run_hook(
            cmd,
            env,
            monitor,
            "post_merge",
            project_name,
            runner,
            project_id,
            true,
        ) {
            return vec![result];
        }
    }
    Vec::new()
}

/// Run `post_merge` synchronously without creating a PTY or detached thread.
pub fn fire_post_merge_headless_sync(
    project_hooks: &HooksConfig,
    global_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    branch: &str,
    target_branch: &str,
    main_repo_path: &str,
    folder_id: Option<&str>,
    folder_name: Option<&str>,
    monitor: Option<&HookMonitor>,
) -> Result<(), String> {
    let Some(command) = resolve_hook(project_hooks, global_hooks, |h| &h.worktree.post_merge)
    else {
        return Ok(());
    };
    let env = merge_env(
        project_id,
        project_name,
        project_path,
        branch,
        target_branch,
        main_repo_path,
        folder_id,
        folder_name,
    );
    log::info!("Running post_merge hook synchronously for project '{project_name}'");
    run_hook_sync(
        &command,
        env,
        monitor,
        "post_merge",
        project_name,
        None,
        project_id,
    )?;
    Ok(())
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
    folder_id: Option<&str>,
    folder_name: Option<&str>,
    monitor: Option<&HookMonitor>,
    runner: Option<&HookRunner>,
) -> Result<Option<HookTerminalResult>, String> {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.worktree.before_remove) {
        let mut env = project_env(
            project_id,
            project_name,
            project_path,
            folder_id,
            folder_name,
        );
        env.insert("OKENA_BRANCH".into(), branch.into());
        env.insert("OKENA_MAIN_REPO_PATH".into(), main_repo_path.into());
        log::info!(
            "Running before_worktree_remove hook for project '{}'",
            project_name
        );
        return run_hook_sync(
            &cmd,
            env,
            monitor,
            "before_worktree_remove",
            project_name,
            runner,
            project_id,
        );
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
    folder_id: Option<&str>,
    folder_name: Option<&str>,
    monitor: Option<&HookMonitor>,
    runner: Option<&HookRunner>,
) -> Vec<HookTerminalResult> {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.worktree.before_remove) {
        let mut env = project_env(
            project_id,
            project_name,
            project_path,
            folder_id,
            folder_name,
        );
        env.insert("OKENA_BRANCH".into(), branch.into());
        env.insert("OKENA_MAIN_REPO_PATH".into(), main_repo_path.into());
        log::info!(
            "Running before_worktree_remove hook (async) for project '{}'",
            project_name
        );
        if let Some(result) = run_hook(
            cmd,
            env,
            monitor,
            "before_worktree_remove",
            project_name,
            runner,
            project_id,
            false,
        ) {
            return vec![result];
        }
    }
    Vec::new()
}

/// Resolve `on_rebase_conflict` without spawning any PTYs.
#[allow(clippy::too_many_arguments)]
pub fn plan_on_rebase_conflict(
    project_hooks: &HooksConfig,
    global_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    branch: &str,
    target_branch: &str,
    main_repo_path: &str,
    rebase_error: &str,
    folder_id: Option<&str>,
    folder_name: Option<&str>,
) -> Option<HookActionPlan> {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| {
        &h.worktree.on_rebase_conflict
    }) {
        let mut env = merge_env(
            project_id,
            project_name,
            project_path,
            branch,
            target_branch,
            main_repo_path,
            folder_id,
            folder_name,
        );
        env.insert("OKENA_REBASE_ERROR".into(), rebase_error.into());
        log::info!(
            "Running on_rebase_conflict hook for project '{}'",
            project_name
        );
        return Some(HookActionPlan {
            command: cmd,
            env_vars: env,
            hook_type: "on_rebase_conflict",
            project_name: project_name.to_string(),
            project_id: project_id.to_string(),
            keep_alive: true,
        });
    }
    None
}

/// Fire the `on_rebase_conflict` hook immediately.
#[allow(clippy::too_many_arguments)]
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
    folder_id: Option<&str>,
    folder_name: Option<&str>,
    monitor: Option<&HookMonitor>,
    runner: Option<&HookRunner>,
) -> HookActionOutcome {
    let Some(plan) = plan_on_rebase_conflict(
        project_hooks,
        global_hooks,
        project_id,
        project_name,
        project_path,
        branch,
        target_branch,
        main_repo_path,
        rebase_error,
        folder_id,
        folder_name,
    ) else {
        return (Vec::new(), Vec::new());
    };
    execute_hook_action_plan(plan, monitor, runner)
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
    folder_id: Option<&str>,
    folder_name: Option<&str>,
    monitor: Option<&HookMonitor>,
    runner: Option<&HookRunner>,
) -> HookActionOutcome {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.worktree.on_dirty_close) {
        let mut env = project_env(
            project_id,
            project_name,
            project_path,
            folder_id,
            folder_name,
        );
        env.insert("OKENA_BRANCH".into(), branch.into());
        log::info!(
            "Running on_dirty_worktree_close hook for project '{}'",
            project_name
        );
        return run_hook_actions(
            &cmd,
            env,
            monitor,
            "on_dirty_worktree_close",
            project_name,
            runner,
            project_id,
            true,
        );
    }
    (Vec::new(), Vec::new())
}

/// Run the dirty-close safety hook to completion without creating project-owned PTYs.
pub fn fire_on_dirty_worktree_close_headless(
    project_hooks: &HooksConfig,
    global_hooks: &HooksConfig,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    branch: &str,
    folder_id: Option<&str>,
    folder_name: Option<&str>,
    monitor: Option<&HookMonitor>,
) -> Result<(), String> {
    let Some(command) = resolve_hook(project_hooks, global_hooks, |h| &h.worktree.on_dirty_close)
    else {
        return Ok(());
    };

    let mut env = project_env(
        project_id,
        project_name,
        project_path,
        folder_id,
        folder_name,
    );
    env.insert("OKENA_BRANCH".into(), branch.into());
    log::info!(
        "Running on_dirty_worktree_close hook for project '{}'",
        project_name
    );

    for action in parse_hook_actions(&command) {
        let command = match action {
            HookAction::Background(command) | HookAction::Terminal(command) => command,
        };
        run_hook_sync(
            &command,
            env.clone(),
            monitor,
            "on_dirty_worktree_close",
            project_name,
            None,
            project_id,
        )?;
    }
    Ok(())
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
    folder_id: Option<&str>,
    folder_name: Option<&str>,
    monitor: Option<&HookMonitor>,
    runner: Option<&HookRunner>,
) -> Vec<HookTerminalResult> {
    if let Some(cmd) = resolve_hook(project_hooks, global_hooks, |h| &h.worktree.after_remove) {
        let mut env = project_env(
            project_id,
            project_name,
            project_path,
            folder_id,
            folder_name,
        );
        env.insert("OKENA_BRANCH".into(), branch.into());
        env.insert("OKENA_MAIN_REPO_PATH".into(), main_repo_path.into());
        log::info!(
            "Running worktree_removed hook for project '{}'",
            project_name
        );
        if let Some(result) = run_hook(
            cmd,
            env,
            monitor,
            "worktree_removed",
            project_name,
            runner,
            project_id,
            true,
        ) {
            return vec![result];
        }
    }
    Vec::new()
}

/// Resolve the `terminal.on_create` hook command.
/// Returns the command string if configured at any level (project/parent/global).
#[cfg(feature = "gpui")]
pub fn resolve_terminal_on_create(
    project_hooks: &HooksConfig,
    parent_hooks: Option<&HooksConfig>,
    global_hooks: &HooksConfig,
    _cx: &App,
) -> Option<String> {
    resolve_hook_with_parent(project_hooks, parent_hooks, global_hooks, |h| {
        &h.terminal.on_create
    })
}

/// Resolve the `terminal.on_create` hook command (without GPUI context).
/// Returns the command string if configured at any level (project/parent/global).
pub fn resolve_terminal_on_create_simple(
    project_hooks: &HooksConfig,
    parent_hooks: Option<&HooksConfig>,
    global_hooks: &HooksConfig,
) -> Option<String> {
    resolve_hook_with_parent(project_hooks, parent_hooks, global_hooks, |h| {
        &h.terminal.on_create
    })
}

/// Apply the `terminal.on_create` command by wrapping the shell to run
/// the command first, then `exec` into the original shell.
/// On POSIX, environment variables are exported so they persist in the shell session.
/// Windows callers must use [`terminal_launch_plan`] to propagate environment safely.
/// Produces: `sh -c 'export K=V; ...; <on_create_cmd>; exec <shell_cmd>'`
pub fn apply_on_create(
    shell: &ShellType,
    on_create_cmd: &str,
    env_vars: &HashMap<String, String>,
) -> ShellType {
    let shell_cmd = shell.to_command_string();
    let prefix = build_export_prefix(env_vars);
    let script = format!("{}{}; exec {}", prefix, on_create_cmd, shell_cmd);
    ShellType::for_command(script)
}

/// Compose create-only hooks without losing the shell used for backend routing.
pub fn terminal_launch_plan(
    shell: ShellType,
    shell_wrapper: Option<&str>,
    on_create: Option<&str>,
    env_vars: &HashMap<String, String>,
) -> TerminalLaunchPlan {
    if shell_wrapper.is_none() && on_create.is_none() {
        return TerminalLaunchPlan::for_shell(shell);
    }

    let (program, args, handoff, separator, needs_handoff) = terminal_launch_parts(&shell);
    let wrapped = shell_wrapper
        .map(|wrapper| wrapper.replace("{shell}", &handoff))
        .unwrap_or_else(|| handoff.clone());
    let body = match (on_create, shell_wrapper, needs_handoff) {
        (Some(command), _, true) => format!("{command}{separator}{wrapped}"),
        (Some(command), None, false) => command.to_string(),
        (Some(command), Some(_), false) => format!("{command}{separator}{wrapped}"),
        (None, Some(_), _) => wrapped,
        (None, None, _) => unreachable!("empty hooks returned above"),
    };
    let mut command_args = args;
    command_args.push(body);
    TerminalLaunchPlan {
        route: shell,
        initial_command: Some(TerminalLaunchCommand {
            program,
            args: command_args,
        }),
        environment: safe_hook_environment(env_vars),
    }
}

fn terminal_launch_parts(shell: &ShellType) -> (String, Vec<String>, String, &'static str, bool) {
    #[cfg(windows)]
    match shell {
        ShellType::Cmd | ShellType::Default => {
            return (
                "cmd.exe".to_string(),
                vec!["/D".to_string(), "/S".to_string(), "/C".to_string()],
                "cmd.exe /K".to_string(),
                " & ",
                true,
            );
        }
        ShellType::PowerShell { core } => {
            let program = if *core { "pwsh.exe" } else { "powershell.exe" };
            return (
                program.to_string(),
                vec![
                    "-NoLogo".to_string(),
                    "-NoExit".to_string(),
                    "-Command".to_string(),
                ],
                format!("{program} -NoLogo -NoExit"),
                "; ",
                false,
            );
        }
        ShellType::Wsl { .. } => {
            return (
                "sh".to_string(),
                vec!["-lc".to_string()],
                "exec \"${SHELL:-sh}\"".to_string(),
                "; ",
                true,
            );
        }
        ShellType::Custom { path, args } => {
            let mut command_args = args.clone();
            command_args.push("-ic".to_string());
            return (
                path.clone(),
                command_args,
                format!("exec {}", shell.to_command_string()),
                "; ",
                true,
            );
        }
    }

    #[cfg(not(windows))]
    {
        let (program, mut args) = match shell {
            ShellType::Custom { path, args } => (path.clone(), args.clone()),
            ShellType::Default => (
                std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
                Vec::new(),
            ),
        };
        args.push("-ic".to_string());
        (
            program,
            args,
            format!("exec {}", shell.to_command_string()),
            "; ",
            true,
        )
    }
}

/// Fire the `terminal.on_close` hook after a terminal PTY exits, taking the
/// `HookMonitor` explicitly (GPUI-free). Runs headlessly (no PTY runner) since
/// the terminal just exited.
///
/// This is the core; the GPUI [`fire_terminal_on_close`] wrapper just reads the
/// monitor global from `&App` and delegates here. The daemon (no GPUI globals)
/// calls this directly with the monitor it owns.
pub fn fire_terminal_on_close_with_services(
    project_hooks: &HooksConfig,
    parent_hooks: Option<&HooksConfig>,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    terminal_id: &str,
    terminal_name: Option<&str>,
    is_worktree: bool,
    exit_code: Option<u32>,
    folder_id: Option<&str>,
    folder_name: Option<&str>,
    global_hooks: &HooksConfig,
    monitor: Option<&HookMonitor>,
) {
    if let Some(cmd) = resolve_hook_with_parent(project_hooks, parent_hooks, global_hooks, |h| {
        &h.terminal.on_close
    }) {
        let mut env = project_env(
            project_id,
            project_name,
            project_path,
            folder_id,
            folder_name,
        );
        env.insert("OKENA_TERMINAL_ID".into(), terminal_id.into());
        if let Some(name) = terminal_name {
            env.insert("OKENA_TERMINAL_NAME".into(), name.into());
        }
        if let Some(code) = exit_code {
            env.insert("OKENA_EXIT_CODE".into(), code.to_string());
        }
        if is_worktree {
            let path = std::path::Path::new(project_path);
            let branch = okena_git::get_git_status(path)
                .and_then(|s| s.branch)
                .or_else(|| okena_git::get_current_branch(path));
            if let Some(branch) = branch {
                env.insert("OKENA_BRANCH".into(), branch);
            }
        }
        log::info!(
            "Running terminal.on_close hook for terminal '{}'",
            terminal_id
        );
        run_hook(
            cmd,
            env,
            monitor,
            "terminal.on_close",
            project_name,
            None,
            project_id,
            true,
        );
    }
}

/// GPUI wrapper around [`fire_terminal_on_close_with_services`]: reads the
/// `HookMonitor` global from `&App` and delegates. Kept so existing `&App`
/// callers (e.g. okena-app's PTY exit loop) compile unchanged.
#[cfg(feature = "gpui")]
#[allow(clippy::too_many_arguments)]
pub fn fire_terminal_on_close(
    project_hooks: &HooksConfig,
    parent_hooks: Option<&HooksConfig>,
    project_id: &str,
    project_name: &str,
    project_path: &str,
    terminal_id: &str,
    terminal_name: Option<&str>,
    is_worktree: bool,
    exit_code: Option<u32>,
    folder_id: Option<&str>,
    folder_name: Option<&str>,
    global_hooks: &HooksConfig,
    cx: &App,
) {
    let monitor = try_monitor(cx);
    fire_terminal_on_close_with_services(
        project_hooks,
        parent_hooks,
        project_id,
        project_name,
        project_path,
        terminal_id,
        terminal_name,
        is_worktree,
        exit_code,
        folder_id,
        folder_name,
        global_hooks,
        monitor.as_ref(),
    );
}

/// Resolve the shell_wrapper for terminal creation.
/// Returns the wrapper command template if configured (project or global level).
pub fn resolve_shell_wrapper(
    project_hooks: &HooksConfig,
    parent_hooks: Option<&HooksConfig>,
    global_hooks: &HooksConfig,
) -> Option<String> {
    resolve_hook_with_parent(project_hooks, parent_hooks, global_hooks, |h| {
        &h.terminal.shell_wrapper
    })
}

/// Apply shell_wrapper to a ShellType, producing a new ShellType.
/// The wrapper template uses `{shell}` as a placeholder for the resolved shell command.
/// On POSIX, environment variables are exported so they persist in the shell session.
/// Windows callers must use [`terminal_launch_plan`] to propagate environment safely.
///
/// If the result contains shell metacharacters (`&&`, `||`, `;`, `|`), it is wrapped
/// in `sh -c` for proper execution. Otherwise, it is split into executable + args directly,
/// avoiding an extra `sh` process layer (important for session backends like dtach/tmux).
///
/// The shell is expected to be already resolved (not `ShellType::Default`).
pub fn apply_shell_wrapper(
    shell: &ShellType,
    wrapper: &str,
    env_vars: &HashMap<String, String>,
) -> ShellType {
    let shell_cmd = shell.to_command_string();
    // Replace {shell} with `exec <shell>` so the shell replaces the wrapper process.
    // This is critical for session backends (dtach/tmux) that monitor the top-level process.
    let wrapped = wrapper.replace("{shell}", &format!("exec {}", shell_cmd));
    let prefix = build_export_prefix(env_vars);
    // Always use for_command (sh -c '...') so that build_terminal_command can extract
    // the inner command for session backend integration (dtach/tmux/screen).
    ShellType::for_command(format!("{}{}", prefix, wrapped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_state::WorktreeHooks;

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
    fn pty_sync_failure_releases_terminal_ownership() {
        use okena_terminal::backend::LocalBackend;
        use okena_terminal::pty_manager::{PtyEvent, PtyManager};
        use okena_terminal::session_backend::SessionBackend;

        let monitor = HookMonitor::new();
        let (pty_manager, events) = PtyManager::new(SessionBackend::None);
        let pty_manager = Arc::new(pty_manager);
        let backend: Arc<dyn TerminalBackend> = Arc::new(LocalBackend::new(pty_manager.clone()));
        let terminals: TerminalsRegistry = Default::default();
        let runner = HookRunner::new(backend, terminals.clone());
        let event_monitor = monitor.clone();
        let event_thread = std::thread::spawn(move || {
            loop {
                if let PtyEvent::Exit {
                    terminal_id,
                    exit_code,
                    ..
                } = events.recv_blocking().expect("receive hook PTY event")
                {
                    event_monitor.notify_exit(&terminal_id, exit_code);
                    return terminal_id;
                }
            }
        });

        let result = run_hook_sync(
            "exit 7",
            HashMap::new(),
            Some(&monitor),
            "pre_merge",
            "Project",
            Some(&runner),
            "p1",
        );
        let terminal_id = event_thread.join().expect("event thread joins");

        assert!(matches!(result, Err(message) if message == "Hook failed (exit 7)"));
        assert!(!terminals.lock().contains_key(&terminal_id));
        assert!(pty_manager.current_generation(&terminal_id).is_none());
        assert!(matches!(
            monitor.history()[0].status,
            HookStatus::Failed { exit_code: 7, .. }
        ));
        pty_manager.flush_teardown();
    }

    #[test]
    fn resolve_hook_prefers_project_over_global() {
        let project = HooksConfig {
            worktree: WorktreeHooks {
                pre_merge: Some("project-cmd".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let global = HooksConfig {
            worktree: WorktreeHooks {
                pre_merge: Some("global-cmd".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let resolved = resolve_hook(&project, &global, |h| &h.worktree.pre_merge);
        assert_eq!(resolved, Some("project-cmd".into()));
    }

    #[test]
    fn resolve_hook_falls_back_to_global() {
        let project = HooksConfig::default();
        let global = HooksConfig {
            worktree: WorktreeHooks {
                pre_merge: Some("global-cmd".into()),
                ..Default::default()
            },
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
        let actions =
            parse_hook_actions("terminal: claude -p \"fix\"\necho logged\n\nterminal: htop");
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
        let (terminal_actions, _hook_results) = run_hook_actions(
            "terminal: my-cmd\necho bg",
            env,
            None,
            "test",
            "proj",
            None,
            "proj-id",
            true,
        );
        assert_eq!(terminal_actions.len(), 1);
        assert_eq!(terminal_actions[0].0, "my-cmd");
        assert_eq!(terminal_actions[0].1.get("KEY").unwrap(), "val");
    }

    #[test]
    fn rebase_conflict_plan_defers_actions_until_execution() {
        let hooks = HooksConfig {
            worktree: WorktreeHooks {
                on_rebase_conflict: Some("terminal: fix-conflict".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let plan = plan_on_rebase_conflict(
            &hooks,
            &HooksConfig::default(),
            "p1",
            "Project",
            ".",
            "feature",
            "main",
            ".",
            "conflict",
            None,
            None,
        )
        .unwrap();

        let (terminal_actions, hook_results) = execute_hook_action_plan(plan, None, None);

        assert_eq!(terminal_actions.len(), 1);
        assert_eq!(terminal_actions[0].0, "fix-conflict");
        assert!(hook_results.is_empty());
    }

    #[test]
    fn dirty_close_terminal_actions_run_headless_to_completion() {
        let hooks = HooksConfig {
            worktree: WorktreeHooks {
                on_dirty_close: Some("terminal: true".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let monitor = HookMonitor::new();

        fire_on_dirty_worktree_close_headless(
            &hooks,
            &HooksConfig::default(),
            "project-id",
            "project",
            ".",
            "feature",
            None,
            None,
            Some(&monitor),
        )
        .expect("headless dirty-close action should succeed");

        let history = monitor.history();
        assert_eq!(history.len(), 1);
        assert!(history[0].terminal_id.is_none());
        assert!(matches!(history[0].status, HookStatus::Succeeded { .. }));
    }

    #[test]
    fn post_merge_headless_sync_finishes_before_returning() {
        let hooks = HooksConfig {
            worktree: WorktreeHooks {
                post_merge: Some("true".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let monitor = HookMonitor::new();

        fire_post_merge_headless_sync(
            &hooks,
            &HooksConfig::default(),
            "project-id",
            "project",
            ".",
            "feature",
            "main",
            ".",
            None,
            None,
            Some(&monitor),
        )
        .expect("post_merge should complete synchronously");

        let history = monitor.history();
        assert_eq!(history.len(), 1);
        assert!(history[0].terminal_id.is_none());
        assert!(matches!(history[0].status, HookStatus::Succeeded { .. }));
    }

    #[test]
    fn build_hook_label_uses_branch() {
        let mut env = HashMap::new();
        env.insert("OKENA_BRANCH".into(), "feature/foo".into());
        assert_eq!(
            build_hook_label("on_project_open", &env, "my-project"),
            "on_project_open (feature/foo)"
        );
    }

    #[test]
    fn build_hook_label_falls_back_to_project_name() {
        let env = HashMap::new();
        assert_eq!(
            build_hook_label("on_project_open", &env, "my-project"),
            "on_project_open (my-project)"
        );
    }

    #[test]
    fn resolve_hook_with_parent_three_tier() {
        use okena_state::TerminalHooks;

        let project = HooksConfig::default();
        let parent = HooksConfig {
            terminal: TerminalHooks {
                on_create: Some("parent-cmd".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let global = HooksConfig {
            terminal: TerminalHooks {
                on_create: Some("global-cmd".into()),
                ..Default::default()
            },
            ..Default::default()
        };

        // Project empty → falls through to parent
        let resolved =
            resolve_hook_with_parent(&project, Some(&parent), &global, |h| &h.terminal.on_create);
        assert_eq!(resolved, Some("parent-cmd".into()));

        // Project empty, no parent → falls through to global
        let resolved = resolve_hook_with_parent(&project, None, &global, |h| &h.terminal.on_create);
        assert_eq!(resolved, Some("global-cmd".into()));

        // Project set → wins over parent and global
        let project_with_hook = HooksConfig {
            terminal: TerminalHooks {
                on_create: Some("project-cmd".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let resolved = resolve_hook_with_parent(&project_with_hook, Some(&parent), &global, |h| {
            &h.terminal.on_create
        });
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

    #[cfg(not(windows))]
    #[test]
    fn apply_shell_wrapper_simple() {
        use super::apply_shell_wrapper;
        let shell = ShellType::Custom {
            path: "/bin/zsh".to_string(),
            args: vec!["--login".to_string()],
        };
        let wrapper = "devcontainer exec -- {shell}";
        let wrapped = apply_shell_wrapper(&shell, wrapper, &HashMap::new());
        match &wrapped {
            ShellType::Custom { path: _, args } => {
                // for_command uses $SHELL -ic on Unix
                assert!(args[0] == "-c" || args[0] == "-ic", "got: {}", args[0]);
                assert!(
                    args[1].contains("devcontainer exec -- exec /bin/zsh --login"),
                    "got: {}",
                    args[1]
                );
            }
            other => panic!("Expected ShellType::Custom, got: {:?}", other),
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn apply_shell_wrapper_with_metacharacters() {
        use super::apply_shell_wrapper;
        let shell = ShellType::Custom {
            path: "/bin/zsh".to_string(),
            args: vec![],
        };
        let wrapper = "echo hello && {shell}";
        let wrapped = apply_shell_wrapper(&shell, wrapper, &HashMap::new());
        match &wrapped {
            ShellType::Custom { path: _, args } => {
                // for_command uses $SHELL -ic on Unix
                assert!(args[0] == "-c" || args[0] == "-ic", "got: {}", args[0]);
                assert!(
                    args[1].contains("echo hello && exec /bin/zsh"),
                    "got: {}",
                    args[1]
                );
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

    #[cfg(not(windows))]
    #[test]
    fn build_export_prefix_empty() {
        assert_eq!(build_export_prefix(&HashMap::new()), "");
    }

    #[cfg(not(windows))]
    #[test]
    fn build_export_prefix_single_var() {
        let mut env = HashMap::new();
        env.insert("MY_VAR".into(), "hello".into());
        let prefix = build_export_prefix(&env);
        assert!(prefix.contains("MY_VAR"), "got: {}", prefix);
        assert!(prefix.contains("hello"), "got: {}", prefix);
        if cfg!(windows) {
            assert!(prefix.contains("set"), "got: {}", prefix);
        } else {
            assert!(prefix.contains("export"), "got: {}", prefix);
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn build_export_prefix_escapes_single_quotes() {
        let mut env = HashMap::new();
        env.insert("VAR".into(), "it's a test".into());
        let prefix = build_export_prefix(&env);
        if !cfg!(windows) {
            // POSIX: single quotes with '\'' escaping
            assert!(
                prefix.contains("'\\''"),
                "Expected single-quote escape in: {}",
                prefix
            );
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn build_export_prefix_filters_invalid_keys() {
        let mut env = HashMap::new();
        env.insert("GOOD_KEY".into(), "val".into());
        env.insert("BAD;KEY".into(), "val".into());
        env.insert("123BAD".into(), "val".into());
        let prefix = build_export_prefix(&env);
        assert!(prefix.contains("GOOD_KEY"), "got: {}", prefix);
        assert!(!prefix.contains("BAD;KEY"), "got: {}", prefix);
        assert!(!prefix.contains("123BAD"), "got: {}", prefix);
    }

    #[cfg(not(windows))]
    #[test]
    fn keep_alive_hook_shell_reports_exit_and_reopens_shell_on_unix() {
        let shell = build_keep_alive_hook_shell("export KEY='value'; false", false);
        let ShellType::Custom { args, .. } = shell else {
            panic!("expected custom command shell");
        };
        let script = &args[1];

        assert!(script.contains("export KEY='value'; false"));
        assert!(script.contains("__okena_hook_exit:%d"));
        assert!(script.contains("exec \"${SHELL:-sh}\""));
    }

    #[test]
    fn keep_alive_hook_shell_uses_cmd_completion_protocol_on_windows() {
        let shell = build_keep_alive_hook_shell("set \"KEY=value\" && exit /B 4", true);
        let ShellType::Custom { path, args } = shell else {
            panic!("expected custom command shell");
        };

        assert_eq!(path, "cmd");
        assert_eq!(args[0], "/V:ON");
        assert_eq!(args[1], "/C");
        assert!(args[2].contains("!ERRORLEVEL!"));
        assert!(args[2].contains("__okena_hook_exit:!__okena_rc!"));
        assert!(args[2].contains("cmd /K"));
        assert!(!args[2].contains("${SHELL:-sh}"));
    }

    #[cfg(not(windows))]
    #[test]
    fn apply_on_create_with_env_vars() {
        let shell = ShellType::Custom {
            path: "/bin/bash".to_string(),
            args: vec![],
        };
        let mut env = HashMap::new();
        env.insert("OKENA_PROJECT_ID".into(), "proj-123".into());
        let result = apply_on_create(&shell, "echo hello", &env);
        match &result {
            ShellType::Custom { path: _, args } => {
                let cmd = &args[1];
                assert!(cmd.contains("export OKENA_PROJECT_ID="), "got: {}", cmd);
                assert!(cmd.contains("echo hello"), "got: {}", cmd);
                assert!(cmd.contains("exec /bin/bash"), "got: {}", cmd);
            }
            other => panic!("Expected ShellType::Custom, got: {:?}", other),
        }
    }

    #[cfg(windows)]
    #[test]
    fn cmd_on_create_uses_cmd_handoff_without_posix_exec() {
        let plan = terminal_launch_plan(ShellType::Cmd, None, Some("echo ready"), &HashMap::new());
        let command = plan.initial_command.expect("create command");

        assert_eq!(plan.route, ShellType::Cmd);
        assert_eq!(command.program, "cmd.exe");
        assert_eq!(&command.args[..3], ["/D", "/S", "/C"]);
        assert!(command.args[3].contains("echo ready & cmd.exe /K"));
        assert!(!command.args[3].contains("; exec"));
    }

    #[cfg(windows)]
    #[test]
    fn cmd_launch_keeps_hostile_environment_out_of_script() {
        let hostile = "quote\" & echo owned>%TEMP%\\okena-sentinel !bang!\r\nnext".to_string();
        let env = HashMap::from([("OKENA_PROJECT_NAME".to_string(), hostile.clone())]);
        let plan = terminal_launch_plan(ShellType::Cmd, None, Some("echo ready"), &env);
        let command = plan.initial_command.expect("create command");

        assert_eq!(
            plan.environment,
            vec![("OKENA_PROJECT_NAME".to_string(), hostile.clone())]
        );
        assert!(!command.args.iter().any(|arg| arg.contains(&hostile)));
    }

    #[cfg(windows)]
    #[test]
    fn rerunnable_cmd_hook_encodes_environment_and_command() {
        use base64::Engine;

        let hostile = "quote\" & %PATH% !bang!\r\nnext";
        let env = HashMap::from([("OKENA_PROJECT_NAME".to_string(), hostile.to_string())]);
        let rerunnable = windows_encoded_hook_command("echo ready", &env);
        let encoded = rerunnable
            .split_whitespace()
            .last()
            .expect("encoded command");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("base64 command");
        let utf16: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let script = String::from_utf16(&utf16).expect("UTF-16LE command");

        assert!(!script.contains(hostile));
        assert!(script.contains(&base64::engine::general_purpose::STANDARD.encode(hostile)));
        assert!(script.contains(&base64::engine::general_purpose::STANDARD.encode("echo ready")));
    }

    #[cfg(windows)]
    #[test]
    fn powershell_on_create_uses_no_exit_command_argv() {
        let plan = terminal_launch_plan(
            ShellType::PowerShell { core: true },
            None,
            Some("Write-Host ready"),
            &HashMap::new(),
        );
        let command = plan.initial_command.expect("create command");

        assert_eq!(command.program, "pwsh.exe");
        assert_eq!(&command.args[..3], ["-NoLogo", "-NoExit", "-Command"]);
        assert_eq!(command.args[3], "Write-Host ready");
        assert!(!command.args[3].contains("exec"));
    }

    #[cfg(windows)]
    #[test]
    fn wsl_wrapper_and_on_create_preserve_wsl_route() {
        let route = ShellType::Wsl {
            distro: Some("Ubuntu".to_string()),
        };
        let plan = terminal_launch_plan(
            route.clone(),
            Some("envbox -- {shell}"),
            Some("echo ready"),
            &HashMap::new(),
        );
        let command = plan.initial_command.expect("create command");

        assert_eq!(plan.route, route);
        assert_eq!(command.program, "sh");
        assert_eq!(&command.args[..1], ["-lc"]);
        assert!(command.args[1].contains("echo ready; envbox -- exec \"${SHELL:-sh}\""));
        assert!(!command.args[1].contains("cmd.exe"));
    }

    #[cfg(not(windows))]
    #[test]
    fn apply_shell_wrapper_with_env_vars() {
        let shell = ShellType::Custom {
            path: "/bin/zsh".to_string(),
            args: vec![],
        };
        let mut env = HashMap::new();
        env.insert("OKENA_PROJECT_NAME".into(), "my-project".into());
        let result = apply_shell_wrapper(&shell, "wrapper {shell}", &env);
        match &result {
            ShellType::Custom { path: _, args } => {
                let cmd = &args[1];
                assert!(cmd.contains("export OKENA_PROJECT_NAME="), "got: {}", cmd);
                assert!(cmd.contains("wrapper exec /bin/zsh"), "got: {}", cmd);
            }
            other => panic!("Expected ShellType::Custom, got: {:?}", other),
        }
    }
}
