use crate::api::terminal::{CellData, CursorShape, CursorState};

use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use okena_core::theme::ThemeColors;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// No-op event listener for mobile.
///
/// On mobile, the server's Term already handles PtyWrite responses (cursor reports, DA, etc.).
/// Forwarding them from mobile Term would cause duplicates.
struct NoopEventListener;

impl EventListener for NoopEventListener {
    fn send_event(&self, _event: TermEvent) {}
}

/// Wraps `alacritty_terminal::Term` for processing PTY output on mobile.
pub struct TerminalHolder {
    term: Mutex<Term<NoopEventListener>>,
    processor: Mutex<Processor>,
    dirty: AtomicBool,
    cols: Mutex<u16>,
    rows: Mutex<u16>,
}

impl TerminalHolder {
    pub fn new(cols: u16, rows: u16) -> Self {
        let config = TermConfig::default();
        let term_size = TermSize::new(cols as usize, rows as usize);
        let term = Term::new(config, &term_size, NoopEventListener);

        Self {
            term: Mutex::new(term),
            processor: Mutex::new(Processor::new()),
            dirty: AtomicBool::new(false),
            cols: Mutex::new(cols),
            rows: Mutex::new(rows),
        }
    }

    /// Feed raw PTY output data into the terminal emulator.
    pub fn process_output(&self, data: &[u8]) {
        let mut term = self.term.lock();
        let mut processor = self.processor.lock();
        processor.advance(&mut *term, data);
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Extract all visible cells from the terminal grid for FFI transfer.
    pub fn get_visible_cells(&self, theme_colors: &ThemeColors) -> Vec<CellData> {
        let term = self.term.lock();
        let grid = term.grid();
        let screen_lines = grid.screen_lines();
        let cols = grid.columns();
        let display_offset = grid.display_offset() as i32;

        let mut cells = Vec::with_capacity(screen_lines * cols);

        for row in 0..screen_lines {
            let buffer_line = row as i32 - display_offset;
            for col in 0..cols {
                let cell_point = alacritty_terminal::index::Point {
                    line: Line(buffer_line),
                    column: Column(col),
                };
                let cell = &grid[cell_point];

                // Wide char spacers are the second cell of a double-width character.
                // Push an empty cell to keep the cell count at cols*rows so the
                // Dart painter's index-to-position mapping stays correct.
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    cells.push(CellData {
                        character: String::new(),
                        fg: theme_colors.ansi_to_argb(&cell.fg),
                        bg: theme_colors.ansi_to_argb(&cell.bg),
                        flags: 0,
                    });
                    continue;
                }

                let mut fg = cell.fg;
                let mut bg = cell.bg;
                if cell.flags.contains(Flags::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg);
                }

                let fg_argb = theme_colors.ansi_to_argb(&fg);
                let bg_argb = theme_colors.ansi_to_argb(&bg);

                let mut flags: u8 = 0;
                if cell.flags.contains(Flags::BOLD) {
                    flags |= 1;
                }
                if cell.flags.contains(Flags::ITALIC) {
                    flags |= 2;
                }
                if cell.flags.contains(Flags::UNDERLINE) {
                    flags |= 4;
                }
                if cell.flags.contains(Flags::STRIKEOUT) {
                    flags |= 8;
                }
                if cell.flags.contains(Flags::INVERSE) {
                    flags |= 16;
                }
                if cell.flags.contains(Flags::DIM) {
                    flags |= 32;
                }

                // alacritty keeps the zero-width marks beside `cell.c`, so the
                // base char alone loses the accent of decomposed text.
                let mut character = String::from(cell.c);
                character.extend(cell.zerowidth().into_iter().flatten());

                cells.push(CellData {
                    character,
                    fg: fg_argb,
                    bg: bg_argb,
                    flags,
                });
            }
        }

        cells
    }

    /// Get the current cursor state.
    pub fn get_cursor(&self) -> CursorState {
        let term = self.term.lock();
        let cursor = term.grid().cursor.point;
        let display_offset = term.grid().display_offset() as i32;
        let cursor_shape = term.cursor_style().shape;
        let shape = match cursor_shape {
            alacritty_terminal::vte::ansi::CursorShape::Block
            | alacritty_terminal::vte::ansi::CursorShape::HollowBlock => CursorShape::Block,
            alacritty_terminal::vte::ansi::CursorShape::Underline => CursorShape::Underline,
            alacritty_terminal::vte::ansi::CursorShape::Beam => CursorShape::Beam,
            alacritty_terminal::vte::ansi::CursorShape::Hidden => CursorShape::Block,
        };
        // Hide cursor when scrolled into history (cursor would be off-screen)
        let cursor_visual_line = cursor.line.0 + display_offset;
        let screen_lines = term.grid().screen_lines() as i32;
        let visible = term
            .mode()
            .contains(alacritty_terminal::term::TermMode::SHOW_CURSOR)
            && !matches!(
                cursor_shape,
                alacritty_terminal::vte::ansi::CursorShape::Hidden
            )
            && cursor_visual_line >= 0
            && cursor_visual_line < screen_lines;

        CursorState {
            col: cursor.column.0 as u16,
            row: cursor_visual_line.max(0) as u16,
            shape,
            visible,
        }
    }

    /// Resize the terminal grid.
    pub fn resize(&self, cols: u16, rows: u16) {
        let mut term = self.term.lock();
        let size = TermSize::new(cols as usize, rows as usize);
        term.resize(size);
        *self.cols.lock() = cols;
        *self.rows.lock() = rows;
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Scroll the terminal display by delta lines (positive = up, negative = down).
    pub fn scroll(&self, delta: i32) {
        let mut term = self.term.lock();
        term.scroll_display(Scroll::Delta(delta));
        drop(term);
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Snap the display back to the live screen (display_offset = 0). Mirrors
    /// the desktop `Terminal::scroll_to_bottom`, which every input path calls.
    pub fn scroll_to_bottom(&self) {
        let mut term = self.term.lock();
        if term.grid().display_offset() == 0 {
            return;
        }
        term.scroll_display(Scroll::Bottom);
        drop(term);
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Wheel-event bytes for `count` notches at visual cell `col`/`row`, or
    /// `None` when the app has not requested mouse reporting (the caller then
    /// scrolls the local scrollback). Same split as the desktop
    /// `TerminalContent::handle_scroll`: the encoding (SGR vs legacy X10) follows
    /// the mode the mirrored app enabled.
    pub fn mouse_wheel_input(&self, up: bool, col: u16, row: u16, count: u32) -> Option<Vec<u8>> {
        let mode = *self.term.lock().mode();
        if !mode.intersects(TermMode::MOUSE_MODE) {
            return None;
        }
        Some(mouse_wheel_bytes(
            up,
            col,
            row,
            count,
            mode.contains(TermMode::SGR_MOUSE),
        ))
    }

    /// Get scroll information: (total_lines, visible_lines, display_offset).
    pub fn scroll_info(&self) -> (usize, usize, usize) {
        let term = self.term.lock();
        let grid = term.grid();
        let visible_lines = grid.screen_lines();
        let history_size = grid.history_size();
        let total_lines = visible_lines + history_size;
        let display_offset = grid.display_offset();
        (total_lines, visible_lines, display_offset)
    }

    /// Start a simple (character-level) selection at the given visual col/row.
    pub fn start_selection(&self, col: usize, row: usize) {
        let mut term = self.term.lock();
        let display_offset = term.grid().display_offset() as i32;
        let buffer_row = row as i32 - display_offset;
        let point = Point::new(Line(buffer_row), Column(col));
        let selection = Selection::new(SelectionType::Simple, point, Side::Left);
        term.selection = Some(selection);
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Start a word (semantic) selection at the given visual col/row.
    pub fn start_word_selection(&self, col: usize, row: usize) {
        let mut term = self.term.lock();
        let display_offset = term.grid().display_offset() as i32;
        let buffer_row = row as i32 - display_offset;
        let point = Point::new(Line(buffer_row), Column(col));
        let selection = Selection::new(SelectionType::Semantic, point, Side::Left);
        term.selection = Some(selection);
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Extend the current selection to the given visual col/row.
    pub fn update_selection(&self, col: usize, row: usize) {
        let mut term = self.term.lock();
        let display_offset = term.grid().display_offset() as i32;
        let buffer_row = row as i32 - display_offset;
        if let Some(ref mut selection) = term.selection {
            let point = Point::new(Line(buffer_row), Column(col));
            selection.update(point, Side::Right);
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    /// Clear the current selection.
    pub fn clear_selection(&self) {
        let mut term = self.term.lock();
        if term.selection.take().is_some() {
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    /// Get the selected text, if any.
    pub fn get_selected_text(&self) -> Option<String> {
        let term = self.term.lock();
        term.selection_to_string()
    }

    /// Get selection bounds as ((start_col, start_row), (end_col, end_row))
    /// where rows are buffer coordinates (adjusted for display_offset for rendering).
    pub fn selection_bounds(&self) -> Option<((usize, i32), (usize, i32))> {
        let term = self.term.lock();
        if let Some(ref selection) = term.selection
            && let Some(range) = selection.to_range(&*term)
        {
            let start = (range.start.column.0, range.start.line.0);
            let end = (range.end.column.0, range.end.line.0);
            return Some((start, end));
        }
        None
    }

    /// Check if the terminal has unprocessed changes.
    pub fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    /// Take the dirty flag (returns true if it was dirty, resets to false).
    ///
    /// The RN render loop consumes this once per frame (migration plan
    /// Decision C), so an idle terminal stops rebuilding its picture.
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }
}

/// Encode `count` wheel notches (button 64 = up, 65 = down) at 0-based cell
/// `col`/`row`. Byte-for-byte the desktop `Terminal::send_mouse_scroll`.
fn mouse_wheel_bytes(up: bool, col: u16, row: u16, count: u32, sgr: bool) -> Vec<u8> {
    let button: u8 = if up { 64 } else { 65 };
    let mut buf = Vec::new();
    for _ in 0..count {
        if sgr {
            buf.extend_from_slice(
                format!("\x1b[<{};{};{}M", button, u32::from(col) + 1, u32::from(row) + 1)
                    .as_bytes(),
            );
        } else {
            let clamp = |v: u16| u8::try_from(v).unwrap_or(u8::MAX).saturating_add(33);
            buf.extend_from_slice(&[
                0x1b,
                b'[',
                b'M',
                button.saturating_add(32),
                clamp(col),
                clamp(row),
            ]);
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_core::theme::DARK_THEME;

    #[test]
    fn process_simple_text() {
        let holder = TerminalHolder::new(80, 24);
        holder.process_output(b"Hello, world!");
        let cells = holder.get_visible_cells(&DARK_THEME);
        // Cells should contain H, e, l, l, o, etc. (minus WIDE_CHAR_SPACERs)
        let text: String = cells
            .iter()
            .take(13)
            .map(|c| c.character.as_str())
            .collect();
        assert_eq!(text, "Hello, world!");
    }

    #[test]
    fn combining_marks_stay_with_their_base_char() {
        let holder = TerminalHolder::new(80, 24);
        holder.process_output("e\u{0301}x".as_bytes());
        let cells = holder.get_visible_cells(&DARK_THEME);
        assert_eq!(cells[0].character, "e\u{0301}");
        assert_eq!(cells[1].character, "x");
    }

    #[test]
    fn a_mark_standing_over_a_blank_cell_survives() {
        let holder = TerminalHolder::new(80, 24);
        holder.process_output(" \u{0301}x".as_bytes());
        let cells = holder.get_visible_cells(&DARK_THEME);
        assert_eq!(cells[0].character, " \u{0301}");
        assert_eq!(cells[1].character, "x");
    }

    #[test]
    fn dirty_flag_lifecycle() {
        let holder = TerminalHolder::new(80, 24);
        assert!(!holder.is_dirty());

        holder.process_output(b"test");
        assert!(holder.is_dirty());

        assert!(holder.take_dirty());
        assert!(!holder.is_dirty());
    }

    #[test]
    fn selection_changes_mark_dirty() {
        let holder = TerminalHolder::new(80, 24);
        holder.start_selection(0, 0);
        assert!(holder.take_dirty());
        holder.update_selection(3, 0);
        assert!(holder.take_dirty());
        holder.clear_selection();
        assert!(holder.take_dirty());
        holder.clear_selection();
        assert!(!holder.take_dirty());
    }

    #[test]
    fn scroll_to_bottom_returns_to_live_screen() {
        let holder = TerminalHolder::new(10, 4);
        for i in 0..20 {
            holder.process_output(format!("line {i}\r\n").as_bytes());
        }
        holder.scroll(5);
        assert_eq!(holder.scroll_info().2, 5);
        holder.take_dirty();
        holder.scroll_to_bottom();
        assert_eq!(holder.scroll_info().2, 0);
        assert!(holder.take_dirty());
        holder.scroll_to_bottom();
        assert!(!holder.take_dirty());
    }

    #[test]
    fn wheel_input_is_none_without_mouse_reporting() {
        let holder = TerminalHolder::new(80, 24);
        assert_eq!(holder.mouse_wheel_input(true, 0, 0, 1), None);
    }

    #[test]
    fn wheel_input_uses_sgr_when_the_app_enabled_it() {
        let holder = TerminalHolder::new(80, 24);
        // 1000 = button reporting, 1006 = SGR encoding.
        holder.process_output(b"\x1b[?1000h\x1b[?1006h");
        assert_eq!(
            holder.mouse_wheel_input(true, 4, 2, 2),
            Some(b"\x1b[<64;5;3M\x1b[<64;5;3M".to_vec())
        );
        assert_eq!(
            holder.mouse_wheel_input(false, 0, 0, 1),
            Some(b"\x1b[<65;1;1M".to_vec())
        );
    }

    #[test]
    fn wheel_input_falls_back_to_legacy_encoding() {
        let holder = TerminalHolder::new(80, 24);
        holder.process_output(b"\x1b[?1000h");
        assert_eq!(
            holder.mouse_wheel_input(false, 4, 2, 1),
            Some(vec![0x1b, b'[', b'M', 65 + 32, 4 + 33, 2 + 33])
        );
    }

    #[test]
    fn resize_changes_grid() {
        let holder = TerminalHolder::new(80, 24);
        let cells_before = holder.get_visible_cells(&DARK_THEME);
        // 80 cols * 24 rows = 1920 cells (no wide chars to skip)
        assert_eq!(cells_before.len(), 80 * 24);

        holder.resize(120, 40);
        let cells_after = holder.get_visible_cells(&DARK_THEME);
        assert_eq!(cells_after.len(), 120 * 40);
    }

    #[test]
    fn cursor_position_after_output() {
        let holder = TerminalHolder::new(80, 24);
        holder.process_output(b"ABCDE");
        let cursor = holder.get_cursor();
        assert_eq!(cursor.col, 5);
        assert_eq!(cursor.row, 0);
        assert!(cursor.visible);
    }

    #[test]
    fn inverse_flag_swaps_colors() {
        let holder = TerminalHolder::new(80, 24);
        // SGR 7 = inverse, then "X", then SGR 0 = reset
        holder.process_output(b"\x1b[7mX\x1b[0m");
        let cells = holder.get_visible_cells(&DARK_THEME);
        let normal_cell = &cells[1]; // second cell is normal (space after reset)
        let inverse_cell = &cells[0]; // first cell has INVERSE
        // INVERSE flag should be set
        assert!(inverse_cell.flags & 16 != 0);
        // fg and bg should be swapped compared to a normal cell
        assert_eq!(inverse_cell.fg, normal_cell.bg);
        assert_eq!(inverse_cell.bg, normal_cell.fg);
    }
}
