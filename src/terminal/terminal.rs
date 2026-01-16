use crate::terminal::pty_manager::PtyManager;
use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::index::{Point, Line, Column, Side};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::grid::{Scroll, Dimensions};
use async_channel::{Sender, Receiver, unbounded};
use parking_lot::Mutex;
use regex::Regex;
use std::sync::Arc;

/// Terminal size in cells and pixels
#[derive(Clone, Copy, Debug)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            cell_width: 8.0,
            cell_height: 16.0,
        }
    }
}


/// Event listener for alacritty_terminal that captures title changes and bell
pub struct ZedEventListener {
    /// Shared title storage - OSC 0/1/2 sequences update this
    title: Arc<Mutex<Option<String>>>,
    /// Bell notification flag
    has_bell: Arc<Mutex<bool>>,
}

impl ZedEventListener {
    pub fn new(title: Arc<Mutex<Option<String>>>, has_bell: Arc<Mutex<bool>>) -> Self {
        Self { title, has_bell }
    }
}

impl EventListener for ZedEventListener {
    fn send_event(&self, event: TermEvent) {
        match event {
            TermEvent::Title(title) => {
                *self.title.lock() = Some(title);
            }
            TermEvent::Bell => {
                *self.has_bell.lock() = true;
            }
            _ => {
                // Ignore other events - we handle them through our own channel
            }
        }
    }
}

/// Selection state for the terminal
#[derive(Clone, Debug)]
pub struct SelectionState {
    pub start: Option<(usize, usize)>,
    pub end: Option<(usize, usize)>,
    pub is_selecting: bool,
}

impl Default for SelectionState {
    fn default() -> Self {
        Self {
            start: None,
            end: None,
            is_selecting: false,
        }
    }
}

/// A terminal instance wrapping alacritty_terminal
pub struct Terminal {
    term: Arc<Mutex<Term<ZedEventListener>>>,
    processor: Mutex<Processor>,
    pub terminal_id: String,
    pub size: Mutex<TerminalSize>,
    pty_manager: Arc<PtyManager>,
    selection_state: Mutex<SelectionState>,
    scroll_offset: Mutex<i32>,
    /// Terminal title from OSC sequences
    title: Arc<Mutex<Option<String>>>,
    /// Bell notification flag (set when terminal receives bell, cleared on focus)
    has_bell: Arc<Mutex<bool>>,
    /// Dirty flag - set when terminal content changes, cleared after render
    dirty: std::sync::atomic::AtomicBool,
    /// Last PTY resize timestamp for debouncing (only PTY resize is debounced, grid resize is immediate)
    last_pty_resize: Mutex<std::time::Instant>,
    /// Pending PTY resize (stored when debounced, applied on next resize or after timeout)
    pending_pty_resize: Mutex<Option<(u16, u16)>>,
    /// Channel for notifying subscribers when terminal content changes (event-driven, no polling)
    dirty_notify: Sender<()>,
    /// Receiver for dirty notifications - subscribers can clone this to listen for changes
    dirty_receiver: Receiver<()>,
}

impl Terminal {
    /// Create a new terminal
    pub fn new(
        terminal_id: String,
        size: TerminalSize,
        pty_manager: Arc<PtyManager>,
    ) -> Self {
        let config = TermConfig::default();
        let term_size = TermSize::new(size.cols as usize, size.rows as usize);

        // Create shared storage for OSC sequence handling and bell
        let title = Arc::new(Mutex::new(None));
        let has_bell = Arc::new(Mutex::new(false));
        let event_listener = ZedEventListener::new(title.clone(), has_bell.clone());
        let term = Term::new(config, &term_size, event_listener);

        // Create unbounded channel for dirty notifications (don't drop any updates)
        let (dirty_notify, dirty_receiver) = unbounded();

        Self {
            term: Arc::new(Mutex::new(term)),
            processor: Mutex::new(Processor::new()),
            terminal_id,
            size: Mutex::new(size),
            pty_manager,
            selection_state: Mutex::new(SelectionState::default()),
            scroll_offset: Mutex::new(0),
            title,
            has_bell,
            dirty: std::sync::atomic::AtomicBool::new(false),
            last_pty_resize: Mutex::new(std::time::Instant::now()),
            pending_pty_resize: Mutex::new(None),
            dirty_notify,
            dirty_receiver,
        }
    }

    /// Process output from PTY
    pub fn process_output(&self, data: &[u8]) {
        let mut term = self.term.lock();
        let mut processor = self.processor.lock();

        processor.advance(&mut *term, data);
        self.dirty.store(true, std::sync::atomic::Ordering::Relaxed);

        // Notify subscribers that content changed (non-blocking, coalesces rapid updates)
        let _ = self.dirty_notify.try_send(());
    }

    /// Check if terminal has pending changes (and clear the flag)
    /// Note: Kept for potential external use, main path uses subscribe_dirty()
    #[allow(dead_code)]
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, std::sync::atomic::Ordering::Relaxed)
    }

    /// Get a receiver for dirty notifications (for event-driven updates instead of polling)
    pub fn subscribe_dirty(&self) -> Receiver<()> {
        self.dirty_receiver.clone()
    }

    /// Send input to the PTY
    pub fn send_input(&self, input: &str) {
        self.pty_manager.send_input(&self.terminal_id, input.as_bytes());
    }

    /// Send raw bytes to the PTY
    pub fn send_bytes(&self, data: &[u8]) {
        self.pty_manager.send_input(&self.terminal_id, data);
    }

    /// Resize the terminal with debounced PTY resize
    ///
    /// The terminal grid is always resized immediately for correct rendering.
    /// PTY resize signals are debounced to avoid flooding the shell during
    /// rapid resize operations (e.g., dragging a split divider).
    ///
    /// Debounce interval: 16ms (~60fps) - enough to batch rapid resize events
    /// while still feeling responsive.
    pub fn resize(&self, new_size: TerminalSize) {
        const DEBOUNCE_MS: u64 = 16;

        // Always update local size and terminal grid immediately
        *self.size.lock() = new_size;
        let mut term = self.term.lock();
        let term_size = TermSize::new(new_size.cols as usize, new_size.rows as usize);
        term.resize(term_size);
        drop(term);

        // Debounce PTY resize to avoid excessive SIGWINCH signals
        let now = std::time::Instant::now();
        let mut last_resize = self.last_pty_resize.lock();
        let elapsed = now.duration_since(*last_resize);

        if elapsed.as_millis() >= DEBOUNCE_MS as u128 {
            // Enough time has passed - send resize immediately
            // Also flush any pending resize
            *self.pending_pty_resize.lock() = None;
            *last_resize = now;
            self.pty_manager.resize(&self.terminal_id, new_size.cols, new_size.rows);
        } else {
            // Store pending resize - will be applied on next resize that passes debounce
            *self.pending_pty_resize.lock() = Some((new_size.cols, new_size.rows));
        }
    }

    /// Flush any pending PTY resize (call this when resize operations complete)
    pub fn flush_pending_resize(&self) {
        if let Some((cols, rows)) = self.pending_pty_resize.lock().take() {
            self.pty_manager.resize(&self.terminal_id, cols, rows);
            *self.last_pty_resize.lock() = std::time::Instant::now();
        }
    }

    /// Access the terminal content for rendering
    pub fn with_content<R>(&self, f: impl FnOnce(&Term<ZedEventListener>) -> R) -> R {
        let term = self.term.lock();
        f(&*term)
    }

    /// Scroll the terminal
    pub fn scroll(&self, delta: i32) {
        let mut term = self.term.lock();
        let scroll = if delta > 0 {
            Scroll::Delta(delta)
        } else {
            Scroll::Delta(delta)
        };
        term.scroll_display(scroll);
        *self.scroll_offset.lock() += delta;
    }

    /// Scroll up by lines
    pub fn scroll_up(&self, lines: i32) {
        self.scroll(lines);
    }

    /// Scroll down by lines
    pub fn scroll_down(&self, lines: i32) {
        self.scroll(-lines);
    }

    /// Start selection at a point
    pub fn start_selection(&self, col: usize, row: i32) {
        self.start_selection_with_type(col, row, SelectionType::Simple);
    }

    /// Start word (semantic) selection at a point
    pub fn start_word_selection(&self, col: usize, row: i32) {
        self.start_selection_with_type(col, row, SelectionType::Semantic);
    }

    /// Start line selection at a point
    pub fn start_line_selection(&self, col: usize, row: i32) {
        self.start_selection_with_type(col, row, SelectionType::Lines);
    }

    /// Start selection with a specific type
    fn start_selection_with_type(&self, col: usize, row: i32, selection_type: SelectionType) {
        let mut state = self.selection_state.lock();
        state.start = Some((col, row as usize));
        state.end = Some((col, row as usize));
        state.is_selecting = true;

        // Also set selection in the terminal
        let mut term = self.term.lock();
        let point = Point::new(Line(row), Column(col));
        let selection = Selection::new(selection_type, point, Side::Left);
        term.selection = Some(selection);
    }

    /// Update selection to a new point
    pub fn update_selection(&self, col: usize, row: i32) {
        let mut state = self.selection_state.lock();
        if state.is_selecting {
            state.end = Some((col, row as usize));

            // Update terminal selection
            let mut term = self.term.lock();
            if let Some(ref mut selection) = term.selection {
                let point = Point::new(Line(row), Column(col));
                selection.update(point, Side::Right);
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
    pub fn selection_bounds(&self) -> Option<((usize, usize), (usize, usize))> {
        let term = self.term.lock();
        if let Some(ref selection) = term.selection {
            if let Some(range) = selection.to_range(&*term) {
                let start = (range.start.column.0, range.start.line.0 as usize);
                let end = (range.end.column.0, range.end.line.0 as usize);
                return Some((start, end));
            }
        }
        None
    }

    /// Get cell dimensions (width, height) for coordinate conversion
    pub fn cell_dimensions(&self) -> (f32, f32) {
        let size = self.size.lock();
        (size.cell_width, size.cell_height)
    }

    /// Get the terminal title (from OSC sequences)
    pub fn title(&self) -> Option<String> {
        self.title.lock().clone()
    }

    /// Check if terminal has unread bell notification
    pub fn has_bell(&self) -> bool {
        *self.has_bell.lock()
    }

    /// Clear the bell notification flag (call when terminal receives focus)
    pub fn clear_bell(&self) {
        *self.has_bell.lock() = false;
    }

    /// Search the terminal grid for occurrences of a query string
    /// Returns a list of (line, col, length) for each match
    /// Supports case-sensitive and regex search, and searches through scrollback buffer
    pub fn search_grid(&self, query: &str, case_sensitive: bool, is_regex: bool) -> Vec<(i32, usize, usize)> {
        if query.is_empty() {
            return Vec::new();
        }

        // Build regex pattern if needed
        let regex = if is_regex {
            let pattern = if case_sensitive {
                query.to_string()
            } else {
                format!("(?i){}", query)
            };
            match Regex::new(&pattern) {
                Ok(r) => Some(r),
                Err(_) => return Vec::new(), // Invalid regex, return no matches
            }
        } else {
            None
        };

        let mut matches = Vec::new();

        self.with_content(|term| {
            let grid = term.grid();
            let screen_lines = grid.screen_lines() as i32;
            let history_size = grid.history_size() as i32;
            let cols = grid.columns();
            let display_offset = grid.display_offset() as i32;

            // Search from top of history to bottom of screen
            // Line numbers: negative = history, 0..screen_lines = visible
            // We iterate from -(history_size) to (screen_lines - 1)
            for row in (-history_size)..screen_lines {
                // Calculate the actual line index for grid access
                // The grid uses Line() which handles the offset automatically
                let line = row;

                // Build the line text
                let mut line_text = String::with_capacity(cols);
                for col in 0..cols {
                    let cell_point = Point::new(Line(line), Column(col));
                    let cell = &grid[cell_point];
                    line_text.push(cell.c);
                }

                if let Some(ref regex) = regex {
                    // Regex search
                    for mat in regex.find_iter(&line_text) {
                        // Convert line to display-relative coordinate
                        let display_line = line + display_offset;
                        matches.push((display_line, mat.start(), mat.len()));
                    }
                } else {
                    // Plain text search
                    let (search_text, query_text) = if case_sensitive {
                        (line_text.clone(), query.to_string())
                    } else {
                        (line_text.to_lowercase(), query.to_lowercase())
                    };

                    let mut search_start = 0;
                    while let Some(pos) = search_text[search_start..].find(&query_text) {
                        let col = search_start + pos;
                        // Convert line to display-relative coordinate
                        let display_line = line + display_offset;
                        matches.push((display_line, col, query.len()));
                        search_start = col + 1;
                        if search_start >= search_text.len() {
                            break;
                        }
                    }
                }
            }
        });

        matches
    }

    /// Get the number of screen lines
    pub fn screen_lines(&self) -> usize {
        self.with_content(|term| term.grid().screen_lines())
    }

    /// Get scroll information for scrollbar rendering
    /// Returns (total_lines, visible_lines, scroll_offset)
    /// total_lines: Total number of lines in history + screen
    /// visible_lines: Number of lines visible on screen
    /// scroll_offset: Current scroll position (0 = bottom, positive = scrolled up)
    pub fn scroll_info(&self) -> (usize, usize, i32) {
        let term = self.term.lock();
        let grid = term.grid();
        let visible_lines = grid.screen_lines();
        let history_size = grid.history_size();
        let total_lines = visible_lines + history_size;
        let display_offset = grid.display_offset();
        (total_lines, visible_lines, display_offset as i32)
    }

    /// Get the current display offset (how many lines scrolled from bottom)
    pub fn display_offset(&self) -> usize {
        let term = self.term.lock();
        term.grid().display_offset()
    }

    /// Detect URLs in the visible terminal content
    /// Returns a list of (line, col, length, url_string) for each detected URL
    /// Handles URLs that wrap across multiple lines by creating multiple match entries
    pub fn detect_urls(&self) -> Vec<(i32, usize, usize, String)> {
        // URL regex pattern - matches http:// and https:// URLs
        let url_regex = match Regex::new(r#"https?://[^\s<>"'`{}\[\]|\\^)]+"#) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        // Characters that can appear in a URL (for continuation detection)
        let url_char = |c: char| -> bool {
            c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~' | ':' | '/' | '?' | '#' | '[' | ']' | '@' | '!' | '$' | '&' | '\'' | '(' | ')' | '*' | '+' | ',' | ';' | '=' | '%')
        };

        let mut matches = Vec::new();

        self.with_content(|term| {
            let grid = term.grid();
            let screen_lines = grid.screen_lines() as i32;
            let cols = grid.columns();
            let last_col = Column(cols - 1);

            // Build logical lines by joining wrapped physical lines
            // A line is considered wrapped if:
            // 1. It has WRAPLINE flag, OR
            // 2. It ends with a URL-valid character (no trailing space) and next line starts with URL-valid char
            let mut row = 0i32;
            while row < screen_lines {
                let mut combined_text = String::new();
                // Track where each physical row starts in the combined string
                let mut row_offsets: Vec<(i32, usize)> = Vec::new();

                // Collect all wrapped lines into one logical line
                loop {
                    row_offsets.push((row, combined_text.len()));

                    // Build this row's text (trim trailing spaces for cleaner matching)
                    let mut row_text = String::with_capacity(cols);
                    for col in 0..cols {
                        let cell_point = Point::new(Line(row), Column(col));
                        let cell = &grid[cell_point];
                        row_text.push(cell.c);
                    }
                    combined_text.push_str(&row_text);

                    // Check if this row wraps to the next
                    let last_cell = &grid[Point::new(Line(row), last_col)];
                    let has_wrapline_flag = last_cell.flags.contains(Flags::WRAPLINE);

                    // Also check for visual wrapping: last char is URL-valid and next row starts with URL-valid
                    let last_char = last_cell.c;
                    let next_row = row + 1;
                    let visual_wrap = if next_row < screen_lines && url_char(last_char) {
                        let first_cell_next = &grid[Point::new(Line(next_row), Column(0))];
                        url_char(first_cell_next.c)
                    } else {
                        false
                    };

                    let is_wrapped = has_wrapline_flag || visual_wrap;

                    row += 1;

                    if !is_wrapped || row >= screen_lines {
                        break;
                    }
                }

                // Find all URLs in the combined text
                for mat in url_regex.find_iter(&combined_text) {
                    let url = mat.as_str().to_string();
                    // Clean up trailing punctuation that's likely not part of URL
                    let url = url.trim_end_matches(|c| matches!(c, '.' | ',' | ';' | ':' | '!' | '?'));
                    if url.is_empty() {
                        continue;
                    }

                    let url_start = mat.start();
                    let url_end = url_start + url.len();

                    // Map the URL position back to physical rows
                    // For wrapped URLs, create one match per physical row
                    for i in 0..row_offsets.len() {
                        let (phys_row, row_start_offset) = row_offsets[i];
                        let row_end_offset = if i + 1 < row_offsets.len() {
                            row_offsets[i + 1].1
                        } else {
                            combined_text.len()
                        };

                        // Check if URL overlaps with this row
                        if url_end <= row_start_offset || url_start >= row_end_offset {
                            continue;
                        }

                        // Calculate the portion of URL on this row
                        let match_start_in_combined = url_start.max(row_start_offset);
                        let match_end_in_combined = url_end.min(row_end_offset);

                        let col_start = match_start_in_combined - row_start_offset;
                        let len = match_end_in_combined - match_start_in_combined;

                        if len > 0 {
                            matches.push((phys_row, col_start, len, url.to_string()));
                        }
                    }
                }
            }
        });

        matches
    }

    /// Scroll to a specific position (0 = bottom, positive = towards top)
    pub fn scroll_to(&self, offset: usize) {
        let mut term = self.term.lock();
        let current = term.grid().display_offset();
        let delta = offset as i32 - current as i32;
        if delta != 0 {
            term.scroll_display(Scroll::Delta(delta));
        }
    }

    /// Check if terminal is in mouse reporting mode (for tmux, vim, etc.)
    /// Also returns true if using tmux backend (which handles mouse with `set mouse on`)
    pub fn is_mouse_mode(&self) -> bool {
        // If using tmux backend, tmux handles mouse events directly
        if self.pty_manager.uses_mouse_backend() {
            return true;
        }
        // Otherwise check if the terminal itself requested mouse mode
        let term = self.term.lock();
        term.mode().contains(TermMode::MOUSE_MODE)
    }

    /// Check if terminal uses SGR mouse encoding
    pub fn is_sgr_mouse(&self) -> bool {
        let term = self.term.lock();
        term.mode().contains(TermMode::SGR_MOUSE)
    }

    /// Send scroll event to PTY
    /// For tmux backend: sends SGR mouse wheel sequences (tmux with mouse on expects these)
    /// For other mouse mode apps: checks terminal mode for format
    /// button: 64 for scroll up, 65 for scroll down
    pub fn send_mouse_scroll(&self, button: u8, col: usize, row: usize) {
        // Check if using tmux backend - always use SGR format (tmux supports it)
        let use_sgr = if self.pty_manager.uses_mouse_backend() {
            true
        } else {
            let term = self.term.lock();
            term.mode().contains(TermMode::SGR_MOUSE)
        };

        if use_sgr {
            // SGR format: \x1b[<button;col;rowM
            let seq = format!("\x1b[<{};{};{}M", button, col + 1, row + 1);
            self.send_bytes(seq.as_bytes());
        } else {
            // Legacy format: \x1b[M + (button+32) + (col+33) + (row+33)
            // This format has limitations for coordinates > 223
            let bytes = [
                0x1b, b'[', b'M',
                button.saturating_add(32),
                (col as u8).saturating_add(33).min(255),
                (row as u8).saturating_add(33).min(255),
            ];
            self.send_bytes(&bytes);
        }
    }
}
