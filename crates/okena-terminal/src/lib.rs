#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

pub mod backend;
/// Ownership guards for the shared dtach session pool.
pub mod dtach_pool;
pub mod input;
/// macOS process introspection via libproc (replaces `pgrep`/`lsof`/`ps`).
#[cfg(target_os = "macos")]
pub mod macos_proc;
pub mod process;
pub mod pty_manager;
mod pty_write_queue;
pub mod session_backend;
pub mod shell_config;
pub mod terminal;
/// Per-pane pointer to the pane's current slave pty (`$OKENA_TTY_FILE`).
#[cfg(unix)]
pub mod tty_pointer;

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

/// Shared terminals registry for PTY event routing.
/// Maps terminal ID → Terminal instance.
pub type TerminalsRegistry = Arc<Mutex<HashMap<String, Arc<terminal::Terminal>>>>;

/// Remove `terminal` from the registry unless another model has replaced it
/// under the same id since.
pub fn release_terminal(
    terminals: &TerminalsRegistry,
    terminal_id: &str,
    terminal: &Arc<terminal::Terminal>,
) {
    let mut registry = terminals.lock();
    if registry
        .get(terminal_id)
        .is_some_and(|registered| Arc::ptr_eq(registered, terminal))
    {
        registry.remove(terminal_id);
    }
}
