use crate::pty_manager::PtyManager;
#[cfg(windows)]
use crate::session_backend::ResolvedBackend;
use crate::session_backend::SessionBackend;
use crate::shell_config::ShellType;
use crate::terminal::TerminalTransport;
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Exact startup command carried separately from the shell used to route it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalLaunchCommand {
    pub program: String,
    pub args: Vec<String>,
}

/// Transient launch data; only `route` comes from persisted workspace state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalLaunchPlan {
    pub route: ShellType,
    pub initial_command: Option<TerminalLaunchCommand>,
    /// Environment owned by this launch, kept out of shell command strings.
    pub environment: Vec<(String, String)>,
}

impl TerminalLaunchPlan {
    pub fn for_shell(route: ShellType) -> Self {
        Self {
            route,
            initial_command: None,
            environment: Vec::new(),
        }
    }

    pub fn with_environment(mut self, mut environment: Vec<(String, String)>) -> Self {
        environment.sort_by(|a, b| a.0.cmp(&b.0));
        self.environment = environment;
        self
    }

    fn legacy_shell(&self) -> ShellType {
        self.initial_command.as_ref().map_or_else(
            || self.route.clone(),
            |command| ShellType::Custom {
                path: command.program.clone(),
                args: command.args.clone(),
            },
        )
    }
}

/// Route needed to remove a persistent session when no live PTY handle exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalTeardownRoute {
    Host,
    #[cfg(windows)]
    Wsl {
        distro: Option<String>,
        backend: ResolvedBackend,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalSessionTeardown {
    pub terminal_id: String,
    pub route: TerminalTeardownRoute,
}

impl TerminalSessionTeardown {
    pub fn host(terminal_id: String) -> Self {
        Self {
            terminal_id,
            route: TerminalTeardownRoute::Host,
        }
    }
}

impl PartialEq<String> for TerminalSessionTeardown {
    fn eq(&self, other: &String) -> bool {
        self.terminal_id == *other
    }
}

/// Terminal lifecycle management trait.
/// Used by TerminalPane and LayoutContainer.
pub trait TerminalBackend: Send + Sync {
    fn transport(&self) -> Arc<dyn TerminalTransport>;
    fn create_terminal(&self, cwd: &str, shell: Option<&ShellType>) -> Result<String>;
    fn create_terminal_with_plan(&self, cwd: &str, plan: &TerminalLaunchPlan) -> Result<String> {
        let shell = plan.legacy_shell();
        self.create_terminal(cwd, Some(&shell))
    }
    fn reconnect_terminal(
        &self,
        terminal_id: &str,
        cwd: &str,
        shell: Option<&ShellType>,
    ) -> Result<String>;
    fn reconnect_terminal_with_plan(
        &self,
        terminal_id: &str,
        cwd: &str,
        plan: &TerminalLaunchPlan,
    ) -> Result<String> {
        let shell = plan.legacy_shell();
        self.reconnect_terminal(terminal_id, cwd, Some(&shell))
    }
    fn kill(&self, terminal_id: &str);
    fn kill_session(&self, teardown: &TerminalSessionTeardown) {
        self.kill(&teardown.terminal_id);
    }
    /// Wait for teardown work queued before this call to finish.
    fn flush_teardown(&self) {}
    /// Whether this backend can switch persistence routes without replacement.
    fn supports_session_backend_reconfiguration(&self) -> bool {
        false
    }
    /// Verify that the old persistence route no longer owns live work.
    fn ensure_session_backend_reconfigurable(&self) -> Result<()> {
        anyhow::bail!("terminal backend does not support live session route changes")
    }
    /// Switch the live persistence route after old teardown and settings commit.
    fn apply_session_backend(&self, _backend: SessionBackend) -> Result<()> {
        anyhow::bail!("terminal backend does not support live session route changes")
    }
    fn capture_buffer(&self, terminal_id: &str) -> Option<PathBuf>;
    fn supports_buffer_capture(&self) -> bool;
    fn is_remote(&self) -> bool;
    fn get_shell_pid(&self, terminal_id: &str) -> Option<u32>;
    /// Get the real foreground shell pid. With session backends this walks
    /// through dtach / tmux proxies to return the actual shell process; for
    /// plain PTYs it is the same as `get_shell_pid`. Callers inspecting
    /// running children (e.g. for the click-to-cursor guard) should use this.
    fn get_foreground_shell_pid(&self, terminal_id: &str) -> Option<u32> {
        self.get_shell_pid(terminal_id)
    }
    /// Get root PIDs for port detection. With session backends (dtach/tmux),
    /// this returns the daemon/pane PID instead of the attach client PID.
    fn get_service_pids(&self, terminal_id: &str) -> Vec<u32>;
    /// Batch version of `get_service_pids` — returns root PIDs for multiple terminals at once.
    /// On Linux with dtach, this reads `/proc` once instead of spawning `lsof` per terminal.
    fn get_batch_service_pids(&self, terminal_ids: &[&str]) -> HashMap<String, Vec<u32>> {
        terminal_ids
            .iter()
            .map(|tid| (tid.to_string(), self.get_service_pids(tid)))
            .collect()
    }
}

/// Local backend wrapping PtyManager for local terminal processes.
pub struct LocalBackend {
    pty_manager: Arc<PtyManager>,
}

impl LocalBackend {
    pub fn new(pty_manager: Arc<PtyManager>) -> Self {
        Self { pty_manager }
    }
}

impl TerminalBackend for LocalBackend {
    fn transport(&self) -> Arc<dyn TerminalTransport> {
        self.pty_manager.clone()
    }

    fn create_terminal(&self, cwd: &str, shell: Option<&ShellType>) -> Result<String> {
        self.pty_manager.create_terminal_with_shell(cwd, shell)
    }

    fn create_terminal_with_plan(&self, cwd: &str, plan: &TerminalLaunchPlan) -> Result<String> {
        self.pty_manager.create_terminal_with_plan(cwd, plan)
    }

    fn reconnect_terminal(
        &self,
        terminal_id: &str,
        cwd: &str,
        shell: Option<&ShellType>,
    ) -> Result<String> {
        self.pty_manager
            .create_or_reconnect_terminal_with_shell(Some(terminal_id), cwd, shell)
    }

    fn reconnect_terminal_with_plan(
        &self,
        terminal_id: &str,
        cwd: &str,
        plan: &TerminalLaunchPlan,
    ) -> Result<String> {
        self.pty_manager
            .create_or_reconnect_terminal_with_plan(Some(terminal_id), cwd, plan)
    }

    fn kill(&self, terminal_id: &str) {
        self.pty_manager.kill(terminal_id)
    }

    fn kill_session(&self, teardown: &TerminalSessionTeardown) {
        self.pty_manager.kill_session(teardown)
    }

    fn flush_teardown(&self) {
        self.pty_manager.flush_teardown()
    }

    fn supports_session_backend_reconfiguration(&self) -> bool {
        true
    }

    fn ensure_session_backend_reconfigurable(&self) -> Result<()> {
        self.pty_manager.ensure_session_backend_reconfigurable()
    }

    fn apply_session_backend(&self, backend: SessionBackend) -> Result<()> {
        self.pty_manager.apply_session_backend(backend);
        Ok(())
    }

    fn capture_buffer(&self, terminal_id: &str) -> Option<PathBuf> {
        self.pty_manager.capture_buffer(terminal_id)
    }

    fn supports_buffer_capture(&self) -> bool {
        self.pty_manager.supports_buffer_capture()
    }

    fn is_remote(&self) -> bool {
        false
    }

    fn get_shell_pid(&self, terminal_id: &str) -> Option<u32> {
        self.pty_manager.get_shell_pid(terminal_id)
    }

    fn get_foreground_shell_pid(&self, terminal_id: &str) -> Option<u32> {
        self.pty_manager.get_foreground_shell_pid(terminal_id)
    }

    fn get_service_pids(&self, terminal_id: &str) -> Vec<u32> {
        self.pty_manager.get_service_pids(terminal_id)
    }

    fn get_batch_service_pids(&self, terminal_ids: &[&str]) -> HashMap<String, Vec<u32>> {
        self.pty_manager.get_batch_service_pids(terminal_ids)
    }
}
