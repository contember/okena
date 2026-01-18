use crate::terminal::session_backend::{ResolvedBackend, SessionBackend};
use crate::terminal::shell_config::ShellType;
use anyhow::Result;
use async_channel::{Receiver, Sender};
use parking_lot::Mutex;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::mpsc;

/// Events from PTY processes
#[derive(Debug)]
pub enum PtyEvent {
    /// Data received from PTY
    Data { terminal_id: String, data: Vec<u8> },
    /// PTY process exited
    Exit {
        terminal_id: String,
        #[allow(dead_code)] // Exit code available for future use
        exit_code: Option<u32>,
    },
}

/// Handle to a single PTY process
struct PtyHandle {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    /// Channel to send input to the writer thread
    input_tx: mpsc::Sender<Vec<u8>>,
}

/// Manages all PTY processes
pub struct PtyManager {
    terminals: Arc<Mutex<HashMap<String, PtyHandle>>>,
    event_tx: Sender<PtyEvent>,
    /// Session backend for persistence (tmux/screen/none)
    session_backend: ResolvedBackend,
}

impl PtyManager {
    /// Create a new PTY manager
    pub fn new() -> (Self, Receiver<PtyEvent>) {
        let (tx, rx) = async_channel::unbounded();
        let session_backend = SessionBackend::from_env().resolve();

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

        // Spawn reader thread
        let tx = self.event_tx.clone();
        let id = terminal_id.to_string();
        std::thread::spawn(move || {
            Self::read_loop(id, reader, tx);
        });

        // Create input channel and spawn writer thread
        let (input_tx, input_rx) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            Self::write_loop(writer, input_rx);
        });

        // Store the handle
        self.terminals.lock().insert(
            terminal_id.to_string(),
            PtyHandle {
                master: pair.master,
                child,
                input_tx,
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
        let mut cmd = if let Some((program, args)) = self
            .session_backend
            .build_command(&self.session_backend.session_name(terminal_id), cwd)
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

        cmd
    }

    /// Read loop for PTY output
    fn read_loop(
        terminal_id: String,
        mut reader: Box<dyn Read + Send>,
        tx: Sender<PtyEvent>,
    ) {
        // Use larger buffer like alacritty (they use 1MB, we use 64KB)
        let mut buf = [0u8; 65536];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    // EOF - process exited
                    let _ = tx.send_blocking(PtyEvent::Exit {
                        terminal_id,
                        exit_code: None,
                    });
                    break;
                }
                Ok(n) => {
                    log::debug!("PTY {} received {} bytes: {:?}", terminal_id, n, String::from_utf8_lossy(&buf[..n.min(100)]));
                    let _ = tx.send_blocking(PtyEvent::Data {
                        terminal_id: terminal_id.clone(),
                        data: buf[..n].to_vec(),
                    });
                }
                Err(e) => {
                    log::error!("PTY read error: {}", e);
                    let _ = tx.send_blocking(PtyEvent::Exit {
                        terminal_id,
                        exit_code: None,
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
                log::error!("Failed to write to PTY: {}", e);
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
        if let Some(mut handle) = self.terminals.lock().remove(terminal_id) {
            if let Err(e) = handle.child.kill() {
                log::warn!("Failed to kill PTY process: {}", e);
            }
        }
        // Also kill the session backend session
        self.session_backend
            .kill_session(&self.session_backend.session_name(terminal_id));
    }

    /// Detach from all terminals without killing sessions
    /// Sessions will persist and can be reconnected on next app start
    pub fn detach_all(&self) {
        let mut terminals = self.terminals.lock();
        for (_, mut handle) in terminals.drain() {
            let _ = handle.child.kill();
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
}

impl Drop for PtyManager {
    fn drop(&mut self) {
        // On drop, just detach - don't kill sessions
        // This allows sessions to persist across app restarts
        self.detach_all();
    }
}

