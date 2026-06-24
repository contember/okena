//! Shared daemon state: the GPUI-free analogue of the app's entity graph.
//!
//! The GUI keeps `Workspace` and `ServiceManager` as GPUI entities and reacts to
//! their `notify` via observers. The daemon instead holds them behind
//! `Arc<parking_lot::Mutex<…>>` and turns each `notify` into a `watch` channel
//! bump, so future observer tasks can `await` a change and re-derive state /
//! broadcast it over the protocol.
//!
//! Three independent `watch` channels mirror the three notify surfaces:
//! - [`state_version`](DaemonReactor::state_version) — coarse "something
//!   persistent changed" tick used by the autosave / snapshot observer.
//! - [`workspace_tick`](DaemonReactor::workspace_tick) — bumped by
//!   [`WorkspaceCx::notify`](okena_workspace::context::WorkspaceCx::notify).
//! - [`service_tick`](DaemonReactor::service_tick) — bumped by the service
//!   reactor's `notify`.

use std::sync::Arc;

use okena_hooks::{HookMonitor, HookRunner};
use okena_services::manager::ServiceManager;
use okena_terminal::backend::TerminalBackend;
use okena_terminal::TerminalsRegistry;
use okena_workspace::state::Workspace;
use parking_lot::Mutex;
use tokio::runtime::Handle;
use tokio::sync::watch;

/// The shared, GPUI-free daemon state driven by the tokio reactor.
///
/// Cheaply clonable bits (the `Arc<Mutex<…>>` handles, the `watch::Sender`s, the
/// tokio [`Handle`], the `Arc`-backed hook services) are what the trait impls
/// capture so spawned tasks can re-enter the managers and signal changes.
pub struct DaemonReactor {
    /// The workspace state, shared with reactor contexts. `Workspace::new` is
    /// gpui-free, so this is a plain mutex rather than a GPUI entity.
    pub workspace: Arc<Mutex<Workspace>>,

    /// The service manager state, shared with reactor contexts.
    pub service_manager: Arc<Mutex<ServiceManager>>,

    /// Coarse "persistent state changed" tick (autosave / snapshot observer).
    pub state_version: watch::Sender<u64>,

    /// Bumped on every `WorkspaceCx::notify`.
    pub workspace_tick: watch::Sender<u64>,

    /// Bumped on every service-reactor `notify`.
    pub service_tick: watch::Sender<u64>,

    /// Hook runner (creates PTY-backed hook terminals), if configured.
    pub hook_runner: Option<HookRunner>,

    /// Hook monitor (tracks in-flight/completed hook runs), if configured.
    pub hook_monitor: Option<HookMonitor>,

    /// Handle to the multi-thread tokio runtime the reactor tasks spawn onto.
    pub runtime: Handle,
}

impl DaemonReactor {
    /// Build the shared daemon state.
    ///
    /// The terminal backend + registry are the same dependencies the GUI hands
    /// to `ServiceManager::new` / `HookRunner::new`; the daemon's bootstrap owns
    /// them and passes them in. The three `watch` channels start at `0`.
    pub fn new(
        workspace: Workspace,
        backend: Arc<dyn TerminalBackend>,
        terminals: TerminalsRegistry,
        hook_runner: Option<HookRunner>,
        hook_monitor: Option<HookMonitor>,
        runtime: Handle,
    ) -> Self {
        let service_manager = ServiceManager::new(backend, terminals);
        Self {
            workspace: Arc::new(Mutex::new(workspace)),
            service_manager: Arc::new(Mutex::new(service_manager)),
            state_version: watch::Sender::new(0),
            workspace_tick: watch::Sender::new(0),
            service_tick: watch::Sender::new(0),
            hook_runner,
            hook_monitor,
            runtime,
        }
    }
}
