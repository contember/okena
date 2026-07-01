//! GPUI-free [`WorkspaceCx`] implementer.
//!
//! The GUI satisfies `WorkspaceCx` with `gpui::Context<'_, Workspace>`; the
//! daemon satisfies it with [`DaemonWorkspaceCx`], a thin borrow of the
//! reactor's notify channel + hook services. It is constructed around a
//! `&mut Workspace` mutation site (e.g. just after locking
//! [`DaemonReactor::workspace`](crate::reactor::DaemonReactor::workspace)) so the
//! workspace action methods — which take `cx: &mut impl WorkspaceCx` — run
//! unchanged.

use okena_hooks::{HookMonitor, HookRunner};
use okena_workspace::context::WorkspaceCx;
use tokio::sync::watch;

/// Daemon-side [`WorkspaceCx`]: `notify` bumps the workspace tick, `refresh_views`
/// is a no-op (no local views), and the hook accessors clone the held services.
///
/// Borrows the reactor's `workspace_tick` sender and hook-service options for the
/// duration of a single mutation, so it never holds the `Workspace` mutex lock
/// across the borrow.
pub struct DaemonWorkspaceCx<'a> {
    workspace_tick: &'a watch::Sender<u64>,
    hook_runner: &'a Option<HookRunner>,
    hook_monitor: &'a Option<HookMonitor>,
}

impl<'a> DaemonWorkspaceCx<'a> {
    /// Construct a context from the reactor's notify channel + hook services.
    pub fn new(
        workspace_tick: &'a watch::Sender<u64>,
        hook_runner: &'a Option<HookRunner>,
        hook_monitor: &'a Option<HookMonitor>,
    ) -> Self {
        Self {
            workspace_tick,
            hook_runner,
            hook_monitor,
        }
    }
}

impl WorkspaceCx for DaemonWorkspaceCx<'_> {
    fn notify(&mut self) {
        // Bump the tick; observer tasks `await` the change. `send_modify` always
        // notifies receivers even if no value comparison would (it can't return
        // an error — there is always the internal receiver), matching GPUI's
        // unconditional `Context::notify`.
        self.workspace_tick.send_modify(|v| *v += 1);
    }

    fn refresh_views(&mut self) {
        // No local views in the daemon — nothing to invalidate.
    }

    fn hook_runner(&self) -> Option<HookRunner> {
        self.hook_runner.clone()
    }

    fn hook_monitor(&self) -> Option<HookMonitor> {
        self.hook_monitor.clone()
    }
}
