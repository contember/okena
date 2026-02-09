use crate::client::manager::ConnectionManager;
use okena_core::client::WsClientMessage;
use okena_core::theme::DARK_THEME;

/// Cell data for FFI transfer (flat, no pointers).
#[derive(Debug, Clone)]
pub struct CellData {
    /// The character in this cell.
    pub character: String,
    /// Foreground color as ARGB packed u32.
    pub fg: u32,
    /// Background color as ARGB packed u32.
    pub bg: u32,
    /// Flags: bold(1) | italic(2) | underline(4) | strikethrough(8) | inverse(16) | dim(32).
    pub flags: u8,
}

/// Cursor shape variants.
#[derive(Debug, Clone)]
pub enum CursorShape {
    Block,
    Underline,
    Beam,
}

/// Cursor state for FFI transfer.
#[derive(Debug, Clone)]
pub struct CursorState {
    pub col: u16,
    pub row: u16,
    pub shape: CursorShape,
    pub visible: bool,
}

/// Get the visible terminal cells for rendering.
#[flutter_rust_bridge::frb(sync)]
pub fn get_visible_cells(conn_id: String, terminal_id: String) -> Vec<CellData> {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| {
        holder.get_visible_cells(&DARK_THEME)
    })
    .unwrap_or_default()
}

/// Get the current cursor state.
#[flutter_rust_bridge::frb(sync)]
pub fn get_cursor(conn_id: String, terminal_id: String) -> CursorState {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| holder.get_cursor())
        .unwrap_or(CursorState {
            col: 0,
            row: 0,
            shape: CursorShape::Block,
            visible: true,
        })
}

/// Send text input to a terminal.
pub async fn send_text(conn_id: String, terminal_id: String, text: String) -> anyhow::Result<()> {
    let mgr = ConnectionManager::get();
    mgr.send_ws_message(
        &conn_id,
        WsClientMessage::SendText {
            terminal_id,
            text,
        },
    );
    Ok(())
}

/// Resize a terminal.
pub async fn resize_terminal(
    conn_id: String,
    terminal_id: String,
    cols: u16,
    rows: u16,
) -> anyhow::Result<()> {
    let mgr = ConnectionManager::get();
    mgr.resize_terminal(&conn_id, &terminal_id, cols, rows);
    Ok(())
}
