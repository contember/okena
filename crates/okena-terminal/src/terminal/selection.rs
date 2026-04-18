use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;

use super::Terminal;

impl Terminal {
    /// Select all visible text in the terminal
    pub fn select_all(&self) {
        let mut term = self.term.lock();
        let grid = term.grid();
        let rows = grid.screen_lines() as i32;
        let cols = grid.columns();
        let history = grid.history_size() as i32;

        // Clear any existing selection
        term.selection = None;
        drop(term);

        // Create selection from start of history to end of screen
        // Start at top-left of history
        let start_row = -history;
        let start_col = 0;

        // End at bottom-right of visible area
        let end_row = rows - 1;
        let end_col = cols.saturating_sub(1);

        // Use the existing selection infrastructure
        self.start_selection(start_col, start_row, Side::Left);
        self.update_selection(end_col, end_row, Side::Right);
        self.end_selection();
    }

    /// Start selection at a point
    pub fn start_selection(&self, col: usize, row: i32, side: Side) {
        self.start_selection_with_type(col, row, SelectionType::Simple, side);
    }

    /// Start word (semantic) selection at a point
    pub fn start_word_selection(&self, col: usize, row: i32) {
        self.start_selection_with_type(col, row, SelectionType::Semantic, Side::Left);
    }

    /// Start line selection at a point
    pub fn start_line_selection(&self, col: usize, row: i32) {
        self.start_selection_with_type(col, row, SelectionType::Lines, Side::Left);
    }

    /// Start selection with a specific type
    /// Note: row is the visual row on screen (0 to screen_lines-1)
    /// We convert it to buffer coordinates by accounting for display_offset
    fn start_selection_with_type(&self, col: usize, row: i32, selection_type: SelectionType, side: Side) {
        let mut term = self.term.lock();

        // Convert visual row to buffer row
        // When scrolled up (display_offset > 0), visual row 0 shows history (negative buffer lines)
        let display_offset = term.grid().display_offset() as i32;
        let buffer_row = row - display_offset;

        let mut state = self.selection_state.lock();
        state.start = Some((col, buffer_row as usize));
        state.end = Some((col, buffer_row as usize));
        state.is_selecting = true;

        // Set selection in the terminal using buffer coordinates
        let point = Point::new(Line(buffer_row), Column(col));
        let selection = Selection::new(selection_type, point, side);
        term.selection = Some(selection);
    }

    /// Update selection to a new point
    /// Note: row is the visual row on screen (0 to screen_lines-1)
    /// We convert it to buffer coordinates by accounting for display_offset
    pub fn update_selection(&self, col: usize, row: i32, side: Side) {
        let mut state = self.selection_state.lock();
        if state.is_selecting {
            // Update terminal selection
            let mut term = self.term.lock();

            // Convert visual row to buffer row
            let display_offset = term.grid().display_offset() as i32;
            let buffer_row = row - display_offset;

            state.end = Some((col, buffer_row as usize));

            if let Some(ref mut selection) = term.selection {
                let point = Point::new(Line(buffer_row), Column(col));
                selection.update(point, side);
            }
        }
    }

    /// End selection
    pub fn end_selection(&self) {
        let mut state = self.selection_state.lock();
        state.is_selecting = false;
    }

    /// Clear selection
    pub fn clear_selection(&self) {
        let mut state = self.selection_state.lock();
        state.start = None;
        state.end = None;
        state.is_selecting = false;

        let mut term = self.term.lock();
        term.selection = None;
    }

    /// Get selected text
    pub fn get_selected_text(&self) -> Option<String> {
        let term = self.term.lock();
        term.selection_to_string()
    }

    /// Check if there is an active selection
    pub fn has_selection(&self) -> bool {
        let term = self.term.lock();
        term.selection.is_some()
    }

    /// Get selection bounds for rendering
    /// Uses alacritty's selection which properly handles semantic (word) and line selection
    /// Returns ((start_col, start_row), (end_col, end_row)) where rows are buffer coordinates (can be negative for history)
    pub fn selection_bounds(&self) -> Option<((usize, i32), (usize, i32))> {
        let term = self.term.lock();
        if let Some(ref selection) = term.selection {
            if let Some(range) = selection.to_range(&*term) {
                let start = (range.start.column.0, range.start.line.0);
                let end = (range.end.column.0, range.end.line.0);
                return Some((start, end));
            }
        }
        None
    }

    /// Delete the currently selected text by sending arrow keys + backspaces to the PTY.
    /// Only works for single-row selections on the cursor's row in a plain shell.
    /// Returns true if deletion was performed.
    pub fn delete_selection(&self) -> bool {
        let mut term = self.term.lock();

        let display_offset = term.grid().display_offset();
        if display_offset != 0 {
            return false;
        }

        let range = match term.selection.as_ref().and_then(|s| s.to_range(&*term)) {
            Some(r) => r,
            None => return false,
        };

        let cursor = term.grid().cursor.point;

        // Only single-row selections on the cursor's row
        if range.start.line != range.end.line || range.start.line != cursor.line {
            return false;
        }

        let sel_start = range.start.column.0;
        let sel_end = range.end.column.0; // inclusive
        let cursor_col = cursor.column.0;
        let app_cursor = term.mode().contains(TermMode::APP_CURSOR);

        // Count logical characters in selection (skip WIDE_CHAR_SPACER)
        let mut char_count = 0usize;
        for c in sel_start..=sel_end {
            let cell = &term.grid()[Point::new(cursor.line, Column(c))];
            if !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                char_count += 1;
            }
        }

        if char_count == 0 {
            return false;
        }

        // Move cursor to end of selection + 1 (position right after last selected char)
        let target_col = sel_end + 1;
        let mut arrow_count = 0usize;
        if target_col > cursor_col {
            for c in cursor_col..target_col {
                let cell = &term.grid()[Point::new(cursor.line, Column(c))];
                if !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    arrow_count += 1;
                }
            }
        } else if target_col < cursor_col {
            // Cursor is to the right — need left arrows (negative direction)
            for c in target_col..cursor_col {
                let cell = &term.grid()[Point::new(cursor.line, Column(c))];
                if !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    arrow_count += 1;
                }
            }
        }

        let right_seq: &[u8] = if app_cursor { b"\x1bOC" } else { b"\x1b[C" };
        let left_seq: &[u8] = if app_cursor { b"\x1bOD" } else { b"\x1b[D" };

        let mut buf = Vec::new();

        // Send arrow keys to move cursor to end of selection + 1
        if target_col > cursor_col {
            for _ in 0..arrow_count {
                buf.extend_from_slice(right_seq);
            }
        } else if target_col < cursor_col {
            for _ in 0..arrow_count {
                buf.extend_from_slice(left_seq);
            }
        }

        // Send backspaces for each character in the selection
        for _ in 0..char_count {
            buf.push(0x7f); // DEL / Backspace
        }

        // Clear selection
        term.selection = None;

        drop(term);
        self.send_bytes(&buf);
        true
    }
}
