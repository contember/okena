#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use crossterm::{
    cursor,
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode,
        KeyEvent as CrosstermKeyEvent, KeyEventKind, KeyModifiers as CrosstermKeyModifiers,
    },
    execute, queue,
    style::{Attribute, Print, SetAttribute},
    terminal::{self, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use okena_core::api::{ApiLayoutNode, ApiProject, StateResponse};
use okena_terminal::input::{KeyEvent, KeyModifiers, key_to_bytes};
use okena_terminal::terminal::{Terminal, TerminalSize, TerminalTransport};
use okena_transport::client::{
    ConnectionEvent, ConnectionHandler, ConnectionStatus, LocalEndpoint,
    REMOTE_TERMINAL_ANSWERS_QUERIES, REMOTE_TERMINAL_RESIZE_DEBOUNCE_MS,
    REMOTE_TERMINAL_USES_MOUSE_BACKEND, RemoteClient, RemoteConnectionConfig, WsClientMessage,
    is_remote_terminal, make_prefixed_id, resize_remote_terminal, send_remote_terminal_input,
    strip_prefix,
};
use parking_lot::RwLock;

#[derive(Parser, Debug)]
#[command(
    name = "okena-tui",
    about = "Proof-of-concept terminal UI remote client for a running Okena daemon"
)]
struct Args {
    /// Remote host. Defaults to discovered local daemon host when omitted.
    #[arg(long)]
    host: Option<String>,

    /// Remote port. Defaults to discovered local daemon port when omitted.
    #[arg(long)]
    port: Option<u16>,

    /// Bearer token for TCP remotes. Also read from OKENA_TOKEN.
    #[arg(long, env = "OKENA_TOKEN")]
    token: Option<String>,

    /// Pairing code to exchange for a token for this run.
    #[arg(long)]
    pair: Option<String>,

    /// Force TLS for TCP remotes.
    #[arg(long)]
    tls: bool,

    /// Expected SHA-256 fingerprint of the remote TLS certificate, as printed
    /// by `okena pair` on the host. Colons and spaces are ignored.
    #[arg(long, env = "OKENA_CERT_SHA256")]
    cert_fingerprint: Option<String>,

    /// Send credentials to a TCP remote whose certificate is not pinned. The
    /// first handshake is then trusted blindly and can be intercepted.
    #[arg(long)]
    insecure_no_cert_pin: bool,

    /// Connect over a same-user Unix socket.
    #[arg(long)]
    socket: Option<PathBuf>,

    /// Profile id used only for local remote.json discovery.
    #[arg(long, env = "OKENA_PROFILE")]
    profile: Option<String>,

    /// Start focused on this terminal id.
    #[arg(long)]
    terminal: Option<String>,
}

struct TuiRemoteTransport {
    ws_tx: async_channel::Sender<WsClientMessage>,
    connection_id: String,
}

impl TerminalTransport for TuiRemoteTransport {
    fn send_input(&self, terminal_id: &str, data: &[u8]) {
        send_remote_terminal_input(&self.ws_tx, &self.connection_id, terminal_id, data);
    }

    fn send_response(&self, _terminal_id: &str, _data: &[u8]) {}

    fn resize(&self, terminal_id: &str, cols: u16, rows: u16) {
        resize_remote_terminal(&self.ws_tx, &self.connection_id, terminal_id, cols, rows);
    }

    fn uses_mouse_backend(&self) -> bool {
        REMOTE_TERMINAL_USES_MOUSE_BACKEND
    }

    fn resize_debounce_ms(&self) -> u64 {
        REMOTE_TERMINAL_RESIZE_DEBOUNCE_MS
    }

    fn answers_terminal_queries(&self) -> bool {
        REMOTE_TERMINAL_ANSWERS_QUERIES
    }

    /// The proof-of-concept TUI has no clipboard integration and never drains
    /// the OSC 52 queues, so queueing there would only leak.
    fn handles_clipboard(&self) -> bool {
        false
    }
}

struct TuiConnectionHandler {
    terminals: Arc<RwLock<HashMap<String, Arc<Terminal>>>>,
    dirty_tx: async_channel::Sender<()>,
}

impl TuiConnectionHandler {
    fn new(
        terminals: Arc<RwLock<HashMap<String, Arc<Terminal>>>>,
        dirty_tx: async_channel::Sender<()>,
    ) -> Self {
        Self {
            terminals,
            dirty_tx,
        }
    }
}

impl ConnectionHandler for TuiConnectionHandler {
    fn create_terminal(
        &self,
        connection_id: &str,
        _terminal_id: &str,
        prefixed_id: &str,
        ws_sender: async_channel::Sender<WsClientMessage>,
        cols: u16,
        rows: u16,
    ) {
        if self.terminals.read().contains_key(prefixed_id) {
            return;
        }

        let size = if cols > 0 && rows > 0 {
            TerminalSize {
                cols,
                rows,
                ..TerminalSize::default()
            }
        } else {
            TerminalSize::default()
        };
        let transport = Arc::new(TuiRemoteTransport {
            ws_tx: ws_sender,
            connection_id: connection_id.to_string(),
        });
        let terminal = Arc::new(Terminal::new(
            prefixed_id.to_string(),
            size,
            transport,
            String::new(),
        ));
        self.terminals
            .write()
            .insert(prefixed_id.to_string(), terminal);
    }

    fn on_terminal_output(&self, prefixed_id: &str, data: &[u8]) {
        if let Some(terminal) = self.terminals.read().get(prefixed_id) {
            terminal.enqueue_output(data);
            let _ = self.dirty_tx.try_send(());
        }
    }

    fn resize_terminal(&self, prefixed_id: &str, cols: u16, rows: u16, server_owns: bool) {
        if let Some(terminal) = self.terminals.read().get(prefixed_id) {
            if server_owns {
                terminal.claim_resize_remote();
            }
            terminal.resize_grid_only(cols, rows);
            let _ = self.dirty_tx.try_send(());
        }
    }

    fn remove_terminal(&self, prefixed_id: &str) {
        self.terminals.write().remove(prefixed_id);
        let _ = self.dirty_tx.try_send(());
    }

    fn remove_all_terminals(&self, connection_id: &str) {
        let mut terminals = self.terminals.write();
        let to_remove: Vec<String> = terminals
            .keys()
            .filter(|key| is_remote_terminal(key, connection_id))
            .cloned()
            .collect();
        for key in to_remove {
            terminals.remove(&key);
        }
        let _ = self.dirty_tx.try_send(());
    }

    fn remove_terminals_except(
        &self,
        connection_id: &str,
        keep_ids: &std::collections::HashSet<String>,
    ) {
        let mut terminals = self.terminals.write();
        let to_remove: Vec<String> = terminals
            .keys()
            .filter(|key| {
                is_remote_terminal(key, connection_id)
                    && !keep_ids.contains(&strip_prefix(key, connection_id))
            })
            .cloned()
            .collect();
        for key in to_remove {
            terminals.remove(&key);
        }
        let _ = self.dirty_tx.try_send(());
    }
}

#[derive(Clone)]
struct TerminalEntry {
    id: String,
    label: String,
}

struct TuiState {
    status: ConnectionStatus,
    state: Option<StateResponse>,
    active_terminal: Option<String>,
    terminal_request: Option<String>,
    message: Option<String>,
    last_resize: Option<(String, u16, u16)>,
}

impl TuiState {
    fn new(terminal_request: Option<String>) -> Self {
        Self {
            status: ConnectionStatus::Disconnected,
            state: None,
            active_terminal: terminal_request.clone(),
            terminal_request,
            message: None,
            last_resize: None,
        }
    }

    fn entries(&self) -> Vec<TerminalEntry> {
        self.state
            .as_ref()
            .map(collect_terminal_entries)
            .unwrap_or_default()
    }

    fn ensure_active_terminal(&mut self) {
        let entries = self.entries();
        if entries.is_empty() {
            self.active_terminal = None;
            return;
        }

        if let Some(requested) = self.terminal_request.as_deref()
            && let Some(entry) = entries.iter().find(|entry| entry.id.starts_with(requested))
        {
            self.active_terminal = Some(entry.id.clone());
            self.terminal_request = None;
            return;
        }

        if let Some(active) = self.active_terminal.as_deref()
            && entries.iter().any(|entry| entry.id == active)
        {
            return;
        }

        self.active_terminal = entries.first().map(|entry| entry.id.clone());
    }

    fn cycle_terminal(&mut self) {
        let entries = self.entries();
        if entries.is_empty() {
            self.active_terminal = None;
            return;
        }

        let next = match self.active_terminal.as_deref() {
            Some(active) => entries
                .iter()
                .position(|entry| entry.id == active)
                .map(|index| (index + 1) % entries.len())
                .unwrap_or(0),
            None => 0,
        };
        self.active_terminal = Some(entries[next].id.clone());
        self.last_resize = None;
    }
}

/// Owns every host terminal mode the TUI changes, so exiting restores the shell
/// exactly as it was found. Nothing else may write mode control to the host.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableBracketedPaste,
            cursor::Hide
        )?;
        // Absolute row addressing needs origin mode off, and no-wrap stops an
        // oversized snapshot row from scrolling the screen. DECOM stays off on
        // exit: only xterm-class hosts restore it with the alt-screen cursor.
        stdout.write_all(b"\x1b[?6l\x1b[?7l")?;
        stdout.flush()?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let mut stdout = io::stdout();
        // Autowrap is the default everywhere and `\x1b[?1049l` does not restore it.
        let _ = stdout.write_all(b"\x1b[?7h");
        let _ = execute!(
            stdout,
            SetAttribute(Attribute::Reset),
            cursor::Show,
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
    }
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let args = Args::parse();
    let config = connection_config(&args)?;
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("okena-tui")
            .build()
            .context("creating tokio runtime")?,
    );

    runtime.block_on(run(args, config, runtime.clone()))
}

async fn run(
    args: Args,
    config: RemoteConnectionConfig,
    runtime: Arc<tokio::runtime::Runtime>,
) -> Result<()> {
    let connection_id = config.id.clone();
    let terminals = Arc::new(RwLock::new(HashMap::new()));
    let (dirty_tx, dirty_rx) = async_channel::bounded::<()>(1);
    let handler = Arc::new(TuiConnectionHandler::new(terminals.clone(), dirty_tx));
    let (event_tx, event_rx) = async_channel::bounded::<ConnectionEvent>(256);

    let mut client = RemoteClient::new(config, runtime, handler, event_tx);
    let mut state = TuiState::new(args.terminal.clone());
    client.connect();

    wait_for_initial_state(&mut client, &mut state, &event_rx, args.pair.as_deref()).await?;

    let _guard = TerminalGuard::enter()?;
    render(&connection_id, &terminals, &mut state)?;

    let mut needs_render = false;
    loop {
        while let Ok(event) = event_rx.try_recv() {
            handle_connection_event(&mut client, &mut state, event);
            state.ensure_active_terminal();
            needs_render = true;
        }

        while dirty_rx.try_recv().is_ok() {
            needs_render = true;
        }

        if event::poll(Duration::from_millis(16))?
            && let Event::Key(key) = event::read()?
        {
            match handle_key(&connection_id, &terminals, &mut state, key)? {
                LoopControl::Continue => needs_render = true,
                LoopControl::Quit => break,
            }
        }

        if needs_render {
            render(&connection_id, &terminals, &mut state)?;
            needs_render = false;
        }
    }

    client.disconnect();
    Ok(())
}

async fn wait_for_initial_state(
    client: &mut RemoteClient<TuiConnectionHandler>,
    state: &mut TuiState,
    event_rx: &async_channel::Receiver<ConnectionEvent>,
    pair_code: Option<&str>,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut pair_sent = false;

    loop {
        while let Ok(event) = event_rx.try_recv() {
            handle_connection_event(client, state, event);
        }

        match &state.status {
            ConnectionStatus::Connected if state.state.is_some() => {
                state.ensure_active_terminal();
                return Ok(());
            }
            ConnectionStatus::Pairing => {
                let Some(code) = pair_code else {
                    bail!(
                        "pairing required. Pass --pair <code>, --token <token>, or connect over discovered local Unix socket"
                    );
                };
                if !pair_sent {
                    client.pair(code);
                    pair_sent = true;
                }
            }
            ConnectionStatus::Error(message) => {
                bail!("{message}");
            }
            _ => {}
        }

        if Instant::now() >= deadline {
            bail!("timed out waiting for remote state");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn handle_connection_event(
    client: &mut RemoteClient<TuiConnectionHandler>,
    state: &mut TuiState,
    event: ConnectionEvent,
) {
    match event {
        ConnectionEvent::StatusChanged { status, .. } => {
            state.status = status;
        }
        ConnectionEvent::TokenObtained {
            token,
            cert_fingerprint,
            ..
        } => {
            client.update_shared_token(&token);
            client.config_mut().saved_token = Some(token);
            client.config_mut().token_obtained_at = Some(unix_now());
            client.config_mut().pinned_cert_sha256 = cert_fingerprint;
        }
        ConnectionEvent::TlsUpgraded {
            cert_fingerprint, ..
        } => {
            client.config_mut().tls = true;
            client.config_mut().pinned_cert_sha256 = cert_fingerprint;
        }
        ConnectionEvent::StateReceived {
            state: new_state, ..
        } => {
            client.set_remote_state(Some(new_state.clone()));
            state.state = Some(new_state);
        }
        ConnectionEvent::SettingsChanged { .. } => {}
        ConnectionEvent::SubscriptionMappings { mappings, .. } => {
            client.update_stream_mappings(mappings);
        }
        ConnectionEvent::ServerWarning { message, .. } => {
            state.message = Some(message);
        }
        ConnectionEvent::GitStatusChanged { statuses, .. } => {
            if let Some(remote_state) = state.state.as_mut() {
                for project in &mut remote_state.projects {
                    project.git_status = statuses.get(&project.id).cloned();
                }
            }
        }
        ConnectionEvent::SystemStatsChanged { .. }
        | ConnectionEvent::TerminalFocusRequested { .. } => {}
        ConnectionEvent::Toast { toast, .. } => {
            state.message = Some(format!("{}: {}", toast.level, toast.message));
        }
        ConnectionEvent::TokenRefreshed { token, .. } => {
            client.update_shared_token(&token);
            client.config_mut().saved_token = Some(token);
            client.config_mut().token_obtained_at = Some(unix_now());
        }
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
        .unwrap_or_default()
}

enum LoopControl {
    Continue,
    Quit,
}

fn handle_key(
    connection_id: &str,
    terminals: &Arc<RwLock<HashMap<String, Arc<Terminal>>>>,
    state: &mut TuiState,
    key: CrosstermKeyEvent,
) -> Result<LoopControl> {
    if key.kind != KeyEventKind::Press {
        return Ok(LoopControl::Continue);
    }

    // Ctrl+] is the byte 0x1D, which crossterm reports as Ctrl+'5' — the whole
    // 0x1C..=0x1F range maps onto '4'..='7'. Matching only ']' left the TUI with
    // no reachable way out at all.
    if key.modifiers.contains(CrosstermKeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char(']') | KeyCode::Char('5'))
    {
        return Ok(LoopControl::Quit);
    }

    if key.modifiers.contains(CrosstermKeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('t'))
    {
        state.cycle_terminal();
        return Ok(LoopControl::Continue);
    }

    let Some(active) = state.active_terminal.as_deref() else {
        return Ok(LoopControl::Continue);
    };
    let prefixed = make_prefixed_id(connection_id, active);
    let terminal = terminals.read().get(&prefixed).cloned();
    let Some(terminal) = terminal else {
        return Ok(LoopControl::Continue);
    };

    if let Some(bytes) = key_bytes(&terminal, key) {
        terminal.send_bytes(&bytes);
    }

    Ok(LoopControl::Continue)
}

fn key_bytes(terminal: &Terminal, key: CrosstermKeyEvent) -> Option<Vec<u8>> {
    let modifiers = key_modifiers(key.modifiers);

    if let KeyCode::Char(ch) = key.code
        && !modifiers.control
        && !modifiers.alt
        && !modifiers.platform
    {
        return Some(ch.to_string().into_bytes());
    }

    let key_name = match key.code {
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::PageUp => "pageup".to_string(),
        KeyCode::PageDown => "pagedown".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::BackTab => "tab".to_string(),
        KeyCode::Delete => "delete".to_string(),
        KeyCode::Insert => "insert".to_string(),
        KeyCode::Esc => "escape".to_string(),
        KeyCode::F(n) => format!("f{n}"),
        KeyCode::Char(ch) => ch.to_string(),
        KeyCode::Null
        | KeyCode::CapsLock
        | KeyCode::ScrollLock
        | KeyCode::NumLock
        | KeyCode::PrintScreen
        | KeyCode::Pause
        | KeyCode::Menu
        | KeyCode::KeypadBegin
        | KeyCode::Media(_)
        | KeyCode::Modifier(_) => return None,
    };

    let mut event = KeyEvent {
        key: key_name,
        key_char: None,
        modifiers,
    };
    if matches!(key.code, KeyCode::BackTab) {
        event.modifiers.shift = true;
    }

    key_to_bytes(
        &event,
        terminal.is_app_cursor_mode(),
        terminal.kitty_keyboard_flags(),
    )
}

fn key_modifiers(modifiers: CrosstermKeyModifiers) -> KeyModifiers {
    KeyModifiers {
        control: modifiers.contains(CrosstermKeyModifiers::CONTROL),
        shift: modifiers.contains(CrosstermKeyModifiers::SHIFT),
        alt: modifiers.contains(CrosstermKeyModifiers::ALT),
        platform: false,
    }
}

fn render(
    connection_id: &str,
    terminals: &Arc<RwLock<HashMap<String, Arc<Terminal>>>>,
    state: &mut TuiState,
) -> Result<()> {
    let (cols, rows) = terminal::size()?;
    let terminal_rows = rows.saturating_sub(1).max(1);
    resize_active_terminal(connection_id, terminals, state, cols, terminal_rows);

    let mut stdout = io::stdout();
    queue!(stdout, cursor::Hide, terminal::Clear(ClearType::All))?;

    if let Some(active) = state.active_terminal.as_deref() {
        let prefixed = make_prefixed_id(connection_id, active);
        let terminal = terminals.read().get(&prefixed).cloned();
        if let Some(terminal) = terminal {
            stdout.write_all(&terminal.render_snapshot_for_host_screen())?;
        } else {
            queue!(
                stdout,
                cursor::MoveTo(0, 0),
                Print("Waiting for terminal stream...")
            )?;
        }
    } else {
        queue!(
            stdout,
            cursor::MoveTo(0, 0),
            Print("No remote terminals in workspace.")
        )?;
    }

    draw_status(&mut stdout, state, cols, rows)?;
    stdout.flush()?;
    Ok(())
}

fn resize_active_terminal(
    connection_id: &str,
    terminals: &Arc<RwLock<HashMap<String, Arc<Terminal>>>>,
    state: &mut TuiState,
    cols: u16,
    rows: u16,
) {
    let Some(active) = state.active_terminal.as_deref() else {
        return;
    };
    let next = (active.to_string(), cols, rows);
    if state.last_resize.as_ref() == Some(&next) {
        return;
    }

    let prefixed = make_prefixed_id(connection_id, active);
    if let Some(terminal) = terminals.read().get(&prefixed) {
        terminal.claim_resize_local();
        terminal.resize(TerminalSize {
            cols,
            rows,
            ..TerminalSize::default()
        });
        state.last_resize = Some(next);
    }
}

fn draw_status(stdout: &mut io::Stdout, state: &TuiState, cols: u16, rows: u16) -> Result<()> {
    let entries = state.entries();
    let active_index = state
        .active_terminal
        .as_deref()
        .and_then(|active| entries.iter().position(|entry| entry.id == active))
        .map(|index| index + 1)
        .unwrap_or(0);
    let active_label = state
        .active_terminal
        .as_deref()
        .and_then(|active| entries.iter().find(|entry| entry.id == active))
        .map(|entry| entry.label.as_str())
        .unwrap_or("none");
    let message = state.message.as_deref().unwrap_or("");
    let status = format!(
        " Okena TUI | {} | {}/{} {} | Ctrl-] quit | Ctrl-T next {}{}",
        status_label(&state.status),
        active_index,
        entries.len(),
        active_label,
        if message.is_empty() { "" } else { "| " },
        message
    );

    queue!(
        stdout,
        cursor::MoveTo(0, rows.saturating_sub(1)),
        SetAttribute(Attribute::Reverse),
        Print(fit_line(&status, cols)),
        terminal::Clear(ClearType::UntilNewLine),
        SetAttribute(Attribute::Reset)
    )?;
    Ok(())
}

fn status_label(status: &ConnectionStatus) -> String {
    match status {
        ConnectionStatus::Disconnected => "disconnected".to_string(),
        ConnectionStatus::Connecting => "connecting".to_string(),
        ConnectionStatus::Pairing => "pairing".to_string(),
        ConnectionStatus::Connected => "connected".to_string(),
        ConnectionStatus::Reconnecting { attempt } => format!("reconnecting:{attempt}"),
        ConnectionStatus::Error(message) => format!("error:{message}"),
    }
}

fn fit_line(line: &str, cols: u16) -> String {
    line.chars().take(usize::from(cols)).collect()
}

fn collect_terminal_entries(state: &StateResponse) -> Vec<TerminalEntry> {
    let mut entries = Vec::new();
    for project in &state.projects {
        if let Some(layout) = &project.layout {
            collect_layout_entries(project, layout, &mut entries);
        }
    }
    entries
}

fn collect_layout_entries(
    project: &ApiProject,
    node: &ApiLayoutNode,
    entries: &mut Vec<TerminalEntry>,
) {
    match node {
        ApiLayoutNode::Terminal {
            terminal_id: Some(id),
            ..
        } => {
            let name = project
                .terminal_names
                .get(id)
                .map(String::as_str)
                .unwrap_or(id);
            entries.push(TerminalEntry {
                id: id.clone(),
                label: format!("{}:{}", project.name, name),
            });
        }
        ApiLayoutNode::Terminal { .. } => {}
        ApiLayoutNode::Split { children, .. } | ApiLayoutNode::Tabs { children, .. } => {
            for child in children {
                collect_layout_entries(project, child, entries);
            }
        }
    }
}

fn connection_config(args: &Args) -> Result<RemoteConnectionConfig> {
    let discovered = if args.port.is_none() && args.socket.is_none() {
        discover_local(args.profile.as_deref()).transpose()?
    } else {
        None
    };

    let local_endpoint = match &args.socket {
        Some(path) => Some(LocalEndpoint::UnixSocket {
            path: path.to_string_lossy().into_owned(),
        }),
        None => discovered
            .as_ref()
            .and_then(|discovered| discovered.local_endpoint.clone()),
    };
    let host = args
        .host
        .clone()
        .or_else(|| {
            discovered
                .as_ref()
                .map(|discovered| discovered.host.clone())
        })
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port = args
        .port
        .or_else(|| discovered.as_ref().map(|discovered| discovered.port))
        .ok_or_else(|| anyhow!("missing --port and no local remote.json was discovered"))?;
    let pinned_cert_sha256 = args
        .cert_fingerprint
        .as_deref()
        .map(normalize_cert_fingerprint)
        .transpose()?;
    if pinned_cert_sha256.is_some() && uses_local_socket(local_endpoint.as_ref()) {
        bail!("--cert-fingerprint applies to TCP remotes; the local socket transport has no TLS");
    }
    // A pin only binds a TLS handshake: without forcing TLS the client would
    // fall back to plain http and send the token in the clear.
    let tls = args.tls
        || pinned_cert_sha256.is_some()
        || discovered.as_ref().is_some_and(|discovered| discovered.tls);

    let config = RemoteConnectionConfig {
        id: uuid::Uuid::new_v4().to_string(),
        name: "Okena TUI".to_string(),
        host,
        port,
        saved_token: args.token.clone(),
        token_obtained_at: None,
        tls,
        pinned_cert_sha256,
        local_endpoint,
    };
    check_credential_exposure(
        &config,
        args.token.is_some() || args.pair.is_some(),
        args.insecure_no_cert_pin,
    )?;
    Ok(config)
}

/// Whether the connection task will really bypass TCP. Mirrors `local_unix_path`
/// in `okena-transport`: any other endpoint still dials host:port, so the pin
/// and credential rules must apply to it.
fn uses_local_socket(endpoint: Option<&LocalEndpoint>) -> bool {
    cfg!(unix) && matches!(endpoint, Some(LocalEndpoint::UnixSocket { .. }))
}

/// Refuse to hand a credential to a TCP peer whose certificate is not pinned.
fn check_credential_exposure(
    config: &RemoteConnectionConfig,
    has_credentials: bool,
    insecure: bool,
) -> Result<()> {
    if !has_credentials
        || insecure
        || config.pinned_cert_sha256.is_some()
        || uses_local_socket(config.local_endpoint.as_ref())
        || is_loopback_host(&config.host)
    {
        return Ok(());
    }
    bail!(
        "refusing to send credentials to {}:{} without a pinned certificate. \
         Pass --cert-fingerprint <sha256> (run `okena pair` on the host to read it), \
         or --insecure-no-cert-pin to accept any certificate",
        config.host,
        config.port
    );
}

/// Accept the `aa:bb:cc:dd ee:ff …` form `okena pair` prints, and plain hex.
fn normalize_cert_fingerprint(value: &str) -> Result<String> {
    let hex: String = value
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ':')
        .collect();
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("--cert-fingerprint must be a 64-character hex SHA-256, got {value:?}");
    }
    Ok(hex.to_ascii_lowercase())
}

/// A same-host daemon needs no pin: intercepting loopback already requires
/// running code as this user.
fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

struct DiscoveredDaemon {
    host: String,
    port: u16,
    tls: bool,
    local_endpoint: Option<LocalEndpoint>,
}

fn discover_local(profile: Option<&str>) -> Option<Result<DiscoveredDaemon>> {
    let root = okena_core::profiles::config_root();
    let mut candidates = Vec::new();

    if let Some(profile) = profile {
        candidates.push(root.join("profiles").join(profile).join("remote.json"));
    } else if let Ok(index) = okena_core::profiles::ProfileIndex::load(&root) {
        if let Some(last_used) = index.last_used {
            candidates.push(root.join("profiles").join(last_used).join("remote.json"));
        }
        if index.profiles.len() == 1
            && let Some(profile) = index.profiles.first()
        {
            candidates.push(root.join("profiles").join(&profile.id).join("remote.json"));
        }
        candidates.push(
            root.join("profiles")
                .join(index.default_profile)
                .join("remote.json"),
        );
    }

    candidates.push(root.join("remote.json"));
    candidates
        .into_iter()
        .find(|path| path.exists())
        .map(|path| parse_remote_json(&path))
}

fn parse_remote_json(path: &Path) -> Result<DiscoveredDaemon> {
    let data =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&data).with_context(|| format!("parsing {}", path.display()))?;
    let port = value
        .get("port")
        .and_then(|port| port.as_u64())
        .and_then(|port| u16::try_from(port).ok())
        .ok_or_else(|| anyhow!("{} is missing a valid port", path.display()))?;
    let host = value
        .get("local_host")
        .and_then(|host| host.as_str())
        .filter(|host| !host.is_empty())
        .unwrap_or("127.0.0.1")
        .to_string();
    let tls = value
        .get("tls")
        .and_then(|tls| tls.as_bool())
        .unwrap_or(false);
    let local_endpoint = value
        .get("local_endpoint")
        .and_then(|endpoint| serde_json::from_value(endpoint.clone()).ok());

    Ok(DiscoveredDaemon {
        host,
        port,
        tls,
        local_endpoint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tcp_args(host: &str) -> Args {
        Args {
            host: Some(host.to_string()),
            port: Some(19100),
            token: None,
            pair: None,
            tls: false,
            cert_fingerprint: None,
            insecure_no_cert_pin: false,
            socket: None,
            profile: None,
            terminal: None,
        }
    }

    const FINGERPRINT: &str = "aa:bb:cc:dd ee:ff:00:11 22:33:44:55 66:77:88:99 \
                               aa:bb:cc:dd ee:ff:00:11 22:33:44:55 66:77:88:99";

    #[test]
    fn cert_fingerprint_accepts_the_printed_format() {
        assert_eq!(
            normalize_cert_fingerprint(FINGERPRINT).expect("valid fingerprint"),
            "aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899"
        );
        assert_eq!(
            normalize_cert_fingerprint(&"AB".repeat(32)).expect("valid fingerprint"),
            "ab".repeat(32)
        );
    }

    #[test]
    fn cert_fingerprint_rejects_malformed_input() {
        assert!(normalize_cert_fingerprint("ab:cd").is_err());
        assert!(normalize_cert_fingerprint(&"zz".repeat(32)).is_err());
        assert!(normalize_cert_fingerprint(&"ab".repeat(33)).is_err());
    }

    #[test]
    fn token_to_a_remote_host_requires_a_pin() {
        let mut args = tcp_args("10.0.0.5");
        args.token = Some("secret".into());
        assert!(connection_config(&args).is_err());

        args.tls = true;
        assert!(connection_config(&args).is_err());
    }

    #[test]
    fn pair_code_to_a_remote_host_requires_a_pin() {
        let mut args = tcp_args("10.0.0.5");
        args.pair = Some("123456".into());
        assert!(connection_config(&args).is_err());
    }

    #[test]
    fn pin_is_applied_and_forces_tls() {
        let mut args = tcp_args("10.0.0.5");
        args.token = Some("secret".into());
        args.cert_fingerprint = Some(FINGERPRINT.into());

        let config = connection_config(&args).expect("pinned config");
        assert_eq!(
            config.pinned_cert_sha256.as_deref(),
            Some("aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899")
        );
        assert!(config.tls);
    }

    #[test]
    fn explicit_opt_out_connects_without_a_pin() {
        let mut args = tcp_args("10.0.0.5");
        args.token = Some("secret".into());
        args.insecure_no_cert_pin = true;

        let config = connection_config(&args).expect("unpinned config");
        assert!(config.pinned_cert_sha256.is_none());
    }

    #[test]
    fn loopback_and_credential_free_remotes_need_no_pin() {
        let mut loopback = tcp_args("127.0.0.1");
        loopback.token = Some("secret".into());
        assert!(connection_config(&loopback).is_ok());

        let mut ipv6 = tcp_args("::1");
        ipv6.pair = Some("123456".into());
        assert!(connection_config(&ipv6).is_ok());

        assert!(connection_config(&tcp_args("10.0.0.5")).is_ok());
    }

    #[test]
    fn only_a_real_local_socket_exempts_a_credential() {
        let config = |endpoint: LocalEndpoint| RemoteConnectionConfig {
            id: "id".into(),
            name: "Okena TUI".into(),
            host: "10.0.0.5".into(),
            port: 19100,
            saved_token: Some("secret".into()),
            token_obtained_at: None,
            tls: false,
            pinned_cert_sha256: None,
            local_endpoint: Some(endpoint),
        };

        let pipe = config(LocalEndpoint::NamedPipe {
            name: "okena".into(),
        });
        assert!(check_credential_exposure(&pipe, true, false).is_err());

        let socket = config(LocalEndpoint::UnixSocket {
            path: "/tmp/okena.sock".into(),
        });
        assert_eq!(
            check_credential_exposure(&socket, true, false).is_ok(),
            cfg!(unix)
        );
    }

    #[test]
    fn pin_is_rejected_for_socket_transport() {
        let mut args = tcp_args("10.0.0.5");
        args.socket = Some(PathBuf::from("/tmp/okena.sock"));
        args.cert_fingerprint = Some(FINGERPRINT.into());
        assert_eq!(connection_config(&args).is_err(), cfg!(unix));
    }
}
