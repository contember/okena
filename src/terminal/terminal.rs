use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::index::{Point, Line, Column, Side};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::grid::{Scroll, Dimensions};
use parking_lot::Mutex;
use regex::Regex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

/// Transport trait for terminal I/O operations.
/// Implemented by PtyManager (local) and RemoteTransport (remote).
pub trait TerminalTransport: Send + Sync {
    fn send_input(&self, terminal_id: &str, data: &[u8]);
    fn resize(&self, terminal_id: &str, cols: u16, rows: u16);
    fn uses_mouse_backend(&self) -> bool;
    /// Debounce interval for transport resize calls (ms).
    /// Local PTY uses 16ms (just enough to batch rapid resizes).
    /// Remote uses longer interval to avoid flooding the network.
    fn resize_debounce_ms(&self) -> u64 { 16 }
}

/// Tracks who currently controls a terminal's resize.
/// The "last to type" wins: local input sets Local, remote input sets Remote.
/// After 30s of no remote input, the server auto-reclaims Local authority.
#[derive(Clone, Debug)]
pub enum ResizeOwner {
    /// Server's own UI controls resize (default).
    Local,
    /// A remote client controls resize (set when remote input arrives).
    Remote { last_input: std::time::Instant },
}

impl ResizeOwner {
    const STALE_TIMEOUT_SECS: u64 = 30;

    /// Returns true if the server should perform resize (Local or stale Remote).
    pub fn is_local(&self) -> bool {
        match self {
            ResizeOwner::Local => true,
            ResizeOwner::Remote { last_input } => {
                last_input.elapsed().as_secs() >= Self::STALE_TIMEOUT_SECS
            }
        }
    }
}

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


/// Event listener for alacritty_terminal that captures title changes, bell, and PTY write requests
pub struct ZedEventListener {
    /// Shared title storage - OSC 0/1/2 sequences update this
    title: Arc<Mutex<Option<String>>>,
    /// Bell notification flag
    has_bell: Arc<Mutex<bool>>,
    /// Transport for writing responses back to the terminal
    transport: Arc<dyn TerminalTransport>,
    /// Terminal ID for PTY write operations
    terminal_id: String,
}

impl ZedEventListener {
    pub fn new(
        title: Arc<Mutex<Option<String>>>,
        has_bell: Arc<Mutex<bool>>,
        transport: Arc<dyn TerminalTransport>,
        terminal_id: String,
    ) -> Self {
        Self { title, has_bell, transport, terminal_id }
    }
}

impl EventListener for ZedEventListener {
    fn send_event(&self, event: TermEvent) {
        match event {
            TermEvent::Title(title) => {
                *self.title.lock() = Some(title);
            }
            TermEvent::ResetTitle => {
                *self.title.lock() = None;
            }
            TermEvent::Bell => {
                *self.has_bell.lock() = true;
            }
            TermEvent::PtyWrite(data) => {
                // Write response back to PTY (e.g., cursor position report)
                log::debug!("PtyWrite event: {:?}", data);
                self.transport.send_input(&self.terminal_id, data.as_bytes());
            }
            _ => {
                // Ignore other events
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

/// A detected link in terminal content (URL or file path)
#[derive(Clone, Debug)]
pub struct DetectedLink {
    pub line: i32,
    pub col: usize,
    pub len: usize,
    pub text: String,
    pub file_line: Option<u32>,
    pub file_col: Option<u32>,
    pub is_url: bool,
}

/// Trim trailing punctuation from a URL/path, handling balanced parentheses.
///
/// Ghostty-style: strip trailing `.,:;!?)` but keep closing parens if they have
/// a matching opening paren inside the URL (e.g. Wikipedia links).
fn trim_url_trailing(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut end = bytes.len();

    loop {
        if end == 0 {
            break;
        }
        let c = bytes[end - 1];
        match c {
            b'.' | b',' | b':' | b';' | b'!' | b'?' => {
                end -= 1;
            }
            b')' => {
                // Only strip closing paren if unbalanced
                let open = s[..end].matches('(').count();
                let close = s[..end].matches(')').count();
                if close > open {
                    end -= 1;
                } else {
                    break;
                }
            }
            _ => break,
        }
    }

    &s[..end]
}

/// Parse optional `:line:col` suffix from a file path string.
/// Returns (display_text_including_suffix, optional_line, optional_col).
fn parse_path_line_col(s: &str) -> (String, Option<u32>, Option<u32>) {
    // Try to match :line:col at the end
    if let Some(colon_pos) = s.rfind(':') {
        let after = &s[colon_pos + 1..];
        if let Ok(num) = after.parse::<u32>() {
            let before = &s[..colon_pos];
            // Check for another :line before this
            if let Some(colon_pos2) = before.rfind(':') {
                let after2 = &before[colon_pos2 + 1..];
                if let Ok(line_num) = after2.parse::<u32>() {
                    // path:line:col
                    return (s.to_string(), Some(line_num), Some(num));
                }
            }
            // path:line
            return (s.to_string(), Some(num), None);
        }
    }
    (s.to_string(), None, None)
}

/// Consolidated resize-related state, protected by a single mutex
pub struct ResizeState {
    pub size: TerminalSize,
    last_pty_resize: std::time::Instant,
    pending_pty_resize: Option<(u16, u16)>,
    /// True when a background flush timer is scheduled to send the pending resize.
    flush_timer_active: bool,
}

/// A terminal instance wrapping alacritty_terminal
pub struct Terminal {
    term: Arc<Mutex<Term<ZedEventListener>>>,
    processor: Mutex<Processor>,
    pub terminal_id: String,
    pub resize_state: Arc<Mutex<ResizeState>>,
    transport: Arc<dyn TerminalTransport>,
    selection_state: Mutex<SelectionState>,
    scroll_offset: Mutex<i32>,
    /// Terminal title from OSC sequences
    title: Arc<Mutex<Option<String>>>,
    /// Bell notification flag (set when terminal receives bell, cleared on focus)
    has_bell: Arc<Mutex<bool>>,
    /// Dirty flag - set when terminal content changes, cleared after render
    dirty: AtomicBool,
    /// Who controls resize for this terminal (server UI vs remote client).
    resize_owner: Mutex<ResizeOwner>,
    /// Initial working directory (for resolving relative file paths in URL detection)
    initial_cwd: String,
    /// Timestamp of last terminal output (for idle detection)
    last_output_time: Arc<Mutex<Instant>>,
    /// Shell process PID (for foreground process check)
    shell_pid: Mutex<Option<u32>>,
    /// Cached "waiting for input" state — updated by background loop, read by renderers
    waiting_for_input: AtomicBool,
    /// Whether the user has ever sent input to this terminal (prevents flagging fresh terminals)
    had_user_input: AtomicBool,
    /// Timestamp of when the user last viewed this terminal (on blur)
    last_viewed_time: Arc<Mutex<Instant>>,
}

impl Terminal {
    /// Create a new terminal
    pub fn new(
        terminal_id: String,
        size: TerminalSize,
        transport: Arc<dyn TerminalTransport>,
        initial_cwd: String,
    ) -> Self {
        let config = TermConfig::default();
        let term_size = TermSize::new(size.cols as usize, size.rows as usize);

        // Create shared storage for OSC sequence handling and bell
        let title = Arc::new(Mutex::new(None));
        let has_bell = Arc::new(Mutex::new(false));
        let event_listener = ZedEventListener::new(
            title.clone(),
            has_bell.clone(),
            transport.clone(),
            terminal_id.clone(),
        );
        let term = Term::new(config, &term_size, event_listener);

        Self {
            term: Arc::new(Mutex::new(term)),
            processor: Mutex::new(Processor::new()),
            terminal_id,
            resize_state: Arc::new(Mutex::new(ResizeState {
                size,
                // Use a time in the past so the first resize from paint() always
                // passes the debounce check and sends SIGWINCH to the PTY immediately
                last_pty_resize: std::time::Instant::now() - std::time::Duration::from_secs(1),
                flush_timer_active: false,
                pending_pty_resize: None,
            })),
            transport,
            selection_state: Mutex::new(SelectionState::default()),
            scroll_offset: Mutex::new(0),
            title,
            has_bell,
            dirty: AtomicBool::new(false),
            resize_owner: Mutex::new(ResizeOwner::Local),
            initial_cwd,
            last_output_time: Arc::new(Mutex::new(Instant::now())),
            shell_pid: Mutex::new(None),
            waiting_for_input: AtomicBool::new(false),
            had_user_input: AtomicBool::new(false),
            last_viewed_time: Arc::new(Mutex::new(Instant::now())),
        }
    }

    /// Process output from PTY
    pub fn process_output(&self, data: &[u8]) {
        let mut term = self.term.lock();
        let mut processor = self.processor.lock();

        processor.advance(&mut *term, data);
        self.dirty.store(true, Ordering::Relaxed);
        *self.last_output_time.lock() = Instant::now();
    }

    /// Check if terminal has pending changes (and clear the flag)
    /// Note: Kept for potential external use, main path uses subscribe_dirty()
    #[allow(dead_code)]
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }

    /// Send input to the PTY
    /// Automatically scrolls to bottom if scrolled into history
    pub fn send_input(&self, input: &str) {
        self.had_user_input.store(true, Ordering::Relaxed);
        self.scroll_to_bottom();
        self.transport.send_input(&self.terminal_id, input.as_bytes());
    }

    /// Send raw bytes to the PTY
    /// Automatically scrolls to bottom if scrolled into history
    pub fn send_bytes(&self, data: &[u8]) {
        self.had_user_input.store(true, Ordering::Relaxed);
        self.scroll_to_bottom();
        self.transport.send_input(&self.terminal_id, data);
    }

    /// Clear the terminal screen by sending the clear sequence
    pub fn clear(&self) {
        // Send ANSI escape sequence to clear screen and move cursor to home
        // \x1b[2J = clear entire screen
        // \x1b[H = move cursor to home position (0,0)
        self.transport.send_input(&self.terminal_id, b"\x1b[2J\x1b[H");
        self.scroll_to_bottom();
    }

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
        self.start_selection(start_col, start_row);
        self.update_selection(end_col, end_row);
        self.end_selection();
    }

    /// Scroll to bottom (display_offset = 0)
    pub fn scroll_to_bottom(&self) {
        let mut term = self.term.lock();
        let current = term.grid().display_offset();
        if current > 0 {
            term.scroll_display(Scroll::Delta(-(current as i32)));
        }
    }

    /// Resize the terminal with debounced transport resize.
    ///
    /// The terminal grid is always resized immediately (optimistic update) for
    /// smooth rendering. Transport resize signals (PTY/remote) are debounced to
    /// avoid flooding the shell or network.
    ///
    /// Debounce interval is transport-dependent: 16ms for local PTY (~60fps),
    /// 150ms for remote connections. A trailing-edge timer ensures the final
    /// resize is always sent even when resize events stop mid-debounce.
    pub fn resize(&self, new_size: TerminalSize) {
        let debounce_ms = self.transport.resize_debounce_ms();

        // Always update local size immediately (optimistic UI)
        self.resize_state.lock().size = new_size;

        // Resize terminal grid immediately (independent mutex)
        let mut term = self.term.lock();
        let term_size = TermSize::new(new_size.cols as usize, new_size.rows as usize);
        term.resize(term_size);
        drop(term);

        // Debounce transport resize
        let now = std::time::Instant::now();
        let mut rs = self.resize_state.lock();
        let elapsed = now.duration_since(rs.last_pty_resize);

        if elapsed.as_millis() >= debounce_ms as u128 {
            // Enough time has passed — send resize immediately
            rs.pending_pty_resize = None;
            rs.last_pty_resize = now;
            drop(rs);
            self.transport.resize(&self.terminal_id, new_size.cols, new_size.rows);
        } else {
            // Store pending resize
            rs.pending_pty_resize = Some((new_size.cols, new_size.rows));

            // Schedule a trailing-edge flush timer if not already active.
            // This ensures the final resize is always sent even when events stop.
            if !rs.flush_timer_active {
                rs.flush_timer_active = true;
                let transport = self.transport.clone();
                let terminal_id = self.terminal_id.clone();
                let resize_state = self.resize_state.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(debounce_ms));
                    let mut rs = resize_state.lock();
                    rs.flush_timer_active = false;
                    if let Some((cols, rows)) = rs.pending_pty_resize.take() {
                        rs.last_pty_resize = std::time::Instant::now();
                        drop(rs);
                        transport.resize(&terminal_id, cols, rows);
                    }
                });
            }
        }
    }

    /// Resize only the local alacritty grid, without sending resize to PTY/transport.
    /// Used by remote clients to pre-resize the grid to match server dimensions before snapshot.
    pub fn resize_grid_only(&self, cols: u16, rows: u16) {
        let rs = self.resize_state.lock();
        let size = TerminalSize {
            cols,
            rows,
            cell_width: rs.size.cell_width,
            cell_height: rs.size.cell_height,
        };
        drop(rs);
        self.resize_state.lock().size = size;
        let mut term = self.term.lock();
        let term_size = TermSize::new(cols as usize, rows as usize);
        term.resize(term_size);
    }

    /// Mark this terminal as locally controlled (server UI input).
    pub fn claim_resize_local(&self) {
        *self.resize_owner.lock() = ResizeOwner::Local;
    }

    /// Mark this terminal as remotely controlled (remote client sent input).
    pub fn claim_resize_remote(&self) {
        *self.resize_owner.lock() = ResizeOwner::Remote {
            last_input: std::time::Instant::now(),
        };
    }

    /// Check if the server's UI should perform resize (Local or stale Remote).
    pub fn is_resize_owner_local(&self) -> bool {
        self.resize_owner.lock().is_local()
    }

    /// Flush any pending PTY resize (call this when resize operations complete)
    pub fn flush_pending_resize(&self) {
        let mut rs = self.resize_state.lock();
        if let Some((cols, rows)) = rs.pending_pty_resize.take() {
            rs.last_pty_resize = std::time::Instant::now();
            drop(rs);
            self.transport.resize(&self.terminal_id, cols, rows);
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
    /// Note: row is the visual row on screen (0 to screen_lines-1)
    /// We convert it to buffer coordinates by accounting for display_offset
    fn start_selection_with_type(&self, col: usize, row: i32, selection_type: SelectionType) {
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
        let selection = Selection::new(selection_type, point, Side::Left);
        term.selection = Some(selection);
    }

    /// Update selection to a new point
    /// Note: row is the visual row on screen (0 to screen_lines-1)
    /// We convert it to buffer coordinates by accounting for display_offset
    pub fn update_selection(&self, col: usize, row: i32) {
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

    /// Get cell dimensions (width, height) for coordinate conversion
    pub fn cell_dimensions(&self) -> (f32, f32) {
        let rs = self.resize_state.lock();
        (rs.size.cell_width, rs.size.cell_height)
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

    /// Get the initial working directory for this terminal
    pub fn initial_cwd(&self) -> &str {
        &self.initial_cwd
    }

    /// Set the shell process PID (for foreground process checking)
    pub fn set_shell_pid(&self, pid: u32) {
        *self.shell_pid.lock() = Some(pid);
    }

    /// Read the cached "waiting for input" state (cheap, no subprocess).
    /// This is safe to call from render paths. Updated by `update_waiting_state()`.
    pub fn is_waiting_for_input(&self) -> bool {
        self.waiting_for_input.load(Ordering::Relaxed)
    }

    /// Human-readable idle duration string (e.g., "5s", "2m", "1h").
    /// Shows time since the unseen output arrived.
    /// Only meaningful when `is_waiting_for_input()` is true.
    pub fn idle_duration_display(&self) -> String {
        let secs = self.last_viewed_time.lock().elapsed().as_secs();
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m", secs / 60)
        } else {
            format!("{}h", secs / 3600)
        }
    }

    /// Get the shell PID (for background thread to run pgrep off the main thread)
    pub fn shell_pid(&self) -> Option<u32> {
        *self.shell_pid.lock()
    }

    /// Get the last output time (for background thread idle check)
    pub fn last_output_time(&self) -> Instant {
        *self.last_output_time.lock()
    }

    /// Whether the user has ever sent input to this terminal
    pub fn had_user_input(&self) -> bool {
        self.had_user_input.load(Ordering::Relaxed)
    }

    /// Update the cached waiting state (called from background thread only)
    pub fn set_waiting_for_input(&self, waiting: bool) {
        self.waiting_for_input.store(waiting, Ordering::Relaxed);
    }

    /// Reset the idle timer to now, clearing the waiting state.
    /// Called when the terminal receives focus so it won't immediately re-trigger.
    pub fn clear_waiting(&self) {
        self.waiting_for_input.store(false, Ordering::Relaxed);
        *self.last_output_time.lock() = Instant::now();
        *self.last_viewed_time.lock() = Instant::now();
    }

    /// Record that the user has seen this terminal's output (called on blur).
    /// After this, the terminal won't be flagged as waiting unless new output arrives.
    pub fn mark_as_viewed(&self) {
        *self.last_viewed_time.lock() = Instant::now();
    }

    /// Whether new output has arrived since the user last viewed this terminal.
    pub fn has_unseen_output(&self) -> bool {
        *self.last_output_time.lock() > *self.last_viewed_time.lock()
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

    /// Detect URLs and file paths in the visible terminal content (Ghostty-style).
    ///
    /// Uses a single combined regex compiled once via OnceLock. Two branches:
    /// - URL: many schemes (http, https, ftp, ssh, git, mailto, etc.)
    /// - Path: explicit prefixes only (`/`, `~/`, `./`, `../`) with optional `:line:col`
    ///
    /// Returns a list of `DetectedLink` for each match. File paths are validated
    /// for existence by the caller (UrlDetector).
    pub fn detect_urls(&self) -> Vec<DetectedLink> {
        static LINK_REGEX: OnceLock<Regex> = OnceLock::new();
        let regex = LINK_REGEX.get_or_init(|| {
            // Combined regex: URL schemes | explicit file paths with optional :line:col
            // Path prefixes: /, ~/, ./, ../, or dotfile dirs like .github/
            Regex::new(
                r#"(?:(?:https?|ftp|file|ssh|git|mailto|tel|magnet|ipfs|gemini|gopher|news)://[^\s<>"'`{}\[\]|\\^]+|(?:~?/|(?:\./|\.\./)|\.[a-zA-Z][\w.-]*/)[^\s<>"'`{}\[\]|\\^()]+(?::(\d+)(?::(\d+))?)?)"#
            ).expect("link detection regex should compile")
        });

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
            let display_offset = grid.display_offset() as i32;

            // Iterate over visual rows (0..screen_lines).
            // When scrolled, visual row 0 maps to buffer line (0 - display_offset).
            let mut visual_row = 0i32;
            while visual_row < screen_lines {
                let mut combined_text = String::new();
                let mut row_offsets: Vec<(i32, usize)> = Vec::new();

                // Collect wrapped lines into one logical line
                loop {
                    row_offsets.push((visual_row, combined_text.len()));

                    // Buffer line accounts for scroll offset
                    let buffer_line = visual_row - display_offset;

                    let mut row_text = String::with_capacity(cols);
                    for col in 0..cols {
                        let cell_point = Point::new(Line(buffer_line), Column(col));
                        let cell = &grid[cell_point];
                        row_text.push(cell.c);
                    }
                    combined_text.push_str(&row_text);

                    let last_cell = &grid[Point::new(Line(buffer_line), last_col)];
                    let has_wrapline_flag = last_cell.flags.contains(Flags::WRAPLINE);

                    let last_char = last_cell.c;
                    let next_visual = visual_row + 1;
                    let visual_wrap = if next_visual < screen_lines && url_char(last_char) {
                        let next_buffer = next_visual - display_offset;
                        let first_cell_next = &grid[Point::new(Line(next_buffer), Column(0))];
                        url_char(first_cell_next.c)
                    } else {
                        false
                    };

                    visual_row += 1;

                    if !(has_wrapline_flag || visual_wrap) || visual_row >= screen_lines {
                        break;
                    }
                }

                for mat in regex.find_iter(&combined_text) {
                    let raw = mat.as_str();
                    let trimmed = trim_url_trailing(raw);
                    if trimmed.is_empty() {
                        continue;
                    }

                    let match_start = mat.start();
                    let trimmed_end = match_start + trimmed.len();

                    // Determine if this is a URL or file path
                    let is_url = trimmed.contains("://");

                    // Parse :line:col from file paths
                    let (display_text, file_line, file_col) = if !is_url {
                        parse_path_line_col(trimmed)
                    } else {
                        (trimmed.to_string(), None, None)
                    };

                    // Map back to physical rows
                    for i in 0..row_offsets.len() {
                        let (phys_row, row_start_offset) = row_offsets[i];
                        let row_end_offset = if i + 1 < row_offsets.len() {
                            row_offsets[i + 1].1
                        } else {
                            combined_text.len()
                        };

                        if trimmed_end <= row_start_offset || match_start >= row_end_offset {
                            continue;
                        }

                        let seg_start = match_start.max(row_start_offset);
                        let seg_end = trimmed_end.min(row_end_offset);

                        let col_start = combined_text[row_start_offset..seg_start].chars().count();
                        let len = combined_text[seg_start..seg_end].chars().count();

                        if len > 0 {
                            matches.push(DetectedLink {
                                line: phys_row,
                                col: col_start,
                                len,
                                text: display_text.clone(),
                                file_line,
                                file_col,
                                is_url,
                            });
                        }
                    }
                }
            }
        });

        matches
    }

    /// Render the terminal's visible content as ANSI escape sequences.
    ///
    /// Produces a byte stream that, when fed to another terminal emulator,
    /// reproduces the current screen state including colors and attributes.
    pub fn render_snapshot(&self) -> Vec<u8> {
        self.with_content(|term| grid_to_ansi(term))
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
        if self.transport.uses_mouse_backend() {
            return true;
        }
        // Otherwise check if the terminal itself requested mouse mode
        let term = self.term.lock();
        term.mode().contains(TermMode::MOUSE_MODE)
    }

    /// Check if terminal is in application cursor keys mode (DECCKM)
    /// When enabled, arrow keys should send SS3 sequences (\x1bOA) instead of CSI (\x1b[A)
    /// This is used by applications like less, vim, htop, etc.
    pub fn is_app_cursor_mode(&self) -> bool {
        let term = self.term.lock();
        term.mode().contains(TermMode::APP_CURSOR)
    }

    /// Send scroll event to PTY
    /// For tmux backend: sends SGR mouse wheel sequences (tmux with mouse on expects these)
    /// For other mouse mode apps: checks terminal mode for format
    /// button: 64 for scroll up, 65 for scroll down
    pub fn send_mouse_scroll(&self, button: u8, col: usize, row: usize) {
        // Check if using tmux backend - always use SGR format (tmux supports it)
        let use_sgr = if self.transport.uses_mouse_backend() {
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

/// Check if a process has child processes (Unix only).
/// On non-Unix platforms, always returns false (falls back to idle-only detection).
#[cfg(unix)]
pub fn has_child_processes(pid: u32) -> bool {
    std::process::Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
pub fn has_child_processes(_pid: u32) -> bool {
    false
}

// ── ANSI snapshot serialization ────────────────────────────────────────────

/// Tracked SGR state to minimize escape sequences in snapshot output.
#[derive(Clone, Default, PartialEq)]
struct SgrState {
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
    strikeout: bool,
    fg: Option<Color>,
    bg: Option<Color>,
}

/// Serialize the visible terminal grid to ANSI escape sequences.
fn grid_to_ansi(term: &Term<ZedEventListener>) -> Vec<u8> {
    let grid = term.grid();
    let screen_lines = grid.screen_lines();
    let cols = grid.columns();
    let cursor = term.grid().cursor.point;
    let cursor_hidden = !term.mode().contains(TermMode::SHOW_CURSOR);

    // Generous pre-allocation
    let mut buf = Vec::with_capacity(screen_lines * cols * 4);

    // Clear screen + move cursor home
    buf.extend_from_slice(b"\x1b[2J\x1b[H");

    let default_fg = Color::Named(NamedColor::Foreground);
    let default_bg = Color::Named(NamedColor::Background);

    let mut current = SgrState::default();

    for row in 0..screen_lines as i32 {
        // Position cursor at start of row
        write_csi_pos(&mut buf, row + 1, 1);

        let mut col_idx = 0usize;
        while col_idx < cols {
            let cell = &grid[Point::new(Line(row), Column(col_idx))];

            // Skip wide char spacer cells
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                col_idx += 1;
                continue;
            }

            // Determine desired SGR state
            let desired = SgrState {
                bold: cell.flags.contains(Flags::BOLD),
                dim: cell.flags.contains(Flags::DIM),
                italic: cell.flags.contains(Flags::ITALIC),
                underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
                inverse: cell.flags.contains(Flags::INVERSE),
                strikeout: cell.flags.contains(Flags::STRIKEOUT),
                fg: if cell.fg == default_fg { None } else { Some(cell.fg.clone()) },
                bg: if cell.bg == default_bg { None } else { Some(cell.bg.clone()) },
            };

            if desired != current {
                emit_sgr(&mut buf, &desired);
                current = desired;
            }

            // Write the character
            let c = cell.c;
            if c == '\0' || c == ' ' {
                buf.push(b' ');
            } else {
                let mut utf8_buf = [0u8; 4];
                let encoded = c.encode_utf8(&mut utf8_buf);
                buf.extend_from_slice(encoded.as_bytes());
            }

            col_idx += 1;
        }
    }

    // Reset attributes
    buf.extend_from_slice(b"\x1b[0m");

    // Position cursor
    write_csi_pos(&mut buf, cursor.line.0 + 1, cursor.column.0 as i32 + 1);

    // Hide cursor if needed
    if cursor_hidden {
        buf.extend_from_slice(b"\x1b[?25l");
    }

    buf
}

/// Write CSI cursor position: `\x1b[{row};{col}H`
fn write_csi_pos(buf: &mut Vec<u8>, row: i32, col: i32) {
    use std::io::Write;
    let _ = write!(buf, "\x1b[{};{}H", row, col);
}

/// Emit a full SGR sequence from the desired state (always resets first).
fn emit_sgr(buf: &mut Vec<u8>, state: &SgrState) {
    use std::io::Write;

    buf.extend_from_slice(b"\x1b[0");

    if state.bold {
        buf.extend_from_slice(b";1");
    }
    if state.dim {
        buf.extend_from_slice(b";2");
    }
    if state.italic {
        buf.extend_from_slice(b";3");
    }
    if state.underline {
        buf.extend_from_slice(b";4");
    }
    if state.inverse {
        buf.extend_from_slice(b";7");
    }
    if state.strikeout {
        buf.extend_from_slice(b";9");
    }
    if let Some(ref color) = state.fg {
        push_color_sgr(buf, color, true);
    }
    if let Some(ref color) = state.bg {
        push_color_sgr(buf, color, false);
    }

    let _ = write!(buf, "m");
}

/// Append color SGR parameters (e.g. `;31` or `;38;5;123` or `;38;2;R;G;B`).
fn push_color_sgr(buf: &mut Vec<u8>, color: &Color, is_fg: bool) {
    use std::io::Write;

    match color {
        Color::Named(named) => {
            let code = named_color_sgr_code(named, is_fg);
            if let Some(code) = code {
                let _ = write!(buf, ";{}", code);
            }
        }
        Color::Indexed(idx) => {
            let base = if is_fg { 38 } else { 48 };
            let _ = write!(buf, ";{};5;{}", base, idx);
        }
        Color::Spec(rgb) => {
            let base = if is_fg { 38 } else { 48 };
            let _ = write!(buf, ";{};2;{};{};{}", base, rgb.r, rgb.g, rgb.b);
        }
    }
}

/// Map a NamedColor to its SGR code.
fn named_color_sgr_code(color: &NamedColor, is_fg: bool) -> Option<u8> {
    let code = match color {
        NamedColor::Black => 0,
        NamedColor::Red => 1,
        NamedColor::Green => 2,
        NamedColor::Yellow => 3,
        NamedColor::Blue => 4,
        NamedColor::Magenta => 5,
        NamedColor::Cyan => 6,
        NamedColor::White => 7,
        NamedColor::BrightBlack => 8,
        NamedColor::BrightRed => 9,
        NamedColor::BrightGreen => 10,
        NamedColor::BrightYellow => 11,
        NamedColor::BrightBlue => 12,
        NamedColor::BrightMagenta => 13,
        NamedColor::BrightCyan => 14,
        NamedColor::BrightWhite => 15,
        // Foreground/Background/Cursor are default colors, no SGR code
        _ => return None,
    };

    if code < 8 {
        Some(if is_fg { 30 + code } else { 40 + code })
    } else {
        // Bright colors: 90-97 / 100-107
        Some(if is_fg { 90 + (code - 8) } else { 100 + (code - 8) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct NullTransport;
    impl TerminalTransport for NullTransport {
        fn send_input(&self, _terminal_id: &str, _data: &[u8]) {}
        fn resize(&self, _terminal_id: &str, _cols: u16, _rows: u16) {}
        fn uses_mouse_backend(&self) -> bool { false }
    }

    #[test]
    fn test_osc_title_set() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "test-id".to_string(),
            TerminalSize::default(),
            transport,
            "/tmp".to_string(),
        );

        // Feed OSC 0 (set title) sequence: ESC ] 0 ; MOJE_JMENO BEL
        let osc_data = b"\x1b]0;MOJE_JMENO\x07";
        terminal.process_output(osc_data);

        assert_eq!(terminal.title(), Some("MOJE_JMENO".to_string()));
    }

    #[test]
    fn test_osc_title_with_surrounding_data() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "test-id".to_string(),
            TerminalSize::default(),
            transport,
            "/tmp".to_string(),
        );

        // Simulate what dtach sends: clear screen + OSC title + some output
        let data = b"\x1b[H\x1b[J\x1b]0;MOJE_JMENO\x07DONE\r\n";
        terminal.process_output(data);

        assert_eq!(terminal.title(), Some("MOJE_JMENO".to_string()));
    }

    #[test]
    fn test_osc_title_split_across_chunks() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "test-id".to_string(),
            TerminalSize::default(),
            transport,
            "/tmp".to_string(),
        );

        // Split the OSC sequence across two process_output calls
        terminal.process_output(b"\x1b]0;MOJE");
        assert_eq!(terminal.title(), None); // Not complete yet

        terminal.process_output(b"_JMENO\x07");
        assert_eq!(terminal.title(), Some("MOJE_JMENO".to_string()));
    }

    #[test]
    fn test_osc_title_reset() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "test-id".to_string(),
            TerminalSize::default(),
            transport,
            "/tmp".to_string(),
        );

        // Set title
        terminal.process_output(b"\x1b]0;MOJE_JMENO\x07");
        assert_eq!(terminal.title(), Some("MOJE_JMENO".to_string()));

        // Reset title (OSC 0 with empty string => set_title(None) in alacritty)
        terminal.process_output(b"\x1b]0;\x07");
        // After reset, title should be cleared or set to empty
        let title = terminal.title();
        assert!(title.is_none() || title.as_deref() == Some(""), "title should be empty or None, got: {:?}", title);
    }

    #[test]
    fn resize_owner_defaults_to_local() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, String::new());
        assert!(terminal.is_resize_owner_local());
    }

    #[test]
    fn resize_owner_transitions() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, String::new());

        terminal.claim_resize_remote();
        assert!(!terminal.is_resize_owner_local());

        terminal.claim_resize_local();
        assert!(terminal.is_resize_owner_local());
    }

    #[test]
    fn resize_owner_stale_remote_reclaims_local() {
        let owner = ResizeOwner::Remote {
            last_input: std::time::Instant::now() - std::time::Duration::from_secs(31),
        };
        assert!(owner.is_local(), "stale remote (>30s) should be treated as local");

        let owner = ResizeOwner::Remote {
            last_input: std::time::Instant::now(),
        };
        assert!(!owner.is_local(), "fresh remote should NOT be treated as local");
    }

    #[test]
    fn resize_grid_only_does_not_call_transport() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct SpyTransport { resize_called: AtomicBool }
        impl TerminalTransport for SpyTransport {
            fn send_input(&self, _: &str, _: &[u8]) {}
            fn resize(&self, _: &str, _: u16, _: u16) {
                self.resize_called.store(true, Ordering::Relaxed);
            }
            fn uses_mouse_backend(&self) -> bool { false }
        }

        let transport = Arc::new(SpyTransport { resize_called: AtomicBool::new(false) });
        let terminal = Terminal::new("t".into(), TerminalSize::default(), transport.clone(), String::new());

        terminal.resize_grid_only(120, 40);
        assert!(!transport.resize_called.load(Ordering::Relaxed));
        assert_eq!(terminal.resize_state.lock().size.cols, 120);
        assert_eq!(terminal.resize_state.lock().size.rows, 40);
    }
}
