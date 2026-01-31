use crate::settings::settings;
use crate::workspace::persistence::HooksConfig;
use gpui::App;
use std::collections::HashMap;

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
        let mut cmd = std::process::Command::new("sh");
        #[cfg(unix)]
        cmd.arg("-c").arg(&command);

        #[cfg(windows)]
        let mut cmd = std::process::Command::new("cmd");
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
    env.insert("TERM_MANAGER_PROJECT_ID".into(), project_id.into());
    env.insert("TERM_MANAGER_PROJECT_NAME".into(), project_name.into());
    env.insert("TERM_MANAGER_PROJECT_PATH".into(), project_path.into());
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
        env.insert("TERM_MANAGER_BRANCH".into(), branch.into());
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
