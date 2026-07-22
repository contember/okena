//! Final GPUI-free daemon assembly shared by both headless entry points.
//!
//! [`DaemonCore`] stands up the workspace, PTY and service managers, git watcher,
//! remote command bridge, and
//! [`RemoteServer`](okena_remote_server::server::RemoteServer) without GPUI. The
//! shared state lives behind `Arc<parking_lot::Mutex<…>>` in
//! [`DaemonReactor`](crate::reactor::DaemonReactor), the `cx.observe` closures
//! become the `watch`-channel-driven observer tasks
//! ([`spawn_observers`](crate::reactor::DaemonReactor::spawn_observers)), and the
//! command bridge is driven by [`daemon_command_loop`](crate::command_loop).
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
//! [`run`](DaemonCore::run) blocks until ctrl-c, a bridge failure, or a graceful
//! shutdown request. A UI-owned daemon shuts down when its final owning desktop
//! client hands off ownership.
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

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use async_channel::Receiver;
use okena_core::api::{ApiGitStatus, ApiTerminalFocusRequest, ApiToast};
use okena_core::git_poll::GitPollTrigger;
use okena_hooks::{HookMonitor, HookRunner};
use okena_remote_server::auth::AuthStore;
use okena_remote_server::bridge::{self, BridgeReceiver};
use okena_remote_server::pty_broadcaster::PtyBroadcaster;
use okena_remote_server::server::RemoteServer;
use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::{LocalBackend, TerminalBackend};
use okena_terminal::pty_manager::{PtyEvent, PtyManager};
use okena_terminal::session_backend::SessionBackend;
use okena_workspace::persistence::{self, AppSettings, LockGuard, acquire_instance_lock};
use okena_workspace::state::{Workspace, WorkspaceData};
use parking_lot::Mutex;
use tokio::sync::{mpsc, watch};

use crate::daemon_config::DaemonConfig;
use crate::reactor::DaemonReactor;

fn kill_stale_terminal_sessions(backend: &dyn TerminalBackend, terminal_ids: &[String]) {
    for terminal_id in terminal_ids {
        backend.kill(terminal_id);
    }
}

/// Inputs needed to construct a daemon.
pub struct DaemonParams {
    /// The persisted workspace state to drive (projects, layouts, windows).
    pub workspace_data: WorkspaceData,
    /// Persistent sessions whose stale worktree rows were discarded on load.
    pub stale_terminal_ids: Vec<String>,
    /// The app settings (font / theme / shell / session backend), loaded once at
    /// startup and shared with [`DaemonConfig`] as the settings write path.
    pub settings: AppSettings,
    /// The session backend (tmux / dtach / screen / none) the PTY manager uses
    /// to spawn terminals.
    pub session_backend: SessionBackend,
    /// TCP addresses the remote server binds to. The UI-owned daemon always
    /// includes loopback for same-host clients; remote mode may add a LAN bind.
    pub listen_addrs: Vec<IpAddr>,
    /// Whether TLS is enabled. Non-loopback listeners require it; loopback also
    /// accepts plain HTTP for local clients.
    pub tls_enabled: bool,
    /// Whether desktop clients own this daemon's process lifetime.
    pub ui_owned: bool,
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
    /// Toast broadcast: a periodic drain task (see [`run`](DaemonCore::run))
    /// pushes the `HookMonitor`'s pending toasts here as [`ApiToast`]s and the
    /// server fans them out to clients. The daemon has no surface of its own, so
    /// this is how hook-failure notifications reach the GUI.
    toast_tx: Arc<tokio::sync::broadcast::Sender<ApiToast>>,
    /// Client terminal subscriptions (connection id -> subscribed terminal ids),
    /// shared with the remote server. The git poll reads it to fan out the
    /// expensive `gh` PR/CI lookups only for projects a client is viewing.
    remote_subscribed_terminals:
        Arc<std::sync::RwLock<HashMap<u64, std::collections::HashSet<String>>>>,
    /// Git poll wake-up sender shared by command handling and the remote server.
    git_poll_trigger_tx: mpsc::UnboundedSender<GitPollTrigger>,
    /// Git poll wake-up receiver consumed by [`run`](DaemonCore::run).
    git_poll_trigger_rx: mpsc::UnboundedReceiver<GitPollTrigger>,
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
    /// Graceful-shutdown trigger fired by `POST /v1/shutdown` (via the remote
    /// server's `AppState`). [`run`](DaemonCore::run) awaits it and returns,
    /// which drops the server (socket unlink + remote.json removal) and releases
    /// the instance lock on drop — a clean teardown, no SIGKILL.
    shutdown_requested: Arc<tokio::sync::Notify>,
}

impl DaemonCore {
    /// Build the daemon and start its remote server.
    ///
    /// Stands up the PTY manager + broadcaster, the terminal registry + backend,
    /// the workspace + reactor, the settings + config, and the server wiring
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
        // Per-profile Claude account isolation: push CLAUDE_CONFIG_DIR (or its
        // active removal for the default ~/.claude) into the PTYs the daemon
        // spawns, so `claude` invocations inside daemon-served terminals read the
        // right account — the same override the GUI's `sync_claude_pty_env`
        // applies, computed by the gpui-free `okena_workspace::claude_env` from
        // the daemon's own settings (the GUI's `ExtensionSettingsStore` is gpui).
        pty_manager.set_extra_env(okena_workspace::claude_env::claude_pty_env_for_settings(
            &params.settings,
        ));
        let broadcaster = Arc::new(PtyBroadcaster::new());
        pty_manager.set_output_sink(broadcaster.clone());
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(HashMap::new()));
        let backend: Arc<dyn TerminalBackend> = Arc::new(LocalBackend::new(pty_manager.clone()));
        kill_stale_terminal_sessions(backend.as_ref(), &params.stale_terminal_ids);

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
        let mut daemon_config = DaemonConfig::new(settings.clone());
        // Seed the process palette from the active theme so daemon terminals
        // answer OSC color queries (no views push per-terminal palettes here;
        // client mirrors deliberately don't answer). Kept in sync afterwards by
        // `DaemonConfig::apply_active_theme` on theme changes.
        {
            use okena_app_core::remote_config::ConfigBackend as _;
            let (mode, custom_id) = {
                let s = settings.lock();
                (s.theme_mode, s.custom_theme_id.clone())
            };
            let colors = daemon_config.active_theme_colors(mode, custom_id.as_deref());
            okena_terminal::terminal::set_process_palette(colors);
        }

        // ── 5. Server wiring channels ────────────────────────────────────────
        // Shared-watch trick: `tokio::sync::watch::Sender` is `Clone` and clones
        // share one underlying channel. The server + command loop READ this
        // `state_version`; the reactor's observers / PTY loop / git poll BUMP
        // `reactor.state_version` (the same channel), so reads observe the bumps.
        let state_version = Arc::new(reactor.state_version.clone());
        let git_status_tx = Arc::new(watch::Sender::new(HashMap::new()));
        // Toast broadcast: the periodic drain task in `run()` is the sole
        // producer; each connected client subscribes a receiver. Capacity bounds
        // the per-client backlog — a lagging client drops non-critical toasts.
        let toast_tx = Arc::new(tokio::sync::broadcast::channel::<ApiToast>(64).0);
        let terminal_focus_tx =
            Arc::new(tokio::sync::broadcast::channel::<ApiTerminalFocusRequest>(64).0);
        let auth_store = Arc::new(AuthStore::new());
        let remote_subscribed_terminals = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let next_connection_id = Arc::new(AtomicU64::new(0));
        // Live-WS-connection count + graceful-shutdown trigger for `/v1/shutdown`.
        let active_connections = Arc::new(AtomicU64::new(0));
        let shutdown_requested = Arc::new(tokio::sync::Notify::new());
        let (bridge_tx, bridge_rx) = bridge::bridge_channel();
        let (git_poll_trigger_tx, git_poll_trigger_rx) = mpsc::unbounded_channel();

        // ── 6. Start the remote server ───────────────────────────────────────
        // It owns its OWN internal tokio runtime and talks to us only via the
        // channels above; that is fine — the daemon's runtime drives the reactor.
        let remote_server = RemoteServer::start(
            bridge_tx,
            auth_store.clone(),
            broadcaster.clone(),
            state_version.clone(),
            params.listen_addrs,
            git_status_tx.clone(),
            toast_tx.clone(),
            terminal_focus_tx,
            remote_subscribed_terminals.clone(),
            Some(git_poll_trigger_tx.clone()),
            next_connection_id,
            active_connections,
            shutdown_requested.clone(),
            params.ui_owned,
            params.tls_enabled,
            env!("CARGO_PKG_VERSION"),
        )?;

        // ── 7. Print pairing info to stdout ──────────────────────────────────
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
            toast_tx,
            remote_subscribed_terminals,
            git_poll_trigger_tx,
            git_poll_trigger_rx,
            settings,
            daemon_config,
            _instance_lock: instance_lock,
            shutdown_requested,
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
            mut remote_server,
            bridge_rx,
            backend,
            terminals,
            pty_manager,
            pty_events,
            state_version,
            git_status_tx,
            toast_tx,
            remote_subscribed_terminals,
            git_poll_trigger_tx,
            git_poll_trigger_rx,
            settings,
            daemon_config,
            // Bound (not `..`) so the lock is held until the end of `run`, then
            // released on drop after the server is stopped.
            _instance_lock,
            shutdown_requested,
        } = self;
        let handle = runtime.handle().clone();
        let local = tokio::task::LocalSet::new();
        let shutdown_workspace = reactor.workspace.clone();
        let shutdown_backend = backend.clone();
        let shutdown_terminals = terminals.clone();
        let shutdown_pty_manager = pty_manager.clone();
        let shutdown_autosaves = reactor.autosave_tracker.clone();
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
                // Daemon-owned workspace + hooks: the PTY loop runs the full
                // terminal-exit lifecycle (hook-terminal exits, terminal.on_close,
                // OSC hook-exit, soft-close reap) directly against this state.
                crate::pty_loop::PtyLoopReactor {
                    workspace: reactor.workspace.clone(),
                    backend: backend.clone(),
                    hook_runner: reactor.hook_runner.clone(),
                    hook_monitor: reactor.hook_monitor.clone(),
                    workspace_tick: reactor.workspace_tick.clone(),
                    settings: settings.clone(),
                },
                reactor.state_version.clone(),
            ));
            tokio::task::spawn_local(crate::git_poll::run_git_poll(
                reactor.workspace.clone(),
                git_status_tx.clone(),
                reactor.state_version.clone(),
                remote_subscribed_terminals,
                git_poll_trigger_rx,
            ));
            tokio::task::spawn_local(crate::git_poll::run_git_head_poll(
                reactor.workspace.clone(),
                git_poll_trigger_tx.clone(),
            ));
            // Forward the daemon's HookMonitor toasts to clients. The daemon has
            // no surface; this drains its pending toasts and broadcasts them over
            // the same channel the remote server fans out (`WsOutbound::Toast`).
            // `(*toast_tx).clone()` clones the inner `broadcast::Sender` (cheap,
            // shares the channel) so the task can outlive this `Arc` handle.
            tokio::task::spawn_local(crate::toast_poll::run_toast_poll(
                reactor.hook_monitor.clone(),
                (*toast_tx).clone(),
                reactor.state_version.clone(),
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

            // Shared soft-close deadline map: the command loop arms a deadline
            // when it ejects a busy terminal; the finalizer loop kills the PTY
            // once it elapses. Spawn the finalizer BEFORE `backend`/`settings`
            // are consumed by the command loop, cloning what both tasks need.
            let soft_close_deadlines: crate::soft_close::SoftCloseDeadlines =
                Arc::new(Mutex::new(HashMap::new()));
            tokio::task::spawn_local(crate::soft_close::run_soft_close_poll(
                reactor.workspace.clone(),
                backend.clone(),
                terminals.clone(),
                reactor.workspace_tick.clone(),
                reactor.hook_runner.clone(),
                reactor.hook_monitor.clone(),
                soft_close_deadlines.clone(),
            ));

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
                soft_close_deadlines,
                git_poll_trigger_tx,
            );
            tokio::select! {
                _ = cmd => log::info!("daemon command loop ended (remote server gone)"),
                r = tokio::signal::ctrl_c() => {
                    if let Err(e) = r {
                        log::warn!("ctrl-c handler error: {e}");
                    }
                    log::info!("daemon received ctrl-c, shutting down");
                }
                // A client-aware `POST /v1/shutdown` accepted: return so the
                // teardown below runs cleanly (no successor to hand off to).
                _ = shutdown_requested.notified() => {
                    log::info!("daemon received shutdown request, shutting down");
                }
            }
        });
        // Cancel every LocalSet task first, then stop accepting new requests.
        // The reactor Arc remains available for the final authoritative save.
        drop(local);
        // Flush BEFORE stopping the server so `remote.json` stays present for the
        // whole teardown. Otherwise `RemoteServer::stop` removes the discovery
        // file up front, leaving a window where the daemon is alive and still
        // holding the instance lock but undiscoverable — a GUI reopening in that
        // window can't attach, spawns a fresh daemon that collides on the lock,
        // and surfaces "Another Okena instance is already running". The command
        // loop is already gone (drop(local)), so no client can mutate state
        // during the flush; `stop()` (below) removes the discovery file last,
        // right before the instance lock drops as `run()` returns.
        flush_shutdown_state(
            &shutdown_workspace,
            &*shutdown_backend,
            &shutdown_terminals,
            || shutdown_autosaves.flush(),
            || shutdown_pty_manager.flush_teardown(),
            persistence::save_workspace,
        )?;
        remote_server.stop();
        Ok(())
    }
}

fn flush_shutdown_state(
    workspace: &Arc<Mutex<Workspace>>,
    backend: &dyn TerminalBackend,
    terminals: &TerminalsRegistry,
    flush_autosaves: impl FnOnce(),
    flush_teardown: impl FnOnce(),
    save: impl FnOnce(&WorkspaceData) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    flush_autosaves();
    let (data, terminal_ids) = {
        let mut ws = workspace.lock();
        let terminal_ids: HashSet<String> = ws
            .drain_pending_closes()
            .into_iter()
            .chain(ws.drain_pending_terminal_kills())
            .chain(
                ws.projects()
                    .iter()
                    .flat_map(|project| project.hook_terminals.keys().cloned()),
            )
            .collect();
        (ws.data().clone(), terminal_ids)
    };

    for terminal_id in terminal_ids {
        backend.kill(&terminal_id);
        terminals.lock().remove(&terminal_id);
    }
    flush_teardown();

    save(&data)
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;
    use okena_terminal::shell_config::ShellType;
    use okena_terminal::terminal::TerminalTransport;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct StubTransport;

    impl TerminalTransport for StubTransport {
        fn send_input(&self, _terminal_id: &str, _data: &[u8]) {}
        fn resize(&self, _terminal_id: &str, _cols: u16, _rows: u16) {}
        fn uses_mouse_backend(&self) -> bool {
            false
        }
    }

    struct RecordingBackend {
        killed: Arc<Mutex<Vec<String>>>,
    }

    impl TerminalBackend for RecordingBackend {
        fn transport(&self) -> Arc<dyn TerminalTransport> {
            Arc::new(StubTransport)
        }

        fn create_terminal(
            &self,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            anyhow::bail!("not used")
        }

        fn reconnect_terminal(
            &self,
            _terminal_id: &str,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            anyhow::bail!("not used")
        }

        fn kill(&self, terminal_id: &str) {
            self.killed.lock().push(terminal_id.to_string());
        }

        fn supports_buffer_capture(&self) -> bool {
            false
        }

        fn capture_buffer(&self, _terminal_id: &str) -> Option<std::path::PathBuf> {
            None
        }

        fn is_remote(&self) -> bool {
            false
        }

        fn get_shell_pid(&self, _terminal_id: &str) -> Option<u32> {
            None
        }

        fn get_service_pids(&self, _terminal_id: &str) -> Vec<u32> {
            Vec::new()
        }
    }

    #[test]
    fn startup_kills_sessions_owned_by_discarded_worktrees() {
        let killed = Arc::new(Mutex::new(Vec::new()));
        let backend = RecordingBackend {
            killed: killed.clone(),
        };

        kill_stale_terminal_sessions(
            &backend,
            &[
                "layout".to_string(),
                "service".to_string(),
                "hook".to_string(),
            ],
        );

        assert_eq!(
            *killed.lock(),
            vec![
                "layout".to_string(),
                "service".to_string(),
                "hook".to_string(),
            ]
        );
    }

    #[test]
    fn shutdown_drains_terminal_kills_before_saving() {
        let mut data = WorkspaceData::empty();
        let mut project = okena_state::ProjectData {
            id: "p1".to_string(),
            name: "Project".to_string(),
            path: "/tmp".to_string(),
            layout: None,
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: Default::default(),
            hooks: Default::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            default_shell: None,
            hook_terminals: HashMap::new(),
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
        };
        project.hook_terminals.insert(
            "persistent-hook".to_string(),
            okena_state::HookTerminalEntry {
                label: "on_project_open".to_string(),
                status: okena_state::HookTerminalStatus::Running,
                hook_type: "on_project_open".to_string(),
                command: "echo hook".to_string(),
                cwd: "/tmp".to_string(),
            },
        );
        data.projects.push(project);
        data.project_order.push("p1".to_string());
        let workspace = Arc::new(Mutex::new(Workspace::new(data)));
        workspace.lock().queue_terminal_kills([
            "terminal-a".to_string(),
            "terminal-a".to_string(),
            "terminal-b".to_string(),
        ]);
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(HashMap::new()));
        let killed = Arc::new(Mutex::new(Vec::new()));
        let backend = RecordingBackend {
            killed: killed.clone(),
        };
        let saved = AtomicBool::new(false);
        let flushed = AtomicBool::new(false);

        flush_shutdown_state(
            &workspace,
            &backend,
            &terminals,
            || {},
            || {
                assert_eq!(killed.lock().len(), 3, "all kills precede teardown flush");
                flushed.store(true, Ordering::Relaxed);
            },
            |_| {
                assert_eq!(killed.lock().len(), 3, "cleanup precedes final save");
                assert!(
                    flushed.load(Ordering::Relaxed),
                    "teardown flush precedes save"
                );
                saved.store(true, Ordering::Relaxed);
                Ok(())
            },
        )
        .unwrap();

        assert!(saved.load(Ordering::Relaxed));
        assert!(killed.lock().contains(&"persistent-hook".to_string()));
        assert!(workspace.lock().drain_pending_terminal_kills().is_empty());
    }

    #[test]
    fn shutdown_waits_for_older_autosave_before_final_save() {
        let tracker = Arc::new(crate::observers::AutosaveTracker::default());
        let autosave_job = tracker.start();
        let events = Arc::new(Mutex::new(Vec::new()));
        let (autosave_started_tx, autosave_started_rx) = std::sync::mpsc::channel();
        let (release_autosave_tx, release_autosave_rx) = std::sync::mpsc::channel();
        let (flush_started_tx, flush_started_rx) = std::sync::mpsc::channel();
        let workspace = Arc::new(Mutex::new(Workspace::new(WorkspaceData::empty())));
        let terminals: TerminalsRegistry = Arc::new(Mutex::new(HashMap::new()));
        let backend = RecordingBackend {
            killed: Arc::new(Mutex::new(Vec::new())),
        };

        std::thread::scope(|scope| {
            let autosave_events = events.clone();
            scope.spawn(move || {
                autosave_started_tx.send(()).unwrap();
                release_autosave_rx.recv().unwrap();
                autosave_events.lock().push("autosave");
                drop(autosave_job);
            });
            autosave_started_rx.recv().unwrap();

            let shutdown_events = events.clone();
            let shutdown = scope.spawn(move || {
                flush_shutdown_state(
                    &workspace,
                    &backend,
                    &terminals,
                    || {
                        flush_started_tx.send(()).unwrap();
                        tracker.flush();
                    },
                    || {},
                    |_| {
                        assert_eq!(&*shutdown_events.lock(), &["autosave"]);
                        shutdown_events.lock().push("final");
                        Ok(())
                    },
                )
            });

            flush_started_rx.recv().unwrap();
            assert!(
                events.lock().is_empty(),
                "final save must wait for autosave"
            );
            release_autosave_tx.send(()).unwrap();
            shutdown.join().unwrap().unwrap();
        });

        assert_eq!(&*events.lock(), &["autosave", "final"]);
    }
}
