use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::collections::HashMap;
#[cfg(windows)]
use std::sync::Mutex;

/// Get the user's login shell, falling back to /bin/sh.
/// On Windows this is only called in the WSL session-backend path where the
/// result ends up inside a `wsl.exe -- sh -c "…"` command, so the /bin/sh
/// fallback is appropriate.
fn user_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}

/// Backend for persistent terminal sessions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SessionBackend {
    /// No persistence - direct shell
    None,
    /// Use tmux for session persistence
    Tmux,
    /// Use screen for session persistence
    Screen,
    /// Use dtach for minimal session persistence (no scrollback management)
    Dtach,
    /// Use psmux for session persistence on Windows (native ConPTY, tmux-compatible)
    Psmux,
    /// Auto-detect: prefer dtach > tmux > screen on Unix; psmux on Windows
    #[default]
    Auto,
}

impl SessionBackend {
    /// Parse from string (for env variable override). Infallible: unknown
    /// values fall back to `None`, so this is not a `FromStr` implementation.
    #[allow(dead_code)]
    pub fn parse_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "tmux" => Self::Tmux,
            "screen" => Self::Screen,
            "dtach" => Self::Dtach,
            "psmux" => Self::Psmux,
            "none" | "off" | "false" | "0" => Self::None,
            "auto" | "smart" | "on" | "true" | "1" => Self::Auto,
            _ => Self::None,
        }
    }

    /// Load from environment variable OKENA_SESSION_BACKEND
    /// Defaults to Auto if not set
    #[allow(dead_code)]
    pub fn from_env() -> Self {
        std::env::var("OKENA_SESSION_BACKEND")
            .map(|s| Self::parse_str(&s))
            .unwrap_or_default()
    }

    /// Resolve Auto to a concrete backend based on availability
    pub fn resolve(self) -> ResolvedBackend {
        match self {
            Self::None => ResolvedBackend::None,
            Self::Tmux => {
                if is_tmux_available() {
                    ResolvedBackend::Tmux
                } else {
                    log::warn!("tmux requested but not available, falling back to none");
                    ResolvedBackend::None
                }
            }
            Self::Screen => {
                if is_screen_available() {
                    ResolvedBackend::Screen
                } else {
                    log::warn!("screen requested but not available, falling back to none");
                    ResolvedBackend::None
                }
            }
            Self::Dtach => {
                if is_dtach_available() {
                    ResolvedBackend::Dtach
                } else {
                    log::warn!("dtach requested but not available, falling back to none");
                    ResolvedBackend::None
                }
            }
            Self::Psmux => {
                if is_psmux_available() {
                    ResolvedBackend::Psmux
                } else {
                    log::warn!("psmux requested but not available, falling back to none");
                    ResolvedBackend::None
                }
            }
            Self::Auto => {
                // On Windows, psmux is the only native session backend (dtach/tmux/screen
                // require a Unix shell). On Unix, prefer dtach (minimal, no scrollback
                // interference) then tmux, then screen.
                #[cfg(windows)]
                {
                    if is_psmux_available() {
                        log::info!("Auto-detected psmux for session persistence");
                        ResolvedBackend::Psmux
                    } else {
                        log::info!("No session backend available, sessions won't persist");
                        ResolvedBackend::None
                    }
                }
                #[cfg(not(windows))]
                {
                    if is_dtach_available() {
                        log::info!("Auto-detected dtach for session persistence");
                        ResolvedBackend::Dtach
                    } else if is_tmux_available() {
                        log::info!("Auto-detected tmux for session persistence");
                        ResolvedBackend::Tmux
                    } else if is_screen_available() {
                        log::info!("Auto-detected screen for session persistence");
                        ResolvedBackend::Screen
                    } else {
                        log::info!("No session backend available, sessions won't persist");
                        ResolvedBackend::None
                    }
                }
            }
        }
    }

    /// Get display name for UI
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::None => "None (Direct Shell)",
            Self::Auto => "Auto (best available)",
            Self::Tmux => "tmux",
            Self::Screen => "screen",
            Self::Dtach => "dtach (minimal)",
            Self::Psmux => "psmux (Windows)",
        }
    }

    /// Get all variants for UI dropdown
    pub fn all_variants() -> &'static [SessionBackend] {
        &[
            SessionBackend::Auto,
            #[cfg(not(windows))]
            SessionBackend::Dtach,
            #[cfg(not(windows))]
            SessionBackend::Tmux,
            #[cfg(not(windows))]
            SessionBackend::Screen,
            #[cfg(windows)]
            SessionBackend::Psmux,
            SessionBackend::None,
        ]
    }
}

/// Resolved (concrete) backend - no Auto variant
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedBackend {
    None,
    Tmux,
    Screen,
    Dtach,
    Psmux,
}

/// Exact command requested for a newly-created persistent session.
///
/// `ShellScript` preserves the historical API used by Unix backends. `Program`
/// keeps the executable and argv boundary intact for host-Windows psmux, where
/// rebuilding a cmd wrapper from only the script would lose flags such as
/// delayed expansion (`/V:ON`).
#[derive(Debug, Clone, Copy)]
pub enum SessionCommand<'a> {
    ShellScript(&'a str),
    Program {
        program: &'a str,
        args: &'a [String],
    },
}

/// Session-name prefix for Okena-managed persistent sessions (tmux/screen/dtach).
/// Shared by the session-name builder and the dtach socket-GC filter so cleanup
/// only ever considers our own `tm-*.sock` files — never the local daemon socket
/// (`<16hex>.sock`) that lives in the same runtime dir.
pub const SESSION_NAME_PREFIX: &str = "tm-";

impl ResolvedBackend {
    /// Check if this backend supports session persistence
    pub fn supports_persistence(&self) -> bool {
        !matches!(self, Self::None)
    }

    /// Generate a session name for a terminal ID
    /// Uses a prefix to avoid conflicts with user sessions
    pub fn session_name(&self, terminal_id: &str) -> String {
        // Use short prefix + first 8 chars of UUID to keep it manageable
        let short_id = if terminal_id.len() > 8 {
            &terminal_id[..8]
        } else {
            terminal_id
        };
        format!("{SESSION_NAME_PREFIX}{short_id}")
    }

    /// Get the socket path for dtach sessions
    /// Returns None for non-dtach backends
    #[allow(dead_code)]
    pub fn socket_path(&self, terminal_id: &str) -> Option<std::path::PathBuf> {
        if !matches!(self, Self::Dtach) {
            return None;
        }
        Some(get_dtach_socket_path(terminal_id))
    }

    /// Build the command to create or attach to a session
    /// Returns (program, args) tuple
    /// When `command` is Some, the session runs that command instead of the default shell.
    /// `extra_env` is injected into newly-created sessions where the backend supports it
    /// (e.g. tmux's `-e KEY=VAL`), so vars set after a long-running daemon was started
    /// still reach the shell.
    pub fn build_command(
        &self,
        session_name: &str,
        cwd: &str,
        command: Option<&str>,
        extra_env: &[(String, Option<String>)],
    ) -> Option<(String, Vec<String>)> {
        self.build_command_with_custom(
            session_name,
            cwd,
            command.map(SessionCommand::ShellScript),
            extra_env,
        )
    }

    /// Build a session command while preserving an exact custom executable and argv.
    pub fn build_command_with_custom(
        &self,
        session_name: &str,
        cwd: &str,
        command: Option<SessionCommand<'_>>,
        extra_env: &[(String, Option<String>)],
    ) -> Option<(String, Vec<String>)> {
        match self {
            Self::None => None,
            Self::Tmux => {
                // Use sh -c to properly chain tmux commands
                // \; is tmux command separator - since args are passed directly via CommandBuilder
                // (not through shell parsing), we only need single escape level
                // -A: attach if exists, create if not
                // -s: session name
                // -c: start directory
                // set status off: hide tmux status bar (we have our own UI)
                // set mouse on: enable mouse for scrolling
                // set default-terminal: ensure inner TERM supports 256color
                // set terminal-features + terminal-overrides: enable 24-bit truecolor (RGB)
                // set automatic-rename off: prevent shell from overwriting window name
                // rename-window: set meaningful window name from directory
                let window_name = extract_dir_name(cwd);
                let initial_program = match command {
                    Some(SessionCommand::ShellScript(cmd)) => {
                        let sh = user_shell();
                        format!(" {} '-ic' {}", shell_escape(&sh), shell_escape(cmd))
                    }
                    Some(SessionCommand::Program { program, args }) => {
                        let mut parts = Vec::with_capacity(args.len() + 1);
                        parts.push(shell_escape(program));
                        parts.extend(args.iter().map(|arg| shell_escape(arg)));
                        format!(" {}", parts.join(" "))
                    }
                    None => String::new(),
                };
                // -e KEY=VAL flags reach the shell even when attaching to a
                // pre-existing tmux server whose global env predates Okena.
                let env_args: String = extra_env
                    .iter()
                    .filter_map(|(k, v)| {
                        v.as_ref()
                            .map(|val| format!(" -e {}", shell_escape(&format!("{k}={val}"))))
                    })
                    .collect();
                // For removals there is no `-e` equivalent: clear the var from the
                // session environment so later splits/panes don't inherit a stale
                // value. (The very first pane is handled by `env_remove` on the
                // tmux client, which a freshly-started server inherits.)
                let unset_args: String = extra_env
                    .iter()
                    .filter(|(_, v)| v.is_none())
                    .map(|(k, _)| format!(" \\; set-environment -u {}", shell_escape(k)))
                    .collect();
                let tmux_cmd = format!(
                    "tmux new-session -A{} -s {} -c {}{} \\; set status off \\; set mouse on \\; set default-terminal xterm-256color \\; set terminal-features 'xterm-256color:RGB' \\; set -as terminal-overrides ',xterm-256color:Tc' \\; set-window-option automatic-rename off \\; rename-window {}{}",
                    env_args,
                    shell_escape(session_name),
                    shell_escape(cwd),
                    initial_program,
                    shell_escape(&window_name),
                    unset_args
                );
                Some(("sh".to_string(), vec!["-c".to_string(), tmux_cmd]))
            }
            Self::Screen => {
                // screen -D -R <name>
                // -D -R: reattach if exists, create if not (and detach other attached sessions)
                // Note: screen doesn't have a direct way to set cwd, we'll handle that separately
                let mut args = vec!["-D".to_string(), "-R".to_string(), session_name.to_string()];
                if let Some(command) = command {
                    match command {
                        SessionCommand::ShellScript(cmd) => {
                            args.push(user_shell());
                            args.push("-ic".to_string());
                            args.push(cmd.to_string());
                        }
                        SessionCommand::Program {
                            program,
                            args: command_args,
                        } => {
                            args.push(program.to_string());
                            args.extend(command_args.iter().cloned());
                        }
                    }
                }
                Some(("screen".to_string(), args))
            }
            Self::Dtach => {
                // dtach -A <socket> -E -r winch <shell>
                // -A: attach if exists, create if not
                // -E: disable detach character (^\ won't detach)
                // -r winch: use SIGWINCH for redraw (needed for apps like less, vim)
                //
                // We use sh -c to:
                // 1. Create the socket directory if needed
                // 2. cd to the working directory
                // 3. Run dtach with the user's shell (or custom command)
                let socket_path = get_dtach_socket_path(session_name);
                let program = match command {
                    Some(SessionCommand::ShellScript(cmd)) => {
                        let sh = user_shell();
                        format!("{} -ic {}", shell_escape(&sh), shell_escape(cmd))
                    }
                    Some(SessionCommand::Program { program, args }) => {
                        let mut parts = Vec::with_capacity(args.len() + 1);
                        parts.push(shell_escape(program));
                        parts.extend(args.iter().map(|arg| shell_escape(arg)));
                        parts.join(" ")
                    }
                    None => shell_escape(&user_shell()),
                };

                let parent = socket_path.parent().and_then(|p| p.to_str())?;
                let socket = socket_path.to_str()?;
                let dtach_cmd = format!(
                    "mkdir -p {} && cd {} && exec dtach -A {} -E -r winch {}",
                    shell_escape(parent),
                    shell_escape(cwd),
                    shell_escape(socket),
                    program
                );
                Some(("sh".to_string(), vec!["-c".to_string(), dtach_cmd]))
            }
            Self::Psmux => {
                // psmux speaks tmux command language but we bypass any host shell:
                // Windows cmd.exe doesn't honor sh-style single-quoting, and we have
                // no need for it — `;` is psmux's literal command separator when
                // received as its own argv token (no `\;` shell escaping required).
                //
                // The initial pane runs psmux's configured default-shell (PowerShell
                // on a fresh install). Okena's `default_shell` setting is currently
                // not propagated into the session — users wanting `cmd.exe` etc.
                // inside persistent sessions need to set `default-shell` in
                // `~/.psmux.conf`.
                let window_name = extract_dir_name(cwd);
                let mut args = vec!["new-session".to_string(), "-A".to_string()];
                // -e KEY=VAL injects sets; removals (None) have no new-session
                // flag, so they're applied as `set-environment -u` commands
                // below — mirroring the tmux backend.
                for (k, v) in extra_env {
                    if let Some(val) = v {
                        args.push("-e".to_string());
                        args.push(format!("{k}={val}"));
                    }
                }
                args.push("-s".to_string());
                args.push(session_name.to_string());
                args.push("-c".to_string());
                args.push(cwd.to_string());
                if let Some(command) = command {
                    match command {
                        SessionCommand::ShellScript(cmd) => {
                            // Preserve the legacy script API through cmd.exe.
                            args.push("cmd.exe".to_string());
                            args.push("/c".to_string());
                            args.push(cmd.to_string());
                        }
                        SessionCommand::Program {
                            program,
                            args: command_args,
                        } => {
                            args.push(program.to_string());
                            args.extend(command_args.iter().cloned());
                        }
                    }
                }
                let push_cmd = |args: &mut Vec<String>, parts: &[&str]| {
                    args.push(";".to_string());
                    for p in parts {
                        args.push((*p).to_string());
                    }
                };
                push_cmd(&mut args, &["set", "status", "off"]);
                push_cmd(&mut args, &["set", "mouse", "on"]);
                push_cmd(&mut args, &["set-window-option", "automatic-rename", "off"]);
                args.push(";".to_string());
                args.push("rename-window".to_string());
                args.push(window_name);
                // Clear any env vars marked for removal so later panes don't
                // inherit a stale value (parallels the tmux `set-environment -u`).
                for (k, v) in extra_env {
                    if v.is_none() {
                        push_cmd(&mut args, &["set-environment", "-u", k.as_str()]);
                    }
                }
                Some(("psmux".to_string(), args))
            }
        }
    }

    /// Stop a persistent session. Success means both the kill command and a
    /// bounded liveness probe confirm that the session no longer exists.
    pub fn kill_session(&self, session_name: &str) -> bool {
        match self {
            Self::None => true,
            Self::Tmux | Self::Screen | Self::Psmux => self.kill_session_with_executor(
                session_name,
                session_backend_output,
                std::time::Duration::from_secs(2),
            ),
            Self::Dtach => {
                let socket_path = get_dtach_socket_path(session_name);
                if socket_path.exists() {
                    #[cfg(unix)]
                    if !terminate_dtach_process_tree(&socket_path, session_name) {
                        // Keep the socket path as the durable retry handle. Unlinking
                        // it while either the master or its child tree survives is
                        // what made leaked agent trees invisible to later cleanup.
                        return false;
                    }

                    if let Err(error) = std::fs::remove_file(&socket_path)
                        && error.kind() != std::io::ErrorKind::NotFound
                    {
                        log::error!("failed to remove dtach socket {:?}: {}", socket_path, error);
                        return false;
                    }
                    log::debug!("Removed dtach socket: {:?}", socket_path);
                }
                true
            }
        }
    }

    fn kill_session_with_executor(
        &self,
        session_name: &str,
        mut execute: impl FnMut(&str, &[&str]) -> std::io::Result<std::process::Output>,
        timeout: std::time::Duration,
    ) -> bool {
        let (program, kill_args, probe_args) = match self {
            Self::Tmux => (
                "tmux",
                vec!["kill-session", "-t", session_name],
                vec!["has-session", "-t", session_name],
            ),
            Self::Screen => (
                "screen",
                vec!["-S", session_name, "-X", "quit"],
                vec!["-S", session_name, "-Q", "select", "."],
            ),
            Self::Psmux => (
                "psmux",
                vec!["kill-session", "-t", session_name],
                vec!["has-session", "-t", session_name],
            ),
            Self::None | Self::Dtach => unreachable!("backend does not use command verification"),
        };
        verify_session_kill(
            execute(program, &kill_args),
            || execute(program, &probe_args).map(|output| output.status.success()),
            timeout,
        )
    }
}

fn session_backend_output(program: &str, args: &[&str]) -> std::io::Result<std::process::Output> {
    let mut command = crate::process::command(program);
    command.args(args);
    #[cfg(target_os = "macos")]
    command.env("PATH", get_extended_path());
    crate::process::safe_output(&mut command)
}

fn verify_session_kill(
    kill_result: std::io::Result<std::process::Output>,
    mut session_is_live: impl FnMut() -> std::io::Result<bool>,
    timeout: std::time::Duration,
) -> bool {
    match kill_result {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            log::error!("session kill command exited with {}", output.status);
            return false;
        }
        Err(error) => {
            log::error!("failed to run session kill command: {error}");
            return false;
        }
    }

    let deadline = std::time::Instant::now() + timeout;
    loop {
        match session_is_live() {
            Ok(false) => return true,
            Ok(true) if std::time::Instant::now() >= deadline => {
                log::error!("session survived bounded kill verification");
                return false;
            }
            Ok(true) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Err(error) => {
                log::error!("failed to verify session liveness: {error}");
                return false;
            }
        }
    }
}

#[cfg(unix)]
fn process_is_live(pid: i32) -> bool {
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(unix)]
fn wait_for_pids_to_exit(pids: &[i32]) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let live: Vec<i32> = pids
            .iter()
            .copied()
            .filter(|pid| process_is_live(*pid))
            .collect();
        if live.is_empty() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            for pid in live {
                // The PID was verified as a holder of this Okena-owned socket
                // immediately before TERM; this is the bounded escalation path.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        let any_live = pids.iter().any(|pid| process_is_live(*pid));
        if !any_live {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct TrackedProcess {
    pid: i32,
    /// Platform process-birth marker. Revalidating this before every signal
    /// prevents a recycled PID from targeting an unrelated process.
    start_marker: Option<u128>,
}

#[cfg(unix)]
fn raw_process_is_alive(pid: i32) -> bool {
    // SAFETY: signal 0 performs existence/permission checking only and takes no
    // pointers. All callers pass a positive PID discovered from process state.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(target_os = "macos")]
fn process_start_marker(pid: i32) -> Option<u128> {
    let (seconds, micros) = crate::macos_proc::process_start_time(pid as u32)?;
    Some(((seconds as u128) << 64) | micros as u128)
}

#[cfg(target_os = "linux")]
fn linux_process_stat(pid: i32) -> Option<(i32, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // `comm` is parenthesized and may itself contain spaces or `)`; the final
    // ") " delimiter is the only safe place to begin fixed-field parsing.
    let suffix = stat.rsplit_once(") ")?.1;
    let fields: Vec<&str> = suffix.split_whitespace().collect();
    let parent_pid = fields.get(1)?.parse().ok()?; // field 4
    let start_ticks = fields.get(19)?.parse().ok()?; // field 22
    Some((parent_pid, start_ticks))
}

#[cfg(target_os = "linux")]
fn process_start_marker(pid: i32) -> Option<u128> {
    linux_process_stat(pid).map(|(_, start_ticks)| start_ticks as u128)
}

#[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
fn process_start_marker(_pid: i32) -> Option<u128> {
    None
}

#[cfg(unix)]
fn tracked_process(pid: i32) -> Option<TrackedProcess> {
    let start_marker = process_start_marker(pid);
    (start_marker.is_some() || raw_process_is_alive(pid))
        .then_some(TrackedProcess { pid, start_marker })
}

#[cfg(unix)]
fn same_process_is_alive(process: TrackedProcess) -> bool {
    match process.start_marker {
        Some(marker) => process_start_marker(process.pid) == Some(marker),
        None => raw_process_is_alive(process.pid),
    }
}

#[cfg(target_os = "macos")]
fn process_tree_snapshot() -> std::collections::HashMap<i32, Vec<i32>> {
    crate::macos_proc::process_tree()
        .into_iter()
        .map(|(parent, children)| {
            (
                parent as i32,
                children.into_iter().map(|pid| pid as i32).collect(),
            )
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn process_tree_snapshot() -> std::collections::HashMap<i32, Vec<i32>> {
    let mut tree = std::collections::HashMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return tree;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        if let Some((parent_pid, _)) = linux_process_stat(pid) {
            tree.entry(parent_pid).or_insert_with(Vec::new).push(pid);
        }
    }
    tree
}

#[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
fn process_tree_snapshot() -> std::collections::HashMap<i32, Vec<i32>> {
    let mut tree = std::collections::HashMap::new();
    let Ok(output) = crate::process::command("ps")
        .args(["-axo", "pid=,ppid="])
        .output()
    else {
        return tree;
    };
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(parent)) = (fields.next(), fields.next()) else {
            continue;
        };
        if let (Ok(pid), Ok(parent)) = (pid.parse::<i32>(), parent.parse::<i32>()) {
            tree.entry(parent).or_insert_with(Vec::new).push(pid);
        }
    }
    tree
}

#[cfg(unix)]
fn tracked_descendants(roots: &[TrackedProcess]) -> Vec<TrackedProcess> {
    fn visit(
        pid: i32,
        tree: &std::collections::HashMap<i32, Vec<i32>>,
        visited: &mut std::collections::HashSet<i32>,
        descendants: &mut Vec<TrackedProcess>,
    ) {
        let Some(children) = tree.get(&pid) else {
            return;
        };
        for &child in children {
            if !visited.insert(child) {
                continue;
            }
            visit(child, tree, visited, descendants);
            if let Some(process) = tracked_process(child) {
                descendants.push(process);
            }
        }
    }

    let tree = process_tree_snapshot();
    let mut visited: std::collections::HashSet<i32> =
        roots.iter().map(|process| process.pid).collect();
    let mut descendants = Vec::new();
    for &root in roots {
        if same_process_is_alive(root) {
            visit(root.pid, &tree, &mut visited, &mut descendants);
        }
    }
    descendants
}

#[cfg(unix)]
fn signal_tracked_processes(processes: &[TrackedProcess], signal: i32, session_name: &str) {
    for &process in processes {
        if !same_process_is_alive(process) {
            continue;
        }
        // SAFETY: `kill(2)` takes plain integer values and no pointers. The PID
        // has just been revalidated against its platform birth marker.
        unsafe {
            libc::kill(process.pid, signal);
        }
        log::debug!(
            "Sent signal {signal} to process {} for dtach session {session_name}",
            process.pid
        );
    }
}

#[cfg(unix)]
fn dtach_socket_holders(socket_path: &std::path::PathBuf) -> Vec<i32> {
    let my_pid = std::process::id() as i32;
    crate::pty_manager::find_pids_for_unix_sockets(std::slice::from_ref(socket_path))
        .remove(socket_path)
        .unwrap_or_default()
        .into_iter()
        .map(|pid| pid as i32)
        .filter(|pid| *pid != my_pid)
        .collect()
}

#[cfg(unix)]
fn tracked_dtach_socket_holders(socket_path: &std::path::PathBuf) -> Vec<TrackedProcess> {
    let tracked: Vec<TrackedProcess> = dtach_socket_holders(socket_path)
        .into_iter()
        .filter_map(tracked_process)
        .collect();
    let current: std::collections::HashSet<i32> =
        dtach_socket_holders(socket_path).into_iter().collect();
    tracked
        .into_iter()
        .filter(|process| current.contains(&process.pid) && same_process_is_alive(*process))
        .collect()
}

#[cfg(unix)]
fn dtach_socket_is_definitively_dead(socket_path: &std::path::Path) -> bool {
    match std::os::unix::net::UnixStream::connect(socket_path) {
        Ok(_) => false,
        Err(error) => matches!(
            error.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
        ),
    }
}

#[cfg(unix)]
fn terminate_dtach_process_tree(socket_path: &std::path::PathBuf, session_name: &str) -> bool {
    let mut holders = tracked_dtach_socket_holders(socket_path);
    if holders.is_empty() {
        if dtach_socket_is_definitively_dead(socket_path) {
            return true;
        }
        log::warn!(
            "Refusing to unlink live dtach session {session_name}: no socket holder PID was discoverable"
        );
        return false;
    }

    // Freeze verified holders first. Besides pinning their PID identities, this
    // keeps the dtach master as a stable parentage anchor while descendants are
    // discovered and frozen below.
    signal_tracked_processes(&holders, libc::SIGSTOP, session_name);
    let stopped_holders = holders.clone();
    std::thread::sleep(std::time::Duration::from_millis(10));
    let confirmed_holders: std::collections::HashSet<TrackedProcess> =
        tracked_dtach_socket_holders(socket_path)
            .into_iter()
            .collect();
    holders.retain(|holder| confirmed_holders.contains(holder));
    if holders.is_empty() {
        signal_tracked_processes(&stopped_holders, libc::SIGCONT, session_name);
        return dtach_socket_is_definitively_dead(socket_path);
    }

    // dtach exits on SIGTERM without forwarding it to the child PTY process
    // group. Iteratively freeze descendants parent-first until two snapshots are
    // stable. Once every anchored process is SIGSTOPed, none can fork during the
    // destructive pass and the socket remains a durable retry handle on failure.
    let mut descendants = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut stable_snapshots = 0;
    for _ in 0..8 {
        let snapshot = tracked_descendants(&holders);
        let mut newly_seen: Vec<TrackedProcess> = snapshot
            .into_iter()
            .filter(|process| seen.insert(*process))
            .collect();
        if newly_seen.is_empty() {
            stable_snapshots += 1;
            if stable_snapshots == 2 {
                break;
            }
        } else {
            stable_snapshots = 0;
            // `tracked_descendants` is child-first; reverse it so spawning
            // parents are stopped before their children.
            newly_seen.reverse();
            signal_tracked_processes(&newly_seen, libc::SIGSTOP, session_name);
            descendants.extend(newly_seen);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    if stable_snapshots < 2 {
        signal_tracked_processes(&descendants, libc::SIGCONT, session_name);
        signal_tracked_processes(&stopped_holders, libc::SIGCONT, session_name);
        log::error!(
            "Dtach session {session_name} descendant tree did not quiesce; preserving {:?} for retry",
            socket_path
        );
        return false;
    }

    signal_tracked_processes(&descendants, libc::SIGKILL, session_name);
    std::thread::sleep(std::time::Duration::from_millis(50));
    let surviving_descendants = descendants
        .iter()
        .filter(|process| same_process_is_alive(**process))
        .count();
    if surviving_descendants > 0 {
        signal_tracked_processes(&descendants, libc::SIGCONT, session_name);
        signal_tracked_processes(&stopped_holders, libc::SIGCONT, session_name);
        log::error!(
            "Dtach session {session_name} still has {surviving_descendants} live descendant(s); preserving {:?} for retry",
            socket_path
        );
        return false;
    }

    // SIGTERM is queued while holders are stopped; SIGCONT lets dtach run its
    // normal exit/unlink path. Escalate only freshly revalidated survivors.
    signal_tracked_processes(&stopped_holders, libc::SIGTERM, session_name);
    signal_tracked_processes(&stopped_holders, libc::SIGCONT, session_name);
    std::thread::sleep(std::time::Duration::from_millis(50));

    let surviving_holders = tracked_dtach_socket_holders(socket_path);
    if !surviving_holders.is_empty() {
        signal_tracked_processes(&surviving_holders, libc::SIGKILL, session_name);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let terminated = dtach_socket_holders(socket_path).is_empty()
        && dtach_socket_is_definitively_dead(socket_path);
    if !terminated {
        log::error!(
            "Dtach session {session_name} is still live after teardown; preserving {:?} for retry",
            socket_path
        );
    }
    terminated
}

/// Minimum age before a `tm-*.sock` file is a GC candidate. A socket created
/// just before this scan may not yet appear in the process/socket snapshot, so
/// treat recent files as live.
const DTACH_SOCKET_GC_MIN_AGE: std::time::Duration = std::time::Duration::from_secs(60);

/// Whether a filename matches the dtach/tmux socket naming scheme (`tm-*.sock`).
/// Pure so it's unit-testable. The dtach GC runs in the daemon's own process and
/// scans the shared runtime dir, which also holds the daemon's control socket
/// (`<16hex>.sock`) — this filter keeps GC from ever considering those.
#[cfg(unix)]
fn is_stale_gc_candidate(name: &str) -> bool {
    name.starts_with(SESSION_NAME_PREFIX) && name.ends_with(".sock")
}

/// Whether a socket that old is too fresh to GC (see `DTACH_SOCKET_GC_MIN_AGE`).
/// Pure so the age threshold is unit-testable independent of filesystem mtime.
#[cfg(unix)]
fn is_too_fresh_to_gc(age: std::time::Duration) -> bool {
    age < DTACH_SOCKET_GC_MIN_AGE
}

/// Age of a socket file from its mtime. `None` when the file is gone/unreadable;
/// a future mtime (clock skew) reads as "just now" so it's treated as fresh.
#[cfg(unix)]
fn socket_age(path: &std::path::Path) -> Option<std::time::Duration> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(modified.elapsed().unwrap_or(std::time::Duration::ZERO))
}

#[cfg(unix)]
fn dtach_session_name_from_path(path: &std::path::Path) -> Option<String> {
    let file_name = path.file_name()?.to_str()?;
    is_stale_gc_candidate(file_name)
        .then(|| file_name.strip_suffix(".sock").map(str::to_owned))
        .flatten()
}

#[cfg(unix)]
fn orphaned_dtach_session_names<'a>(
    socket_paths: impl IntoIterator<Item = &'a std::path::PathBuf>,
    retained_session_names: &std::collections::HashSet<String>,
) -> Vec<String> {
    let mut orphaned: Vec<String> = socket_paths
        .into_iter()
        .filter_map(|path| dtach_session_name_from_path(path))
        .filter(|name| !retained_session_names.contains(name))
        .collect();
    orphaned.sort();
    orphaned.dedup();
    orphaned
}

/// Reconcile live dtach sessions against the workspace that owns this profile.
/// Sessions absent from authoritative state are leftovers from an interrupted or
/// incomplete close and must not survive another daemon start.
#[cfg(unix)]
pub fn reconcile_dtach_sessions(retained_terminal_ids: &std::collections::HashSet<String>) {
    // Always reconcile dtach artifacts, even when the newly selected backend is
    // tmux/screen/none or Auto now resolves differently.
    let backend = ResolvedBackend::Dtach;
    let dir = get_dtach_socket_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let socket_paths: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_stale_gc_candidate)
        })
        // A recently-created socket may belong to a terminal added after the
        // caller's last disk snapshot. Reconciliation never destroys it.
        .filter(|path| socket_age(path).is_some_and(|age| !is_too_fresh_to_gc(age)))
        .collect();
    let retained_session_names: std::collections::HashSet<String> = retained_terminal_ids
        .iter()
        .map(|terminal_id| backend.session_name(terminal_id))
        .collect();
    let orphaned = orphaned_dtach_session_names(socket_paths.iter(), &retained_session_names);

    for session_name in &orphaned {
        backend.kill_session(session_name);
    }
    if !orphaned.is_empty() {
        log::info!(
            "Reconciled {} orphaned dtach session(s) in {:?}",
            orphaned.len(),
            dir
        );
    }
}

#[cfg(not(unix))]
pub fn reconcile_dtach_sessions(_retained_terminal_ids: &std::collections::HashSet<String>) {}

/// Tear down every persistent dtach session in a stopped profile before its
/// authoritative profile directory is deleted.
#[cfg(unix)]
pub fn reap_dtach_profile_sessions(profile_id: &str) -> std::io::Result<usize> {
    let dir = okena_core::profiles::dtach_socket_dir_for_profile(profile_id);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut reaped = 0;
    for path in entries.flatten().map(|entry| entry.path()) {
        let Some(session_name) = dtach_session_name_from_path(&path) else {
            continue;
        };
        if !terminate_dtach_process_tree(&path, &session_name) {
            return Err(std::io::Error::other(format!(
                "persistent terminal session {session_name} did not terminate"
            )));
        }
        let _ = std::fs::remove_file(path);
        reaped += 1;
    }
    Ok(reaped)
}

#[cfg(not(unix))]
pub fn reap_dtach_profile_sessions(_profile_id: &str) -> std::io::Result<usize> {
    Ok(0)
}

/// Remove dtach socket files whose dtach process is no longer running.
/// Called once at startup to clean up after crashes or ungraceful exits.
///
/// Only `tm-*.sock` files (our own session sockets) are ever candidates — the
/// daemon control socket living in the same dir is off-limits (see
/// `is_stale_gc_candidate`) — and recently-created sockets are skipped to avoid
/// a TOCTOU delete of a socket not yet in the `/proc` snapshot.
#[cfg(unix)]
pub fn cleanup_stale_dtach_sockets() {
    let dir = get_dtach_socket_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return, // dir doesn't exist yet — nothing to clean
    };

    let socket_paths: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_stale_gc_candidate)
        })
        // TOCTOU guard: skip freshly-created sockets that may not be in the
        // upcoming /proc snapshot yet.
        .filter(|p| !socket_age(p).is_some_and(is_too_fresh_to_gc))
        .collect();

    // One /proc socket scan for every socket at once, instead of an `lsof -t`
    // spawn (~1s each) per file.
    let holders = crate::pty_manager::find_pids_for_unix_sockets(&socket_paths);

    let mut removed = 0;
    for path in &socket_paths {
        let has_listener = holders.get(path).map(|v| !v.is_empty()).unwrap_or(false);
        if !has_listener && dtach_socket_is_definitively_dead(path) {
            let _ = std::fs::remove_file(path);
            removed += 1;
        }
    }

    if removed > 0 {
        log::info!(
            "Cleaned up {} stale dtach socket(s) from {:?}",
            removed,
            dir
        );
    }
}

/// Resolve a session backend for a specific WSL distro.
/// Runs `wsl.exe -d <distro> -- sh -c "command -v <tool>"` to check availability.
/// Results are cached per (distro, preference) pair so detection runs at most once.
#[cfg(windows)]
pub fn resolve_for_wsl(distro: Option<&str>, preference: SessionBackend) -> ResolvedBackend {
    use std::sync::LazyLock;

    static CACHE: LazyLock<Mutex<HashMap<(Option<String>, SessionBackend), ResolvedBackend>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    let key = (distro.map(|s| s.to_string()), preference);
    let cache = CACHE.lock().unwrap_or_else(|poisoned| {
        log::warn!("WSL backend cache mutex was poisoned, recovering");
        poisoned.into_inner()
    });
    if let Some(cached) = cache.get(&key) {
        return *cached;
    }
    drop(cache);

    let result =
        resolve_wsl_backend_with_probe(preference, |tool| is_wsl_tool_available(distro, tool));

    CACHE
        .lock()
        .unwrap_or_else(|poisoned| {
            log::warn!("WSL backend cache mutex was poisoned, recovering");
            poisoned.into_inner()
        })
        .insert(key, result);
    result
}

#[cfg(windows)]
fn resolve_wsl_backend_with_probe(
    preference: SessionBackend,
    mut available: impl FnMut(&str) -> bool,
) -> ResolvedBackend {
    match preference {
        SessionBackend::None => ResolvedBackend::None,
        SessionBackend::Tmux => {
            if available("tmux") {
                ResolvedBackend::Tmux
            } else {
                log::warn!("tmux requested but not available in WSL, falling back to none");
                ResolvedBackend::None
            }
        }
        SessionBackend::Screen => {
            if available("screen") {
                ResolvedBackend::Screen
            } else {
                log::warn!("screen requested but not available in WSL, falling back to none");
                ResolvedBackend::None
            }
        }
        SessionBackend::Dtach => {
            if wsl_dtach_available(&mut available) {
                ResolvedBackend::Dtach
            } else {
                log::warn!(
                    "dtach requested but its WSL teardown tools are unavailable, falling back to none"
                );
                ResolvedBackend::None
            }
        }
        SessionBackend::Psmux => {
            // psmux is a host-Windows backend; WSL terminals need a Unix tool inside the
            // distro. Pick the best available there instead of refusing persistence.
            if wsl_dtach_available(&mut available) {
                log::info!("psmux requested but inside WSL — using dtach instead");
                ResolvedBackend::Dtach
            } else if available("tmux") {
                log::info!("psmux requested but inside WSL — using tmux instead");
                ResolvedBackend::Tmux
            } else if available("screen") {
                log::info!("psmux requested but inside WSL — using screen instead");
                ResolvedBackend::Screen
            } else {
                log::warn!(
                    "psmux requested but no session tool available in WSL, falling back to none"
                );
                ResolvedBackend::None
            }
        }
        SessionBackend::Auto => {
            if wsl_dtach_available(&mut available) {
                log::info!("Auto-detected dtach in WSL for session persistence");
                ResolvedBackend::Dtach
            } else if available("tmux") {
                log::info!("Auto-detected tmux in WSL for session persistence");
                ResolvedBackend::Tmux
            } else if available("screen") {
                log::info!("Auto-detected screen in WSL for session persistence");
                ResolvedBackend::Screen
            } else {
                log::info!("No session backend available in WSL");
                ResolvedBackend::None
            }
        }
    }
}

#[cfg(windows)]
fn wsl_dtach_available(available: &mut impl FnMut(&str) -> bool) -> bool {
    available("dtach") && available("lsof") && available("xargs")
}

/// Check if a tool is available inside a WSL distro using `command -v`.
#[cfg(windows)]
fn is_wsl_tool_available(distro: Option<&str>, tool: &str) -> bool {
    let mut cmd = crate::process::command("wsl.exe");
    if let Some(d) = distro {
        cmd.args(["-d", d]);
    }
    cmd.args([
        "--",
        "sh",
        "-c",
        &format!("command -v {}", shell_escape(tool)),
    ]);
    crate::process::safe_output(&mut cmd)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// WSL-native socket directory for dtach sessions (lives inside WSL, not on Windows host).
/// Uses a fixed path since we can't read XDG_RUNTIME_DIR from outside WSL.
#[cfg(windows)]
const WSL_DTACH_SOCKET_DIR: &str = "/tmp/okena-dtach";

/// Get the WSL-native socket path for a dtach session.
#[cfg(windows)]
fn get_wsl_dtach_socket_path(session_name: &str) -> String {
    format!("{}/{}.sock", WSL_DTACH_SOCKET_DIR, session_name)
}

impl ResolvedBackend {
    /// Build a session command wrapped through `wsl.exe` for running inside WSL.
    /// Returns `("wsl.exe", [args...])` or `None` for `ResolvedBackend::None`.
    ///
    /// Unlike `build_command()` (which runs on the host), this constructs commands
    /// that execute inside WSL. Key differences:
    /// - dtach socket paths use WSL-native `/tmp/` instead of Windows temp dir
    /// - Default shell uses `"$SHELL"` (resolved inside WSL) instead of host env var
    #[cfg(windows)]
    pub fn build_wsl_session_command(
        &self,
        distro: Option<&str>,
        session_name: &str,
        wsl_cwd: &str,
        command: Option<SessionCommand<'_>>,
        environment: &[(String, Option<String>)],
    ) -> Option<(String, Vec<String>)> {
        let inner_cmd = match self {
            // psmux runs on the Windows host, never inside WSL. resolve_for_wsl
            // never returns Psmux, so reaching this arm is a programming error.
            Self::None | Self::Psmux => return None,
            Self::Tmux => {
                // Tmux doesn't reference host paths or $SHELL, so delegate to build_command
                let (_program, inner_args) =
                    self.build_command_with_custom(session_name, wsl_cwd, command, environment)?;
                inner_args.last()?.to_string()
            }
            Self::Screen => {
                let (_program, inner_args) =
                    self.build_command_with_custom(session_name, wsl_cwd, command, &[])?;
                let mut parts = vec!["screen".to_string()];
                parts.extend(inner_args.iter().map(|a| shell_escape(a)));
                parts.join(" ")
            }
            Self::Dtach => {
                // Build dtach command with WSL-native socket path and $SHELL
                // (can't delegate to build_command — it uses Windows temp dir and host $SHELL)
                let socket_path = get_wsl_dtach_socket_path(session_name);
                let program = match command {
                    Some(SessionCommand::ShellScript(cmd)) => {
                        format!("sh -c {}", shell_escape(cmd))
                    }
                    Some(SessionCommand::Program { program, args }) => {
                        let mut parts = Vec::with_capacity(args.len() + 1);
                        parts.push(shell_escape(program));
                        parts.extend(args.iter().map(|arg| shell_escape(arg)));
                        parts.join(" ")
                    }
                    // Use $SHELL (resolved inside WSL) — not shell_escape'd so it expands
                    None => "\"$SHELL\"".to_string(),
                };
                format!(
                    "mkdir -p {} && cd {} && exec dtach -A {} -E -r winch {}",
                    shell_escape(WSL_DTACH_SOCKET_DIR),
                    shell_escape(wsl_cwd),
                    shell_escape(&socket_path),
                    program
                )
            }
        };

        let mut args = Vec::new();
        if let Some(d) = distro {
            args.push("-d".to_string());
            args.push(d.to_string());
        }
        args.push("--".to_string());
        if !environment.is_empty() {
            args.push("env".to_string());
            for (key, value) in environment {
                match value {
                    Some(value) => args.push(format!("{key}={value}")),
                    None => {
                        args.push("-u".to_string());
                        args.push(key.clone());
                    }
                }
            }
        }
        args.extend(["sh".to_string(), "-c".to_string(), inner_cmd]);

        Some(("wsl.exe".to_string(), args))
    }
}

/// Kill a session backend running inside WSL.
#[cfg(windows)]
pub fn kill_wsl_session(backend: ResolvedBackend, distro: Option<&str>, session_name: &str) {
    let kill_cmd = match backend {
        // Psmux is host-Windows only; resolve_for_wsl never returns it,
        // and kill_wsl_session is only called for WSL terminals.
        ResolvedBackend::None | ResolvedBackend::Psmux => return,
        ResolvedBackend::Tmux => {
            format!("tmux kill-session -t {}", shell_escape(session_name))
        }
        ResolvedBackend::Screen => {
            format!("screen -S {} -X quit", shell_escape(session_name))
        }
        ResolvedBackend::Dtach => {
            let socket = get_wsl_dtach_socket_path(session_name);
            format!(
                "lsof -t {} 2>/dev/null | xargs -r kill; rm -f {}",
                shell_escape(&socket),
                shell_escape(&socket)
            )
        }
    };

    let mut cmd = crate::process::command("wsl.exe");
    if let Some(d) = distro {
        cmd.args(["-d", d]);
    }
    cmd.args(["--", "sh", "-c", &kill_cmd]);
    let _ = crate::process::safe_output(&mut cmd);
    log::debug!("Killed WSL session {} ({:?})", session_name, backend);
}

/// Escape a string for safe use in shell commands
#[allow(dead_code)]
fn shell_escape(s: &str) -> String {
    // Wrap in single quotes and escape any existing single quotes
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Get the socket directory for dtach sessions
#[allow(dead_code)]
fn profile_scoped_dtach_socket_dir(
    base: std::path::PathBuf,
    profile_id: Option<&str>,
) -> std::path::PathBuf {
    let Some(profile_id) = profile_id else {
        return base;
    };
    let safe = !profile_id.is_empty()
        && !profile_id.contains('/')
        && !profile_id.contains('\\')
        && !profile_id.contains("..")
        && !profile_id.contains('\0');
    if profile_id == "default" || !safe {
        base
    } else {
        base.join("profiles").join(profile_id)
    }
}

fn dtach_socket_base_dir() -> std::path::PathBuf {
    okena_core::profiles::dtach_socket_base_dir()
}

fn active_profile_id() -> Option<String> {
    okena_core::profiles::try_current()
        .map(|profile| profile.id.clone())
        .or_else(|| std::env::var("OKENA_PROFILE").ok())
}

fn get_dtach_socket_dir() -> std::path::PathBuf {
    // Keep the default profile in the legacy root so existing sessions survive
    // the profile migration. Every named profile gets an isolated socket pool.
    profile_scoped_dtach_socket_dir(dtach_socket_base_dir(), active_profile_id().as_deref())
}

fn profile_dtach_socket_path(
    base: &std::path::Path,
    profile_id: Option<&str>,
    session_name: &str,
) -> std::path::PathBuf {
    let file_name = format!("{session_name}.sock");
    let scoped = profile_scoped_dtach_socket_dir(base.to_path_buf(), profile_id).join(&file_name);
    if scoped.exists() {
        return scoped;
    }

    let legacy = base.join(file_name);
    if scoped != legacy && legacy.exists() {
        legacy
    } else {
        scoped
    }
}

/// Get the socket path for a specific dtach session. A named profile first looks
/// in its isolated directory, then falls back to the pre-upgrade shared root so
/// retained legacy sessions remain attachable and closable. New sessions are
/// created in the isolated path once no legacy socket exists.
#[allow(dead_code)]
fn get_dtach_socket_path(session_name: &str) -> std::path::PathBuf {
    profile_dtach_socket_path(
        &dtach_socket_base_dir(),
        active_profile_id().as_deref(),
        session_name,
    )
}

/// Extract directory name from a path for use as window name
#[allow(dead_code)] // Used only on Unix for tmux window naming
fn extract_dir_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("terminal")
        .to_string()
}

/// Build a complete PATH for child processes (terminals and services).
///
/// Desktop entries and app bundles inherit a minimal PATH that misses
/// user-installed tools. We scan well-known directories directly instead
/// of spawning a shell, which is fragile (login vs interactive, .bash_profile
/// vs .bashrc, hangs, extra output, etc.).
#[cfg(not(windows))]
pub fn get_extended_path() -> String {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    let current_path = std::env::var("PATH").unwrap_or_default();
    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => return current_path,
    };

    // Well-known user bin directories, checked in order.
    // Only existing directories are added.
    let candidates: Vec<PathBuf> = vec![
        // Rust / Cargo
        home.join(".cargo/bin"),
        // Bun
        home.join(".bun/bin"),
        // Deno
        home.join(".deno/bin"),
        // Go
        home.join("go/bin"),
        // pnpm
        home.join(".local/share/pnpm"),
        // fnm (fast node manager)
        home.join(".local/share/fnm"),
        // pip / pipx / user scripts
        home.join(".local/bin"),
        // user bin
        home.join("bin"),
        // Fly.io
        home.join(".fly/bin"),
        // Homebrew (macOS)
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/opt/homebrew/sbin"),
        // Manual installs / Homebrew on Intel
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/local/sbin"),
        // MacPorts
        PathBuf::from("/opt/local/bin"),
        // Snap (Linux)
        PathBuf::from("/snap/bin"),
    ];

    // Preserve insertion order, deduplicate via HashSet.
    // User dirs first, then inherited PATH entries.
    let mut result: Vec<String> = Vec::new();
    let mut seen = HashSet::new();

    let mut push = |s: String| {
        if seen.insert(s.clone()) {
            result.push(s);
        }
    };

    for dir in &candidates {
        if dir.is_dir()
            && let Some(s) = dir.to_str()
        {
            push(s.to_string());
        }
    }

    // Also resolve fnm's current Node version if fnm is installed
    resolve_fnm_path(&home, &mut result, &mut seen);

    // Source .cargo/env if it exists — it may define CARGO_HOME in a non-default location
    if let Some(extra) = source_cargo_env(&home) {
        let cargo_bin = Path::new(&extra).join("bin");
        if cargo_bin.is_dir()
            && let Some(s) = cargo_bin.to_str()
            && seen.insert(s.to_string())
        {
            result.push(s.to_string());
        }
    }

    // Append inherited PATH entries (keeps system paths at the end)
    for entry in current_path.split(':') {
        if !entry.is_empty() && seen.insert(entry.to_string()) {
            result.push(entry.to_string());
        }
    }

    log::info!("Extended PATH ({} entries)", result.len());
    result.join(":")
}

/// Try to find fnm's current Node bin directory.
#[cfg(not(windows))]
fn resolve_fnm_path(
    home: &std::path::Path,
    result: &mut Vec<String>,
    seen: &mut std::collections::HashSet<String>,
) {
    // fnm stores the active version in $FNM_MULTISHELL_PATH or we can run `fnm env`.
    // But to avoid spawning processes, check the default symlink location.
    let fnm_dir = home.join(".local/share/fnm");
    if !fnm_dir.is_dir() {
        return;
    }
    let fnm_canonical = match fnm_dir.canonicalize() {
        Ok(p) => p,
        Err(_) => return,
    };
    // fnm aliases: default → specific version
    let default_alias = fnm_dir.join("aliases/default");
    if let Ok(version) = std::fs::read_link(&default_alias)
        .or_else(|_| std::fs::read_to_string(&default_alias).map(std::path::PathBuf::from))
    {
        // version is either an absolute path or just a version string like "v22.14.0"
        let node_bin = if version.is_absolute() {
            version.join("installation/bin")
        } else {
            fnm_dir
                .join("node-versions")
                .join(version.to_string_lossy().trim())
                .join("installation/bin")
        };
        // Validate the resolved path stays within fnm directory to prevent symlink escape
        if let Ok(canonical_bin) = node_bin.canonicalize() {
            if !canonical_bin.starts_with(&fnm_canonical) {
                log::warn!(
                    "fnm alias points outside fnm directory, skipping: {:?}",
                    node_bin
                );
                return;
            }
            if let Some(s) = canonical_bin.to_str()
                && seen.insert(s.to_string())
            {
                result.push(s.to_string());
            }
        }
    }
}

/// Check if .cargo/env defines a custom CARGO_HOME.
#[cfg(not(windows))]
fn source_cargo_env(home: &std::path::Path) -> Option<String> {
    let env_file = home.join(".cargo/env");
    let content = std::fs::read_to_string(env_file).ok()?;
    // Look for: export CARGO_HOME="..." or CARGO_HOME="..."
    for line in content.lines() {
        let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
        if let Some(rest) = line.strip_prefix("CARGO_HOME=") {
            let val = rest.trim_matches('"').trim_matches('\'');
            if !val.is_empty() && val != "$HOME/.cargo" {
                return Some(val.replace("$HOME", &home.to_string_lossy()));
            }
        }
    }
    None
}

/// Check if dtach is available on the system
/// Always returns false on Windows as dtach is not natively available
fn is_dtach_available() -> bool {
    #[cfg(windows)]
    {
        false
    }

    #[cfg(target_os = "macos")]
    {
        crate::process::safe_output(
            crate::process::command("dtach")
                .arg("-v")
                .env("PATH", get_extended_path()),
        )
        // dtach -v exits with 0 and prints version
        .map(|o| o.status.success() || !o.stdout.is_empty() || !o.stderr.is_empty())
        .unwrap_or(false)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        crate::process::safe_output(crate::process::command("dtach").arg("-v"))
            // dtach -v exits with 0 and prints version
            .map(|o| o.status.success() || !o.stdout.is_empty() || !o.stderr.is_empty())
            .unwrap_or(false)
    }
}

/// Check if tmux is available on the system
/// Always returns false on Windows as tmux is not natively available
fn is_tmux_available() -> bool {
    #[cfg(windows)]
    {
        false
    }

    #[cfg(target_os = "macos")]
    {
        crate::process::safe_output(
            crate::process::command("tmux")
                .arg("-V")
                .env("PATH", get_extended_path()),
        )
        .map(|o| o.status.success())
        .unwrap_or(false)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        crate::process::safe_output(crate::process::command("tmux").arg("-V"))
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

/// Check if psmux is available on the system.
/// Only enabled on Windows — psmux's purpose is being a native Windows tmux
/// alternative, and the rest of okena's session-backend chain prefers dtach/tmux/screen
/// on Unix.
fn is_psmux_available() -> bool {
    #[cfg(not(windows))]
    {
        false
    }

    #[cfg(windows)]
    {
        let mut cmd = crate::process::command("psmux");
        cmd.arg("-V");
        crate::process::safe_output(&mut cmd)
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

/// Check if screen is available on the system
/// Always returns false on Windows as screen is not natively available
fn is_screen_available() -> bool {
    #[cfg(windows)]
    {
        false
    }

    #[cfg(target_os = "macos")]
    {
        crate::process::safe_output(
            crate::process::command("screen")
                .arg("-v")
                .env("PATH", get_extended_path()),
        )
        .map(|o| o.status.success())
        .unwrap_or(false)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        crate::process::safe_output(crate::process::command("screen").arg("-v"))
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command_output(success: bool) -> std::process::Output {
        #[cfg(windows)]
        let mut command = {
            let mut command = std::process::Command::new("cmd.exe");
            command.args(["/C", if success { "exit 0" } else { "exit 1" }]);
            command
        };
        #[cfg(not(windows))]
        let mut command = std::process::Command::new(if success { "true" } else { "false" });
        command.output().expect("run command")
    }

    fn successful_command_output() -> std::process::Output {
        command_output(true)
    }

    #[test]
    fn verified_session_kill_rejects_command_failure() {
        assert!(!verify_session_kill(
            Err(std::io::Error::other("kill command failed")),
            || panic!("liveness must not be checked after command failure"),
            std::time::Duration::ZERO,
        ));
    }

    #[test]
    fn verified_session_kill_rejects_session_that_survives() {
        let mut probes = 0;
        assert!(!verify_session_kill(
            Ok(successful_command_output()),
            || {
                probes += 1;
                Ok(true)
            },
            std::time::Duration::ZERO,
        ));
        assert_eq!(probes, 1, "the live session was probed before failure");
    }

    #[test]
    fn verified_session_kill_accepts_confirmed_disappearance() {
        assert!(verify_session_kill(
            Ok(successful_command_output()),
            || Ok(false),
            std::time::Duration::ZERO,
        ));
    }

    #[test]
    fn command_backends_issue_exact_kill_and_probe_commands() {
        let cases = [
            (
                ResolvedBackend::Tmux,
                "tmux",
                ["kill-session", "-t", "tm-test"].as_slice(),
                ["has-session", "-t", "tm-test"].as_slice(),
            ),
            (
                ResolvedBackend::Screen,
                "screen",
                ["-S", "tm-test", "-X", "quit"].as_slice(),
                ["-S", "tm-test", "-Q", "select", "."].as_slice(),
            ),
            (
                ResolvedBackend::Psmux,
                "psmux",
                ["kill-session", "-t", "tm-test"].as_slice(),
                ["has-session", "-t", "tm-test"].as_slice(),
            ),
        ];

        for (backend, program, kill_args, probe_args) in cases {
            let mut calls = Vec::new();
            let mut invocation = 0;
            assert!(backend.kill_session_with_executor(
                "tm-test",
                |actual_program, actual_args| {
                    calls.push((
                        actual_program.to_string(),
                        actual_args
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>(),
                    ));
                    invocation += 1;
                    Ok(command_output(invocation == 1))
                },
                std::time::Duration::ZERO,
            ));
            assert_eq!(
                calls,
                vec![
                    (
                        program.to_string(),
                        kill_args.iter().map(ToString::to_string).collect(),
                    ),
                    (
                        program.to_string(),
                        probe_args.iter().map(ToString::to_string).collect(),
                    ),
                ],
                "{backend:?} must verify its exact session after killing it"
            );
        }
    }

    #[test]
    fn command_backend_kill_failure_preserves_dependent_checkout() {
        for backend in [
            ResolvedBackend::Tmux,
            ResolvedBackend::Screen,
            ResolvedBackend::Psmux,
        ] {
            let mut calls = Vec::new();
            assert!(!backend.kill_session_with_executor(
                "tm-test",
                |program, args| {
                    calls.push((
                        program.to_string(),
                        args.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    ));
                    Err(std::io::Error::other("kill failed"))
                },
                std::time::Duration::ZERO,
            ));
            assert_eq!(
                calls.len(),
                1,
                "{backend:?} must not probe after kill failure"
            );
        }
    }

    #[test]
    fn test_parse_backend() {
        assert_eq!(SessionBackend::parse_str("tmux"), SessionBackend::Tmux);
        assert_eq!(SessionBackend::parse_str("TMUX"), SessionBackend::Tmux);
        assert_eq!(SessionBackend::parse_str("screen"), SessionBackend::Screen);
        assert_eq!(SessionBackend::parse_str("dtach"), SessionBackend::Dtach);
        assert_eq!(SessionBackend::parse_str("DTACH"), SessionBackend::Dtach);
        assert_eq!(SessionBackend::parse_str("psmux"), SessionBackend::Psmux);
        assert_eq!(SessionBackend::parse_str("PSMUX"), SessionBackend::Psmux);
        assert_eq!(SessionBackend::parse_str("none"), SessionBackend::None);
        assert_eq!(SessionBackend::parse_str("auto"), SessionBackend::Auto);
        assert_eq!(SessionBackend::parse_str("smart"), SessionBackend::Auto);
        assert_eq!(SessionBackend::parse_str("invalid"), SessionBackend::None);
    }

    #[test]
    fn test_session_name() {
        let backend = ResolvedBackend::Tmux;
        let name = backend.session_name("12345678-1234-1234-1234-123456789012");
        assert_eq!(name, "tm-12345678");

        // Dtach uses same naming scheme
        let dtach_backend = ResolvedBackend::Dtach;
        let dtach_name = dtach_backend.session_name("12345678-1234-1234-1234-123456789012");
        assert_eq!(dtach_name, "tm-12345678");
    }

    #[cfg(unix)]
    #[test]
    fn stale_gc_candidate_matches_only_dtach_sockets() {
        // Our own session sockets: candidates.
        assert!(is_stale_gc_candidate("tm-01736dcb.sock"));
        assert!(is_stale_gc_candidate("tm-12345678.sock"));
        // Daemon control socket (`<16hex>.sock`): must NEVER be a candidate.
        assert!(!is_stale_gc_candidate("b7253cd8ed7892af.sock"));
        // Wrong extension / unrelated files.
        assert!(!is_stale_gc_candidate("tm-x.txt"));
        assert!(!is_stale_gc_candidate("okena.lock"));
        assert!(!is_stale_gc_candidate("remote.json"));
    }

    #[cfg(unix)]
    #[test]
    fn orphan_reconciliation_preserves_retained_sessions() {
        let sockets = [
            std::path::PathBuf::from("/runtime/tm-keep1234.sock"),
            std::path::PathBuf::from("/runtime/tm-drop5678.sock"),
            std::path::PathBuf::from("/runtime/daemon.sock"),
        ];
        let retained = std::collections::HashSet::from(["tm-keep1234".to_string()]);

        assert_eq!(
            orphaned_dtach_session_names(sockets.iter(), &retained),
            vec!["tm-drop5678".to_string()]
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_default_profiles_get_isolated_dtach_socket_directories() {
        let base = std::path::PathBuf::from("/tmp/okena-501");

        assert_eq!(profile_scoped_dtach_socket_dir(base.clone(), None), base);
        assert_eq!(
            profile_scoped_dtach_socket_dir(base.clone(), Some("default")),
            base
        );
        assert_eq!(
            profile_scoped_dtach_socket_dir(base.clone(), Some("work-client")),
            base.join("profiles").join("work-client")
        );
    }

    #[cfg(unix)]
    #[test]
    fn named_profile_reuses_legacy_socket_before_creating_an_isolated_one() {
        let base = std::env::temp_dir().join(format!(
            "okena-profile-socket-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let legacy = base.join("tm-legacy.sock");
        std::fs::write(&legacy, b"").unwrap();

        assert_eq!(
            profile_dtach_socket_path(&base, Some("work"), "tm-legacy"),
            legacy
        );
        std::fs::remove_file(&legacy).unwrap();
        assert_eq!(
            profile_dtach_socket_path(&base, Some("work"), "tm-legacy"),
            base.join("profiles/work/tm-legacy.sock")
        );
        let _ = std::fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn profile_reaping_refuses_live_sessions_and_removes_dead_sockets() {
        let profile_id = format!("test-profile-{}", uuid::Uuid::new_v4());
        let dir = okena_core::profiles::dtach_socket_dir_for_profile(&profile_id);
        std::fs::create_dir_all(&dir).unwrap();
        let socket_path = dir.join("tm-profile-test.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();

        assert!(reap_dtach_profile_sessions(&profile_id).is_err());
        assert!(socket_path.exists());

        drop(listener);
        assert_eq!(reap_dtach_profile_sessions(&profile_id).unwrap(), 1);
        assert!(!socket_path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn dtach_teardown_preserves_socket_until_its_master_is_dead() {
        let session_name = format!("tm-live-test-{}", std::process::id());
        let socket_path = get_dtach_socket_path(&session_name);
        std::fs::create_dir_all(socket_path.parent().expect("socket parent")).unwrap();
        let _ = std::fs::remove_file(&socket_path);
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();

        ResolvedBackend::Dtach.kill_session(&session_name);
        assert!(
            socket_path.exists(),
            "a socket that still accepts connections must stay discoverable"
        );

        drop(listener);
        ResolvedBackend::Dtach.kill_session(&session_name);
        assert!(
            !socket_path.exists(),
            "a dead socket should be removed once liveness is verified"
        );
    }

    #[cfg(unix)]
    #[test]
    fn dtach_teardown_reaps_the_real_child_process_tree() {
        if std::process::Command::new("dtach")
            .arg("--help")
            .output()
            .is_err()
        {
            return;
        }

        let unique = format!("{}-{}", std::process::id(), uuid::Uuid::new_v4());
        let session_name = format!("tm-tree-test-{unique}");
        let socket_path = get_dtach_socket_path(&session_name);
        std::fs::create_dir_all(socket_path.parent().expect("socket parent")).unwrap();
        let _ = std::fs::remove_file(&socket_path);
        let temp_dir = std::env::temp_dir().join(format!("okena-dtach-tree-{unique}"));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let shell_pid_file = temp_dir.join("shell.pid");
        let child_pid_file = temp_dir.join("child.pid");
        let command = format!(
            "echo $$ > {}; sleep 30 & echo $! > {}; wait",
            shell_escape(&shell_pid_file.to_string_lossy()),
            shell_escape(&child_pid_file.to_string_lossy())
        );
        let status = std::process::Command::new("dtach")
            .args([
                "-n",
                socket_path.to_str().unwrap(),
                "-E",
                "sh",
                "-c",
                &command,
            ])
            .status()
            .unwrap();
        assert!(status.success());

        for _ in 0..100 {
            if socket_path.exists() && shell_pid_file.exists() && child_pid_file.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let shell_pid: i32 = std::fs::read_to_string(&shell_pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let child_pid: i32 = std::fs::read_to_string(&child_pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();

        ResolvedBackend::Dtach.kill_session(&session_name);
        for _ in 0..100 {
            if !raw_process_is_alive(shell_pid) && !raw_process_is_alive(child_pid) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let shell_alive = raw_process_is_alive(shell_pid);
        let child_alive = raw_process_is_alive(child_pid);
        if shell_alive {
            // SAFETY: positive PID was written by this test-owned shell; kill(2)
            // takes no pointers and cleanup ignores a concurrent exit.
            unsafe { libc::kill(shell_pid, libc::SIGKILL) };
        }
        if child_alive {
            // SAFETY: positive PID was written by this test-owned child; kill(2)
            // takes no pointers and cleanup ignores a concurrent exit.
            unsafe { libc::kill(child_pid, libc::SIGKILL) };
        }
        let _ = std::fs::remove_file(&socket_path);
        let _ = std::fs::remove_dir_all(&temp_dir);

        assert!(!shell_alive, "dtach shell survived teardown");
        assert!(!child_alive, "dtach grandchild survived teardown");
    }

    #[cfg(unix)]
    #[test]
    fn too_fresh_to_gc_respects_min_age() {
        use std::time::Duration;
        assert!(is_too_fresh_to_gc(Duration::from_secs(0)));
        assert!(is_too_fresh_to_gc(Duration::from_secs(30)));
        // At or beyond the threshold the file is old enough to GC.
        assert!(!is_too_fresh_to_gc(DTACH_SOCKET_GC_MIN_AGE));
        assert!(!is_too_fresh_to_gc(Duration::from_secs(120)));
    }

    #[test]
    fn test_dtach_socket_path() {
        let backend = ResolvedBackend::Dtach;
        // socket_path expects terminal_id directly, not session_name
        let path = backend.socket_path("tm-12345678");
        assert!(path.is_some());
        let path = path.unwrap();
        assert!(path.to_string_lossy().contains("tm-12345678.sock"));

        // Non-dtach backends should return None
        let tmux_backend = ResolvedBackend::Tmux;
        assert!(tmux_backend.socket_path("tm-12345678").is_none());
    }

    #[test]
    fn test_dtach_build_command() {
        let backend = ResolvedBackend::Dtach;
        let result = backend.build_command("test-session", "/home/user", None, &[]);
        assert!(result.is_some());
        let (program, args) = result.unwrap();
        assert_eq!(program, "sh");
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-c");
        assert!(args[1].contains("dtach -A"));
        assert!(args[1].contains("-E -r winch"));
    }

    #[test]
    fn test_dtach_build_command_with_custom_command() {
        let backend = ResolvedBackend::Dtach;
        let result = backend.build_command("test-session", "/home/user", Some("npm run dev"), &[]);
        assert!(result.is_some());
        let (program, args) = result.unwrap();
        assert_eq!(program, "sh");
        assert_eq!(args[0], "-c");
        assert!(args[1].contains("dtach -A"));
        // Inner command uses the user's shell with -ic
        assert!(args[1].contains("-ic"));
        assert!(args[1].contains("npm run dev"));
    }

    #[test]
    fn test_tmux_build_command_with_custom_command() {
        let backend = ResolvedBackend::Tmux;
        let result = backend.build_command("test-session", "/home/user", Some("npm run dev"), &[]);
        assert!(result.is_some());
        let (program, args) = result.unwrap();
        assert_eq!(program, "sh");
        assert_eq!(args[0], "-c");
        assert!(args[1].contains("tmux new-session -A"));
        // Inner command uses the user's shell with -ic
        assert!(args[1].contains("'-ic'"));
        assert!(args[1].contains("npm run dev"));
    }

    #[test]
    fn test_tmux_build_command_without_command() {
        let backend = ResolvedBackend::Tmux;
        let result = backend.build_command("test-session", "/home/user", None, &[]);
        assert!(result.is_some());
        let (_, args) = result.unwrap();
        // Without a command, no '-ic' should appear after the cwd
        assert!(!args[1].contains("'-ic'"));
    }

    #[test]
    fn test_screen_build_command_with_custom_command() {
        let backend = ResolvedBackend::Screen;
        let result = backend.build_command("test-session", "/home/user", Some("npm run dev"), &[]);
        assert!(result.is_some());
        let (program, args) = result.unwrap();
        assert_eq!(program, "screen");
        assert_eq!(args[0], "-D");
        assert_eq!(args[1], "-R");
        assert_eq!(args[2], "test-session");
        // Inner command uses the user's shell with -ic
        assert_eq!(args[3], user_shell());
        assert_eq!(args[4], "-ic");
        assert_eq!(args[5], "npm run dev");
    }

    #[test]
    fn test_psmux_build_command_minimal() {
        let backend = ResolvedBackend::Psmux;
        // Use forward slashes so the test passes on Unix CI; on Windows in
        // production Path::file_name handles both separators identically.
        let result = backend.build_command("tm-12345678", "C:/projects/app", None, &[]);
        assert!(result.is_some());
        let (program, args) = result.unwrap();
        assert_eq!(program, "psmux");
        assert_eq!(args[0], "new-session");
        assert_eq!(args[1], "-A");
        // -A is followed directly by -s NAME -c CWD (no -e) when no extra_env
        assert_eq!(args[2], "-s");
        assert_eq!(args[3], "tm-12345678");
        assert_eq!(args[4], "-c");
        assert_eq!(args[5], "C:/projects/app");
        // No initial program, then ';' separators with set/rename commands
        assert!(args.contains(&";".to_string()));
        let semi_count = args.iter().filter(|a| a.as_str() == ";").count();
        assert_eq!(
            semi_count, 4,
            "expected four `;` separators (status, mouse, automatic-rename, rename-window)"
        );
        assert!(args.iter().any(|a| a == "rename-window"));
        assert_eq!(
            args.last().unwrap(),
            "app",
            "window name = last cwd segment"
        );
    }

    #[test]
    fn test_psmux_build_command_with_custom_command() {
        let backend = ResolvedBackend::Psmux;
        let result = backend.build_command("tm-test", "C:\\src", Some("npm run dev"), &[]);
        assert!(result.is_some());
        let (program, args) = result.unwrap();
        assert_eq!(program, "psmux");
        // custom command is wrapped via cmd.exe /c
        let cmd_pos = args
            .iter()
            .position(|a| a == "cmd.exe")
            .expect("cmd.exe in args");
        assert_eq!(args[cmd_pos + 1], "/c");
        assert_eq!(args[cmd_pos + 2], "npm run dev");
    }

    #[test]
    fn test_psmux_preserves_exact_custom_command_argv() {
        let backend = ResolvedBackend::Psmux;
        let command_args = vec![
            "/V:ON".to_string(),
            "/C".to_string(),
            "echo !ERRORLEVEL! & cmd /K".to_string(),
        ];
        let (_, args) = backend
            .build_command_with_custom(
                "tm-test",
                "C:\\src",
                Some(SessionCommand::Program {
                    program: "cmd.exe",
                    args: &command_args,
                }),
                &[],
            )
            .unwrap();

        let command_pos = args
            .iter()
            .position(|arg| arg == "cmd.exe")
            .expect("custom executable in psmux argv");
        assert_eq!(
            &args[command_pos..command_pos + 4],
            ["cmd.exe", "/V:ON", "/C", "echo !ERRORLEVEL! & cmd /K"]
        );
    }

    #[test]
    fn test_psmux_build_command_with_extra_env() {
        let backend = ResolvedBackend::Psmux;
        let env = vec![("CLAUDE_CONFIG_DIR".to_string(), Some("C:\\tmp".to_string()))];
        let (_, args) = backend
            .build_command("tm-test", "C:\\tmp", None, &env)
            .unwrap();
        // -e KEY=VAL must appear before -s so psmux applies it to the new session
        let e_pos = args.iter().position(|a| a == "-e").expect("-e in args");
        let s_pos = args.iter().position(|a| a == "-s").expect("-s in args");
        assert!(e_pos < s_pos, "expected -e before -s");
        assert_eq!(args[e_pos + 1], "CLAUDE_CONFIG_DIR=C:\\tmp");
    }

    #[test]
    fn test_psmux_socket_path_returns_none() {
        // psmux uses TCP IPC, not socket files
        assert!(ResolvedBackend::Psmux.socket_path("tm-anything").is_none());
    }

    #[test]
    fn test_psmux_supports_persistence() {
        assert!(ResolvedBackend::Psmux.supports_persistence());
    }

    #[test]
    fn test_none_build_command() {
        let backend = ResolvedBackend::None;
        assert!(
            backend
                .build_command("test-session", "/home/user", None, &[])
                .is_none()
        );
        assert!(
            backend
                .build_command("test-session", "/home/user", Some("echo hi"), &[])
                .is_none()
        );
    }

    #[test]
    fn test_tmux_build_command_with_extra_env() {
        let backend = ResolvedBackend::Tmux;
        let env = vec![(
            "CLAUDE_CONFIG_DIR".to_string(),
            Some("/tmp/foo".to_string()),
        )];
        let (_, args) = backend
            .build_command("tm-test", "/tmp", None, &env)
            .unwrap();
        // -e KEY=VAL must appear before -s so tmux applies it to the new session
        assert!(
            args[1].contains("-e 'CLAUDE_CONFIG_DIR=/tmp/foo'"),
            "expected -e flag, got: {}",
            args[1]
        );
        let env_pos = args[1].find("-e ").unwrap();
        let s_pos = args[1].find("-s ").unwrap();
        assert!(env_pos < s_pos, "expected -e before -s in: {}", args[1]);
    }

    #[test]
    fn test_tmux_build_command_unsets_env() {
        let backend = ResolvedBackend::Tmux;
        let env = vec![("CLAUDE_CONFIG_DIR".to_string(), None)];
        let (_, args) = backend
            .build_command("tm-test", "/tmp", None, &env)
            .unwrap();
        // A removal has no `-e` flag but clears the var from the session env.
        assert!(
            !args[1].contains("-e "),
            "removal must not emit an -e flag, got: {}",
            args[1]
        );
        assert!(
            args[1].contains("set-environment -u 'CLAUDE_CONFIG_DIR'"),
            "expected session-env unset, got: {}",
            args[1]
        );
    }

    #[test]
    #[cfg(windows)]
    fn test_build_wsl_session_command_dtach() {
        let backend = ResolvedBackend::Dtach;
        let result = backend.build_wsl_session_command(
            Some("Ubuntu"),
            "tm-12345678",
            "/home/user/project",
            None,
            &[],
        );
        assert!(result.is_some());
        let (program, args) = result.unwrap();
        assert_eq!(program, "wsl.exe");
        assert!(args.contains(&"-d".to_string()));
        assert!(args.contains(&"Ubuntu".to_string()));
        assert!(args.contains(&"--".to_string()));
        assert!(args.contains(&"sh".to_string()));
        assert!(args.contains(&"-c".to_string()));
        // The inner command should contain dtach with WSL-native socket path
        let inner_cmd = args.last().unwrap();
        assert!(inner_cmd.contains("dtach -A"), "inner cmd: {}", inner_cmd);
        assert!(
            inner_cmd.contains("-E -r winch"),
            "inner cmd: {}",
            inner_cmd
        );
        // Must use WSL-native socket path, not Windows temp dir
        assert!(
            inner_cmd.contains("/tmp/okena-dtach/"),
            "socket path should be WSL-native: {}",
            inner_cmd
        );
        // Must use $SHELL (resolved inside WSL), not /bin/sh
        assert!(
            inner_cmd.contains("\"$SHELL\""),
            "should use $SHELL not /bin/sh: {}",
            inner_cmd
        );
        assert!(
            !inner_cmd.contains("/bin/sh"),
            "should not contain /bin/sh: {}",
            inner_cmd
        );
    }

    #[test]
    #[cfg(windows)]
    fn wsl_dtach_requires_teardown_dependencies() {
        let resolved = resolve_wsl_backend_with_probe(SessionBackend::Dtach, |tool| {
            matches!(tool, "dtach" | "xargs")
        });

        assert_eq!(resolved, ResolvedBackend::None);
    }

    #[test]
    #[cfg(windows)]
    fn wsl_auto_skips_unteardownable_dtach() {
        let resolved = resolve_wsl_backend_with_probe(SessionBackend::Auto, |tool| {
            matches!(tool, "dtach" | "tmux" | "xargs")
        });

        assert_eq!(resolved, ResolvedBackend::Tmux);
    }

    #[test]
    #[cfg(windows)]
    fn test_build_wsl_session_command_tmux() {
        let backend = ResolvedBackend::Tmux;
        let result = backend.build_wsl_session_command(
            Some("Ubuntu"),
            "tm-12345678",
            "/home/user/project",
            None,
            &[],
        );
        assert!(result.is_some());
        let (program, args) = result.unwrap();
        assert_eq!(program, "wsl.exe");
        let inner_cmd = args.last().unwrap();
        assert!(
            inner_cmd.contains("tmux new-session -A"),
            "inner cmd: {}",
            inner_cmd
        );
        assert!(
            inner_cmd.contains("set status off"),
            "inner cmd: {}",
            inner_cmd
        );
    }

    #[test]
    #[cfg(windows)]
    fn wsl_session_environment_stays_in_argv() {
        let backend = ResolvedBackend::Tmux;
        let hostile = "quote\" & %PATH% !bang!\r\nnext".to_string();
        let (_, args) = backend
            .build_wsl_session_command(
                Some("Ubuntu"),
                "tm-12345678",
                "/home/user/project",
                None,
                &[("OKENA_PROJECT_NAME".to_string(), Some(hostile.clone()))],
            )
            .expect("WSL command");

        let env_index = args.iter().position(|arg| arg == "env").expect("env argv");
        assert_eq!(
            args.get(env_index + 1),
            Some(&format!("OKENA_PROJECT_NAME={hostile}"))
        );
    }

    #[test]
    #[cfg(windows)]
    fn test_build_wsl_session_command_preserves_custom_argv() {
        let backend = ResolvedBackend::Tmux;
        let command_args = vec!["-lc".to_string(), "printf '%s' \"hello world\"".to_string()];
        let (_, args) = backend
            .build_wsl_session_command(
                Some("Ubuntu"),
                "tm-12345678",
                "/home/user/project",
                Some(SessionCommand::Program {
                    program: "/bin/bash",
                    args: &command_args,
                }),
                &[],
            )
            .unwrap();

        let inner_cmd = args.last().unwrap();
        assert!(
            inner_cmd.contains("'/bin/bash' '-lc' 'printf '\\''%s'\\'' \"hello world\"'"),
            "inner cmd: {inner_cmd}"
        );
    }

    #[test]
    #[cfg(windows)]
    fn test_build_wsl_session_command_none() {
        let backend = ResolvedBackend::None;
        let result = backend.build_wsl_session_command(
            Some("Ubuntu"),
            "tm-12345678",
            "/home/user/project",
            None,
            &[],
        );
        assert!(result.is_none());
    }

    #[test]
    #[cfg(windows)]
    fn test_build_wsl_session_command_default_distro() {
        let backend = ResolvedBackend::Tmux;
        let result = backend.build_wsl_session_command(
            None, // default distro
            "tm-12345678",
            "/home/user/project",
            None,
            &[],
        );
        assert!(result.is_some());
        let (program, args) = result.unwrap();
        assert_eq!(program, "wsl.exe");
        // Should NOT contain -d flag when distro is None
        assert!(!args.contains(&"-d".to_string()));
        assert!(args.contains(&"--".to_string()));
    }
}
