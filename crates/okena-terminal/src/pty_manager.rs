#[cfg(windows)]
use crate::session_backend::SessionCommand;
#[cfg(not(windows))]
use crate::session_backend::get_extended_path;
use crate::session_backend::{ResolvedBackend, SessionBackend};
use crate::shell_config::{ShellCommandExt, ShellType};
use anyhow::Result;
use async_channel::{Receiver, Sender};
use parking_lot::{Condvar, Mutex};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

/// Trait for broadcasting PTY output to external consumers (e.g. remote WebSocket clients).
/// Implementations must be thread-safe as this is called from PTY reader threads.
pub trait PtyOutputSink: Send + Sync {
    fn publish(&self, terminal_id: String, data: Vec<u8>) -> u64;
    /// `server_owns` is true when the origin's local user currently holds resize
    /// authority. Clients use it to stop re-asserting their own window size and
    /// defer to the origin instead of fighting it back over the next round-trip.
    fn publish_resize(
        &self,
        _terminal_id: String,
        _cols: u16,
        _rows: u16,
        _server_owns: bool,
        _owner_connection_id: Option<String>,
    ) {
    }
}

/// Events from PTY processes
#[derive(Debug)]
pub enum PtyEvent {
    /// Data received from PTY
    Data {
        terminal_id: String,
        generation: PtyGeneration,
        data: Vec<u8>,
        sequence: u64,
    },
    /// PTY process exited
    Exit {
        terminal_id: String,
        generation: PtyGeneration,
        exit_code: Option<u32>,
    },
}

/// Identity of one concrete PTY process attached to a logical terminal ID.
///
/// Logical IDs are reused when reconnecting persistent sessions. The generation
/// prevents delayed events from an older reader/writer pair from affecting the
/// newly attached process with the same terminal ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PtyGeneration(u64);

#[derive(Default)]
struct PtyInstances {
    current: HashMap<String, PtyGeneration>,
    creating: HashMap<String, PtyGeneration>,
    exited: HashMap<String, ExitedPty>,
    pending_session_kills: HashMap<String, usize>,
}

struct ExitedPty {
    generation: PtyGeneration,
    #[cfg(windows)]
    wsl_distro: Option<String>,
    #[cfg(windows)]
    wsl_backend: Option<ResolvedBackend>,
}

impl ExitedPty {
    fn new(generation: PtyGeneration) -> Self {
        Self {
            generation,
            #[cfg(windows)]
            wsl_distro: None,
            #[cfg(windows)]
            wsl_backend: None,
        }
    }
}

impl PtyInstances {
    fn begin_creation(&mut self, terminal_id: &str, generation: PtyGeneration) {
        self.creating.insert(terminal_id.to_string(), generation);
        self.exited.remove(terminal_id);
    }

    fn is_creating(&self, terminal_id: &str, generation: PtyGeneration) -> bool {
        self.creating.get(terminal_id).copied() == Some(generation)
    }

    fn publish(&mut self, terminal_id: &str, generation: PtyGeneration) {
        self.creating.remove(terminal_id);
        self.current.insert(terminal_id.to_string(), generation);
    }

    fn is_current(&self, terminal_id: &str, generation: PtyGeneration) -> bool {
        self.current.get(terminal_id).copied() == Some(generation)
    }

    /// Claim lifecycle handling for this generation exactly once.
    fn claim_exit(&mut self, terminal_id: &str, generation: PtyGeneration) -> bool {
        if !self.is_current(terminal_id, generation) {
            return false;
        }
        self.current.remove(terminal_id);
        self.exited
            .insert(terminal_id.to_string(), ExitedPty::new(generation));
        true
    }

    #[cfg(windows)]
    fn record_exited_wsl_metadata(
        &mut self,
        terminal_id: &str,
        generation: PtyGeneration,
        wsl_distro: Option<String>,
        wsl_backend: Option<ResolvedBackend>,
    ) {
        if let Some(exited) = self.exited.get_mut(terminal_id)
            && exited.generation == generation
        {
            exited.wsl_distro = wsl_distro;
            exited.wsl_backend = wsl_backend;
        }
    }

    fn cancel_creation(&mut self, terminal_id: &str, generation: PtyGeneration) {
        if self.is_creating(terminal_id, generation) {
            self.creating.remove(terminal_id);
        }
    }

    fn remove_current(&mut self, terminal_id: &str, generation: PtyGeneration) {
        if self.is_current(terminal_id, generation) {
            self.current.remove(terminal_id);
        }
    }

    fn claim_exited(&mut self, terminal_id: &str, generation: PtyGeneration) -> Option<ExitedPty> {
        if self.exited.get(terminal_id)?.generation != generation {
            return None;
        }
        self.exited.remove(terminal_id)
    }

    fn queue_session_kill(&mut self, terminal_id: &str) {
        *self
            .pending_session_kills
            .entry(terminal_id.to_string())
            .or_default() += 1;
    }

    fn complete_session_kill(&mut self, terminal_id: &str) {
        let Some(pending) = self.pending_session_kills.get_mut(terminal_id) else {
            return;
        };
        *pending = pending.saturating_sub(1);
        if *pending == 0 {
            self.pending_session_kills.remove(terminal_id);
        }
    }

    fn has_pending_session_kill(&self, terminal_id: &str) -> bool {
        self.pending_session_kills.contains_key(terminal_id)
    }
}

/// Shared shutdown coordination between reader/writer threads
struct PtyShutdownState {
    broken: AtomicBool,
    terminal_id: String,
    generation: PtyGeneration,
}

impl PtyShutdownState {
    fn new(terminal_id: String, generation: PtyGeneration) -> Self {
        Self {
            broken: AtomicBool::new(false),
            terminal_id,
            generation,
        }
    }

    fn is_broken(&self) -> bool {
        self.broken.load(Ordering::Relaxed)
    }

    fn mark_broken(&self) {
        self.broken.store(true, Ordering::Relaxed);
    }
}

struct PtyReservation<'a> {
    terminal_id: &'a str,
    generation: PtyGeneration,
    instances: &'a Mutex<PtyInstances>,
    changed: &'a Condvar,
    active: bool,
}

impl PtyReservation<'_> {
    fn publish(&mut self) {
        self.active = false;
    }
}

impl Drop for PtyReservation<'_> {
    fn drop(&mut self) {
        if self.active {
            self.instances
                .lock()
                .cancel_creation(self.terminal_id, self.generation);
            self.changed.notify_all();
        }
    }
}

/// Extract a human-readable message from a panic payload
fn format_panic(payload: &dyn std::any::Any) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Number of shared teardown worker threads. Bounds how many PTY teardowns
/// (thread joins + `lsof`/`tmux kill-session`/SIGTERM subprocess calls) can run
/// concurrently. On bulk shutdown we enqueue N jobs but only this many run at once,
/// instead of spawning one detached OS thread per `kill()`/`cleanup_exited()` call.
const TEARDOWN_WORKERS: usize = 4;

/// What a teardown worker should do with a job. Modeling the two paths explicitly
/// keeps the deliberate double-fire behavior (see `kill` / `cleanup_exited`) legible.
enum TeardownKind {
    /// The process already EOF'd (reader saw it); we only reap the reader/writer
    /// threads via `shutdown_handle`. No session kill — there's nothing left to
    /// SIGTERM from our side. Enqueued by `cleanup_exited`.
    ReapOnly,
    /// Kill the underlying session backend (tmux/screen/dtach), and on Windows the
    /// WSL session. Enqueued by `kill`. The job's `handle` may be `None`: that is the
    /// "client already exited (cleanup_exited ran first), SIGTERM the lingering
    /// session/daemon" path — in that case the worker does ONLY the session kill.
    KillSession {
        session_backend: ResolvedBackend,
        session_name: String,
        /// WSL distro for the session (Windows only).
        #[cfg(windows)]
        wsl_distro: Option<String>,
        /// Resolved WSL backend; when present the kill happens inside WSL (Windows only).
        #[cfg(windows)]
        wsl_backend: Option<ResolvedBackend>,
    },
}

/// A unit of teardown work handed to the shared worker pool. Everything is owned so
/// the job is `Send` and the workers never touch `PtyManager` state.
struct TeardownJob {
    /// The PTY handle to reap, if we own it. `None` for the `KillSession` path when
    /// `cleanup_exited` already took the handle (double-fire) — then only the session
    /// kill runs.
    handle: Option<PtyHandle>,
    kind: TeardownKind,
    pending_session_kill: Option<PendingSessionKill>,
}

struct PendingSessionKill {
    terminal_id: String,
    instances: Arc<Mutex<PtyInstances>>,
    changed: Arc<Condvar>,
}

impl Drop for PendingSessionKill {
    fn drop(&mut self) {
        self.instances
            .lock()
            .complete_session_kill(&self.terminal_id);
        self.changed.notify_all();
    }
}

#[derive(Default)]
struct TeardownTracker {
    pending: Mutex<usize>,
    drained: Condvar,
}

impl TeardownTracker {
    fn queued(&self) {
        *self.pending.lock() += 1;
    }

    fn completed(&self) {
        let mut pending = self.pending.lock();
        *pending = pending.saturating_sub(1);
        if *pending == 0 {
            self.drained.notify_all();
        }
    }

    fn flush(&self) {
        let mut pending = self.pending.lock();
        while *pending != 0 {
            self.drained.wait(&mut pending);
        }
    }
}

/// Handle to a single PTY process
struct PtyHandle {
    generation: PtyGeneration,
    /// `Option` so teardown (`shutdown_handle`) and the `Drop` backstop can both
    /// `take()` it idempotently to close the PTY and unblock the reader thread.
    master: Option<Box<dyn MasterPty + Send>>,
    child: Box<dyn Child + Send + Sync>,
    /// Channel to send input to the writer thread.
    /// `Option` so teardown and the `Drop` backstop can both `take()` it
    /// idempotently to close the channel and unblock the writer thread.
    input_tx: Option<mpsc::Sender<Vec<u8>>>,
    /// Shared PTY writer, also held by the batched writer thread. Lets
    /// `write_response` write query replies synchronously (see that method).
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    reader_handle: Option<JoinHandle<()>>,
    writer_handle: Option<JoinHandle<()>>,
    shutdown: Arc<PtyShutdownState>,
    /// WSL distro name if this terminal runs inside WSL (Windows only)
    #[cfg(windows)]
    wsl_distro: Option<String>,
    /// Resolved session backend for WSL terminals (Windows only)
    #[cfg(windows)]
    wsl_backend: Option<ResolvedBackend>,
}

impl Drop for PtyHandle {
    /// Non-blocking teardown backstop.
    ///
    /// The normal teardown path is [`PtyManager::shutdown_handle`], which kills
    /// the child, drops the channel/master, and joins the reader/writer threads.
    /// This `Drop` impl only exists for the off-happy-path case where a handle is
    /// dropped without `shutdown_handle` having run (e.g. a future code path that
    /// removes it from the map directly). In that case we still want the threads
    /// to observe EOF and exit instead of leaking silently.
    ///
    /// It is idempotent (safe to run after `shutdown_handle` already took the
    /// fields) and must NOT block: it signals shutdown and drops `input_tx` /
    /// `master` so the channel and PTY close, but it does NOT join the threads
    /// and does NOT call `child.kill()` (the PID may have been reaped/recycled).
    fn drop(&mut self) {
        // Signal the reader/writer threads to stop (idempotent: just sets a bool).
        self.shutdown.mark_broken();
        // Closing the input channel makes the writer thread's `recv` return Err;
        // dropping the master unblocks a reader still stuck in `read`. Both are
        // no-ops if `shutdown_handle` already took them.
        drop(self.input_tx.take());
        drop(self.master.take());
        // Intentionally do NOT join reader_handle / writer_handle here — a Drop
        // must not block. The threads exit on their own once the channel/master
        // close (or the process exits).
    }
}

/// Manages all PTY processes
pub struct PtyManager {
    terminals: Arc<Mutex<HashMap<String, PtyHandle>>>,
    instances: Arc<Mutex<PtyInstances>>,
    instance_changed: Arc<Condvar>,
    next_generation: AtomicU64,
    event_tx: Sender<PtyEvent>,
    /// Session backend for persistence (tmux/screen/none)
    session_backend: ResolvedBackend,
    /// Raw user preference (needed for WSL per-terminal resolution)
    #[cfg(windows)]
    session_backend_preference: SessionBackend,
    /// Optional sink for streaming PTY output to external consumers (e.g. remote clients).
    /// Publishing happens directly from reader threads to avoid UI event loop latency.
    output_sink: Arc<Mutex<Option<Arc<dyn PtyOutputSink>>>>,
    /// Extra environment overrides applied to every spawned PTY. `Some(val)` sets
    /// the variable; `None` removes it from the inherited environment so a stale
    /// value (e.g. a `CLAUDE_CONFIG_DIR` exported in the user's shell that launched
    /// Okena) cannot leak into the terminal.
    extra_env: Mutex<Vec<(String, Option<String>)>>,
    /// Sender for the shared teardown worker pool. `kill`/`cleanup_exited` enqueue
    /// jobs here instead of spawning a detached thread per call. Wrapped in `Option`
    /// only so `Drop` can `take()` it and close the channel, signaling workers to
    /// drain remaining jobs and exit.
    teardown_tx: Option<Sender<TeardownJob>>,
    teardown_tracker: Arc<TeardownTracker>,
}

impl PtyManager {
    /// Create a new PTY manager with the specified session backend
    pub fn new(backend: SessionBackend) -> (Self, Receiver<PtyEvent>) {
        let (tx, rx) = async_channel::bounded(4096);
        let session_backend = backend.resolve();

        if session_backend.supports_persistence() {
            log::info!("Session persistence enabled with {:?}", session_backend);
        }

        // Clean up stale dtach sockets from previous crashes
        #[cfg(unix)]
        if matches!(session_backend, ResolvedBackend::Dtach)
            && let Err(e) = std::thread::Builder::new()
                .name("dtach-socket-gc".into())
                .spawn(|| {
                    crate::session_backend::cleanup_stale_dtach_sockets();
                })
        {
            log::warn!("failed to spawn dtach cleanup thread: {e}");
        }

        // Shared teardown worker pool. `async-channel` is MPMC, so all workers share
        // one `Receiver` and pull jobs via `recv_blocking`. Unbounded so enqueuing
        // never blocks the GPUI thread; concurrency is bounded by the worker count.
        let (teardown_tx, teardown_rx) = async_channel::unbounded::<TeardownJob>();
        let teardown_tracker = Arc::new(TeardownTracker::default());
        for i in 0..TEARDOWN_WORKERS {
            let rx = teardown_rx.clone();
            let tracker = Arc::clone(&teardown_tracker);
            if let Err(e) = std::thread::Builder::new()
                .name(format!("pty-teardown-{i}"))
                .spawn(move || {
                    // Exits when the channel is closed AND drained (Drop closes the
                    // sender, then `recv_blocking` returns Err once buffered jobs run).
                    while let Ok(job) = rx.recv_blocking() {
                        if let Err(panic) = std::panic::catch_unwind(AssertUnwindSafe(|| {
                            Self::run_teardown_job(job);
                        })) {
                            log::error!("PTY teardown worker panicked: {}", format_panic(&*panic));
                        }
                        tracker.completed();
                    }
                })
            {
                log::error!("failed to spawn teardown worker {i}: {e}");
            }
        }
        // Drop our copy of the receiver so the only receivers are the workers.
        drop(teardown_rx);

        (
            Self {
                terminals: Arc::new(Mutex::new(HashMap::new())),
                instances: Arc::new(Mutex::new(PtyInstances::default())),
                instance_changed: Arc::new(Condvar::new()),
                next_generation: AtomicU64::new(1),
                event_tx: tx,
                session_backend,
                #[cfg(windows)]
                session_backend_preference: backend,
                output_sink: Arc::new(Mutex::new(None)),
                extra_env: Mutex::new(Vec::new()),
                teardown_tx: Some(teardown_tx),
                teardown_tracker,
            },
            rx,
        )
    }

    /// Execute one teardown job on a worker thread. This is exactly what the old
    /// per-call detached closures did: reap the handle's reader/writer threads (if a
    /// handle is present), then run the session kill (only for `KillSession` jobs).
    fn run_teardown_job(mut job: TeardownJob) {
        if let Some(handle) = job.handle {
            Self::shutdown_handle(handle);
        }
        match job.kind {
            // Process already EOF'd; nothing to SIGTERM from our side.
            TeardownKind::ReapOnly => {}
            TeardownKind::KillSession {
                session_backend,
                session_name,
                #[cfg(windows)]
                wsl_distro,
                #[cfg(windows)]
                wsl_backend,
            } => {
                // On Windows, if this was a WSL terminal with a session backend,
                // kill the session inside WSL instead of on the host.
                #[cfg(windows)]
                {
                    if let Some(backend) = wsl_backend {
                        crate::session_backend::kill_wsl_session(
                            backend,
                            wsl_distro.as_deref(),
                            &session_name,
                        );
                        return;
                    }
                }
                session_backend.kill_session(&session_name);
            }
        }
        drop(job.pending_session_kill.take());
    }

    /// Set the output sink for streaming PTY output to external consumers.
    /// Must be called after construction, before spawning terminals.
    pub fn set_output_sink(&self, sink: Arc<dyn PtyOutputSink>) {
        *self.output_sink.lock() = Some(sink);
    }

    /// Set the extra environment overrides applied to every spawned PTY.
    /// `Some(val)` sets the variable; `None` removes it from the inherited
    /// environment. Replaces any previously configured overrides.
    pub fn set_extra_env(&self, env: Vec<(String, Option<String>)>) {
        *self.extra_env.lock() = env;
    }

    /// Return whether an event belongs to the currently attached PTY instance.
    pub fn is_current_generation(&self, terminal_id: &str, generation: PtyGeneration) -> bool {
        self.instances.lock().is_current(terminal_id, generation)
    }

    /// Return the current generation for diagnostics and event-routing tests.
    pub fn current_generation(&self, terminal_id: &str) -> Option<PtyGeneration> {
        self.instances.lock().current.get(terminal_id).copied()
    }

    /// Create a new terminal with a PTY process (uses system default shell)
    #[allow(dead_code)] // Kept for API compatibility, prefer create_terminal_with_shell
    pub fn create_terminal(&self, cwd: &str) -> Result<String> {
        self.create_terminal_with_shell(cwd, None)
    }

    /// Create a new terminal with a specific shell type
    pub fn create_terminal_with_shell(
        &self,
        cwd: &str,
        shell: Option<&ShellType>,
    ) -> Result<String> {
        let terminal_id = uuid::Uuid::new_v4().to_string();
        self.create_terminal_with_id(&terminal_id, cwd, shell)?;
        Ok(terminal_id)
    }

    /// Create or reconnect to a terminal (uses system default shell)
    /// If terminal_id is provided and session backend supports persistence,
    /// it will try to reconnect to an existing session.
    #[allow(dead_code)] // Kept for API compatibility, prefer create_or_reconnect_terminal_with_shell
    pub fn create_or_reconnect_terminal(
        &self,
        terminal_id: Option<&str>,
        cwd: &str,
    ) -> Result<String> {
        self.create_or_reconnect_terminal_with_shell(terminal_id, cwd, None)
    }

    /// Create or reconnect to a terminal with a specific shell type
    pub fn create_or_reconnect_terminal_with_shell(
        &self,
        terminal_id: Option<&str>,
        cwd: &str,
        shell: Option<&ShellType>,
    ) -> Result<String> {
        match terminal_id {
            Some(id) => {
                // Check if we already have this terminal running
                if self.terminals.lock().contains_key(id) {
                    return Ok(id.to_string());
                }
                // Try to reconnect or create with this ID
                self.create_terminal_with_id(id, cwd, shell)?;
                Ok(id.to_string())
            }
            None => self.create_terminal_with_shell(cwd, shell),
        }
    }

    /// Internal: create a terminal with a specific ID
    fn create_terminal_with_id(
        &self,
        terminal_id: &str,
        cwd: &str,
        shell: Option<&ShellType>,
    ) -> Result<()> {
        let generation = PtyGeneration(self.next_generation.fetch_add(1, Ordering::Relaxed));
        let mut instances = self.instances.lock();
        while instances.has_pending_session_kill(terminal_id)
            || instances.creating.contains_key(terminal_id)
        {
            self.instance_changed.wait(&mut instances);
        }
        if instances.current.contains_key(terminal_id) {
            if self.terminals.lock().contains_key(terminal_id) {
                return Ok(());
            }
            instances.current.remove(terminal_id);
        }
        instances.begin_creation(terminal_id, generation);
        drop(instances);
        let mut reservation = PtyReservation {
            terminal_id,
            generation,
            instances: &self.instances,
            changed: &self.instance_changed,
            active: true,
        };

        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        // Build command based on session backend and shell config
        #[cfg(unix)]
        let mut cmd = self.build_terminal_command(terminal_id, cwd, shell);
        #[cfg(windows)]
        let (mut cmd, wsl_distro, wsl_backend) =
            self.build_terminal_command(terminal_id, cwd, shell);

        // Apply caller-configured env overrides to the PTY unconditionally.
        // These are profile-scoped values (e.g. CLAUDE_CONFIG_DIR) that must
        // override whatever the user's shell rc or the parent process has set.
        // `None` removes the variable so a stale inherited value cannot leak in.
        for (key, val) in &*self.extra_env.lock() {
            match val {
                Some(val) => cmd.env(key, val),
                None => cmd.env_remove(key),
            }
        }

        // Spawn the process
        let child = pair.slave.spawn_command(cmd)?;

        // Get reader and writer. The writer is shared (`Arc<Mutex>`) so query
        // replies can be written synchronously via `write_response`, ahead of the
        // batched writer thread, without racing the querying program's exit.
        let reader = pair.master.try_clone_reader()?;
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(pair.master.take_writer()?));

        let shutdown = Arc::new(PtyShutdownState::new(terminal_id.to_string(), generation));
        let child_pid = child.process_id();

        // Spawn reader thread with panic guard
        let tx = self.event_tx.clone();
        let id = terminal_id.to_string();
        let reader_shutdown = Arc::clone(&shutdown);
        let output_sink = self.output_sink.lock().clone();
        let reader_instances = Arc::clone(&self.instances);
        let (reader_start_tx, reader_start_rx) = mpsc::channel::<()>();
        let reader_handle = std::thread::Builder::new()
            .name(format!(
                "pty-reader-{}",
                &terminal_id[..8.min(terminal_id.len())]
            ))
            .spawn(move || {
                if reader_start_rx.recv().is_err() {
                    return;
                }
                let tx_panic = tx.clone();
                let shutdown_panic = Arc::clone(&reader_shutdown);
                let id_panic = id.clone();
                if let Err(panic) = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    Self::read_loop(
                        id,
                        reader,
                        tx,
                        reader_shutdown,
                        child_pid,
                        output_sink,
                        reader_instances,
                    );
                })) {
                    log::error!("PTY reader thread panicked: {}", format_panic(&*panic));
                    shutdown_panic.mark_broken();
                    let _ = tx_panic.send_blocking(PtyEvent::Exit {
                        terminal_id: id_panic,
                        generation: shutdown_panic.generation,
                        exit_code: None,
                    });
                }
            })?;

        // Create input channel and spawn writer thread with panic guard
        let (input_tx, input_rx) = mpsc::channel::<Vec<u8>>();
        let writer_shutdown = Arc::clone(&shutdown);
        let writer_event_tx = self.event_tx.clone();
        let writer_id = terminal_id.to_string();
        let (writer_start_tx, writer_start_rx) = mpsc::channel::<()>();
        // The batched writer thread shares the writer with `write_response`.
        let writer_for_thread = Arc::clone(&writer);
        let writer_handle = std::thread::Builder::new()
            .name(format!(
                "pty-writer-{}",
                &terminal_id[..8.min(terminal_id.len())]
            ))
            .spawn(move || {
                if writer_start_rx.recv().is_err() {
                    return;
                }
                let tx_panic = writer_event_tx.clone();
                let shutdown_panic = Arc::clone(&writer_shutdown);
                let id_panic = writer_id.clone();
                if let Err(panic) = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    Self::write_loop(
                        writer_for_thread,
                        input_rx,
                        writer_shutdown,
                        writer_event_tx,
                        writer_id,
                    );
                })) {
                    log::error!("PTY writer thread panicked: {}", format_panic(&*panic));
                    shutdown_panic.mark_broken();
                    let _ = tx_panic.send_blocking(PtyEvent::Exit {
                        terminal_id: id_panic,
                        generation: shutdown_panic.generation,
                        exit_code: None,
                    });
                }
            })?;

        // Publish the generation and handle before either worker can emit an event.
        // Taking the locks in lifecycle order keeps exit cleanup from observing a
        // generation without its handle.
        let mut instances = self.instances.lock();
        if !instances.is_creating(terminal_id, generation) {
            drop(instances);
            anyhow::bail!("terminal creation was cancelled before publication");
        }
        self.terminals.lock().insert(
            terminal_id.to_string(),
            PtyHandle {
                generation,
                master: Some(pair.master),
                child,
                input_tx: Some(input_tx),
                writer: Some(writer),
                reader_handle: Some(reader_handle),
                writer_handle: Some(writer_handle),
                shutdown,
                #[cfg(windows)]
                wsl_distro,
                #[cfg(windows)]
                wsl_backend,
            },
        );
        instances.publish(terminal_id, generation);
        drop(instances);
        reservation.publish();
        self.instance_changed.notify_all();
        let _ = reader_start_tx.send(());
        let _ = writer_start_tx.send(());

        Ok(())
    }

    /// Build the command to run in the terminal.
    /// On Unix, returns just the CommandBuilder.
    /// On Windows, also returns WSL distro/backend info for session persistence.
    #[cfg(unix)]
    fn build_terminal_command(
        &self,
        terminal_id: &str,
        cwd: &str,
        shell: Option<&ShellType>,
    ) -> CommandBuilder {
        // Extract custom command from ShellType::Custom{path:<shell>, args:["-c"/"-ic", cmd]}
        // so it can be passed to the session backend
        let custom_command = match shell {
            Some(ShellType::Custom { args, .. })
                if args.len() == 2 && (args[0] == "-c" || args[0] == "-ic") =>
            {
                Some(args[1].as_str())
            }
            _ => None,
        };

        let extra_env = self.extra_env.lock().clone();
        let mut cmd = if let Some((program, args)) = self.session_backend.build_command(
            &self.session_backend.session_name(terminal_id),
            cwd,
            custom_command,
            &extra_env,
        ) {
            let mut cmd = CommandBuilder::new(program);
            for arg in args {
                cmd.arg(arg);
            }
            // For screen, we need to set cwd separately as it doesn't have -c flag
            if matches!(self.session_backend, ResolvedBackend::Screen) {
                cmd.cwd(cwd);
            }
            cmd
        } else {
            // No session backend - use shell config or default
            match shell {
                Some(shell_type) => shell_type.build_command(cwd),
                None => {
                    let mut cmd = CommandBuilder::new_default_prog();
                    cmd.cwd(cwd);
                    cmd
                }
            }
        };

        Self::set_terminal_env(&mut cmd, terminal_id);
        cmd
    }

    /// Build the command to run in the terminal (Windows version).
    /// Returns (cmd, wsl_distro, wsl_backend) for WSL session tracking.
    #[cfg(windows)]
    fn build_terminal_command(
        &self,
        terminal_id: &str,
        cwd: &str,
        shell: Option<&ShellType>,
    ) -> (CommandBuilder, Option<String>, Option<ResolvedBackend>) {
        use crate::session_backend::resolve_for_wsl;
        use crate::shell_config::windows_path_to_wsl;

        // Preserve the exact executable and argv for psmux. In particular,
        // keep-alive hooks require cmd.exe delayed expansion via `/V:ON`.
        let custom_command = match shell {
            Some(ShellType::Custom { path, args }) => Some(SessionCommand::Program {
                program: path,
                args,
            }),
            _ => None,
        };

        // Wrap a non-WSL shell through the host session backend (psmux) when one
        // is available. WSL terminals get their own per-distro backend below
        // because the daemon must live inside WSL, not on the host.
        let wrap_with_host_backend = |fallback: CommandBuilder| -> CommandBuilder {
            if !self.session_backend.supports_persistence() {
                return fallback;
            }
            let session_name = self.session_backend.session_name(terminal_id);
            let extra_env = self.extra_env.lock().clone();
            match self.session_backend.build_command_with_custom(
                &session_name,
                cwd,
                custom_command,
                &extra_env,
            ) {
                Some((program, args)) => {
                    let mut cmd = CommandBuilder::new(program);
                    for arg in args {
                        cmd.arg(arg);
                    }
                    cmd
                }
                None => fallback,
            }
        };

        let (mut cmd, wsl_distro, wsl_backend) = match shell {
            Some(ShellType::Wsl { distro }) => {
                let wsl_backend =
                    resolve_for_wsl(distro.as_deref(), self.session_backend_preference);
                let session_name = wsl_backend.session_name(terminal_id);
                let wsl_cwd = windows_path_to_wsl(cwd);

                if let Some((program, args)) = wsl_backend.build_wsl_session_command(
                    distro.as_deref(),
                    &session_name,
                    &wsl_cwd,
                    custom_command,
                ) {
                    let mut cmd = CommandBuilder::new(program);
                    for arg in args {
                        cmd.arg(arg);
                    }
                    (cmd, distro.clone(), Some(wsl_backend))
                } else {
                    (
                        ShellType::Wsl {
                            distro: distro.clone(),
                        }
                        .build_command(cwd),
                        distro.clone(),
                        None,
                    )
                }
            }
            Some(shell_type) => (
                wrap_with_host_backend(shell_type.build_command(cwd)),
                None,
                None,
            ),
            None => {
                let mut default_cmd = CommandBuilder::new_default_prog();
                default_cmd.cwd(cwd);
                (wrap_with_host_backend(default_cmd), None, None)
            }
        };

        Self::set_terminal_env(&mut cmd, terminal_id);
        (cmd, wsl_distro, wsl_backend)
    }

    /// Set common terminal environment variables on a command.
    fn set_terminal_env(cmd: &mut CommandBuilder, terminal_id: &str) {
        // Allow processes inside the terminal to identify which Okena terminal they run in
        cmd.env("OKENA_TERMINAL_ID", terminal_id);

        // Set TERM environment variable - required for proper terminal operation
        // especially when running as a macOS app bundle which doesn't inherit shell environment
        cmd.env("TERM", "xterm-256color");
        // COLORTERM enables 24-bit truecolor support in many applications
        cmd.env("COLORTERM", "truecolor");

        // Ensure UTF-8 locale for child processes. macOS app bundles launched from
        // Finder/Spotlight don't inherit shell environment, so LANG defaults to
        // C/POSIX (ASCII-only). This breaks non-ASCII text in shells and CLI tools.
        #[cfg(not(windows))]
        if std::env::var("LANG").is_err() {
            cmd.env("LANG", "en_US.UTF-8");
        }

        // Extend PATH for child processes. Desktop entries and app bundles start
        // with a minimal PATH missing user tools (~/.cargo/bin, ~/.bun/bin, etc.)
        #[cfg(not(windows))]
        cmd.env("PATH", get_extended_path());
    }

    /// Read loop for PTY output
    fn read_loop(
        terminal_id: String,
        mut reader: Box<dyn Read + Send>,
        tx: Sender<PtyEvent>,
        shutdown: Arc<PtyShutdownState>,
        child_pid: Option<u32>,
        output_sink: Option<Arc<dyn PtyOutputSink>>,
        instances: Arc<Mutex<PtyInstances>>,
    ) {
        // Use larger buffer like alacritty (they use 1MB, we use 64KB)
        let mut buf = [0u8; 65536];
        loop {
            if shutdown.is_broken() {
                log::debug!("PTY reader {} stopping: shutdown signaled", terminal_id);
                break;
            }
            match reader.read(&mut buf) {
                Ok(0) => {
                    // EOF - process exited, try to get exit code
                    let exit_code = child_pid.and_then(wait_for_exit_code);
                    let _ = tx.send_blocking(PtyEvent::Exit {
                        terminal_id,
                        generation: shutdown.generation,
                        exit_code,
                    });
                    break;
                }
                Ok(n) => {
                    if shutdown.is_broken() {
                        break;
                    }
                    let data = buf[..n].to_vec();
                    log::debug!(
                        "PTY {} received {} bytes: {:?}",
                        terminal_id,
                        n,
                        String::from_utf8_lossy(&data[..n.min(100)])
                    );
                    // Broadcast to external consumers immediately (bypasses UI event loop)
                    let sequence = {
                        let instances = instances.lock();
                        if !instances.is_current(&terminal_id, shutdown.generation) {
                            break;
                        }
                        output_sink
                            .as_ref()
                            .map_or(0, |sink| sink.publish(terminal_id.clone(), data.clone()))
                    };
                    // send_blocking will block when channel is full (backpressure)
                    if tx
                        .send_blocking(PtyEvent::Data {
                            terminal_id: terminal_id.clone(),
                            generation: shutdown.generation,
                            data,
                            sequence,
                        })
                        .is_err()
                    {
                        // Receiver dropped - app is shutting down
                        break;
                    }
                }
                Err(e) => {
                    if !shutdown.is_broken() {
                        log::error!("PTY read error: {}", e);
                    }
                    let exit_code = child_pid.and_then(wait_for_exit_code);
                    let _ = tx.send_blocking(PtyEvent::Exit {
                        terminal_id,
                        generation: shutdown.generation,
                        exit_code,
                    });
                    break;
                }
            }
        }
    }

    /// Write loop for PTY input - batches writes for better performance.
    /// Shares the writer with `write_response` (query replies) via the `Mutex`;
    /// the lock is held only for the duration of each `write_all`.
    fn write_loop(
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
        rx: mpsc::Receiver<Vec<u8>>,
        shutdown: Arc<PtyShutdownState>,
        event_tx: Sender<PtyEvent>,
        terminal_id: String,
    ) {
        // Loop exits when the channel is closed (`recv` returns Err).
        while let Ok(first) = rx.recv() {
            // Collect any additional pending messages (non-blocking)
            let mut batch = first;
            while let Ok(data) = rx.try_recv() {
                batch.extend(data);
            }

            // Write the batched data
            if let Err(e) = writer.lock().write_all(&batch) {
                log::error!("Failed to write to PTY {}: {}", terminal_id, e);
                shutdown.mark_broken();
                let _ = event_tx.send_blocking(PtyEvent::Exit {
                    terminal_id,
                    generation: shutdown.generation,
                    exit_code: None,
                });
                break;
            }
        }
    }

    /// Send input to a terminal
    /// Input is sent through a channel to a dedicated writer thread,
    /// which batches writes for better performance.
    pub fn send_input(&self, terminal_id: &str, data: &[u8]) {
        if let Some(handle) = self.terminals.lock().get(terminal_id)
            && let Some(input_tx) = handle.input_tx.as_ref()
        {
            let _ = input_tx.send(data.to_vec());
        }
    }

    /// Write a query reply straight to the PTY master + flush, bypassing the
    /// batched input channel and writer-thread scheduling. This shrinks the
    /// window between a program's Device-Attributes/cursor query and its exit
    /// back to the shell, so the reply reaches the program instead of leaking to
    /// the shell prompt (e.g. a stray `6c` after closing nvim). The writer `Arc`
    /// is cloned out under the registry lock, which is released before the
    /// (potentially blocking) PTY write.
    pub fn write_response(&self, terminal_id: &str, data: &[u8]) {
        let writer = {
            let terminals = self.terminals.lock();
            terminals.get(terminal_id).and_then(|h| h.writer.clone())
        };
        if let Some(writer) = writer {
            let mut w = writer.lock();
            if let Err(e) = w.write_all(data).and_then(|_| w.flush()) {
                log::debug!("write_response to PTY {} failed: {}", terminal_id, e);
            }
        }
    }

    /// Resize a terminal
    pub fn resize(&self, terminal_id: &str, cols: u16, rows: u16) {
        if let Some(handle) = self.terminals.lock().get(terminal_id)
            && let Some(master) = handle.master.as_ref()
            && let Err(e) = master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
        {
            log::error!("Failed to resize PTY: {}", e);
        }
        // Notify remote clients about the resize so they can update their grids.
        // Carry the current resize authority so a client knows whether this
        // resize comes from the origin's local user reclaiming control — in
        // which case the client must stop re-asserting its own size.
        if let Some(sink) = self.output_sink.lock().as_ref() {
            let authority = crate::terminal::resize_authority_snapshot(terminal_id);
            log::debug!(
                "pty resize publish: terminal={terminal_id} {cols}x{rows} local={} owner={:?}",
                authority.local,
                authority.remote_owner_id
            );
            sink.publish_resize(
                terminal_id.to_string(),
                cols,
                rows,
                authority.local,
                authority.remote_owner_id,
            );
        }
    }

    /// Kill a terminal
    /// Also kills the underlying tmux/screen session if applicable
    pub fn kill(&self, terminal_id: &str) {
        let (handle, exited) = {
            let mut instances = self.instances.lock();
            while instances.creating.contains_key(terminal_id) {
                self.instance_changed.wait(&mut instances);
            }
            let mut terminals = self.terminals.lock();
            let handle = terminals.remove(terminal_id);
            if let Some(handle) = handle.as_ref() {
                instances.remove_current(terminal_id, handle.generation);
            }
            let exited = instances.exited.remove(terminal_id);
            instances.queue_session_kill(terminal_id);
            (handle, exited)
        };
        self.enqueue_session_kill(terminal_id, handle, exited);
    }

    /// Kill the persistent session only if this exact exited generation still
    /// owns the logical terminal ID. A reconnect either replaces the exit claim
    /// or waits for the queued backend kill to finish.
    pub fn kill_exited(&self, terminal_id: &str, generation: PtyGeneration) -> bool {
        let (handle, exited) = {
            let mut instances = self.instances.lock();
            let Some(exited) = instances.claim_exited(terminal_id, generation) else {
                return false;
            };
            let mut terminals = self.terminals.lock();
            let handle = terminals
                .get(terminal_id)
                .is_some_and(|handle| handle.generation == generation)
                .then(|| terminals.remove(terminal_id))
                .flatten();
            instances.queue_session_kill(terminal_id);
            (handle, exited)
        };
        self.enqueue_session_kill(terminal_id, handle, Some(exited));
        true
    }

    fn enqueue_session_kill(
        &self,
        terminal_id: &str,
        handle: Option<PtyHandle>,
        _exited: Option<ExitedPty>,
    ) {
        let session_backend = self.session_backend;
        let session_name = session_backend.session_name(terminal_id);

        // Read WSL info before moving the handle
        #[cfg(windows)]
        let (wsl_distro, wsl_backend) =
            Self::wsl_teardown_target(handle.as_ref(), _exited.as_ref());

        let job = TeardownJob {
            handle,
            kind: TeardownKind::KillSession {
                session_backend,
                session_name,
                #[cfg(windows)]
                wsl_distro,
                #[cfg(windows)]
                wsl_backend,
            },
            pending_session_kill: Some(PendingSessionKill {
                terminal_id: terminal_id.to_string(),
                instances: Arc::clone(&self.instances),
                changed: Arc::clone(&self.instance_changed),
            }),
        };
        self.enqueue_teardown(job);
    }

    #[cfg(windows)]
    fn wsl_teardown_target(
        handle: Option<&PtyHandle>,
        exited: Option<&ExitedPty>,
    ) -> (Option<String>, Option<ResolvedBackend>) {
        let distro = handle
            .and_then(|handle| handle.wsl_distro.clone())
            .or_else(|| exited.and_then(|exited| exited.wsl_distro.clone()));
        let backend = handle
            .and_then(|handle| handle.wsl_backend)
            .or_else(|| exited.and_then(|exited| exited.wsl_backend));
        (distro, backend)
    }

    /// Hand a teardown job to the shared worker pool. If the channel is closed
    /// (only happens once `Drop` has taken the sender during quit), run the teardown
    /// inline so a handle is never silently leaked — better to block briefly on the
    /// calling thread than to drop reader/writer threads on the floor.
    fn enqueue_teardown(&self, job: TeardownJob) {
        self.teardown_tracker.queued();
        match self.teardown_tx.as_ref() {
            Some(tx) => {
                if let Err(e) = tx.send_blocking(job) {
                    log::warn!("teardown channel closed; running teardown inline");
                    self.run_tracked_teardown(e.into_inner());
                }
            }
            // Sender already taken by Drop — fall back to inline teardown.
            None => {
                self.run_tracked_teardown(job);
            }
        }
    }

    fn run_tracked_teardown(&self, job: TeardownJob) {
        if let Err(panic) =
            std::panic::catch_unwind(AssertUnwindSafe(|| Self::run_teardown_job(job)))
        {
            log::error!("PTY teardown panicked: {}", format_panic(&*panic));
        }
        self.teardown_tracker.completed();
    }

    /// Block until every teardown job queued before this call has completed.
    ///
    /// Normal kill paths remain asynchronous; graceful shutdown calls this once
    /// after enqueueing the persistent-session kills it must not abandon.
    pub fn flush_teardown(&self) {
        self.teardown_tracker.flush();
    }

    /// Perform coordinated shutdown of a single PTY handle
    fn shutdown_handle(mut handle: PtyHandle) {
        let id = &handle.shutdown.terminal_id;

        // 1. Signal shutdown to threads
        handle.shutdown.mark_broken();

        // 2. Kill child process - closes PTY slave, reader gets EOF
        if let Err(e) = handle.child.kill() {
            log::warn!("Failed to kill PTY process {}: {}", id, e);
        }

        // 3. Drop input_tx - writer gets Err from rx.recv()
        drop(handle.input_tx.take());

        // 4. Drop master - safety net to unblock reader if still stuck
        drop(handle.master.take());

        // 5. Join writer thread (should exit quickly after input_tx drop)
        if let Some(h) = handle.writer_handle.take()
            && let Err(e) = h.join()
        {
            log::warn!("PTY writer thread panicked on join: {}", format_panic(&*e));
        }

        // 6. Join reader thread (should exit after child kill + master drop)
        if let Some(h) = handle.reader_handle.take()
            && let Err(e) = h.join()
        {
            log::warn!("PTY reader thread panicked on join: {}", format_panic(&*e));
        }

        // 7. Reap the child to prevent a zombie. The reader normally reaps via
        //    `wait_for_exit_code` on EOF, but that is a bounded `WNOHANG` poll that
        //    gives up if the SIGKILL'd child is briefly unreapable (e.g. stuck in
        //    D-state on slow IO). Now that the reader has joined, a blocking wait
        //    guarantees the PID is reaped. If the reader already reaped it via raw
        //    `waitpid`, this just returns ECHILD, which is harmless.
        if let Err(e) = handle.child.wait() {
            log::debug!("PTY child {} already reaped or wait failed: {}", id, e);
        }
    }

    /// Detach from all terminals without killing sessions
    /// Sessions will persist and can be reconnected on next app start
    pub fn detach_all(&self) {
        // Drain all handles in lifecycle lock order, then release both locks
        // before joining worker threads.
        let mut instances = self.instances.lock();
        let handles: Vec<PtyHandle> = self.terminals.lock().drain().map(|(_, h)| h).collect();
        for handle in &handles {
            instances.remove_current(&handle.shutdown.terminal_id, handle.generation);
        }
        drop(instances);
        for handle in handles {
            Self::shutdown_handle(handle);
        }
    }

    /// Get the shell process PID for a terminal
    pub fn get_shell_pid(&self, terminal_id: &str) -> Option<u32> {
        self.terminals
            .lock()
            .get(terminal_id)
            .and_then(|h| h.child.process_id())
    }

    /// Get the real foreground shell pid for this terminal, resolving through
    /// session-backend proxies (dtach / tmux). For plain PTYs this is the same
    /// as `get_shell_pid`. For dtach, walks from the daemon to its direct child
    /// (the actual shell). For tmux, the pane pid returned by `list-panes` IS
    /// the shell pid. Callers get a pid they can pgrep / `/proc`-inspect for
    /// running children.
    pub fn get_foreground_shell_pid(&self, terminal_id: &str) -> Option<u32> {
        #[cfg(unix)]
        {
            match self.session_backend {
                ResolvedBackend::Dtach => {
                    if let Some(daemon) =
                        self.get_dtach_service_pids(terminal_id).into_iter().next()
                    {
                        return first_proc_child(daemon).or(Some(daemon));
                    }
                }
                ResolvedBackend::Tmux => {
                    if let Some(pane) = self.get_tmux_service_pids(terminal_id).into_iter().next() {
                        return Some(pane);
                    }
                }
                _ => {}
            }
        }
        self.get_shell_pid(terminal_id)
    }

    /// Get root PIDs for port detection.
    /// With session backends (dtach/tmux), the PTY child is the attach process,
    /// not the actual service. This method finds the real service root PID.
    pub fn get_service_pids(&self, terminal_id: &str) -> Vec<u32> {
        #[cfg(unix)]
        {
            match self.session_backend {
                ResolvedBackend::Dtach => {
                    return self.get_dtach_service_pids(terminal_id);
                }
                ResolvedBackend::Tmux => {
                    return self.get_tmux_service_pids(terminal_id);
                }
                _ => {}
            }
        }
        self.get_shell_pid(terminal_id).into_iter().collect()
    }

    /// Find the dtach daemon PID holding the session socket, excluding the
    /// attach PID. Uses the /proc-based socket scan (no `lsof` subprocess on
    /// Linux — a per-poll `lsof -t` was ~1s each).
    #[cfg(unix)]
    fn get_dtach_service_pids(&self, terminal_id: &str) -> Vec<u32> {
        let session_name = self.session_backend.session_name(terminal_id);
        let socket_path = match self.session_backend.socket_path(&session_name) {
            Some(p) if p.exists() => p,
            _ => return self.get_shell_pid(terminal_id).into_iter().collect(),
        };

        let holders = find_pids_for_unix_sockets(std::slice::from_ref(&socket_path));
        let attach_pid = self.get_shell_pid(terminal_id);
        let pids: Vec<u32> = holders
            .get(&socket_path)
            .into_iter()
            .flatten()
            .copied()
            .filter(|pid| Some(*pid) != attach_pid)
            .collect();

        if pids.is_empty() {
            self.get_shell_pid(terminal_id).into_iter().collect()
        } else {
            pids
        }
    }

    /// Find the shell PID inside a tmux session pane.
    #[cfg(unix)]
    fn get_tmux_service_pids(&self, terminal_id: &str) -> Vec<u32> {
        let session_name = self.session_backend.session_name(terminal_id);
        let output = match crate::process::safe_output(crate::process::command("tmux").args([
            "list-panes",
            "-t",
            &session_name,
            "-F",
            "#{pane_pid}",
        ])) {
            Ok(o) if o.status.success() => o,
            _ => return self.get_shell_pid(terminal_id).into_iter().collect(),
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let pids: Vec<u32> = stdout
            .lines()
            .filter_map(|line| line.trim().parse::<u32>().ok())
            .collect();

        if pids.is_empty() {
            self.get_shell_pid(terminal_id).into_iter().collect()
        } else {
            pids
        }
    }

    /// Batch version of `get_service_pids` for multiple terminals at once.
    /// On Linux with dtach, reads `/proc` once instead of spawning `lsof` per terminal.
    pub fn get_batch_service_pids(&self, terminal_ids: &[&str]) -> HashMap<String, Vec<u32>> {
        #[cfg(unix)]
        {
            if self.session_backend == ResolvedBackend::Dtach {
                return self.get_batch_dtach_service_pids(terminal_ids);
            }
        }
        // Fallback: call per-terminal method
        terminal_ids
            .iter()
            .map(|tid| (tid.to_string(), self.get_service_pids(tid)))
            .collect()
    }

    /// Batch dtach PID lookup. On Linux, reads `/proc/net/unix` + `/proc/*/fd/`
    /// once for all sockets. On other Unix, falls back to lsof per terminal.
    #[cfg(unix)]
    fn get_batch_dtach_service_pids(&self, terminal_ids: &[&str]) -> HashMap<String, Vec<u32>> {
        // Collect socket paths for all terminals
        let mut socket_to_terminal: HashMap<std::path::PathBuf, &str> = HashMap::new();
        let mut attach_pids: HashMap<&str, Option<u32>> = HashMap::new();

        for &tid in terminal_ids {
            let session_name = self.session_backend.session_name(tid);
            if let Some(p) = self.session_backend.socket_path(&session_name)
                && p.exists()
            {
                socket_to_terminal.insert(p, tid);
                attach_pids.insert(tid, self.get_shell_pid(tid));
            }
        }

        // Resolve PIDs for all sockets at once
        let socket_pids =
            find_pids_for_unix_sockets(&socket_to_terminal.keys().cloned().collect::<Vec<_>>());

        // Build result map
        let mut result: HashMap<String, Vec<u32>> = HashMap::new();
        for &tid in attach_pids.keys() {
            let session_name = self.session_backend.session_name(tid);
            let socket_path = match self.session_backend.socket_path(&session_name) {
                Some(p) => p,
                None => {
                    result.insert(
                        tid.to_string(),
                        self.get_shell_pid(tid).into_iter().collect(),
                    );
                    continue;
                }
            };

            let attach_pid = attach_pids.get(tid).copied().flatten();
            let pids: Vec<u32> = socket_pids
                .get(&socket_path)
                .map(|pids| {
                    pids.iter()
                        .copied()
                        .filter(|pid| Some(*pid) != attach_pid)
                        .collect()
                })
                .unwrap_or_default();

            if pids.is_empty() {
                result.insert(
                    tid.to_string(),
                    self.get_shell_pid(tid).into_iter().collect(),
                );
            } else {
                result.insert(tid.to_string(), pids);
            }
        }

        // Terminals without a valid socket path
        for &tid in terminal_ids {
            result
                .entry(tid.to_string())
                .or_insert_with(|| self.get_shell_pid(tid).into_iter().collect());
        }

        result
    }

    /// Check if the session backend handles mouse events (tmux with mouse on)
    pub fn uses_mouse_backend(&self) -> bool {
        matches!(self.session_backend, ResolvedBackend::Tmux)
    }

    /// Capture the terminal buffer to a file (only works with tmux backend)
    /// Returns the path to the captured file, or None if not using tmux
    pub fn capture_buffer(&self, terminal_id: &str) -> Option<std::path::PathBuf> {
        // Check for WSL tmux first (Windows only)
        #[cfg(windows)]
        {
            let terminals = self.terminals.lock();
            if let Some(handle) = terminals.get(terminal_id) {
                if matches!(handle.wsl_backend, Some(ResolvedBackend::Tmux)) {
                    let session_name = ResolvedBackend::Tmux.session_name(terminal_id);
                    let output_path = std::env::temp_dir().join(format!(
                        "terminal-{}.txt",
                        &terminal_id[..8.min(terminal_id.len())]
                    ));
                    let distro = handle.wsl_distro.clone();
                    drop(terminals); // Release lock before subprocess call

                    let mut cmd = crate::process::command("wsl.exe");
                    if let Some(d) = &distro {
                        cmd.args(["-d", d]);
                    }
                    cmd.args([
                        "--",
                        "tmux",
                        "capture-pane",
                        "-t",
                        &session_name,
                        "-p",
                        "-S",
                        "-",
                    ]);
                    return match crate::process::safe_output(&mut cmd) {
                        Ok(output) if output.status.success() => {
                            match std::fs::write(&output_path, &output.stdout) {
                                Ok(_) => {
                                    log::info!("Captured WSL terminal buffer to {:?}", output_path);
                                    Some(output_path)
                                }
                                Err(e) => {
                                    log::error!("Failed to write capture file: {}", e);
                                    None
                                }
                            }
                        }
                        Ok(output) => {
                            log::error!(
                                "WSL tmux capture-pane failed: {}",
                                String::from_utf8_lossy(&output.stderr)
                            );
                            None
                        }
                        Err(e) => {
                            log::error!("Failed to run WSL tmux capture-pane: {}", e);
                            None
                        }
                    };
                }
            }
        }

        if !matches!(self.session_backend, ResolvedBackend::Tmux) {
            log::warn!("Buffer capture only supported with tmux backend");
            return None;
        }

        let session_name = self.session_backend.session_name(terminal_id);
        let output_path = std::env::temp_dir().join(format!(
            "terminal-{}.txt",
            &terminal_id[..8.min(terminal_id.len())]
        ));

        // Use tmux capture-pane to get the entire scrollback buffer
        let result = crate::process::safe_output(crate::process::command("tmux").args([
            "capture-pane",
            "-t",
            &session_name,
            "-p", // output to stdout
            "-S",
            "-", // start from beginning of scrollback
        ]));

        match result {
            Ok(output) if output.status.success() => {
                match std::fs::write(&output_path, &output.stdout) {
                    Ok(_) => {
                        log::info!("Captured terminal buffer to {:?}", output_path);
                        Some(output_path)
                    }
                    Err(e) => {
                        log::error!("Failed to write capture file: {}", e);
                        None
                    }
                }
            }
            Ok(output) => {
                log::error!(
                    "tmux capture-pane failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                None
            }
            Err(e) => {
                log::error!("Failed to run tmux capture-pane: {}", e);
                None
            }
        }
    }

    /// Check if buffer capture is supported (tmux backend)
    pub fn supports_buffer_capture(&self) -> bool {
        matches!(self.session_backend, ResolvedBackend::Tmux)
    }

    /// Clean up a PtyHandle after the process exited naturally (reader got EOF).
    /// Removes the handle from the internal map and joins threads in the background.
    pub fn cleanup_exited(&self, terminal_id: &str, generation: PtyGeneration) -> bool {
        let mut instances = self.instances.lock();
        if !instances.claim_exit(terminal_id, generation) {
            return false;
        }
        let handle = {
            let mut terminals = self.terminals.lock();
            if terminals
                .get(terminal_id)
                .is_some_and(|handle| handle.generation == generation)
            {
                terminals.remove(terminal_id)
            } else {
                None
            }
        };
        #[cfg(windows)]
        if let Some(handle) = handle.as_ref() {
            instances.record_exited_wsl_metadata(
                terminal_id,
                generation,
                handle.wsl_distro.clone(),
                handle.wsl_backend,
            );
        }
        drop(instances);
        if let Some(handle) = handle {
            // Process already EOF'd — only reap the reader/writer threads. The later
            // `kill()` in the exit-events loop does the session kill (and finds the
            // handle already gone here).
            self.enqueue_teardown(TeardownJob {
                handle: Some(handle),
                kind: TeardownKind::ReapOnly,
                pending_session_kill: None,
            });
        }
        true
    }
}

impl crate::terminal::TerminalTransport for PtyManager {
    fn send_input(&self, terminal_id: &str, data: &[u8]) {
        self.send_input(terminal_id, data)
    }

    fn send_response(&self, terminal_id: &str, data: &[u8]) {
        self.write_response(terminal_id, data)
    }

    fn resize(&self, terminal_id: &str, cols: u16, rows: u16) {
        self.resize(terminal_id, cols, rows)
    }

    fn uses_mouse_backend(&self) -> bool {
        self.uses_mouse_backend()
    }
}

impl Drop for PtyManager {
    fn drop(&mut self) {
        // On drop, just detach - don't kill sessions
        // This allows sessions to persist across app restarts
        self.detach_all();

        // Close the teardown channel so the worker threads drain any buffered jobs
        // and then exit on `Closed`. We intentionally do NOT join the workers here:
        // teardown can block on `lsof`/`tmux kill-session`/`waitpid`, and joining
        // would risk stalling app quit on a hung subprocess. This matches the prior
        // behavior of detached-per-call threads (also never joined). As a result,
        // teardown of already-enqueued jobs is best-effort at quit — the process may
        // exit before slow jobs finish, which is acceptable for graceful detach.
        drop(self.teardown_tx.take());
    }
}

/// Try to retrieve the exit code for a process that has exited.
/// Uses `waitpid` on Unix to get the actual exit status.
fn wait_for_exit_code(pid: u32) -> Option<u32> {
    #[cfg(unix)]
    {
        // The process should have exited by now (reader got EOF).
        // Try a few times with small delays in case it hasn't fully terminated yet.
        for _ in 0..10 {
            let mut status: libc::c_int = 0;
            let result = unsafe { libc::waitpid(pid as i32, &mut status, libc::WNOHANG) };
            if result > 0 {
                if libc::WIFEXITED(status) {
                    return Some(libc::WEXITSTATUS(status) as u32);
                }
                // Killed by signal — no exit code
                return None;
            }
            if result < 0 {
                // ECHILD — already reaped by someone else
                return None;
            }
            // result == 0: not exited yet, wait briefly
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

/// Find which PIDs have the given Unix sockets open.
///
/// On Linux, reads `/proc/net/unix` to map socket paths to inode numbers,
/// then scans `/proc/*/fd/` to find PIDs holding those inodes.
/// On macOS, uses `libproc` to scan each process's socket fds — no subprocess.
/// On other Unix systems, falls back to a single `lsof` invocation.
#[cfg(unix)]
pub(crate) fn find_pids_for_unix_sockets(
    socket_paths: &[std::path::PathBuf],
) -> HashMap<std::path::PathBuf, Vec<u32>> {
    if socket_paths.is_empty() {
        return HashMap::new();
    }

    #[cfg(target_os = "linux")]
    {
        find_pids_for_unix_sockets_linux(socket_paths)
    }

    #[cfg(target_os = "macos")]
    {
        crate::macos_proc::pids_holding_unix_sockets(socket_paths)
    }

    #[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
    {
        find_pids_for_unix_sockets_lsof(socket_paths)
    }
}

/// Linux implementation: read `/proc/net/unix` and `/proc/*/fd/` — no subprocess spawning.
#[cfg(target_os = "linux")]
fn find_pids_for_unix_sockets_linux(
    socket_paths: &[std::path::PathBuf],
) -> HashMap<std::path::PathBuf, Vec<u32>> {
    // Step 1: Read /proc/net/unix to find inodes for our socket paths.
    // Format: "Num RefCount Protocol Flags Type St Inode Path"
    let proc_net = match std::fs::read_to_string("/proc/net/unix") {
        Ok(s) => s,
        Err(_) => return HashMap::new(),
    };

    // Build a set of canonical socket paths for fast lookup
    let canonical_paths: HashMap<std::path::PathBuf, &std::path::PathBuf> = socket_paths
        .iter()
        .filter_map(|p| std::fs::canonicalize(p).ok().map(|c| (c, p)))
        .collect();

    // Map inode -> original socket path
    let mut inode_to_path: HashMap<u64, &std::path::PathBuf> = HashMap::new();
    for line in proc_net.lines().skip(1) {
        // Fields are space-separated; path is the last field (may be absent)
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 8 {
            continue;
        }
        let inode: u64 = match fields[6].parse() {
            Ok(i) => i,
            Err(_) => continue,
        };
        let path_str = fields[7];
        let path = std::path::Path::new(path_str);

        // Check against canonical paths
        if let Some(&orig) = canonical_paths.get(path) {
            inode_to_path.insert(inode, orig);
        } else if let Ok(canon) = std::fs::canonicalize(path)
            && let Some(&orig) = canonical_paths.get(&canon)
        {
            inode_to_path.insert(inode, orig);
        }
    }

    if inode_to_path.is_empty() {
        return HashMap::new();
    }

    // Step 2: Scan /proc/*/fd/ to find PIDs that hold these inodes.
    let mut result: HashMap<std::path::PathBuf, Vec<u32>> = HashMap::new();

    let proc_dir = match std::fs::read_dir("/proc") {
        Ok(d) => d,
        Err(_) => return HashMap::new(),
    };

    for entry in proc_dir.flatten() {
        let pid: u32 = match entry.file_name().to_str().and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => continue,
        };

        let fd_dir = entry.path().join("fd");
        let fd_entries = match std::fs::read_dir(&fd_dir) {
            Ok(d) => d,
            Err(_) => continue, // permission denied or process gone
        };

        for fd_entry in fd_entries.flatten() {
            // readlink on /proc/<pid>/fd/<n> gives "socket:[<inode>]"
            let link = match std::fs::read_link(fd_entry.path()) {
                Ok(l) => l,
                Err(_) => continue,
            };
            let link_str = match link.to_str() {
                Some(s) => s,
                None => continue,
            };
            // Parse "socket:[12345]"
            if let Some(inode_str) = link_str
                .strip_prefix("socket:[")
                .and_then(|s| s.strip_suffix(']'))
                && let Ok(inode) = inode_str.parse::<u64>()
                && let Some(&socket_path) = inode_to_path.get(&inode)
            {
                result.entry(socket_path.clone()).or_default().push(pid);
            }
            // Early exit if we found all inodes
            // (not worth the bookkeeping for a small set)
        }
    }

    result
}

/// Fallback for non-Linux, non-macOS Unix (e.g. BSD): single `lsof` call for
/// all sockets. macOS uses `crate::macos_proc` instead.
#[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
fn find_pids_for_unix_sockets_lsof(
    socket_paths: &[std::path::PathBuf],
) -> HashMap<std::path::PathBuf, Vec<u32>> {
    // lsof can take multiple file arguments at once
    let mut cmd = crate::process::command("lsof");
    cmd.arg("-t");
    for path in socket_paths {
        cmd.arg(path);
    }

    let output = match crate::process::safe_output(&mut cmd) {
        Ok(o) if o.status.success() => o,
        _ => return HashMap::new(),
    };

    // lsof -t with multiple files just lists PIDs (no file association).
    // We need per-file results, so use full output instead.
    drop(output);

    let mut cmd = crate::process::command("lsof");
    cmd.arg("-F").arg("pn"); // machine-readable: p=PID, n=name fields
    for path in socket_paths {
        cmd.arg(path);
    }

    let output = match crate::process::safe_output(&mut cmd) {
        Ok(o) if o.status.success() => o,
        _ => return HashMap::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut result: HashMap<std::path::PathBuf, Vec<u32>> = HashMap::new();
    let mut current_pid: Option<u32> = None;

    // lsof -F output: lines starting with 'p' = PID, 'n' = name (path)
    for line in stdout.lines() {
        if let Some(pid_str) = line.strip_prefix('p') {
            current_pid = pid_str.parse().ok();
        } else if let Some(name) = line.strip_prefix('n')
            && let Some(pid) = current_pid
        {
            let path = std::path::PathBuf::from(name);
            if socket_paths.contains(&path) {
                result.entry(path).or_default().push(pid);
            }
        }
    }

    result
}

/// Return the first direct child pid of `pid` via `/proc/<pid>/task/<pid>/children`.
/// Used to walk from a dtach daemon down to the actual shell process.
#[cfg(target_os = "linux")]
fn first_proc_child(pid: u32) -> Option<u32> {
    let path = format!("/proc/{}/task/{}/children", pid, pid);
    let contents = std::fs::read_to_string(path).ok()?;
    contents
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
}

#[cfg(target_os = "macos")]
fn first_proc_child(pid: u32) -> Option<u32> {
    crate::macos_proc::first_child_pid(pid)
}

#[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
fn first_proc_child(pid: u32) -> Option<u32> {
    let output = crate::process::safe_output(
        crate::process::command("pgrep").args(["-P", &pid.to_string()]),
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .and_then(|s| s.trim().parse().ok())
}

#[cfg(not(unix))]
fn first_proc_child(_pid: u32) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn generation_rejects_delayed_and_duplicate_exits() {
        let mut instances = PtyInstances::default();
        let old = PtyGeneration(1);
        let current = PtyGeneration(2);

        instances.publish("t1", old);
        instances.publish("t1", current);

        assert!(!instances.claim_exit("t1", old));
        assert!(instances.is_current("t1", current));
        assert!(instances.claim_exit("t1", current));
        assert!(!instances.claim_exit("t1", current));
    }

    #[cfg(windows)]
    #[test]
    fn exited_generation_retains_wsl_teardown_target() {
        let mut instances = PtyInstances::default();
        let generation = PtyGeneration(1);
        instances.publish("wsl-terminal", generation);
        assert!(instances.claim_exit("wsl-terminal", generation));

        instances.record_exited_wsl_metadata(
            "wsl-terminal",
            generation,
            Some("Ubuntu".to_string()),
            Some(ResolvedBackend::Dtach),
        );
        let exited = instances
            .claim_exited("wsl-terminal", generation)
            .expect("exited generation");

        assert_eq!(exited.wsl_distro.as_deref(), Some("Ubuntu"));
        assert_eq!(exited.wsl_backend, Some(ResolvedBackend::Dtach));
        assert_eq!(
            PtyManager::wsl_teardown_target(None, Some(&exited)),
            (Some("Ubuntu".to_string()), Some(ResolvedBackend::Dtach))
        );
    }

    #[test]
    fn reconnect_replaces_old_exit_claim_before_session_kill() {
        let (manager, _events) = PtyManager::new(SessionBackend::None);
        let cwd = std::env::temp_dir().to_string_lossy().into_owned();
        let terminal_id = manager
            .create_or_reconnect_terminal(Some("generation-reconnect"), &cwd)
            .expect("create old generation");
        let old_generation = manager
            .current_generation(&terminal_id)
            .expect("old generation");

        assert!(manager.cleanup_exited(&terminal_id, old_generation));
        manager
            .create_or_reconnect_terminal(Some(&terminal_id), &cwd)
            .expect("reserve new generation");
        let new_generation = manager
            .current_generation(&terminal_id)
            .expect("new generation");

        assert_ne!(old_generation, new_generation);
        assert!(!manager.kill_exited(&terminal_id, old_generation));
        assert!(manager.is_current_generation(&terminal_id, new_generation));
        manager.kill(&terminal_id);
        manager.flush_teardown();
    }

    #[test]
    #[cfg(unix)]
    fn immediate_exit_is_observed_after_handle_publication() {
        let (manager, events) = PtyManager::new(SessionBackend::None);
        let cwd = std::env::temp_dir().to_string_lossy().into_owned();
        let shell = ShellType::Custom {
            path: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "exit 0".to_string()],
        };
        let terminal_id = manager
            .create_terminal_with_shell(&cwd, Some(&shell))
            .expect("create immediate-exit PTY");

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let generation = loop {
            match events.try_recv() {
                Ok(PtyEvent::Exit {
                    terminal_id: exited_id,
                    generation,
                    ..
                }) if exited_id == terminal_id => break generation,
                Ok(_) | Err(async_channel::TryRecvError::Empty)
                    if std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(_) | Err(async_channel::TryRecvError::Empty) => {
                    panic!("immediate process did not emit Exit")
                }
                Err(async_channel::TryRecvError::Closed) => panic!("PTY event channel closed"),
            }
        };

        assert!(manager.cleanup_exited(&terminal_id, generation));
        assert!(!manager.terminals.lock().contains_key(&terminal_id));
        manager.flush_teardown();
    }

    #[derive(Default)]
    struct RecordingSink {
        published: Mutex<Vec<(String, Vec<u8>)>>,
    }

    impl PtyOutputSink for RecordingSink {
        fn publish(&self, terminal_id: String, data: Vec<u8>) -> u64 {
            self.published.lock().push((terminal_id, data));
            1
        }
    }

    #[test]
    fn stale_generation_output_is_not_published_to_sink() {
        let old = PtyGeneration(1);
        let current = PtyGeneration(2);
        let instances = Arc::new(Mutex::new(PtyInstances::default()));
        instances.lock().publish("reconnected", current);
        let shutdown = Arc::new(PtyShutdownState::new("reconnected".to_string(), old));
        let (tx, events) = async_channel::bounded(4);
        let sink = Arc::new(RecordingSink::default());

        PtyManager::read_loop(
            "reconnected".to_string(),
            Box::new(std::io::Cursor::new(b"stale".to_vec())),
            tx,
            shutdown,
            None,
            Some(sink.clone()),
            instances,
        );

        assert!(sink.published.lock().is_empty());
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn teardown_flush_waits_for_pending_work() {
        let tracker = Arc::new(TeardownTracker::default());
        tracker.queued();

        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let waiter_tracker = Arc::clone(&tracker);
        let waiter = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            waiter_tracker.flush();
            done_tx.send(()).unwrap();
        });

        started_rx.recv().unwrap();
        assert!(matches!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        tracker.completed();
        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("flush returns after completion");
        waiter.join().unwrap();
    }
}
