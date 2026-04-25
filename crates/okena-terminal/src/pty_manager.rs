use crate::session_backend::{ResolvedBackend, SessionBackend};
#[cfg(not(windows))]
use crate::session_backend::get_extended_path;
use crate::shell_config::ShellType;
use anyhow::Result;
use async_channel::{Receiver, Sender};
use parking_lot::Mutex;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;

/// Trait for broadcasting PTY output to external consumers (e.g. remote WebSocket clients).
/// Implementations must be thread-safe as this is called from PTY reader threads.
pub trait PtyOutputSink: Send + Sync {
    fn publish(&self, terminal_id: String, data: Vec<u8>);
    fn publish_resize(&self, _terminal_id: String, _cols: u16, _rows: u16) {}
}

/// Events from PTY processes
#[derive(Debug)]
pub enum PtyEvent {
    /// Data received from PTY
    Data { terminal_id: String, data: Vec<u8> },
    /// PTY process exited
    Exit {
        terminal_id: String,
        exit_code: Option<u32>,
    },
}

/// Shared shutdown coordination between reader/writer threads
struct PtyShutdownState {
    broken: AtomicBool,
    terminal_id: String,
}

impl PtyShutdownState {
    fn new(terminal_id: String) -> Self {
        Self {
            broken: AtomicBool::new(false),
            terminal_id,
        }
    }

    fn is_broken(&self) -> bool {
        self.broken.load(Ordering::Relaxed)
    }

    fn mark_broken(&self) {
        self.broken.store(true, Ordering::Relaxed);
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

/// Handle to a single PTY process
struct PtyHandle {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    /// Channel to send input to the writer thread
    input_tx: mpsc::Sender<Vec<u8>>,
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

/// Manages all PTY processes
pub struct PtyManager {
    terminals: Arc<Mutex<HashMap<String, PtyHandle>>>,
    event_tx: Sender<PtyEvent>,
    /// Session backend for persistence (tmux/screen/none)
    session_backend: ResolvedBackend,
    /// Raw user preference (needed for WSL per-terminal resolution)
    #[cfg(windows)]
    session_backend_preference: SessionBackend,
    /// Optional sink for streaming PTY output to external consumers (e.g. remote clients).
    /// Publishing happens directly from reader threads to avoid UI event loop latency.
    output_sink: Arc<Mutex<Option<Arc<dyn PtyOutputSink>>>>,
    /// Extra environment variables injected into every spawned PTY.
    /// Only variables not already set in the spawning process are injected.
    extra_env: Mutex<Vec<(String, String)>>,
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
        if matches!(session_backend, ResolvedBackend::Dtach) {
            if let Err(e) = std::thread::Builder::new()
                .name("dtach-socket-gc".into())
                .spawn(|| {
                    crate::session_backend::cleanup_stale_dtach_sockets();
                })
            {
                log::warn!("failed to spawn dtach cleanup thread: {e}");
            }
        }

        (
            Self {
                terminals: Arc::new(Mutex::new(HashMap::new())),
                event_tx: tx,
                session_backend,
                #[cfg(windows)]
                session_backend_preference: backend,
                output_sink: Arc::new(Mutex::new(None)),
                extra_env: Mutex::new(Vec::new()),
            },
            rx,
        )
    }

    /// Set the output sink for streaming PTY output to external consumers.
    /// Must be called after construction, before spawning terminals.
    pub fn set_output_sink(&self, sink: Arc<dyn PtyOutputSink>) {
        *self.output_sink.lock() = Some(sink);
    }

    /// Set extra environment variables to inject into every spawned PTY.
    /// Variables already present in the process environment are not overwritten.
    /// Replaces any previously configured extra env.
    pub fn set_extra_env(&self, env: Vec<(String, String)>) {
        *self.extra_env.lock() = env;
    }

    /// Create a new terminal with a PTY process (uses system default shell)
    #[allow(dead_code)] // Kept for API compatibility, prefer create_terminal_with_shell
    pub fn create_terminal(&self, cwd: &str) -> Result<String> {
        self.create_terminal_with_shell(cwd, None)
    }

    /// Create a new terminal with a specific shell type
    pub fn create_terminal_with_shell(&self, cwd: &str, shell: Option<&ShellType>) -> Result<String> {
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
    fn create_terminal_with_id(&self, terminal_id: &str, cwd: &str, shell: Option<&ShellType>) -> Result<()> {
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
        let (mut cmd, wsl_distro, wsl_backend) = self.build_terminal_command(terminal_id, cwd, shell);

        // Inject caller-configured env vars into the PTY unconditionally.
        // These are profile-scoped values (e.g. CLAUDE_CONFIG_DIR) that must
        // override whatever the user's shell rc or the parent process has set.
        for (key, val) in &*self.extra_env.lock() {
            cmd.env(key, val);
        }

        // Spawn the process
        let child = pair.slave.spawn_command(cmd)?;

        // Get reader and writer
        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        let shutdown = Arc::new(PtyShutdownState::new(terminal_id.to_string()));
        let child_pid = child.process_id();

        // Spawn reader thread with panic guard
        let tx = self.event_tx.clone();
        let id = terminal_id.to_string();
        let reader_shutdown = Arc::clone(&shutdown);
        let output_sink = self.output_sink.lock().clone();
        let reader_handle = std::thread::Builder::new()
            .name(format!("pty-reader-{}", &terminal_id[..8.min(terminal_id.len())]))
            .spawn(move || {
                let tx_panic = tx.clone();
                let shutdown_panic = Arc::clone(&reader_shutdown);
                let id_panic = id.clone();
                if let Err(panic) = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    Self::read_loop(id, reader, tx, reader_shutdown, child_pid, output_sink);
                })) {
                    log::error!("PTY reader thread panicked: {}", format_panic(&*panic));
                    shutdown_panic.mark_broken();
                    let _ = tx_panic.send_blocking(PtyEvent::Exit {
                        terminal_id: id_panic,
                        exit_code: None,
                    });
                }
            })?;

        // Create input channel and spawn writer thread with panic guard
        let (input_tx, input_rx) = mpsc::channel::<Vec<u8>>();
        let writer_shutdown = Arc::clone(&shutdown);
        let writer_event_tx = self.event_tx.clone();
        let writer_id = terminal_id.to_string();
        let writer_handle = std::thread::Builder::new()
            .name(format!("pty-writer-{}", &terminal_id[..8.min(terminal_id.len())]))
            .spawn(move || {
                let tx_panic = writer_event_tx.clone();
                let shutdown_panic = Arc::clone(&writer_shutdown);
                let id_panic = writer_id.clone();
                if let Err(panic) = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    Self::write_loop(writer, input_rx, writer_shutdown, writer_event_tx, writer_id);
                })) {
                    log::error!("PTY writer thread panicked: {}", format_panic(&*panic));
                    shutdown_panic.mark_broken();
                    let _ = tx_panic.send_blocking(PtyEvent::Exit {
                        terminal_id: id_panic,
                        exit_code: None,
                    });
                }
            })?;

        // Store the handle
        self.terminals.lock().insert(
            terminal_id.to_string(),
            PtyHandle {
                master: pair.master,
                child,
                input_tx,
                reader_handle: Some(reader_handle),
                writer_handle: Some(writer_handle),
                shutdown,
                #[cfg(windows)]
                wsl_distro,
                #[cfg(windows)]
                wsl_backend,
            },
        );

        Ok(())
    }

    /// Build the command to run in the terminal.
    /// On Unix, returns just the CommandBuilder.
    /// On Windows, also returns WSL distro/backend info for session persistence.
    #[cfg(unix)]
    fn build_terminal_command(&self, terminal_id: &str, cwd: &str, shell: Option<&ShellType>) -> CommandBuilder {
        // Extract custom command from ShellType::Custom{path:<shell>, args:["-c"/"-ic", cmd]}
        // so it can be passed to the session backend
        let custom_command = match shell {
            Some(ShellType::Custom { args, .. }) if args.len() == 2 && (args[0] == "-c" || args[0] == "-ic") => {
                Some(args[1].as_str())
            }
            _ => None,
        };

        let extra_env = self.extra_env.lock().clone();
        let mut cmd = if let Some((program, args)) = self
            .session_backend
            .build_command(&self.session_backend.session_name(terminal_id), cwd, custom_command, &extra_env)
        {
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

        // Extract custom command from ShellType::Custom{path:<shell>, args:["-c"/"-ic", cmd]}
        let custom_command = match shell {
            Some(ShellType::Custom { args, .. }) if args.len() == 2 && (args[0] == "-c" || args[0] == "-ic") => {
                Some(args[1].as_str())
            }
            _ => None,
        };

        let (mut cmd, wsl_distro, wsl_backend) = match shell {
            Some(ShellType::Wsl { distro }) => {
                let wsl_backend = resolve_for_wsl(distro.as_deref(), self.session_backend_preference);
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
                        ShellType::Wsl { distro: distro.clone() }.build_command(cwd),
                        distro.clone(),
                        None,
                    )
                }
            }
            Some(shell_type) => (shell_type.build_command(cwd), None, None),
            None => {
                let mut cmd = CommandBuilder::new_default_prog();
                cmd.cwd(cwd);
                (cmd, None, None)
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
                        exit_code,
                    });
                    break;
                }
                Ok(n) => {
                    if shutdown.is_broken() {
                        break;
                    }
                    let data = buf[..n].to_vec();
                    log::debug!("PTY {} received {} bytes: {:?}", terminal_id, n, String::from_utf8_lossy(&data[..n.min(100)]));
                    // Broadcast to external consumers immediately (bypasses UI event loop)
                    if let Some(ref sink) = output_sink {
                        sink.publish(terminal_id.clone(), data.clone());
                    }
                    // send_blocking will block when channel is full (backpressure)
                    if tx.send_blocking(PtyEvent::Data {
                        terminal_id: terminal_id.clone(),
                        data,
                    }).is_err() {
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
                        exit_code,
                    });
                    break;
                }
            }
        }
    }

    /// Write loop for PTY input - batches writes for better performance
    fn write_loop(
        mut writer: Box<dyn Write + Send>,
        rx: mpsc::Receiver<Vec<u8>>,
        shutdown: Arc<PtyShutdownState>,
        event_tx: Sender<PtyEvent>,
        terminal_id: String,
    ) {
        loop {
            // Wait for first message
            let first = match rx.recv() {
                Ok(data) => data,
                Err(_) => break, // Channel closed
            };

            // Collect any additional pending messages (non-blocking)
            let mut batch = first;
            while let Ok(data) = rx.try_recv() {
                batch.extend(data);
            }

            // Write the batched data
            if let Err(e) = writer.write_all(&batch) {
                log::error!("Failed to write to PTY {}: {}", terminal_id, e);
                shutdown.mark_broken();
                let _ = event_tx.send_blocking(PtyEvent::Exit {
                    terminal_id,
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
        if let Some(handle) = self.terminals.lock().get(terminal_id) {
            let _ = handle.input_tx.send(data.to_vec());
        }
    }

    /// Resize a terminal
    pub fn resize(&self, terminal_id: &str, cols: u16, rows: u16) {
        if let Some(handle) = self.terminals.lock().get(terminal_id) {
            if let Err(e) = handle.master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            }) {
                log::error!("Failed to resize PTY: {}", e);
            }
        }
        // Notify remote clients about the resize so they can update their grids
        if let Some(sink) = self.output_sink.lock().as_ref() {
            sink.publish_resize(terminal_id.to_string(), cols, rows);
        }
    }

    /// Kill a terminal
    /// Also kills the underlying tmux/screen session if applicable
    pub fn kill(&self, terminal_id: &str) {
        // Remove handle from map immediately (fast, non-blocking)
        let handle = self.terminals.lock().remove(terminal_id);
        let session_backend = self.session_backend;
        let session_name = session_backend.session_name(terminal_id);
        let short_id = terminal_id[..8.min(terminal_id.len())].to_string();

        // Read WSL info before moving the handle
        #[cfg(windows)]
        let wsl_distro = handle.as_ref().and_then(|h| h.wsl_distro.clone());
        #[cfg(windows)]
        let wsl_backend = handle.as_ref().and_then(|h| h.wsl_backend);

        // Move blocking cleanup (thread joins, subprocess calls) to a background thread
        if let Err(e) = std::thread::Builder::new()
            .name(format!("pty-shutdown-{}", short_id))
            .spawn(move || {
                if let Some(handle) = handle {
                    Self::shutdown_handle(handle);
                }
                // On Windows, if this was a WSL terminal with a session backend,
                // kill the session inside WSL instead of on the host
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
            })
        {
            log::error!("Failed to spawn shutdown thread: {}", e);
        }
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
        drop(handle.input_tx);

        // 4. Drop master - safety net to unblock reader if still stuck
        drop(handle.master);

        // 5. Join writer thread (should exit quickly after input_tx drop)
        if let Some(h) = handle.writer_handle.take() {
            if let Err(e) = h.join() {
                log::warn!("PTY writer thread panicked on join: {}", format_panic(&*e));
            }
        }

        // 6. Join reader thread (should exit after child kill + master drop)
        if let Some(h) = handle.reader_handle.take() {
            if let Err(e) = h.join() {
                log::warn!("PTY reader thread panicked on join: {}", format_panic(&*e));
            }
        }
    }

    /// Detach from all terminals without killing sessions
    /// Sessions will persist and can be reconnected on next app start
    pub fn detach_all(&self) {
        // Drain all handles while holding the lock, then release lock before joining
        let handles: Vec<PtyHandle> = self.terminals.lock().drain().map(|(_, h)| h).collect();
        for handle in handles {
            Self::shutdown_handle(handle);
        }
    }

    /// Get the shell process PID for a terminal
    pub fn get_shell_pid(&self, terminal_id: &str) -> Option<u32> {
        self.terminals.lock().get(terminal_id)
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
                    if let Some(daemon) = self.get_dtach_service_pids(terminal_id).into_iter().next() {
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

    /// Find the dtach daemon PID via `lsof -t <socket>`, excluding the attach PID.
    #[cfg(unix)]
    fn get_dtach_service_pids(&self, terminal_id: &str) -> Vec<u32> {
        let session_name = self.session_backend.session_name(terminal_id);
        let socket_path = match self.session_backend.socket_path(&session_name) {
            Some(p) if p.exists() => p,
            _ => return self.get_shell_pid(terminal_id).into_iter().collect(),
        };

        let output = match crate::process::safe_output(
            crate::process::command("lsof").arg("-t").arg(&socket_path),
        ) {
            Ok(o) if o.status.success() => o,
            _ => return self.get_shell_pid(terminal_id).into_iter().collect(),
        };

        let attach_pid = self.get_shell_pid(terminal_id);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let pids: Vec<u32> = stdout
            .lines()
            .filter_map(|line| line.trim().parse::<u32>().ok())
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
        let output = match crate::process::safe_output(
            crate::process::command("tmux")
                .args(["list-panes", "-t", &session_name, "-F", "#{pane_pid}"]),
        ) {
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
            match self.session_backend {
                ResolvedBackend::Dtach => {
                    return self.get_batch_dtach_service_pids(terminal_ids);
                }
                _ => {}
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
            if let Some(p) = self.session_backend.socket_path(&session_name) {
                if p.exists() {
                    socket_to_terminal.insert(p, tid);
                    attach_pids.insert(tid, self.get_shell_pid(tid));
                }
            }
        }

        // Resolve PIDs for all sockets at once
        let socket_pids = find_pids_for_unix_sockets(
            &socket_to_terminal.keys().cloned().collect::<Vec<_>>(),
        );

        // Build result map
        let mut result: HashMap<String, Vec<u32>> = HashMap::new();
        for (&tid, _) in &attach_pids {
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
                        "--", "tmux", "capture-pane", "-t", &session_name, "-p", "-S", "-",
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
        let output_path = std::env::temp_dir().join(format!("terminal-{}.txt", &terminal_id[..8.min(terminal_id.len())]));

        // Use tmux capture-pane to get the entire scrollback buffer
        let result = std::process::Command::new("tmux")
            .args([
                "capture-pane",
                "-t", &session_name,
                "-p",      // output to stdout
                "-S", "-", // start from beginning of scrollback
            ])
            .output();

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
                log::error!("tmux capture-pane failed: {}", String::from_utf8_lossy(&output.stderr));
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
    pub fn cleanup_exited(&self, terminal_id: &str) {
        let handle = self.terminals.lock().remove(terminal_id);
        if let Some(handle) = handle {
            let short_id = terminal_id[..8.min(terminal_id.len())].to_string();
            if let Err(e) = std::thread::Builder::new()
                .name(format!("pty-cleanup-{}", short_id))
                .spawn(move || {
                    Self::shutdown_handle(handle);
                })
            {
                log::error!("Failed to spawn cleanup thread: {}", e);
            }
        }
    }
}

impl crate::terminal::TerminalTransport for PtyManager {
    fn send_input(&self, terminal_id: &str, data: &[u8]) {
        self.send_input(terminal_id, data)
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
/// On other Unix systems, falls back to a single `lsof` invocation.
#[cfg(unix)]
fn find_pids_for_unix_sockets(
    socket_paths: &[std::path::PathBuf],
) -> HashMap<std::path::PathBuf, Vec<u32>> {
    if socket_paths.is_empty() {
        return HashMap::new();
    }

    #[cfg(target_os = "linux")]
    {
        find_pids_for_unix_sockets_linux(socket_paths)
    }

    #[cfg(not(target_os = "linux"))]
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
        } else if let Ok(canon) = std::fs::canonicalize(path) {
            if let Some(&orig) = canonical_paths.get(&canon) {
                inode_to_path.insert(inode, orig);
            }
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
            {
                if let Ok(inode) = inode_str.parse::<u64>() {
                    if let Some(&socket_path) = inode_to_path.get(&inode) {
                        result
                            .entry(socket_path.clone())
                            .or_default()
                            .push(pid);
                    }
                    // Early exit if we found all inodes
                    // (not worth the bookkeeping for a small set)
                }
            }
        }
    }

    result
}

/// Fallback for non-Linux Unix: single `lsof` call for all sockets.
#[cfg(all(unix, not(target_os = "linux")))]
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
        } else if let Some(name) = line.strip_prefix('n') {
            if let Some(pid) = current_pid {
                let path = std::path::PathBuf::from(name);
                if socket_paths.contains(&path) {
                    result.entry(path).or_default().push(pid);
                }
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
    contents.split_whitespace().next().and_then(|s| s.parse().ok())
}

#[cfg(all(unix, not(target_os = "linux")))]
fn first_proc_child(pid: u32) -> Option<u32> {
    let output = std::process::Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
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

