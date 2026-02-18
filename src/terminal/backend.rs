use crate::terminal::terminal::TerminalTransport;
use crate::terminal::pty_manager::PtyManager;
use crate::terminal::shell_config::ShellType;
use anyhow::Result;
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
}
