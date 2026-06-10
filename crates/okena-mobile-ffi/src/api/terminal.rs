//! Plain data structs extracted from the terminal grid.
//!
//! `TerminalHolder` (`crate::client::terminal_holder`) produces these; the
//! uniffi-facing equivalents (`#[derive(uniffi::Record)]`) live in
//! `crate::types` and convert from these via `From`.

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
