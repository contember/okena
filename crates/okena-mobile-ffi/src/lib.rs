//! uniffi FFI surface for the React Native mobile app.
//!
//! Exposes ~60 functions via uniffi proc-macros (`#[uniffi::export]`,
//! `#[derive(uniffi::Record)]`, `#[derive(uniffi::Enum)]`) so
//! `uniffi-bindgen-react-native` (ubrn) can emit a JSI TurboModule. This
//! replaces the `flutter_rust_bridge` `api/` layer of the retired `mobile/native`
//! crate, whose plain-Rust engine now lives here directly (see below).
//!
//! The networking/emulation engine lives in `crate::client`
//! (`ConnectionManager`, `MobileConnectionHandler`, `TerminalHolder`) and the
//! plain state-extraction structs in `crate::api` — both carried over from the
//! retired `mobile/native` Flutter crate (frb attributes stripped). This crate
//! is self-contained: it does not depend on any Flutter tooling.
//!
//! ## Async strategy
//! The frb api split sync vs. async based on whether the body actually awaits:
//!  - Functions whose bodies are synchronous (the `with_terminal` / `get_state`
//!    accessors, `send_text` / `send_special_key`, which only enqueue a WS
//!    message via `send_ws_message`) are exported as plain sync uniffi fns —
//!    important for the render hot path.
//!  - Functions that genuinely `.await` reqwest (`*_terminal` actions, git,
//!    services, project/layout mutations, `read_content`) are exported as
//!    `async fn` with `#[uniffi::export(async_runtime = "tokio")]`, which ubrn
//!    maps to JS Promises.

#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

mod api;
mod claude;
mod client;
mod logging;
pub mod render;
mod text_export;
mod types;
mod usage;

use okena_core::api::ActionRequest;
use okena_core::keys::SpecialKey;
use okena_core::theme::DARK_THEME;
use okena_core::types::{DiffMode, SplitDirection};
use okena_transport::client::{WsClientMessage, collect_state_terminal_ids};
use std::collections::HashMap;

use crate::client::manager::ConnectionManager;

pub use types::{
    CellData, ConnectionStatus, CursorShape, CursorState, FolderInfo, FullscreenInfo, ProjectInfo,
    ScrollInfo, SelectionBounds, ServiceInfo, TerminalActivity,
};
pub use usage::{
    ExtraUsageInfo, PoolAccountInfo, PoolAmountInfo, PoolInfo, PoolNonstopInfo, PoolPaceInfo,
    PoolTodayInfo, ProviderUsageInfo, ResetCreditInfo, ResetCreditsInfo, UsageCreditsInfo,
    UsageInfo, UsagePointInfo, UsageWindowInfo,
};

uniffi::setup_scaffolding!();

// ── Connection lifecycle ────────────────────────────────────────────

/// Initialize the app. Must be called once at startup before any other fn.
#[uniffi::export]
pub fn init_app() {
    logging::init();
    ConnectionManager::init();
}

/// The app returned to the foreground: resume reconnect attempts and retry
/// every connection now with a fresh attempt budget (an established session
/// gets a quick liveness probe instead).
#[uniffi::export]
pub fn on_app_foreground() {
    ConnectionManager::get().on_app_foreground();
}

/// The app went to the background: pause reconnect attempts until
/// [`on_app_foreground`]. Established sessions are left running.
#[uniffi::export]
pub fn on_app_background() {
    ConnectionManager::get().on_app_background();
}

/// Retry one connection now ("Retry now" in the UI): skips the backoff and
/// resets the attempt budget, keeping terminals and cached state.
#[uniffi::export]
pub fn reconnect_now(conn_id: String) {
    ConnectionManager::get().reconnect_now(&conn_id);
}

/// Connect to an Okena remote server. Returns a connection ID.
///
/// `tls` and `pinned_cert_fingerprint` describe the desired transport security
/// for this connection (RN ships TLS-capable from day one). They are accepted
/// at the binding boundary so the RN UI and persisted server config can carry
/// the TLS flag.
///
/// Existing pins are enforced immediately; first-use connections capture and
/// report their fingerprint through the normal connection events.
#[uniffi::export]
pub fn connect(
    host: String,
    port: u16,
    saved_token: Option<String>,
    tls: bool,
    pinned_cert_fingerprint: Option<String>,
) -> String {
    let mgr = ConnectionManager::get();
    let conn_id = mgr.add_connection(&host, port, saved_token, tls, pinned_cert_fingerprint);
    mgr.connect(&conn_id);
    conn_id
}

/// Get the current auth token for a connection (if paired).
#[uniffi::export]
pub fn get_token(conn_id: String) -> Option<String> {
    ConnectionManager::get().get_token(&conn_id)
}

/// Get the server certificate fingerprint this connection is pinned to.
///
/// `None` means no TLS identity has been established, so a token obtained over
/// this connection was accepted trust-on-first-use and must not be replayed as
/// if it were pinned. The RN client persists this alongside the token.
#[uniffi::export]
pub fn get_cert_fingerprint(conn_id: String) -> Option<String> {
    ConnectionManager::get().get_cert_fingerprint(&conn_id)
}

/// Pair with the server using a pairing code. (Body is synchronous: it only
/// kicks off the manager's pairing task.)
#[uniffi::export]
pub fn pair(conn_id: String, code: String) {
    ConnectionManager::get().pair(&conn_id, &code);
}

/// Disconnect from a server.
#[uniffi::export]
pub fn disconnect(conn_id: String) {
    ConnectionManager::get().disconnect(&conn_id);
}

/// Get current connection status.
#[uniffi::export]
pub fn connection_status(conn_id: String) -> ConnectionStatus {
    // Delegate to the native api fn, which maps okena-transport's status into
    // its own enum; we then convert that into our uniffi enum.
    crate::api::connection::connection_status(conn_id).into()
}

/// Seconds since last WS activity (terminal output). Large value if missing.
#[uniffi::export]
pub fn seconds_since_activity(conn_id: String) -> f64 {
    ConnectionManager::get().seconds_since_activity(&conn_id)
}

// ── Terminal rendering / input ──────────────────────────────────────

/// Get the visible terminal cells for rendering (records form).
///
/// Kept for non-hot-path callers; the render loop should prefer
/// [`get_visible_cells_packed`].
#[uniffi::export]
pub fn get_visible_cells(conn_id: String, terminal_id: String) -> Vec<CellData> {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| {
        holder
            .get_visible_cells(&DARK_THEME)
            .into_iter()
            .map(Into::into)
            .collect()
    })
    .unwrap_or_default()
}

/// Get the visible terminal grid as a packed little-endian byte buffer.
///
/// This is the hot-path render bridge (migration plan Decision C): instead of
/// marshaling thousands of `CellData` records per frame across JSI, the RN
/// Skia canvas reads this compact buffer directly as an `ArrayBuffer`.
///
/// ## Format (all multi-byte values little-endian)
/// ```text
/// Header (4 bytes):
///   cols : u16 LE
///   rows : u16 LE
/// Then cols*rows cells, row-major, 13 bytes each:
///   codepoint : u32 LE   Unicode scalar of the cell's base char, without its
///                        zero-width marks — a fixed-width cell cannot carry
///                        them (0x20 for empty or wide-char-spacer cells)
///   fg        : u32 LE   ARGB
///   bg        : u32 LE   ARGB
///   flags     : u8       bold(1)|italic(2)|underline(4)|strikethrough(8)|
///                        inverse(16)|dim(32)
/// ```
/// Total length = 4 + cols*rows*13 bytes. Built from the same `CellData` list
/// `get_visible_cells` returns. If the terminal is missing, returns a 0x0
/// header (`[0, 0, 0, 0]`).
#[uniffi::export]
pub fn get_visible_cells_packed(conn_id: String, terminal_id: String) -> Vec<u8> {
    let mgr = ConnectionManager::get();
    let cells = mgr
        .with_terminal(&conn_id, &terminal_id, |holder| {
            holder.get_visible_cells(&DARK_THEME)
        })
        .unwrap_or_default();

    // Determine grid dimensions from the live cursor/grid via scroll_info would
    // require another lock; instead derive cols from the row width recorded by
    // the holder. The cell list is exactly cols*rows row-major, so we recover
    // dimensions from the holder's reported scroll info (visible_lines = rows)
    // and divide. To avoid a second terminal access we read both in one borrow.
    let (cols, rows) = mgr
        .with_terminal(&conn_id, &terminal_id, |holder| {
            let (_total, visible, _offset) = holder.scroll_info();
            let rows = visible as u16;
            let cols = cells.len().checked_div(visible).unwrap_or(0) as u16;
            (cols, rows)
        })
        .unwrap_or((0, 0));

    let mut buf = Vec::with_capacity(4 + cells.len() * 13);
    buf.extend_from_slice(&cols.to_le_bytes());
    buf.extend_from_slice(&rows.to_le_bytes());
    for cell in &cells {
        // Base scalar only: one fixed u32 per cell has no room for the cell's
        // zero-width marks, so decomposed text reaches the canvas unaccented.
        let codepoint: u32 = cell
            .character
            .chars()
            .next()
            .map(|c| c as u32)
            .unwrap_or(0x20);
        let codepoint = if codepoint == 0 { 0x20 } else { codepoint };
        buf.extend_from_slice(&codepoint.to_le_bytes());
        buf.extend_from_slice(&cell.fg.to_le_bytes());
        buf.extend_from_slice(&cell.bg.to_le_bytes());
        buf.push(cell.flags);
    }
    buf
}

/// Get the current cursor state.
#[uniffi::export]
pub fn get_cursor(conn_id: String, terminal_id: String) -> CursorState {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| holder.get_cursor().into())
        .unwrap_or(CursorState {
            col: 0,
            row: 0,
            shape: CursorShape::Block,
            visible: true,
        })
}

/// Send text input to a terminal. Synchronous: only enqueues a WS message.
/// Like every desktop input path, typing snaps the view back to the bottom.
#[uniffi::export]
pub fn send_text(conn_id: String, terminal_id: String, text: String) {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| holder.scroll_to_bottom());
    mgr.send_ws_message(
        &conn_id,
        WsClientMessage::SendInput {
            terminal_id,
            data: text.into_bytes(),
        },
    );
}

/// Resize a terminal (local grid + WS resize message).
#[uniffi::export]
pub fn resize_terminal(conn_id: String, terminal_id: String, cols: u16, rows: u16) {
    ConnectionManager::get().resize_terminal(&conn_id, &terminal_id, cols, rows);
}

/// Resize only the local alacritty grid (no WS message). Used when adapting to
/// the server's terminal size.
#[uniffi::export]
pub fn resize_local(conn_id: String, terminal_id: String, cols: u16, rows: u16) {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| {
        holder.resize(cols, rows);
    });
}

// ── Scrolling ───────────────────────────────────────────────────────

/// Scroll the terminal display (positive = up, negative = down).
#[uniffi::export]
pub fn scroll(conn_id: String, terminal_id: String, delta: i32) {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| {
        holder.scroll(delta);
    });
}

/// Scroll gesture of `lines` notches at visual cell `col`/`row` (positive = up).
///
/// Mirrors the desktop `TerminalContent::handle_scroll`: when the app enabled
/// mouse reporting (vim, less with mouse, tmux `mouse on`, agent TUIs) the
/// notches go to the PTY as wheel events; otherwise the local scrollback moves.
#[uniffi::export]
pub fn scroll_wheel(conn_id: String, terminal_id: String, lines: i32, col: u16, row: u16) {
    if lines == 0 {
        return;
    }
    let mgr = ConnectionManager::get();
    let wheel = mgr
        .with_terminal(&conn_id, &terminal_id, |holder| {
            let wheel = holder.mouse_wheel_input(lines > 0, col, row, lines.unsigned_abs());
            if wheel.is_none() {
                holder.scroll(lines);
            }
            wheel
        })
        .flatten();
    if let Some(data) = wheel {
        mgr.send_ws_message(&conn_id, WsClientMessage::SendInput { terminal_id, data });
    }
}

/// Get scroll info: total lines, visible lines, display offset.
#[uniffi::export]
pub fn get_scroll_info(conn_id: String, terminal_id: String) -> ScrollInfo {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| {
        let (total, visible, offset) = holder.scroll_info();
        ScrollInfo {
            total_lines: total as u32,
            visible_lines: visible as u32,
            display_offset: offset as u32,
        }
    })
    .unwrap_or(ScrollInfo {
        total_lines: 0,
        visible_lines: 0,
        display_offset: 0,
    })
}

// ── Selection ───────────────────────────────────────────────────────

/// Start a character-level selection at col/row.
#[uniffi::export]
pub fn start_selection(conn_id: String, terminal_id: String, col: u16, row: u16) {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| {
        holder.start_selection(col as usize, row as usize);
    });
}

/// Start a word (semantic) selection at col/row.
#[uniffi::export]
pub fn start_word_selection(conn_id: String, terminal_id: String, col: u16, row: u16) {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| {
        holder.start_word_selection(col as usize, row as usize);
    });
}

/// Extend the current selection to col/row.
#[uniffi::export]
pub fn update_selection(conn_id: String, terminal_id: String, col: u16, row: u16) {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| {
        holder.update_selection(col as usize, row as usize);
    });
}

/// Clear the current selection.
#[uniffi::export]
pub fn clear_selection(conn_id: String, terminal_id: String) {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| {
        holder.clear_selection();
    });
}

/// Get the selected text, if any.
#[uniffi::export]
pub fn get_selected_text(conn_id: String, terminal_id: String) -> Option<String> {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| holder.get_selected_text())
        .flatten()
}

/// Get selection bounds for rendering.
#[uniffi::export]
pub fn get_selection_bounds(conn_id: String, terminal_id: String) -> Option<SelectionBounds> {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| {
        holder
            .selection_bounds()
            .map(|((sc, sr), (ec, er))| SelectionBounds {
                start_col: sc as u16,
                start_row: sr,
                end_col: ec as u16,
                end_row: er,
            })
    })
    .flatten()
}

// ── State queries ───────────────────────────────────────────────────

/// Get all projects from the cached remote state.
#[uniffi::export]
pub fn get_projects(conn_id: String) -> Vec<ProjectInfo> {
    crate::api::state::get_projects(conn_id)
        .into_iter()
        .map(Into::into)
        .collect()
}

/// Activity signals of every mirrored terminal of a connection (unseen bell /
/// notification, time since the last live output). One call per state poll
/// feeds the project list's activity ordering.
#[uniffi::export]
pub fn get_terminal_activity(conn_id: String) -> Vec<TerminalActivity> {
    ConnectionManager::get()
        .terminal_activity(&conn_id)
        .into_iter()
        .map(|(terminal_id, activity)| TerminalActivity::new(terminal_id, activity))
        .collect()
}

/// The user viewed a terminal: clear its unseen bell and notification.
#[uniffi::export]
pub fn mark_terminal_seen(conn_id: String, terminal_id: String) {
    ConnectionManager::get().with_terminal(&conn_id, &terminal_id, |holder| holder.mark_seen());
}

/// Get the focused project ID from the cached remote state.
#[uniffi::export]
pub fn get_focused_project_id(conn_id: String) -> Option<String> {
    crate::api::state::get_focused_project_id(conn_id)
}

/// Get folders from the cached remote state.
#[uniffi::export]
pub fn get_folders(conn_id: String) -> Vec<FolderInfo> {
    crate::api::state::get_folders(conn_id)
        .into_iter()
        .map(Into::into)
        .collect()
}

/// Get the project order from the cached remote state.
#[uniffi::export]
pub fn get_project_order(conn_id: String) -> Vec<String> {
    crate::api::state::get_project_order(conn_id)
}

/// Get fullscreen terminal info.
#[uniffi::export]
pub fn get_fullscreen_terminal(conn_id: String) -> Option<FullscreenInfo> {
    crate::api::state::get_fullscreen_terminal(conn_id).map(Into::into)
}

/// Get layout JSON for a project.
#[uniffi::export]
pub fn get_project_layout_json(conn_id: String, project_id: String) -> Option<String> {
    let mgr = ConnectionManager::get();
    let state = mgr.get_state(&conn_id)?;
    let project = state.projects.iter().find(|p| p.id == project_id)?;
    let layout = project.layout.as_ref()?;
    serde_json::to_string(layout).ok()
}

/// Check if a terminal has unprocessed output (dirty flag).
#[uniffi::export]
pub fn is_dirty(conn_id: String, terminal_id: String) -> bool {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| holder.is_dirty())
        .unwrap_or(false)
}

/// Consume the dirty flag: true once per batch of changes (output, resize,
/// scroll, selection). The render loop polls this so idle panes stop repainting.
#[uniffi::export]
pub fn take_dirty(conn_id: String, terminal_id: String) -> bool {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| holder.take_dirty())
        .unwrap_or(false)
}

// ── Agent usage ─────────────────────────────────────────────────────

/// Latest Claude Code / Codex / Grok usage snapshot the server pushed
/// (`usage_changed`). `None` until the first push (or for a daemon that
/// predates the usage API).
#[uniffi::export]
pub fn get_usage(conn_id: String) -> Option<UsageInfo> {
    ConnectionManager::get()
        .get_usage(&conn_id)
        .map(|usage| UsageInfo::from(&usage))
}

/// Changes whenever the stored usage snapshot changes; `0` before the first
/// one. Lets the state poll call [`get_usage`] (history-heavy) only on change.
#[uniffi::export]
pub fn get_usage_revision(conn_id: String) -> u32 {
    ConnectionManager::get().usage_revision(&conn_id)
}

/// Ask the daemon to refetch usage now. `provider` is `"claude"`, `"codex"`,
/// `"grok"` or `None` for every enabled provider. The daemon throttles to one attempt
/// per provider per 60 s; the fresh snapshot arrives via `usage_changed`.
/// Resolves to whether a fetch is now running for the requested provider(s)
/// (`false` when throttled), so the UI can end its spinner right away.
#[uniffi::export(async_runtime = "tokio")]
pub async fn request_usage_refresh(
    conn_id: String,
    provider: Option<String>,
) -> Result<bool, MobileFfiError> {
    let provider = match provider.as_deref() {
        None => None,
        Some(name) => Some(usage::parse_provider(name).ok_or_else(|| MobileFfiError::Action {
            message: format!("Unknown usage provider: {name}"),
        })?),
    };
    Ok(ConnectionManager::get()
        .request_usage_refresh(&conn_id, provider)
        .await?)
}

/// Get all terminal IDs from the cached remote state (flat list).
#[uniffi::export]
pub fn get_all_terminal_ids(conn_id: String) -> Vec<String> {
    let mgr = ConnectionManager::get();
    match mgr.get_state(&conn_id) {
        Some(state) => collect_state_terminal_ids(&state),
        None => Vec::new(),
    }
}

/// Open Claude dialogs per terminal id, from the cached remote state.
/// Terminals without an open dialog are absent.
#[uniffi::export]
pub fn get_claude_dialog_counts(conn_id: String) -> HashMap<String, u32> {
    ConnectionManager::get()
        .get_state(&conn_id)
        .map(|state| state.claude_dialogs)
        .unwrap_or_default()
}

/// Terminals whose `claude` is connected to Claude remote, from the cached
/// remote state.
#[uniffi::export]
pub fn get_claude_sessions(conn_id: String) -> Vec<String> {
    ConnectionManager::get()
        .get_state(&conn_id)
        .map(|state| state.claude_sessions)
        .unwrap_or_default()
}

/// Send a special key (e.g. `"Enter"`, `"CtrlC"`, `"ArrowUp"`) to a terminal.
///
/// The body is synchronous (it only enqueues a WS message), so this is a sync
/// uniffi fn. The error path mirrors the frb version: an unknown key name
/// yields an error.
#[uniffi::export]
pub fn send_special_key(
    conn_id: String,
    terminal_id: String,
    key: String,
) -> Result<(), MobileFfiError> {
    let special_key: SpecialKey = serde_json::from_value(serde_json::Value::String(key.clone()))
        .map_err(|_| MobileFfiError::Action {
            message: format!("Unknown special key: {key}"),
        })?;
    let bytes = special_key.to_bytes();
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| holder.scroll_to_bottom());
    mgr.send_ws_message(
        &conn_id,
        WsClientMessage::SendInput {
            terminal_id,
            data: bytes.to_vec(),
        },
    );
    Ok(())
}

// ── Error type for async/fallible exports ───────────────────────────

/// Error returned by fallible FFI functions. uniffi maps this to a thrown
/// error / rejected Promise on the RN side.
#[derive(Debug, uniffi::Error)]
pub enum MobileFfiError {
    /// A remote action (HTTP POST /v1/actions) or input validation failed.
    Action { message: String },
}

impl std::fmt::Display for MobileFfiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MobileFfiError::Action { message } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for MobileFfiError {}

impl From<anyhow::Error> for MobileFfiError {
    fn from(e: anyhow::Error) -> Self {
        MobileFfiError::Action {
            message: e.to_string(),
        }
    }
}

async fn send_mobile_action(conn_id: &str, action: ActionRequest) -> Result<(), MobileFfiError> {
    ConnectionManager::get()
        .send_action(conn_id, action)
        .await?;
    Ok(())
}

// ── Terminal actions (async — await reqwest) ────────────────────────

/// Create a new terminal in the given project.
#[uniffi::export(async_runtime = "tokio")]
pub async fn create_terminal(conn_id: String, project_id: String) -> Result<(), MobileFfiError> {
    send_mobile_action(&conn_id, ActionRequest::CreateTerminal { project_id }).await
}

/// Close a terminal in the given project.
#[uniffi::export(async_runtime = "tokio")]
pub async fn close_terminal(
    conn_id: String,
    project_id: String,
    terminal_id: String,
) -> Result<(), MobileFfiError> {
    send_mobile_action(
        &conn_id,
        ActionRequest::CloseTerminal {
            project_id,
            terminal_id,
        },
    )
    .await
}

/// Close multiple terminals in a project.
#[uniffi::export(async_runtime = "tokio")]
pub async fn close_terminals(
    conn_id: String,
    project_id: String,
    terminal_ids: Vec<String>,
) -> Result<(), MobileFfiError> {
    send_mobile_action(
        &conn_id,
        ActionRequest::CloseTerminals {
            project_id,
            terminal_ids,
        },
    )
    .await
}

/// Rename a terminal.
#[uniffi::export(async_runtime = "tokio")]
pub async fn rename_terminal(
    conn_id: String,
    project_id: String,
    terminal_id: String,
    name: String,
) -> Result<(), MobileFfiError> {
    send_mobile_action(
        &conn_id,
        ActionRequest::RenameTerminal {
            project_id,
            terminal_id,
            name,
        },
    )
    .await
}

/// Record activity for a terminal focused in the mobile client's local state.
#[uniffi::export(async_runtime = "tokio")]
pub async fn focus_terminal(
    conn_id: String,
    project_id: String,
    terminal_id: String,
) -> Result<(), MobileFfiError> {
    let _ = terminal_id;
    send_mobile_action(
        &conn_id,
        ActionRequest::RecordProjectActivity { project_id },
    )
    .await
}

/// Toggle minimized state of a terminal.
#[uniffi::export(async_runtime = "tokio")]
pub async fn toggle_minimized(
    conn_id: String,
    project_id: String,
    terminal_id: String,
) -> Result<(), MobileFfiError> {
    ConnectionManager::get()
        .toggle_minimized_local(&conn_id, &project_id, &terminal_id)
        .map_err(|message| MobileFfiError::Action { message })
}

/// Set/clear fullscreen terminal.
#[uniffi::export(async_runtime = "tokio")]
pub async fn set_fullscreen(
    conn_id: String,
    project_id: String,
    terminal_id: Option<String>,
) -> Result<(), MobileFfiError> {
    ConnectionManager::get()
        .set_fullscreen_local(&conn_id, &project_id, terminal_id)
        .map_err(|message| MobileFfiError::Action { message })
}

/// Split a terminal pane. `direction` is "vertical" or "horizontal".
#[uniffi::export(async_runtime = "tokio")]
pub async fn split_terminal(
    conn_id: String,
    project_id: String,
    path: Vec<u32>,
    direction: String,
) -> Result<(), MobileFfiError> {
    let dir = match direction.as_str() {
        "vertical" => SplitDirection::Vertical,
        _ => SplitDirection::Horizontal,
    };
    send_mobile_action(
        &conn_id,
        ActionRequest::SplitTerminal {
            project_id,
            path: path.into_iter().map(|v| v as usize).collect(),
            direction: dir,
        },
    )
    .await
}

/// Run a command in a terminal (presses Enter automatically).
#[uniffi::export(async_runtime = "tokio")]
pub async fn run_command(
    conn_id: String,
    terminal_id: String,
    command: String,
) -> Result<(), MobileFfiError> {
    send_mobile_action(
        &conn_id,
        ActionRequest::RunCommand {
            terminal_id,
            command,
        },
    )
    .await
}

/// Read terminal content as text.
#[uniffi::export(async_runtime = "tokio")]
pub async fn read_content(conn_id: String, terminal_id: String) -> Result<String, MobileFfiError> {
    Ok(ConnectionManager::get()
        .send_action_with_response(&conn_id, ActionRequest::ReadContent { terminal_id })
        .await?)
}

// ── Git actions (async) ─────────────────────────────────────────────

/// Get detailed git status for a project.
#[uniffi::export(async_runtime = "tokio")]
pub async fn git_status(conn_id: String, project_id: String) -> Result<String, MobileFfiError> {
    Ok(ConnectionManager::get()
        .send_action_with_response(&conn_id, ActionRequest::GitStatus { project_id })
        .await?)
}

/// Get git diff summary for a project.
#[uniffi::export(async_runtime = "tokio")]
pub async fn git_diff_summary(
    conn_id: String,
    project_id: String,
) -> Result<String, MobileFfiError> {
    Ok(ConnectionManager::get()
        .send_action_with_response(&conn_id, ActionRequest::GitDiffSummary { project_id })
        .await?)
}

/// Get git diff for a project. `mode` is "working_tree" or "staged".
#[uniffi::export(async_runtime = "tokio")]
pub async fn git_diff(
    conn_id: String,
    project_id: String,
    mode: String,
) -> Result<String, MobileFfiError> {
    let diff_mode = match mode.as_str() {
        "staged" => DiffMode::Staged,
        _ => DiffMode::WorkingTree,
    };
    Ok(ConnectionManager::get()
        .send_action_with_response(
            &conn_id,
            ActionRequest::GitDiff {
                project_id,
                mode: diff_mode,
                ignore_whitespace: false,
            },
        )
        .await?)
}

/// Get git branches for a project.
#[uniffi::export(async_runtime = "tokio")]
pub async fn git_branches(conn_id: String, project_id: String) -> Result<String, MobileFfiError> {
    Ok(ConnectionManager::get()
        .send_action_with_response(&conn_id, ActionRequest::GitBranches { project_id })
        .await?)
}

/// Get file contents from git (working tree or staged).
#[uniffi::export(async_runtime = "tokio")]
pub async fn git_file_contents(
    conn_id: String,
    project_id: String,
    file_path: String,
    mode: String,
) -> Result<String, MobileFfiError> {
    let diff_mode = match mode.as_str() {
        "staged" => DiffMode::Staged,
        _ => DiffMode::WorkingTree,
    };
    Ok(ConnectionManager::get()
        .send_action_with_response(
            &conn_id,
            ActionRequest::GitFileContents {
                project_id,
                file_path,
                mode: diff_mode,
            },
        )
        .await?)
}

// ── Service actions (async) ─────────────────────────────────────────

/// Start a service.
#[uniffi::export(async_runtime = "tokio")]
pub async fn start_service(
    conn_id: String,
    project_id: String,
    service_name: String,
) -> Result<(), MobileFfiError> {
    send_mobile_action(
        &conn_id,
        ActionRequest::StartService {
            project_id,
            service_name,
        },
    )
    .await
}

/// Stop a service.
#[uniffi::export(async_runtime = "tokio")]
pub async fn stop_service(
    conn_id: String,
    project_id: String,
    service_name: String,
) -> Result<(), MobileFfiError> {
    send_mobile_action(
        &conn_id,
        ActionRequest::StopService {
            project_id,
            service_name,
        },
    )
    .await
}

/// Restart a service.
#[uniffi::export(async_runtime = "tokio")]
pub async fn restart_service(
    conn_id: String,
    project_id: String,
    service_name: String,
) -> Result<(), MobileFfiError> {
    send_mobile_action(
        &conn_id,
        ActionRequest::RestartService {
            project_id,
            service_name,
        },
    )
    .await
}

/// Start all services in a project.
#[uniffi::export(async_runtime = "tokio")]
pub async fn start_all_services(conn_id: String, project_id: String) -> Result<(), MobileFfiError> {
    send_mobile_action(&conn_id, ActionRequest::StartAllServices { project_id }).await
}

/// Stop all services in a project.
#[uniffi::export(async_runtime = "tokio")]
pub async fn stop_all_services(conn_id: String, project_id: String) -> Result<(), MobileFfiError> {
    send_mobile_action(&conn_id, ActionRequest::StopAllServices { project_id }).await
}

/// Reload services config for a project.
#[uniffi::export(async_runtime = "tokio")]
pub async fn reload_services(conn_id: String, project_id: String) -> Result<(), MobileFfiError> {
    send_mobile_action(&conn_id, ActionRequest::ReloadServices { project_id }).await
}

// ── Project management (async) ──────────────────────────────────────

/// Add a new project.
#[uniffi::export(async_runtime = "tokio")]
pub async fn add_project(
    conn_id: String,
    name: String,
    path: String,
) -> Result<(), MobileFfiError> {
    send_mobile_action(&conn_id, ActionRequest::AddProject { name, path }).await
}

/// Set project color (named color, e.g. "blue"; unknown → default).
#[uniffi::export(async_runtime = "tokio")]
pub async fn set_project_color(
    conn_id: String,
    project_id: String,
    color: String,
) -> Result<(), MobileFfiError> {
    let folder_color: okena_core::theme::FolderColor =
        serde_json::from_value(serde_json::Value::String(color)).unwrap_or_default();
    send_mobile_action(
        &conn_id,
        ActionRequest::SetProjectColor {
            project_id,
            color: folder_color,
        },
    )
    .await
}

/// Set folder color (named color; unknown → default).
#[uniffi::export(async_runtime = "tokio")]
pub async fn set_folder_color(
    conn_id: String,
    folder_id: String,
    color: String,
) -> Result<(), MobileFfiError> {
    let folder_color: okena_core::theme::FolderColor =
        serde_json::from_value(serde_json::Value::String(color)).unwrap_or_default();
    send_mobile_action(
        &conn_id,
        ActionRequest::SetFolderColor {
            folder_id,
            color: folder_color,
        },
    )
    .await
}

/// Reorder a project within a folder.
#[uniffi::export(async_runtime = "tokio")]
pub async fn reorder_project_in_folder(
    conn_id: String,
    folder_id: String,
    project_id: String,
    new_index: u32,
) -> Result<(), MobileFfiError> {
    send_mobile_action(
        &conn_id,
        ActionRequest::ReorderProjectInFolder {
            folder_id,
            project_id,
            new_index: new_index as usize,
        },
    )
    .await
}

// ── Layout actions (async) ──────────────────────────────────────────

/// Update split sizes for a split pane.
#[uniffi::export(async_runtime = "tokio")]
pub async fn update_split_sizes(
    conn_id: String,
    project_id: String,
    path: Vec<u32>,
    sizes: Vec<f32>,
) -> Result<(), MobileFfiError> {
    send_mobile_action(
        &conn_id,
        ActionRequest::UpdateSplitSizes {
            project_id,
            path: path.into_iter().map(|v| v as usize).collect(),
            sizes,
        },
    )
    .await
}

/// Add a new tab to a tab group.
#[uniffi::export(async_runtime = "tokio")]
pub async fn add_tab(
    conn_id: String,
    project_id: String,
    path: Vec<u32>,
    in_group: bool,
) -> Result<(), MobileFfiError> {
    send_mobile_action(
        &conn_id,
        ActionRequest::AddTab {
            project_id,
            path: path.into_iter().map(|v| v as usize).collect(),
            in_group,
        },
    )
    .await
}

/// Set the active tab in a tab group.
#[uniffi::export(async_runtime = "tokio")]
pub async fn set_active_tab(
    conn_id: String,
    project_id: String,
    path: Vec<u32>,
    index: u32,
) -> Result<(), MobileFfiError> {
    let path = path
        .into_iter()
        .map(usize::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| MobileFfiError::Action {
            message: format!("Invalid tab path: {error}"),
        })?;
    let index = usize::try_from(index).map_err(|error| MobileFfiError::Action {
        message: format!("Invalid tab index: {error}"),
    })?;
    ConnectionManager::get()
        .set_active_tab_local(&conn_id, &project_id, &path, index)
        .map_err(|message| MobileFfiError::Action { message })
}

/// Move a tab within a tab group.
#[uniffi::export(async_runtime = "tokio")]
pub async fn move_tab(
    conn_id: String,
    project_id: String,
    path: Vec<u32>,
    from_index: u32,
    to_index: u32,
) -> Result<(), MobileFfiError> {
    send_mobile_action(
        &conn_id,
        ActionRequest::MoveTab {
            project_id,
            path: path.into_iter().map(|v| v as usize).collect(),
            from_index: from_index as usize,
            to_index: to_index as usize,
        },
    )
    .await
}

/// Move a terminal into a tab group.
#[uniffi::export(async_runtime = "tokio")]
pub async fn move_terminal_to_tab_group(
    conn_id: String,
    project_id: String,
    terminal_id: String,
    target_path: Vec<u32>,
    position: Option<u32>,
    target_project_id: Option<String>,
) -> Result<(), MobileFfiError> {
    send_mobile_action(
        &conn_id,
        ActionRequest::MoveTerminalToTabGroup {
            project_id,
            terminal_id,
            target_path: target_path.into_iter().map(|v| v as usize).collect(),
            position: position.map(|p| p as usize),
            target_project_id,
        },
    )
    .await
}

/// Move a pane to a drop zone relative to another terminal.
#[uniffi::export(async_runtime = "tokio")]
pub async fn move_pane_to(
    conn_id: String,
    project_id: String,
    terminal_id: String,
    target_project_id: String,
    target_terminal_id: String,
    zone: String,
) -> Result<(), MobileFfiError> {
    send_mobile_action(
        &conn_id,
        ActionRequest::MovePaneTo {
            project_id,
            terminal_id,
            target_project_id,
            target_terminal_id,
            zone,
        },
    )
    .await
}

// ── Claude remote (async — return JSON strings) ─────────────────────

/// The Claude session in a pane, as JSON `ApiClaudeSession`.
#[uniffi::export(async_runtime = "tokio")]
pub async fn claude_get_session(
    conn_id: String,
    terminal_id: String,
) -> Result<String, MobileFfiError> {
    claude::get_session(&conn_id, &terminal_id).await
}

/// One transcript page, the server's JSON `ApiClaudeTranscriptPage` forwarded
/// unchanged (entry kinds are left to the RN parser). Cursor 0 asks for
/// the first page; pass back the previous page's `cursor` and `session_id` to
/// get only newer entries.
#[uniffi::export(async_runtime = "tokio")]
pub async fn claude_get_transcript(
    conn_id: String,
    terminal_id: String,
    cursor: u64,
    session_id: Option<String>,
) -> Result<String, MobileFfiError> {
    claude::get_transcript(&conn_id, &terminal_id, cursor, session_id.as_deref()).await
}

/// Send a prompt to Claude, as JSON `ApiClaudePromptResponse`. An undeliverable
/// or empty prompt fails with the server's reason.
#[uniffi::export(async_runtime = "tokio")]
pub async fn claude_send_prompt(
    conn_id: String,
    terminal_id: String,
    text: String,
) -> Result<String, MobileFfiError> {
    claude::send_prompt(&conn_id, &terminal_id, text).await
}

/// Answer an open dialog. `decision_json` is an `ApiClaudeDecision`. A dialog
/// that is no longer open fails with the message "dialog already answered".
#[uniffi::export(async_runtime = "tokio")]
pub async fn claude_answer_dialog(
    conn_id: String,
    terminal_id: String,
    dialog_id: String,
    decision_json: String,
) -> Result<(), MobileFfiError> {
    claude::answer_dialog(&conn_id, &terminal_id, &dialog_id, &decision_json).await
}
