use crate::terminal::TerminalTransport;
use crate::pty_manager::PtyManager;
use crate::shell_config::ShellType;
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Terminal lifecycle management trait.
/// Used by TerminalPane and LayoutContainer.
pub trait TerminalBackend: Send + Sync {
    fn transport(&self) -> Arc<dyn TerminalTransport>;
    fn create_terminal(&self, cwd: &str, shell: Option<&ShellType>) -> Result<String>;
    fn reconnect_terminal(&self, terminal_id: &str, cwd: &str, shell: Option<&ShellType>) -> Result<String>;
    fn kill(&self, terminal_id: &str);
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

    fn reconnect_terminal(&self, terminal_id: &str, cwd: &str, shell: Option<&ShellType>) -> Result<String> {
        self.pty_manager.create_or_reconnect_terminal_with_shell(Some(terminal_id), cwd, shell)
    }

    fn kill(&self, terminal_id: &str) {
        self.pty_manager.kill(terminal_id)
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
