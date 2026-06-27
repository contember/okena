//! Daemon-side grace-period finalizer. The command loop ejects a busy terminal
//! and records a deadline here; this loop kills the PTY once the grace elapses.
//! Undo / Close-now (handled in the command loop) remove the deadline first.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::sync::watch;

use okena_hooks::{HookMonitor, HookRunner};
use okena_terminal::backend::TerminalBackend;
use okena_terminal::TerminalsRegistry;
use okena_workspace::state::Workspace;

use crate::workspace_cx::DaemonWorkspaceCx;

/// Shared `terminal_id -> grace deadline` map for in-flight soft-closes.
pub type SoftCloseDeadlines = Arc<Mutex<HashMap<String, Instant>>>;

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

        // Collect + remove expired ids under the deadline lock only.
        let expired: Vec<String> = {
            let now = Instant::now();
            let mut d = deadlines.lock();
            let exp: Vec<String> =
                d.iter().filter(|(_, dl)| **dl <= now).map(|(t, _)| t.clone()).collect();
            for t in &exp {
                d.remove(t);
            }
            exp
        };
        if expired.is_empty() {
            continue;
        }

        // Finalize on the workspace (queues kills), then drain the kill queue.
        let kills = {
            let mut cx = DaemonWorkspaceCx::new(&workspace_tick, &hook_runner, &hook_monitor);
            let mut ws = workspace.lock();
            for tid in &expired {
                ws.finalize_soft_close(tid, &mut cx);
            }
            ws.drain_pending_terminal_kills()
        };
        for id in kills {
            backend.kill(&id);
            terminals.lock().remove(&id);
        }
    }
}
