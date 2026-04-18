use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Point};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;

use super::Terminal;

impl Terminal {
    /// True if the active app wants drag/motion events (DEC modes 1002/1003).
    /// Press/release alone (1000) is implied by `is_mouse_mode()`.
    pub fn supports_mouse_drag(&self) -> bool {
        if self.transport.uses_mouse_backend() {
            return true;
        }
        let term = self.term.lock();
        term.mode().intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION)
    }

    /// Forward a mouse button press or release to the PTY.
    /// `button` is 0=left, 1=middle, 2=right. `modifiers` is the OR of 4 (shift),
    /// 8 (alt/meta), 16 (control). Coordinates are 0-based cells.
    pub fn send_mouse_button(
        &self,
        button: u8,
        pressed: bool,
        col: usize,
        row: usize,
        modifiers: u8,
    ) {
        let use_sgr = if self.transport.uses_mouse_backend() {
            true
        } else {
            let term = self.term.lock();
            term.mode().contains(TermMode::SGR_MOUSE)
        };

        let cb = (button & 0b11) | (modifiers & 0b1_1100);
        let buf: Vec<u8> = if use_sgr {
            let action = if pressed { 'M' } else { 'm' };
            format!("\x1b[<{};{};{}{}", cb, col + 1, row + 1, action).into_bytes()
        } else {
            // Legacy X10/normal format: release reports button=3, no SGR distinction.
            let legacy_cb = if pressed { cb } else { 3 | (modifiers & 0b1_1100) };
            vec![
                0x1b,
                b'[',
                b'M',
                legacy_cb.saturating_add(32),
                (col as u8).saturating_add(33).min(255),
                (row as u8).saturating_add(33).min(255),
            ]
        };
        self.send_bytes(&buf);
    }

    /// Forward a drag (button-held motion) event to the PTY.
    /// Caller should gate on `supports_mouse_drag()`.
    pub fn send_mouse_drag(&self, button: u8, col: usize, row: usize, modifiers: u8) {
        let use_sgr = if self.transport.uses_mouse_backend() {
            true
        } else {
            let term = self.term.lock();
            term.mode().contains(TermMode::SGR_MOUSE)
        };

        // Motion bit = 32 (0x20) added to button code.
        let cb = (button & 0b11) | (modifiers & 0b1_1100) | 32;
        let buf: Vec<u8> = if use_sgr {
            format!("\x1b[<{};{};{}M", cb, col + 1, row + 1).into_bytes()
        } else {
            vec![
                0x1b,
                b'[',
                b'M',
                cb.saturating_add(32),
                (col as u8).saturating_add(33).min(255),
                (row as u8).saturating_add(33).min(255),
            ]
        };
        self.send_bytes(&buf);
    }

    /// Send scroll events to PTY as a single batched write.
    /// button: 64 for scroll up, 65 for scroll down
    pub fn send_mouse_scroll(&self, button: u8, col: usize, row: usize, count: usize) {
        if count == 0 {
            return;
        }

        let use_sgr = if self.transport.uses_mouse_backend() {
            true
        } else {
            let term = self.term.lock();
            term.mode().contains(TermMode::SGR_MOUSE)
        };

        let mut buf = Vec::new();
        for _ in 0..count {
            if use_sgr {
                // SGR format: \x1b[<button;col;rowM
                buf.extend_from_slice(
                    format!("\x1b[<{};{};{}M", button, col + 1, row + 1).as_bytes(),
                );
            } else {
                // Legacy format: \x1b[M + (button+32) + (col+33) + (row+33)
                buf.extend_from_slice(&[
                    0x1b,
                    b'[',
                    b'M',
                    button.saturating_add(32),
                    (col as u8).saturating_add(33).min(255),
                    (row as u8).saturating_add(33).min(255),
                ]);
            }
        }
        self.send_bytes(&buf);
    }

    /// Move cursor to clicked column by sending arrow key sequences to the PTY.
    /// `target_col` is the visual column, `visual_row` is the visual row from pixel_to_cell().
    /// Only moves if the click is on the cursor's row and not scrolled into history.
    pub fn move_cursor_to_click(&self, target_col: usize, visual_row: i32) {
        let term = self.term.lock();

        let display_offset = term.grid().display_offset();
        if display_offset != 0 {
            return;
        }

        let cursor = term.grid().cursor.point;
        let cursor_visual_row = cursor.line.0; // equals buffer line when display_offset == 0
        if visual_row != cursor_visual_row {
            return;
        }

        let cursor_col = cursor.column.0;
        let cols = term.grid().columns();
        let target_col = target_col.min(cols.saturating_sub(1));

        if cursor_col == target_col {
            return;
        }

        let app_cursor = term.mode().contains(TermMode::APP_CURSOR);

        // Count logical characters (arrow presses) between cursor and target,
        // skipping WIDE_CHAR_SPACER cells (second cell of wide chars).
        let (start, end, going_right) = if target_col > cursor_col {
            (cursor_col, target_col, true)
        } else {
            (target_col, cursor_col, false)
        };

        let mut arrow_count = 0usize;
        for c in start..end {
            let cell = &term.grid()[Point::new(cursor.line, Column(c))];
            if !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                arrow_count += 1;
            }
        }

        if arrow_count == 0 {
            return;
        }

        let arrow_seq: &[u8] = if going_right {
            if app_cursor { b"\x1bOC" } else { b"\x1b[C" }
        } else {
            if app_cursor { b"\x1bOD" } else { b"\x1b[D" }
        };

        let mut buf = Vec::with_capacity(arrow_seq.len() * arrow_count);
        for _ in 0..arrow_count {
            buf.extend_from_slice(arrow_seq);
        }

        drop(term);
        self.send_bytes(&buf);
    }
}
