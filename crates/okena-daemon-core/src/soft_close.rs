//! Daemon-side grace-period finalizer. The command loop ejects a busy terminal
//! and records a deadline; this loop kills the PTY once the grace elapses.
//! Undo / Close-now (handled in the command loop) remove the deadline first.
//!
//! The finalize logic itself lives in the shared, runtime-agnostic engine
//! ([`okena_workspace::actions::soft_close`]); this module only owns the tokio
//! timer that ticks it. The headless loop drives the same engine off a gpui
//! timer instead.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::watch;

use okena_hooks::{HookMonitor, HookRunner};
use okena_terminal::backend::TerminalBackend;
use okena_terminal::TerminalsRegistry;
use okena_workspace::actions::soft_close::finalize_expired;
use okena_workspace::state::Workspace;

use crate::workspace_cx::DaemonWorkspaceCx;

/// Shared `terminal_id -> grace deadline` map for in-flight soft-closes.
///
/// Re-exported from the shared engine so daemon-core callers (`daemon.rs`) stay
/// unchanged now that the type lives in `okena-workspace`.
pub use okena_workspace::actions::soft_close::SoftCloseDeadlines;

const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Periodically finalize soft-closes whose grace period elapsed: drop the
/// pending record (workspace), then kill the PTY + drop it from the registry.
/// The client toast TTLs out on its own.
pub async fn run_soft_close_poll(
    workspace: Arc<Mutex<Workspace>>,
    backend: Arc<dyn TerminalBackend>,
    terminals: TerminalsRegistry,
    workspace_tick: watch::Sender<u64>,
    hook_runner: Option<HookRunner>,
    hook_monitor: Option<HookMonitor>,
    deadlines: SoftCloseDeadlines,
) {
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;

        let mut cx = DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);
        let mut ws = workspace.lock();
        finalize_expired(&deadlines, &mut ws, &*backend, &terminals, &mut cx);
    }
}
