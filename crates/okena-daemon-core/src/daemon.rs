//! Final daemon assembly: the GPUI-free analogue of `okena-app`'s
//! [`HeadlessApp`](okena_app::app::headless::HeadlessApp).
//!
//! [`HeadlessApp`] wires the headless GUI mode: it stands up the workspace
//! entity, PTY manager, service manager, git watcher, the remote command bridge,
//! and the [`RemoteServer`](okena_remote_server::server::RemoteServer), all on
//! GPUI's main-thread reactor. [`DaemonCore`] does the exact same wiring with no
//! GPUI in scope: the shared state lives behind `Arc<parking_lot::Mutex<…>>` in
//! [`DaemonReactor`](crate::reactor::DaemonReactor), the `cx.observe` closures
//! become the `watch`-channel-driven observer tasks
//! ([`spawn_observers`](crate::reactor::DaemonReactor::spawn_observers)), and the
//! GPUI command loop becomes [`daemon_command_loop`](crate::command_loop).
//!
//! [`DaemonCore::new`] builds everything and starts the remote server (so its
//! port + pairing info are printed before [`run`](DaemonCore::run) blocks);
//! [`DaemonCore::run`] drives the reactor tasks on a
//! [`LocalSet`](tokio::task::LocalSet) until the bridge closes or the process
//! receives ctrl-c.
//!
//! ## Why a `LocalSet`
//!
//! The reactor tasks ([`spawn_observers`](crate::reactor::DaemonReactor::spawn_observers),
//! [`run_pty_loop`](crate::pty_loop::run_pty_loop), the service manager's
//! `spawn_main` restarts, and the service arms of
//! [`daemon_command_loop`](crate::command_loop::daemon_command_loop)) use
//! `tokio::task::spawn_local`, which requires a running `LocalSet`. They are
//! therefore spawned from inside [`LocalSet::block_on`](tokio::task::LocalSet::block_on)
//! on the multi-thread runtime; the blocking subprocess offloads still reach the
//! multi-thread pool via the held [`Handle`](tokio::runtime::Handle).
//!
//! ## Lifecycle
//!
//! The daemon is UI-owned: the spawning UI starts it and kills it. So
//! [`run`](DaemonCore::run) blocking until the bridge closes (the remote server
//! is gone) or ctrl-c arrives is the intended behavior — there is no other
//! shutdown surface.
//!
//! ## Testing
//!
//! This type is integration-verified via the `okena-daemon` binary (the next
//! step), not unit tests: [`new`](DaemonCore::new) binds a real TCP port and
//! writes `remote.json` to the real config dir, which would be flaky and racy
//! with any other running instance. The wired-together pieces each have their
//! own unit tests in their respective modules.
//!
//! ## Lifecycle hooks
//!
//! The reactor is built with a real `HookRunner` / `HookMonitor` (constructed
//! from the daemon's terminal backend + registry). The action layer reaches
//! them through `WorkspaceCx::{hook_runner,hook_monitor}`, so project/worktree
//! lifecycle hooks fire in the daemon and their PTYs reach clients over the
//! normal remote terminal path. (Surfacing the `HookMonitor`'s in-flight/run
//! status into `StateResponse` for a client-side hooks panel is a follow-up.)

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use async_channel::Receiver;
use okena_core::api::ApiGitStatus;
use okena_hooks::{HookMonitor, HookRunner};
use okena_terminal::backend::{LocalBackend, TerminalBackend};
use okena_terminal::pty_manager::{PtyEvent, PtyManager};
use okena_terminal::session_backend::SessionBackend;
use okena_terminal::TerminalsRegistry;
use okena_remote_server::auth::AuthStore;
use okena_remote_server::bridge::{self, BridgeReceiver};
use okena_remote_server::pty_broadcaster::PtyBroadcaster;
use okena_remote_server::server::RemoteServer;
use okena_workspace::persistence::{acquire_instance_lock, AppSettings, LockGuard};
use okena_workspace::state::{Workspace, WorkspaceData};
use parking_lot::Mutex;
use tokio::sync::watch;

use crate::daemon_config::DaemonConfig;
use crate::reactor::DaemonReactor;

/// Inputs needed to construct a daemon.
pub struct DaemonParams {
    /// The persisted workspace state to drive (projects, layouts, windows).
    pub workspace_data: WorkspaceData,
    /// The app settings (font / theme / shell / session backend), loaded once at
    /// startup and shared with [`DaemonConfig`] as the settings write path.
    pub settings: AppSettings,
    /// The session backend (tmux / dtach / screen / none) the PTY manager uses
    /// to spawn terminals.
    pub session_backend: SessionBackend,
    /// The address the remote server binds to (loopback for local-only).
    pub listen_addr: IpAddr,
    /// Whether the remote server should serve TLS (dual-stack http+https).
    pub tls_enabled: bool,
}

/// The assembled, GPUI-free daemon: owns the tokio runtime, the shared reactor
/// state, the running remote server, and the channels the reactor tasks use.
///
/// Built by [`new`](DaemonCore::new); driven by [`run`](DaemonCore::run). See the
/// module docs for the lifecycle and the `LocalSet` requirement.
pub struct DaemonCore {
    /// The multi-thread tokio runtime the reactor tasks run on (via a
    /// `LocalSet` in [`run`](DaemonCore::run)).
    runtime: tokio::runtime::Runtime,
    /// Shared, GPUI-free daemon state (workspace + service manager + ticks).
    reactor: Arc<DaemonReactor>,
    /// The running remote server. Kept alive for the daemon's lifetime; dropping
    /// it stops the server and removes `remote.json`.
    remote_server: RemoteServer,
    /// Receiving end of the command bridge — the remote server sends commands,
    /// the command loop consumes them.
    bridge_rx: BridgeReceiver,
    /// Terminal backend over the PTY manager, threaded into the command loop's
    /// `execute_action` / `ensure_terminal`.
    backend: Arc<dyn TerminalBackend>,
    /// Shared terminal registry: PTY `Data` events route into it, the command
    /// loop reads sizes / snapshots from it.
    terminals: TerminalsRegistry,
    /// The PTY manager, for `cleanup_exited` / `kill` in the PTY loop.
    pty_manager: Arc<PtyManager>,
    /// PTY event receiver, drained by [`run_pty_loop`](crate::pty_loop::run_pty_loop).
    pty_events: Receiver<PtyEvent>,
    /// Server-readable view of the reactor's `state_version` (shared channel —
    /// see [`new`](DaemonCore::new)).
    state_version: Arc<watch::Sender<u64>>,
    /// Git-status channel the poll loop publishes into and the server broadcasts.
    git_status_tx: Arc<watch::Sender<HashMap<String, ApiGitStatus>>>,
    /// Client terminal subscriptions (connection id -> subscribed terminal ids),
    /// shared with the remote server. The git poll reads it to fan out the
    /// expensive `gh` PR/CI lookups only for projects a client is viewing.
    remote_subscribed_terminals:
        Arc<std::sync::RwLock<HashMap<u64, std::collections::HashSet<String>>>>,
    /// Shared settings cell (the [`DaemonConfig`] write path; also read by the
    /// command loop's `execute_action` for hooks / worktree / default shell).
    settings: Arc<Mutex<AppSettings>>,
    /// GPUI-free settings/theme handler for the app-scoped remote actions.
    daemon_config: DaemonConfig,
    /// Single-writer instance lock (§5). The daemon is the sole owner of the
    /// profile's `workspace.json` + lock; held for the daemon's lifetime so a
    /// second instance (or a classic in-process GUI) cannot clobber the profile.
    /// Released on drop at the end of [`run`](DaemonCore::run).
    _instance_lock: LockGuard,
}

impl DaemonCore {
    /// Build the daemon and start its remote server.
    ///
    /// Mirrors [`HeadlessApp::new`](okena_app::app::headless::HeadlessApp) with no
    /// GPUI: stands up the PTY manager + broadcaster, the terminal registry +
    /// backend, the workspace + reactor, the settings + config, the server wiring
    /// channels, then starts the [`RemoteServer`] and prints its pairing info.
    /// The reactor tasks are NOT started here — that is [`run`](DaemonCore::run)'s
    /// job (they need a `LocalSet`).
    pub fn new(params: DaemonParams) -> anyhow::Result<Self> {
        // ── 0. Acquire the single-writer instance lock FIRST ─────────────────
        // §5: exactly one process owns the profile's persistence + lock. The
        // daemon is that process; the `--daemon-client` GUI deliberately skips
        // the lock. Acquire before binding a port / writing `remote.json` so a
        // collision fails fast with no side effects. Held for the daemon's
        // lifetime (dropped at the end of `run`).
        let instance_lock = acquire_instance_lock()?;

        // ── 1. Multi-thread tokio runtime backing the reactor ────────────────
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("okena-daemon")
            .build()?;
        let handle = runtime.handle().clone();

        // ── 2. PTY manager + broadcaster + registry + backend ────────────────
        let (pty_manager, pty_events) = PtyManager::new(params.session_backend);
        let pty_manager = Arc::new(pty_manager);
        let broadcaster = Arc::new(PtyBroadcaster::new());
        pty_manager.set_output_sink(broadcaster.clone());
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(HashMap::new()));
        let backend: Arc<dyn TerminalBackend> = Arc::new(LocalBackend::new(pty_manager.clone()));

        // ── 3. Workspace + reactor ───────────────────────────────────────────
        let workspace = Workspace::new(params.workspace_data);
        // Lifecycle hooks: construct the same services the GUI sets as globals
        // (`HookRunner::new(backend, terminals)` in app/mod.rs, `HookMonitor::new()`
        // in main.rs). The action layer already reaches them through
        // `WorkspaceCx::{hook_runner,hook_monitor}` (the daemon's
        // `DaemonWorkspaceCx` returns these), and hook PTYs register in the same
        // `terminals` registry + broadcast over the same `PtyBroadcaster`, so
        // hook terminals reach clients via the normal remote terminal path. Both
        // ctors are gpui-free (okena-hooks built without the gpui feature here).
        let hook_runner = HookRunner::new(backend.clone(), terminals.clone());
        let hook_monitor = HookMonitor::new();
        let reactor = Arc::new(DaemonReactor::new(
            workspace,
            backend.clone(),
            terminals.clone(),
            Some(hook_runner),
            Some(hook_monitor),
            handle.clone(),
        ));

        // ── 4. Settings + config ─────────────────────────────────────────────
        let settings = Arc::new(Mutex::new(params.settings));
        let daemon_config = DaemonConfig::new(settings.clone());

        // ── 5. Server wiring channels ────────────────────────────────────────
        // Shared-watch trick: `tokio::sync::watch::Sender` is `Clone` and clones
        // share one underlying channel. The server + command loop READ this
        // `state_version`; the reactor's observers / PTY loop / git poll BUMP
        // `reactor.state_version` (the same channel), so reads observe the bumps.
        let state_version = Arc::new(reactor.state_version.clone());
        let git_status_tx = Arc::new(watch::Sender::new(HashMap::new()));
        let auth_store = Arc::new(AuthStore::new());
        let remote_subscribed_terminals = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let next_connection_id = Arc::new(AtomicU64::new(0));
        let (bridge_tx, bridge_rx) = bridge::bridge_channel();

        // ── 6. Start the remote server ───────────────────────────────────────
        // It owns its OWN internal tokio runtime and talks to us only via the
        // channels above; that is fine — the daemon's runtime drives the reactor.
        let remote_server = RemoteServer::start(
            bridge_tx,
            auth_store.clone(),
            broadcaster.clone(),
            state_version.clone(),
            params.listen_addr,
            git_status_tx.clone(),
            remote_subscribed_terminals.clone(),
            next_connection_id,
            params.tls_enabled,
        )?;

        // ── 7. Print pairing info to stdout (mirror headless.rs) ──────────────
        let port = remote_server.port();
        let code = auth_store.get_or_create_code();
        log::info!("Remote server started on port {port}");
        println!("Remote server listening on port {port}");
        println!("Pairing code: {code} (expires in 60s)");
        if let Some(fp) = remote_server.cert_fingerprint() {
            // Print the raw fingerprint string rather than pulling in
            // okena-transport's formatter, keeping daemon-core's dep set lean.
            println!("TLS cert fingerprint (SHA-256): {fp}");
        }
        println!("Run `okena pair` for a fresh code.");

        // ── 8. Store exactly what `run()` needs ──────────────────────────────
        // `broadcaster` and `auth_store` are now owned by the server; no
        // duplicates are kept here.
        Ok(Self {
            runtime,
            reactor,
            remote_server,
            bridge_rx,
            backend,
            terminals,
            pty_manager,
            pty_events,
            state_version,
            git_status_tx,
            remote_subscribed_terminals,
            settings,
            daemon_config,
            _instance_lock: instance_lock,
        })
    }

    /// Drive the reactor on a [`LocalSet`](tokio::task::LocalSet) until shutdown.
    ///
    /// Spawns the observer tasks, the PTY loop, and the git poll, then runs the
    /// command loop as the "main" task — racing it against ctrl-c so the daemon
    /// can shut down cleanly in dev. Blocks until the bridge closes (the remote
    /// server is gone) or ctrl-c arrives, then drops the server (stopping it and
    /// removing `remote.json`). See the module docs for why this blocks.
    pub fn run(self) -> anyhow::Result<()> {
        let DaemonCore {
            runtime,
            reactor,
            remote_server,
            bridge_rx,
            backend,
            terminals,
            pty_manager,
            pty_events,
            state_version,
            git_status_tx,
            remote_subscribed_terminals,
            settings,
            daemon_config,
            // Bound (not `..`) so the lock is held until the end of `run`, then
            // released on drop after the server is stopped.
            _instance_lock,
        } = self;
        let handle = runtime.handle().clone();
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async move {
            // Observers MUST be spawned inside the LocalSet (they `spawn_local`).
            reactor.spawn_observers();
            tokio::task::spawn_local(crate::pty_loop::run_pty_loop(
                pty_events,
                terminals.clone(),
                pty_manager.clone(),
                reactor.service_manager.clone(),
                handle.clone(),
                reactor.service_tick.clone(),
                reactor.state_version.clone(),
            ));
            tokio::task::spawn_local(crate::git_poll::run_git_poll(
                reactor.workspace.clone(),
                git_status_tx.clone(),
                reactor.state_version.clone(),
                remote_subscribed_terminals,
            ));

            // Materialize PTYs for every restored project's uninitialized
            // terminal slots BEFORE the command loop starts serving clients.
            // Persisted layouts carry `terminal_id: None` slots that nobody
            // else spawns in daemon-client mode (the GUI client can't self-spawn
            // over a remote backend), so they would render blank forever. This
            // assigns ids + creates PTYs for all loaded projects; the assigned
            // ids bump `data_version` (the existing autosave observer persists
            // them — no second writer) and `workspace_tick` (whose observer,
            // spawned above, bumps `state_version`). Runs on the LocalSet thread
            // because PTY/hook spawning may reach the reactor.
            crate::command_loop::materialize_uninitialized_terminals(
                &*backend,
                &reactor.workspace,
                &reactor.workspace_tick,
                &reactor.hook_runner,
                &reactor.hook_monitor,
                &terminals,
                &settings,
            );

            // The command loop is the "main" task; it runs until the bridge
            // closes. Race it against ctrl-c so the daemon can shut down cleanly.
            let cmd = crate::command_loop::daemon_command_loop(
                bridge_rx,
                backend,
                reactor.workspace.clone(),
                reactor.workspace_tick.clone(),
                reactor.hook_runner.clone(),
                reactor.hook_monitor.clone(),
                terminals.clone(),
                state_version,
                git_status_tx.clone(),
                reactor.service_manager.clone(),
                reactor.service_tick.clone(),
                handle.clone(),
                settings,
                daemon_config,
            );
            tokio::select! {
                _ = cmd => log::info!("daemon command loop ended (remote server gone)"),
                r = tokio::signal::ctrl_c() => {
                    if let Err(e) = r {
                        log::warn!("ctrl-c handler error: {e}");
                    }
                    log::info!("daemon received ctrl-c, shutting down");
                }
            }
        });
        // Keep `remote_server` alive across `block_on`; dropping it here stops
        // the server and removes remote.json.
        drop(remote_server);
        Ok(())
    }
}
