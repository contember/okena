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

/// Scroll info for FFI transfer.
#[derive(Debug, Clone)]
pub struct ScrollInfo {
    pub total_lines: u32,
    pub visible_lines: u32,
    pub display_offset: u32,
}

/// Selection bounds for FFI transfer.
#[derive(Debug, Clone)]
pub struct SelectionBounds {
    pub start_col: u16,
    pub start_row: i32,
    pub end_col: u16,
    pub end_row: i32,
}

/// Scroll the terminal display (positive = up, negative = down).
#[flutter_rust_bridge::frb(sync)]
pub fn scroll(conn_id: String, terminal_id: String, delta: i32) {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| {
        holder.scroll(delta);
    });
}

/// Get scroll info: total lines, visible lines, display offset.
#[flutter_rust_bridge::frb(sync)]
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

/// Start a character-level selection at col/row.
#[flutter_rust_bridge::frb(sync)]
pub fn start_selection(conn_id: String, terminal_id: String, col: u16, row: u16) {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| {
        holder.start_selection(col as usize, row as usize);
    });
}

/// Start a word (semantic) selection at col/row.
#[flutter_rust_bridge::frb(sync)]
pub fn start_word_selection(conn_id: String, terminal_id: String, col: u16, row: u16) {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| {
        holder.start_word_selection(col as usize, row as usize);
    });
}

/// Extend the current selection to col/row.
#[flutter_rust_bridge::frb(sync)]
pub fn update_selection(conn_id: String, terminal_id: String, col: u16, row: u16) {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| {
        holder.update_selection(col as usize, row as usize);
    });
}

/// Clear the current selection.
#[flutter_rust_bridge::frb(sync)]
pub fn clear_selection(conn_id: String, terminal_id: String) {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| {
        holder.clear_selection();
    });
}

/// Get the selected text, if any.
#[flutter_rust_bridge::frb(sync)]
pub fn get_selected_text(conn_id: String, terminal_id: String) -> Option<String> {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| {
        holder.get_selected_text()
    })
    .flatten()
}

/// Get selection bounds for rendering.
#[flutter_rust_bridge::frb(sync)]
pub fn get_selection_bounds(conn_id: String, terminal_id: String) -> Option<SelectionBounds> {
    let mgr = ConnectionManager::get();
    mgr.with_terminal(&conn_id, &terminal_id, |holder| {
        holder.selection_bounds().map(|((sc, sr), (ec, er))| SelectionBounds {
            start_col: sc as u16,
            start_row: sr,
            end_col: ec as u16,
            end_row: er,
        })
    })
    .flatten()
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
