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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};

/// Source of render generations. Global, so a holder that replaces another
/// under the same id never repeats a value a view has already seen.
static GENERATION_SOURCE: AtomicU64 = AtomicU64::new(1);

/// Set in a holder's generation once the holder is dropped, so a native view
/// polling a cached counter knows to look the terminal up again.
pub const GENERATION_DEAD: u64 = 1 << 63;

/// Upper bound on bytes fed to the parser per step while counting lines that
/// scroll into history. The step is also kept below the scrollback limit:
/// each scrolled line needs at least one byte, so the parked display offset
/// (capped at the limit by alacritty) can never saturate within one step.
const MAX_OUTPUT_STEP: usize = 4096;

/// No-op event listener for mobile.
///
/// On mobile, the server's Term already handles PtyWrite responses (cursor reports, DA, etc.).
/// Forwarding them from mobile Term would cause duplicates.
pub(crate) struct NoopEventListener;

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
    /// Render generation for the native views (`crate::render`), bumped with
    /// every change a view could draw. Shared so a view polls it lock-free.
    generation: Arc<AtomicU64>,
    /// Render line bookkeeping; only written while `term` is locked.
    track: RenderTrack,
    /// Bytes per parser step, see [`MAX_OUTPUT_STEP`].
    output_step: usize,
}

/// Bookkeeping behind the native views' absolute line numbers.
///
/// alacritty addresses lines relative to the live screen (`Line(0)` is the
/// top screen row, history is negative), so a line's address changes every
/// time output scrolls. The views need addresses that stay put, so we count
/// every line that ever scrolled from the primary screen into history:
/// `screen_top = lines_scrolled_off`, and a line's absolute number is
/// `screen_top + alacritty_line`. Old lines dropped by the scrollback limit
/// simply fall below the oldest available number.
#[derive(Default)]
pub(crate) struct RenderTrack {
    /// Absolute number of the primary screen's top row.
    lines_scrolled_off: AtomicI64,
    /// Bumped on resize: reflow rewraps lines, so views must re-anchor.
    reflow_epoch: AtomicU32,
    /// History size of the primary grid when it was last active.
    last_primary_history: AtomicU64,
    /// While the primary grid is parked (see `process_output`) but inactive
    /// because the alt screen is up: the old-API display offset to restore,
    /// plus one so 0 means "not parked".
    parked_saved_offset: AtomicU64,
}

/// A consistent read of the render bookkeeping, taken under the term lock.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RenderTrackSnapshot {
    pub screen_top: i64,
    pub reflow_epoch: u32,
}

impl Drop for TerminalHolder {
    fn drop(&mut self) {
        self.generation.fetch_or(GENERATION_DEAD, Ordering::Release);
    }
}

impl TerminalHolder {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self::with_config(cols, rows, TermConfig::default())
    }

    #[cfg(test)]
    pub(crate) fn with_history(cols: u16, rows: u16, scrolling_history: usize) -> Self {
        Self::with_config(
            cols,
            rows,
            TermConfig {
                scrolling_history,
                ..TermConfig::default()
            },
        )
    }

    fn with_config(cols: u16, rows: u16, config: TermConfig) -> Self {
        let term_size = TermSize::new(cols as usize, rows as usize);
        let output_step = config
            .scrolling_history
            .saturating_sub(1)
            .clamp(1, MAX_OUTPUT_STEP);
        let term = Term::new(config, &term_size, NoopEventListener);

        Self {
            term: Mutex::new(term),
            processor: Mutex::new(Processor::new()),
            dirty: AtomicBool::new(false),
            cols: Mutex::new(cols),
            rows: Mutex::new(rows),
            generation: Arc::new(AtomicU64::new(next_generation())),
            track: RenderTrack::default(),
            output_step,
        }
    }

    /// Mark a visible change: old-API dirty flag plus render generation.
    fn changed(&self) {
        self.dirty.store(true, Ordering::Relaxed);
        self.generation.store(next_generation(), Ordering::Release);
    }

    /// Feed raw PTY output data into the terminal emulator.
    ///
    /// Also counts the lines that scroll from the primary screen into history
    /// (see [`RenderTrack`]). alacritty exposes no such counter, but while the
    /// display offset is non-zero its `Grid::scroll_up` raises the offset by
    /// every line that scrolls (alacritty's own `clear_viewport` reads the
    /// count the same way). So around each parser step we "park" the offset at
    /// 1, read how far it moved, and put back the offset the old uniffi API
    /// expects (its scrolled-up view stays anchored exactly as before). With
    /// an empty history the offset cannot be parked; history growth is then
    /// the exact count, since nothing has been dropped yet.
    pub fn process_output(&self, data: &[u8]) {
        let mut term = self.term.lock();
        let mut processor = self.processor.lock();
        for step in data.chunks(self.output_step) {
            self.advance_counting(&mut term, &mut processor, step);
        }
        drop(processor);
        drop(term);
        self.changed();
    }

    fn advance_counting(
        &self,
        term: &mut Term<NoopEventListener>,
        processor: &mut Processor,
        bytes: &[u8],
    ) {
        let track = &self.track;
        if !term.mode().contains(TermMode::ALT_SCREEN) {
            let grid = term.grid_mut();
            track
                .last_primary_history
                .store(grid.history_size() as u64, Ordering::Relaxed);
            if track.parked_saved_offset.load(Ordering::Relaxed) == 0 && grid.history_size() > 0 {
                let saved = grid.display_offset();
                track
                    .parked_saved_offset
                    .store(saved as u64 + 1, Ordering::Relaxed);
                set_display_offset(grid, 1);
            }
        }

        processor.advance(term, bytes);

        if term.mode().contains(TermMode::ALT_SCREEN) {
            // The primary grid (parked or not) is inactive; settle it once it
            // is back.
            return;
        }
        let grid = term.grid_mut();
        let history = grid.history_size();
        let parked = track.parked_saved_offset.swap(0, Ordering::Relaxed);
        let scrolled = if parked != 0 {
            let offset = grid.display_offset();
            let scrolled = offset.saturating_sub(1);
            let saved = (parked - 1) as usize;
            // The old API's view: live (0) stays live, a scrolled-up view
            // moves with its content, as alacritty does on its own.
            let restore = if saved == 0 {
                0
            } else {
                (saved + scrolled).min(history)
            };
            set_display_offset(grid, restore);
            scrolled
        } else {
            let before = track.last_primary_history.load(Ordering::Relaxed) as usize;
            history.saturating_sub(before)
        };
        track
            .lines_scrolled_off
            .fetch_add(scrolled as i64, Ordering::Relaxed);
        track
            .last_primary_history
            .store(history as u64, Ordering::Relaxed);
    }

    /// Lock-free generation counter shared with native views.
    pub(crate) fn generation_counter(&self) -> Arc<AtomicU64> {
        self.generation.clone()
    }

    /// Run `f` with the locked terminal and a consistent render-track read.
    pub(crate) fn with_term<R>(
        &self,
        f: impl FnOnce(&Term<NoopEventListener>, RenderTrackSnapshot) -> R,
    ) -> R {
        let term = self.term.lock();
        let snap = RenderTrackSnapshot {
            screen_top: self.track.lines_scrolled_off.load(Ordering::Relaxed),
            reflow_epoch: self.track.reflow_epoch.load(Ordering::Relaxed),
        };
        f(&term, snap)
    }

    /// Run `f` with the locked terminal for mutation (selection). Marks the
    /// terminal changed when `f` returns true.
    pub(crate) fn with_term_mut(
        &self,
        f: impl FnOnce(&mut Term<NoopEventListener>, RenderTrackSnapshot) -> bool,
    ) {
        let mut term = self.term.lock();
        let snap = RenderTrackSnapshot {
            screen_top: self.track.lines_scrolled_off.load(Ordering::Relaxed),
            reflow_epoch: self.track.reflow_epoch.load(Ordering::Relaxed),
        };
        let changed = f(&mut term, snap);
        drop(term);
        if changed {
            self.changed();
        }
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
    ///
    /// Reflow moves lines between the screen and history. On the primary
    /// screen the screen top's absolute number follows the history size, so
    /// the oldest line keeps its number; the reflow epoch tells native views
    /// that other lines may have moved.
    pub fn resize(&self, cols: u16, rows: u16) {
        let mut term = self.term.lock();
        let alt = term.mode().contains(TermMode::ALT_SCREEN);
        let before = term.grid().history_size() as i64;
        let size = TermSize::new(cols as usize, rows as usize);
        term.resize(size);
        if !alt {
            let after = term.grid().history_size();
            self.track
                .lines_scrolled_off
                .fetch_add(after as i64 - before, Ordering::Relaxed);
            self.track
                .last_primary_history
                .store(after as u64, Ordering::Relaxed);
        }
        self.track.reflow_epoch.fetch_add(1, Ordering::Relaxed);
        drop(term);
        *self.cols.lock() = cols;
        *self.rows.lock() = rows;
        self.changed();
    }

    /// Scroll the terminal display by delta lines (positive = up, negative = down).
    pub fn scroll(&self, delta: i32) {
        let mut term = self.term.lock();
        term.scroll_display(Scroll::Delta(delta));
        drop(term);
        self.changed();
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
        self.changed();
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
        drop(term);
        self.changed();
    }

    /// Start a word (semantic) selection at the given visual col/row.
    pub fn start_word_selection(&self, col: usize, row: usize) {
        let mut term = self.term.lock();
        let display_offset = term.grid().display_offset() as i32;
        let buffer_row = row as i32 - display_offset;
        let point = Point::new(Line(buffer_row), Column(col));
        let selection = Selection::new(SelectionType::Semantic, point, Side::Left);
        term.selection = Some(selection);
        drop(term);
        self.changed();
    }

    /// Extend the current selection to the given visual col/row.
    pub fn update_selection(&self, col: usize, row: usize) {
        let mut term = self.term.lock();
        let display_offset = term.grid().display_offset() as i32;
        let buffer_row = row as i32 - display_offset;
        if let Some(ref mut selection) = term.selection {
            let point = Point::new(Line(buffer_row), Column(col));
            selection.update(point, Side::Right);
            drop(term);
            self.changed();
        }
    }

    /// Clear the current selection.
    pub fn clear_selection(&self) {
        let mut term = self.term.lock();
        if term.selection.take().is_some() {
            drop(term);
            self.changed();
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

fn next_generation() -> u64 {
    GENERATION_SOURCE.fetch_add(1, Ordering::Relaxed) & !GENERATION_DEAD
}

/// Set alacritty's display offset without `Term::scroll_display` side effects
/// (damage, events).
fn set_display_offset(
    grid: &mut alacritty_terminal::grid::Grid<alacritty_terminal::term::cell::Cell>,
    offset: usize,
) {
    let delta = offset as i64 - grid.display_offset() as i64;
    if delta != 0 {
        grid.scroll_display(Scroll::Delta(delta as i32));
    }
}

/// Encode `count` wheel notches (button 64 = up, 65 = down) at 0-based cell
/// `col`/`row`. Byte-for-byte the desktop `Terminal::send_mouse_scroll`.
pub(crate) fn mouse_wheel_bytes(up: bool, col: u16, row: u16, count: u32, sgr: bool) -> Vec<u8> {
    let button: u8 = if up { 64 } else { 65 };
    let mut buf = Vec::new();
    for _ in 0..count {
        if sgr {
            buf.extend_from_slice(
                format!(
                    "\x1b[<{};{};{}M",
                    button,
                    u32::from(col) + 1,
                    u32::from(row) + 1
                )
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
