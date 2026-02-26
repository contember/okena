use crate::terminal::session_backend::{ResolvedBackend, SessionBackend};
#[cfg(target_os = "macos")]
use crate::terminal::session_backend::get_extended_path;
use crate::terminal::shell_config::ShellType;
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
}

/// Manages all PTY processes
pub struct PtyManager {
    terminals: Arc<Mutex<HashMap<String, PtyHandle>>>,
    event_tx: Sender<PtyEvent>,
    /// Session backend for persistence (tmux/screen/none)
    session_backend: ResolvedBackend,
}

impl PtyManager {
    /// Create a new PTY manager with the specified session backend
    pub fn new(backend: SessionBackend) -> (Self, Receiver<PtyEvent>) {
        let (tx, rx) = async_channel::bounded(4096);
        let session_backend = backend.resolve();

        if session_backend.supports_persistence() {
            log::info!("Session persistence enabled with {:?}", session_backend);
        }

        (
            Self {
                terminals: Arc::new(Mutex::new(HashMap::new())),
                event_tx: tx,
                session_backend,
            },
            rx,
        )
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
        let cmd = self.build_terminal_command(terminal_id, cwd, shell);

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
        let reader_handle = std::thread::Builder::new()
            .name(format!("pty-reader-{}", &terminal_id[..8.min(terminal_id.len())]))
            .spawn(move || {
                let tx_panic = tx.clone();
                let shutdown_panic = Arc::clone(&reader_shutdown);
                let id_panic = id.clone();
                if let Err(panic) = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    Self::read_loop(id, reader, tx, reader_shutdown, child_pid);
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
            },
        );

        Ok(())
    }

    /// Build the command to run in the terminal
    #[allow(unused_variables)] // terminal_id used only on Unix for session backend
    fn build_terminal_command(&self, terminal_id: &str, cwd: &str, shell: Option<&ShellType>) -> CommandBuilder {
        // On Unix, if session backend is active, use it for persistence
        // Session backends (tmux/screen) are not available on Windows
        #[cfg(unix)]
        let mut cmd = {
            // Extract custom command from ShellType::Custom{path:"sh", args:["-c", cmd]}
            // so it can be passed to the session backend
            let custom_command = match shell {
                Some(ShellType::Custom { path, args }) if path == "sh" && args.len() == 2 && args[0] == "-c" => {
                    Some(args[1].as_str())
                }
                _ => None,
            };

            if let Some((program, args)) = self
                .session_backend
                .build_command(&self.session_backend.session_name(terminal_id), cwd, custom_command)
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
            }
        };

        // On Windows, always use shell config (no session backend support)
        #[cfg(windows)]
        let mut cmd = match shell {
            Some(shell_type) => shell_type.build_command(cwd),
            None => {
                let mut cmd = CommandBuilder::new_default_prog();
                cmd.cwd(cwd);
                cmd
            }
        };

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

        // On macOS, extend PATH to include Homebrew/MacPorts paths
        // App bundles start with minimal PATH and won't find tmux/screen otherwise
        #[cfg(target_os = "macos")]
        cmd.env("PATH", get_extended_path());

        cmd
    }

    /// Read loop for PTY output
    fn read_loop(
        terminal_id: String,
        mut reader: Box<dyn Read + Send>,
        tx: Sender<PtyEvent>,
        shutdown: Arc<PtyShutdownState>,
        child_pid: Option<u32>,
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
                    log::debug!("PTY {} received {} bytes: {:?}", terminal_id, n, String::from_utf8_lossy(&buf[..n.min(100)]));
                    // send_blocking will block when channel is full (backpressure)
                    if tx.send_blocking(PtyEvent::Data {
                        terminal_id: terminal_id.clone(),
                        data: buf[..n].to_vec(),
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
    }

    /// Kill a terminal
    /// Also kills the underlying tmux/screen session if applicable
    pub fn kill(&self, terminal_id: &str) {
        // Remove handle from map immediately (fast, non-blocking)
        let handle = self.terminals.lock().remove(terminal_id);
        let session_backend = self.session_backend;
        let session_name = session_backend.session_name(terminal_id);
        let short_id = terminal_id[..8.min(terminal_id.len())].to_string();

        // Move blocking cleanup (thread joins, subprocess calls) to a background thread
        if let Err(e) = std::thread::Builder::new()
            .name(format!("pty-shutdown-{}", short_id))
            .spawn(move || {
                if let Some(handle) = handle {
                    Self::shutdown_handle(handle);
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

    /// Check if the session backend handles mouse events (tmux with mouse on)
    pub fn uses_mouse_backend(&self) -> bool {
        matches!(self.session_backend, ResolvedBackend::Tmux)
    }

    /// Capture the terminal buffer to a file (only works with tmux backend)
    /// Returns the path to the captured file, or None if not using tmux
    pub fn capture_buffer(&self, terminal_id: &str) -> Option<std::path::PathBuf> {
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

impl crate::terminal::terminal::TerminalTransport for PtyManager {
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

