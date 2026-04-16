use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config as TermConfig, Term, TermMode};
use alacritty_terminal::vte::Perform;
use alacritty_terminal::vte::ansi::{Color, CursorShape as VteCursorShape, CursorStyle as VteCursorStyle, NamedColor, Processor};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::index::{Point, Line, Column, Side};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::grid::{Scroll, Dimensions};
use parking_lot::Mutex;
use regex::Regex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// Process-global resize authority. "Last to interact wins" across all terminals
/// in this process: whichever side most recently typed or clicked gets to drive
/// resize for every terminal. No time-based reclaim — the origin side takes over
/// by actually interacting, not by waiting.
///
/// Implemented with a monotonically-increasing sequence counter to avoid
/// timestamp collisions. Each claim bumps the counter and records the new value
/// on the claiming side. Higher value wins. Both zero (initial) resolves to
/// Local, so terminals behave normally before any interaction happens.
static RESIZE_AUTH_SEQ: AtomicU64 = AtomicU64::new(0);
static LAST_LOCAL_SEQ: AtomicU64 = AtomicU64::new(0);
static LAST_REMOTE_SEQ: AtomicU64 = AtomicU64::new(0);

pub fn claim_resize_authority_local() {
    let seq = RESIZE_AUTH_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    LAST_LOCAL_SEQ.store(seq, Ordering::Relaxed);
}

pub fn claim_resize_authority_remote() {
    let seq = RESIZE_AUTH_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    LAST_REMOTE_SEQ.store(seq, Ordering::Relaxed);
}

pub fn is_resize_authority_local() -> bool {
    LAST_LOCAL_SEQ.load(Ordering::Relaxed) >= LAST_REMOTE_SEQ.load(Ordering::Relaxed)
}

#[cfg(test)]
fn reset_resize_authority() {
    RESIZE_AUTH_SEQ.store(0, Ordering::Relaxed);
    LAST_LOCAL_SEQ.store(0, Ordering::Relaxed);
    LAST_REMOTE_SEQ.store(0, Ordering::Relaxed);
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
    /// Pending OSC 52 clipboard writes to be picked up by the GPUI thread
    pending_clipboard: Arc<Mutex<Vec<String>>>,
    /// Current theme palette, pushed from the GPUI thread on each render.
    /// Used to answer OSC 10/11/12/4 color queries from apps.
    palette: Arc<Mutex<Option<okena_core::theme::ThemeColors>>>,
    /// Transport for writing responses back to the terminal
    transport: Arc<dyn TerminalTransport>,
    /// Terminal ID for PTY write operations
    terminal_id: String,
}

impl ZedEventListener {
    pub fn new(
        title: Arc<Mutex<Option<String>>>,
        has_bell: Arc<Mutex<bool>>,
        pending_clipboard: Arc<Mutex<Vec<String>>>,
        palette: Arc<Mutex<Option<okena_core::theme::ThemeColors>>>,
        transport: Arc<dyn TerminalTransport>,
        terminal_id: String,
    ) -> Self {
        Self { title, has_bell, pending_clipboard, palette, transport, terminal_id }
    }

    /// Resolve a color index (as passed by alacritty on a color query) to an
    /// (r, g, b) triple.
    ///
    /// Indices 0..=15 and the named foreground/background/cursor slots come
    /// from the active theme palette (so apps that ask "what's your red?"
    /// see Okena's configured red, not xterm's). Indices 16..=231 and the
    /// 24-step grayscale ramp 232..=255 are answered from the standard
    /// xterm 256-color table — these are not themed and match every other
    /// modern terminal.
    fn resolve_color(&self, index: usize) -> Option<(u8, u8, u8)> {
        use alacritty_terminal::vte::ansi::NamedColor;

        if (16..=231).contains(&index) {
            return Some(xterm_256_cube_rgb(index));
        }
        if (232..=255).contains(&index) {
            return Some(xterm_256_grayscale_rgb(index));
        }

        let palette = self.palette.lock();
        let colors = palette.as_ref()?;
        let hex = match index {
            0 => colors.term_black,
            1 => colors.term_red,
            2 => colors.term_green,
            3 => colors.term_yellow,
            4 => colors.term_blue,
            5 => colors.term_magenta,
            6 => colors.term_cyan,
            7 => colors.term_white,
            8 => colors.term_bright_black,
            9 => colors.term_bright_red,
            10 => colors.term_bright_green,
            11 => colors.term_bright_yellow,
            12 => colors.term_bright_blue,
            13 => colors.term_bright_magenta,
            14 => colors.term_bright_cyan,
            15 => colors.term_bright_white,
            i if i == NamedColor::Foreground as usize => colors.term_foreground,
            i if i == NamedColor::Background as usize => colors.term_background,
            i if i == NamedColor::Cursor as usize => colors.cursor,
            _ => return None,
        };
        Some(((hex >> 16) as u8, (hex >> 8) as u8, hex as u8))
    }
}

/// xterm 6x6x6 color cube for palette indices 16..=231.
///
/// Each axis uses the canonical xterm levels [0, 95, 135, 175, 215, 255]
/// (not a linear 0/51/102/... ramp — xterm jumps at 95 for perceptual
/// reasons and every modern terminal matches this).
fn xterm_256_cube_rgb(index: usize) -> (u8, u8, u8) {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let n = index - 16;
    (LEVELS[n / 36], LEVELS[(n / 6) % 6], LEVELS[n % 6])
}

/// xterm 24-step grayscale ramp for palette indices 232..=255. The levels
/// start at 8 and step by 10 (8, 18, ..., 238), skipping true black and
/// true white — apps that need those use cube indices 16 and 231.
fn xterm_256_grayscale_rgb(index: usize) -> (u8, u8, u8) {
    let level = 8 + (index as u8 - 232) * 10;
    (level, level, level)
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
            TermEvent::ClipboardStore(_, text) => {
                self.pending_clipboard.lock().push(text);
            }
            TermEvent::ColorRequest(index, response_fn) => {
                if let Some((r, g, b)) = self.resolve_color(index) {
                    let reply =
                        response_fn(alacritty_terminal::vte::ansi::Rgb { r, g, b });
                    self.transport.send_input(&self.terminal_id, reply.as_bytes());
                }
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

/// Application version used in XTVERSION responses. Injected once at
/// startup from the main binary (which knows its own `CARGO_PKG_VERSION`);
/// defaults to `"0.0.0"` so unit tests and library-only consumers still
/// get a parseable response.
static APP_VERSION: OnceLock<String> = OnceLock::new();

/// Register the application version that will be reported to terminal
/// applications via XTVERSION (`DCS > | okena(<version>) ST`). Safe to
/// call multiple times — the first value wins.
pub fn set_app_version(version: impl Into<String>) {
    let _ = APP_VERSION.set(version.into());
}

fn app_version() -> &'static str {
    APP_VERSION.get().map(String::as_str).unwrap_or("0.0.0")
}

/// Side-channel VTE parser for sequences that alacritty_terminal either
/// ignores or answers in a way Okena wants to override. Runs on the same
/// byte stream as the main `Processor` so we can observe shell-reported
/// state (OSC 7 cwd, later OSC 133) and answer terminal-identification
/// queries (XTVERSION) without patching upstream.
struct OscSidecar {
    parser: alacritty_terminal::vte::Parser,
    perform: SidecarPerform,
}

impl OscSidecar {
    fn new(
        reported_cwd: Arc<Mutex<Option<String>>>,
        pending_notifications: Arc<Mutex<Vec<String>>>,
        transport: Arc<dyn TerminalTransport>,
        terminal_id: String,
    ) -> Self {
        Self {
            parser: alacritty_terminal::vte::Parser::new(),
            perform: SidecarPerform {
                reported_cwd,
                pending_notifications,
                transport,
                terminal_id,
            },
        }
    }

    fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.perform, bytes);
    }
}

struct SidecarPerform {
    reported_cwd: Arc<Mutex<Option<String>>>,
    /// iTerm2-style `OSC 9 ; <message>` notifications, drained by the GPUI
    /// thread on each render (same model as `pending_clipboard`).
    pending_notifications: Arc<Mutex<Vec<String>>>,
    transport: Arc<dyn TerminalTransport>,
    terminal_id: String,
}

impl Perform for SidecarPerform {
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.len() < 2 {
            return;
        }
        match params[0] {
            b"7" => {
                // Rejoin with `;` in case an unencoded semicolon in the URI
                // caused the parser to split the value across multiple
                // params. Well-behaved shell scripts percent-encode `;`,
                // but be forgiving.
                let uri: String = params[1..]
                    .iter()
                    .filter_map(|p| std::str::from_utf8(p).ok())
                    .collect::<Vec<_>>()
                    .join(";");
                if let Some(path) = parse_osc7_file_uri(&uri) {
                    *self.reported_cwd.lock() = Some(path);
                }
            }
            b"9" => {
                // iTerm2-style notification: `OSC 9 ; <message>`. ConEmu's
                // `OSC 9 ; 4 ; state ; progress` progress-bar subtype is
                // treated as a plain-text message for now — we can split
                // off subtypes when there's a UI for them.
                let message: String = params[1..]
                    .iter()
                    .filter_map(|p| std::str::from_utf8(p).ok())
                    .collect::<Vec<_>>()
                    .join(";");
                let message = message.trim();
                if !message.is_empty() {
                    self.pending_notifications.lock().push(message.to_string());
                }
            }
            _ => {}
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &alacritty_terminal::vte::Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        // XTVERSION query: `CSI > Ps q`. Per xterm ctlseqs, only Ps=0 (or
        // omitted) asks for the terminal name+version; other Ps values
        // belong to unrelated private CSI sequences we must not answer.
        if action != 'q' || intermediates != [b'>'] {
            return;
        }
        let ps = params
            .iter()
            .next()
            .and_then(|p| p.first().copied())
            .unwrap_or(0);
        if ps != 0 {
            return;
        }
        let response = format!("\x1bP>|okena({})\x1b\\", app_version());
        self.transport.send_input(&self.terminal_id, response.as_bytes());
    }
}

/// Extract the local path from an `OSC 7` `file://host/path` URI.
///
/// Host component is accepted but ignored — Okena's remote terminals already
/// know which host a session belongs to, so the path alone is what callers
/// care about. Returns `None` if the scheme is missing, the URI has no path
/// component, or percent-decoding yields invalid UTF-8.
fn parse_osc7_file_uri(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let path_start = rest.find('/')?;
    percent_decode(&rest[path_start..])
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16)?;
            let lo = (bytes[i + 2] as char).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Which way `jump_to_prompt` looks relative to the currently visible top.
#[derive(Clone, Copy, Debug)]
enum JumpDirection {
    Above,
    Below,
}

/// Kind of an OSC 133 shell-integration mark.
///
/// Shells that implement OSC 133 emit one of these four sequences at
/// well-defined points in the prompt/command lifecycle. Each kind carries
/// no positional data itself — the row/column are captured by Okena when
/// the sidecar sees the sequence arrive on the byte stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptMarkKind {
    /// `OSC 133 ; A` — beginning of a new prompt (before the user can type).
    PromptStart,
    /// `OSC 133 ; B` — end of prompt, the shell is ready for input.
    CommandStart,
    /// `OSC 133 ; C` — user hit Enter, the command is now executing.
    CommandExecuted,
    /// `OSC 133 ; D [; <exit_code>]` — the command finished. The exit code
    /// is optional because some shells elide it on interactive prompts.
    CommandFinished { exit_code: Option<i32> },
}

/// A shell-integration mark with its captured grid position.
///
/// `line` is in alacritty's `Line` coordinate system: `0..screen_lines-1`
/// is the viewport and negative values live in scrollback. The tracker
/// rebases `line` as content scrolls so the value stays valid across
/// command output — up to the scrollback cap (see [`PromptTracker`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromptMark {
    pub kind: PromptMarkKind,
    pub line: i32,
    pub column: usize,
}

/// Ring buffer of recent OSC 133 marks plus a best-effort scroll tracker.
///
/// **Scrollback-cap caveat**: the tracker rebases marks by watching
/// `grid.history_size()` grow — which is exact until the user hits the
/// configured scrollback limit. Past that, the grid starts evicting from
/// the top without changing `history_size`, so mark line values drift by
/// the number of post-cap scrolls. Follow-up work can tighten this by
/// counting linefeeds directly; until then, callers should treat marks
/// older than the scrollback as "approximate" and prefer jumping to the
/// most recent few.
struct PromptTracker {
    marks: VecDeque<PromptMark>,
    /// Oldest-first ring buffer cap. Shells that run thousands of commands
    /// don't need thousands of marks — the UX only looks at the last few.
    capacity: usize,
}

impl PromptTracker {
    fn new() -> Self {
        Self { marks: VecDeque::with_capacity(64), capacity: 64 }
    }

    /// Record a new mark at the given grid point. Evicts the oldest mark
    /// when the ring buffer is full.
    fn record(&mut self, kind: PromptMarkKind, point: Point) {
        if self.marks.len() == self.capacity {
            self.marks.pop_front();
        }
        self.marks.push_back(PromptMark {
            kind,
            line: point.line.0,
            column: point.column.0,
        });
    }

    /// Shift all stored marks upward by the number of lines that just
    /// scrolled into history. Marks whose new `line` falls off the top of
    /// the grid (below `-history_size`) are dropped.
    fn on_history_changed(&mut self, before: usize, after: usize, topmost: i32) {
        let delta = after.saturating_sub(before);
        if delta == 0 {
            return;
        }
        let delta_i32 = delta as i32;
        self.marks.retain_mut(|mark| {
            mark.line -= delta_i32;
            mark.line >= topmost
        });
    }

    fn snapshot(&self) -> Vec<PromptMark> {
        self.marks.iter().copied().collect()
    }
}

/// Parse the body of an `OSC 133 ; <kind> [; <args...>]` sequence into a
/// [`PromptMarkKind`]. Returns `None` for unrecognized kind bytes; extra
/// key=value parameters (e.g. `aid=...`, `cl=...` used by some shells)
/// are ignored.
fn parse_osc133_kind(kind: u8, rest: &[&[u8]]) -> Option<PromptMarkKind> {
    match kind {
        b'A' => Some(PromptMarkKind::PromptStart),
        b'B' => Some(PromptMarkKind::CommandStart),
        b'C' => Some(PromptMarkKind::CommandExecuted),
        b'D' => {
            // First sub-param, when present and purely numeric, is the
            // exit code. Anything else (key=value metadata, junk) means
            // "unknown exit".
            let exit_code = rest
                .first()
                .and_then(|p| std::str::from_utf8(p).ok())
                .and_then(|s| {
                    let s = s.trim();
                    if s.is_empty() { None } else { s.parse::<i32>().ok() }
                });
            Some(PromptMarkKind::CommandFinished { exit_code })
        }
        _ => None,
    }
}

/// Byte-splitting sidecar for OSC 133. Unlike the observer-only
/// [`OscSidecar`], this one uses `advance_until_terminated` so the caller
/// can snapshot the main processor's cursor position at the exact byte
/// where the mark arrived.
struct PromptSidecar {
    parser: alacritty_terminal::vte::Parser,
    perform: PromptSidecarPerform,
}

impl PromptSidecar {
    fn new() -> Self {
        Self {
            parser: alacritty_terminal::vte::Parser::new(),
            perform: PromptSidecarPerform { pending: None },
        }
    }
}

struct PromptSidecarPerform {
    pending: Option<PromptMarkKind>,
}

impl Perform for PromptSidecarPerform {
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // We only care about `OSC 133 ; X [; args...]`. Everything else
        // is handled by the observer sidecar or ignored.
        if params.first().copied() != Some(b"133".as_ref()) {
            return;
        }
        let Some(kind_param) = params.get(1) else { return };
        let Some(&kind_byte) = kind_param.first() else { return };
        if let Some(kind) = parse_osc133_kind(kind_byte, &params[2..]) {
            self.pending = Some(kind);
        }
    }

    fn terminated(&self) -> bool {
        self.pending.is_some()
    }
}

/// Feed `data` to both the main alacritty processor and the prompt sidecar
/// in lockstep. Whenever the sidecar sees an `OSC 133` sequence it flags
/// itself as terminated; we advance the main processor up to the same byte
/// offset (so the cursor is at its post-OSC position, which is unchanged
/// since OSC sequences are zero-width) and then record the mark.
fn advance_with_prompt_marks<L: EventListener>(
    term: &mut Term<L>,
    processor: &mut Processor,
    sidecar: &mut PromptSidecar,
    tracker: &mut PromptTracker,
    data: &[u8],
) {
    let mut pos = 0;
    while pos < data.len() {
        let consumed = sidecar
            .parser
            .advance_until_terminated(&mut sidecar.perform, &data[pos..]);
        processor.advance(term, &data[pos..pos + consumed]);
        if let Some(kind) = sidecar.perform.pending.take() {
            let point = term.grid().cursor.point;
            tracker.record(kind, point);
        }
        if consumed == 0 {
            // Safety net: `advance_until_terminated` is expected to make
            // progress on every call (at least one byte per inner loop
            // iteration) as long as `terminated()` was false on entry.
            // If the parser ever stalls anyway, bail rather than spin.
            break;
        }
        pos += consumed;
    }
}

/// Cursor shape requested by the terminal application via DECSCUSR.
///
/// Maps onto the three shapes Okena's renderer knows how to paint.
/// Alacritty's `HollowBlock` is used internally as a sentinel for "app has
/// not set any shape" and never reaches callers as this type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppCursorShape {
    Block,
    Bar,
    Underline,
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
    /// Segments of the same wrapped URL share the same wrap_group.
    /// Different occurrences of the same URL get different wrap_groups.
    pub wrap_group: usize,
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
    /// Timestamp of the last local resize (from TerminalElement::paint).
    /// Used to suppress redundant server resize echoes in remote mode.
    pub last_local_resize: std::time::Instant,
}

/// A terminal instance wrapping alacritty_terminal
/// Terminal emulator state.
///
/// # Threading model
///
/// `Terminal` is always stored behind `Arc` (in `TerminalsRegistry`) and all
/// methods take `&self`, using interior mutability for mutation. Three
/// execution contexts access the struct:
///
/// 1. **GPUI thread** — the main UI thread. Runs `process_output` (via the
///    batched PTY event loop in `Okena`), all rendering (`with_content`),
///    user-input methods, resize, selection, scroll, and idle-detection reads.
///    This is where the vast majority of field access happens.
///
/// 2. **Tokio reader task** (remote connections only) — calls `enqueue_output`
///    to buffer incoming data without holding `term.lock()`. Only touches
///    `pending_output`, `dirty`, and `last_output_time`.
///
/// 3. **Resize debounce timer** — a short-lived `std::thread::spawn` that
///    flushes a trailing-edge resize after the debounce window. Only touches
///    `resize_state` and `transport`.
///
/// The PTY reader OS thread does **not** touch `Terminal` directly — it sends
/// `PtyEvent::Data` through an `async_channel`, which the GPUI thread drains.
///
/// # Synchronization primitives
///
/// - **`Arc<Mutex<T>>`** — the `Arc` is needed when the value is shared with a
///   sub-struct (`ZedEventListener`, `OscSidecar`) or handed to a background
///   thread (`resize_state`). The `Mutex` (from `parking_lot`) provides
///   interior mutability.
///
/// - **`Mutex<T>`** — interior mutability for fields that don't need to be
///   shared outside the `Terminal` struct. All current `Mutex`-only fields are
///   accessed exclusively from the GPUI thread; the `Mutex` is required
///   because `&self` methods need interior mutability, not because multiple
///   threads contend.
///
/// - **`AtomicBool` / `AtomicU64`** — lock-free signaling between the GPUI
///   thread and the tokio reader task (for `dirty`), or between the GPUI
///   thread's output path and its render path (for `content_generation`,
///   `waiting_for_input`, `had_user_input`) to avoid mutex overhead on every
///   frame.
pub struct Terminal {
    // ── Immutable after construction ─────────────────────────────────

    /// Unique identifier for this terminal instance. Immutable after
    /// construction; read freely from any thread.
    pub terminal_id: String,

    /// I/O transport (local PTY or remote WebSocket). Immutable ref after
    /// construction. `Arc` for sharing with `ZedEventListener`, `OscSidecar`,
    /// and the resize debounce timer.
    transport: Arc<dyn TerminalTransport>,

    /// Initial working directory passed at creation time. Immutable.
    /// Used as fallback when the shell has not yet reported its cwd via OSC 7.
    initial_cwd: String,

    // ── GPUI-thread only ─────────────────────────────────────────────
    // All fields below are accessed exclusively from the GPUI thread.
    // `Mutex` provides interior mutability for `&self` methods, not
    // cross-thread safety.

    /// ANSI parser state (alacritty_terminal `Term`). Locked by
    /// `process_output`, `with_content`, `resize`, `scroll`, and selection
    /// methods — all on the GPUI thread. The `Arc` is structural: it doesn't
    /// get cloned, but `Terminal` requires `Send + Sync` and `Term` is
    /// mutated through `&self`.
    term: Arc<Mutex<Term<ZedEventListener>>>,

    /// VTE byte processor. Locked together with `term` in `process_output`
    /// and `drain_pending_output`. GPUI thread only.
    processor: Mutex<Processor>,

    /// Mouse/keyboard selection state. GPUI thread only (selection start,
    /// update, finish, cancel — all driven by UI events).
    selection_state: Mutex<SelectionState>,

    /// Cumulative scroll delta in the scrollback buffer. GPUI thread only
    /// (scroll, scroll_page). The `Mutex` is for interior mutability; no
    /// cross-thread contention.
    scroll_offset: Mutex<i32>,

    /// Terminal title set by OSC 0/1/2 sequences. `Arc` shared with
    /// `ZedEventListener` (which lives inside `Term`): the listener writes
    /// on title-change events during `process_output`, and the GPUI render
    /// path reads via `get_title`. Both happen on the GPUI thread.
    title: Arc<Mutex<Option<String>>>,

    /// Bell notification flag. `Arc` shared with `ZedEventListener`: set on
    /// BEL during `process_output`, cleared by the render path on focus.
    /// GPUI thread only.
    has_bell: Arc<Mutex<bool>>,

    /// Pending OSC 52 clipboard writes requested by the running app. `Arc`
    /// shared with `ZedEventListener`: pushed during `process_output`,
    /// drained by the GPUI render path via `drain_clipboard_writes`.
    /// GPUI thread only.
    pending_clipboard: Arc<Mutex<Vec<String>>>,

    /// Theme palette used to answer OSC 10/11/12/4 color queries from
    /// terminal apps. `Arc` shared with `ZedEventListener`: the render path
    /// pushes the current theme via `push_palette`, and the listener reads
    /// it when composing color-query responses. GPUI thread only.
    palette: Arc<Mutex<Option<okena_core::theme::ThemeColors>>>,

    /// Working directory most recently reported by the shell via OSC 7.
    /// `None` until the shell sends its first `ESC ] 7 ; file://...`
    /// sequence. `Arc` shared with `OscSidecar` (the sidecar writes on
    /// parse, GPUI reads via `reported_cwd`). GPUI thread only.
    reported_cwd: Arc<Mutex<Option<String>>>,

    /// Pending iTerm2-style `OSC 9` notifications. `Arc` shared with
    /// `OscSidecar`: pushed during `process_output`, drained by the GPUI
    /// render path via `drain_notifications`. GPUI thread only.
    pending_notifications: Arc<Mutex<Vec<String>>>,

    /// VTE sidecar parser for OSC/CSI sequences (OSC 7 cwd, OSC 9
    /// notifications, XTVERSION) that alacritty_terminal either ignores or
    /// answers differently than Okena wants. GPUI thread only
    /// (`process_output` and `drain_pending_output`).
    osc_sidecar: Mutex<OscSidecar>,

    /// Byte-splitting sidecar for OSC 133 shell-integration marks. Runs
    /// in lockstep with the main `processor` so cursor positions can be
    /// snapshotted at the exact byte each mark arrives. GPUI thread only.
    prompt_sidecar: Mutex<PromptSidecar>,

    /// Ring buffer of captured OSC 133 prompt marks. Written during
    /// `process_output`, read by `prompt_marks` and `jump_to_prompt_*`.
    /// GPUI thread only.
    prompt_tracker: Mutex<PromptTracker>,

    /// Reverse index into the current list of `PromptStart` marks (0 =
    /// newest). `Some` while the user is walking through prompts with
    /// `jump_to_prompt_above/below`; reset to `None` on any output or
    /// scroll so the next walk starts from the most recent prompt again.
    /// GPUI thread only.
    prompt_jump_index: Mutex<Option<usize>>,

    /// Shell process PID. Set by `set_shell_pid` (called from GPUI thread
    /// after PTY spawn), read by `shell_pid` and `has_running_child`.
    /// GPUI thread only.
    shell_pid: Mutex<Option<u32>>,

    /// Timestamp of when the user last viewed this terminal (set on blur
    /// via `mark_as_viewed`). Compared against `last_output_time` to
    /// determine `has_unseen_output`. GPUI thread only.
    ///
    /// The `Arc` is historical — the value is never cloned; a plain `Mutex`
    /// would suffice.
    last_viewed_time: Arc<Mutex<Instant>>,

    // ── GPUI + resize debounce timer ─────────────────────────────────

    /// Terminal size, debounce state, and pending PTY resize. `Arc` is
    /// required: a clone is handed to the short-lived debounce timer thread
    /// (`std::thread::spawn` in `resize`) which flushes the trailing-edge
    /// resize after the debounce window.
    pub resize_state: Arc<Mutex<ResizeState>>,

    // ── Cross-thread (GPUI + tokio reader task) ──────────────────────
    // These fields are touched by the remote-connection tokio reader task
    // via `enqueue_output`. The tokio task buffers data and sets flags;
    // the GPUI thread drains and clears them.

    /// Buffer for remote-connection output. Written by the tokio reader
    /// task (`enqueue_output`), drained by the GPUI thread
    /// (`drain_pending_output` inside `with_content`). Decouples the tokio
    /// task from `term.lock()`, preventing lock contention that would
    /// freeze the UI.
    pending_output: Mutex<Vec<u8>>,

    /// Content-changed flag. Set by `process_output` (GPUI) and
    /// `enqueue_output` (tokio). Cleared by `take_dirty` (GPUI render).
    /// `AtomicBool` for lock-free cross-thread signaling.
    dirty: AtomicBool,

    /// Timestamp of last terminal output. Written by `process_output`
    /// (GPUI), `enqueue_output` (tokio), and `clear_waiting` (GPUI). Read
    /// by idle-detection methods on the GPUI thread.
    ///
    /// The `Arc` is historical — the value is never cloned; a plain `Mutex`
    /// would suffice since `Terminal` is already behind `Arc`.
    last_output_time: Arc<Mutex<Instant>>,

    // ── Atomics (lock-free render reads) ─────────────────────────────
    // These use atomics so the GPUI render path can read them without
    // taking a mutex on every frame.

    /// Monotonically-increasing counter bumped on every `process_output`,
    /// `drain_pending_output`, resize, scroll, and selection change. Used
    /// by `UrlDetector` and `SearchBar` to skip redundant work when
    /// content hasn't changed. GPUI thread only (despite being atomic —
    /// the atomic avoids locking, not cross-thread access).
    content_generation: AtomicU64,

    /// Cached "waiting for input" state. Written by the GPUI idle-check
    /// loop (`set_waiting_for_input`), read lock-free by renderers
    /// (`is_waiting_for_input`). Atomic avoids mutex overhead in the
    /// render hot path.
    waiting_for_input: AtomicBool,

    /// Whether the user has ever typed into this terminal. Set on
    /// `send_input` / `send_paste` / `send_raw_input` (GPUI thread), read
    /// lock-free by the idle-detection loop. Prevents flagging fresh
    /// terminals as idle before the user has interacted.
    had_user_input: AtomicBool,
}

impl Terminal {
    /// Create a new terminal
    pub fn new(
        terminal_id: String,
        size: TerminalSize,
        transport: Arc<dyn TerminalTransport>,
        initial_cwd: String,
    ) -> Self {
        // Use HollowBlock as a sentinel for "app has not set a cursor shape
        // via DECSCUSR" — no real DECSCUSR code maps to HollowBlock, so if
        // `cursor_style()` returns it we know to fall back to the user
        // setting instead of honoring an app override.
        let config = TermConfig {
            default_cursor_style: VteCursorStyle {
                shape: VteCursorShape::HollowBlock,
                blinking: false,
            },
            ..TermConfig::default()
        };
        let term_size = TermSize::new(size.cols as usize, size.rows as usize);

        // Create shared storage for OSC sequence handling and bell
        let title = Arc::new(Mutex::new(None));
        let has_bell = Arc::new(Mutex::new(false));
        let pending_clipboard = Arc::new(Mutex::new(Vec::new()));
        let palette = Arc::new(Mutex::new(None));
        let event_listener = ZedEventListener::new(
            title.clone(),
            has_bell.clone(),
            pending_clipboard.clone(),
            palette.clone(),
            transport.clone(),
            terminal_id.clone(),
        );
        let term = Term::new(config, &term_size, event_listener);

        let reported_cwd = Arc::new(Mutex::new(None));
        let pending_notifications = Arc::new(Mutex::new(Vec::new()));
        let osc_sidecar = Mutex::new(OscSidecar::new(
            reported_cwd.clone(),
            pending_notifications.clone(),
            transport.clone(),
            terminal_id.clone(),
        ));

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
                last_local_resize: std::time::Instant::now() - std::time::Duration::from_secs(1),
            })),
            transport,
            selection_state: Mutex::new(SelectionState::default()),
            scroll_offset: Mutex::new(0),
            title,
            has_bell,
            pending_clipboard,
            palette,
            pending_output: Mutex::new(Vec::new()),
            dirty: AtomicBool::new(false),
            content_generation: AtomicU64::new(0),
            initial_cwd,
            reported_cwd,
            pending_notifications,
            osc_sidecar,
            prompt_sidecar: Mutex::new(PromptSidecar::new()),
            prompt_tracker: Mutex::new(PromptTracker::new()),
            prompt_jump_index: Mutex::new(None),
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
        let mut sidecar = self.osc_sidecar.lock();
        let mut prompt_sidecar = self.prompt_sidecar.lock();
        let mut prompt_tracker = self.prompt_tracker.lock();

        let history_before = term.grid().history_size();

        // OSC 7 / OSC 9 / XTVERSION observer runs on the full chunk in one
        // pass — it never needs cursor-accurate positioning.
        sidecar.advance(data);

        // OSC 133 requires the main processor and the prompt sidecar to
        // advance in lockstep so we can snapshot the cursor at the exact
        // byte where each mark arrives. `advance_until_terminated` stops
        // the prompt sidecar at every OSC 133 so the main processor can
        // catch up before we read `grid.cursor.point`.
        advance_with_prompt_marks(
            &mut *term,
            &mut *processor,
            &mut prompt_sidecar,
            &mut prompt_tracker,
            data,
        );

        let history_after = term.grid().history_size();
        prompt_tracker.on_history_changed(
            history_before,
            history_after,
            term.grid().topmost_line().0,
        );

        // New output disengages the prompt-jump walker so the next
        // Above jump starts from the newest prompt again.
        *self.prompt_jump_index.lock() = None;

        self.dirty.store(true, Ordering::Relaxed);
        self.content_generation.fetch_add(1, Ordering::Relaxed);
        *self.last_output_time.lock() = Instant::now();
    }

    /// Enqueue output data for deferred processing.
    ///
    /// Used by the remote client's tokio reader thread so it never holds
    /// `term.lock()`. The pending data is drained and parsed on the GPUI
    /// thread just before rendering (see `with_content`).
    pub fn enqueue_output(&self, data: &[u8]) {
        self.pending_output.lock().extend_from_slice(data);
        self.dirty.store(true, Ordering::Relaxed);
        *self.last_output_time.lock() = Instant::now();
    }

    /// Drain all pending output and feed it into the terminal emulator.
    ///
    /// Called automatically by `with_content` before rendering.
    fn drain_pending_output(&self) {
        let data = {
            let mut pending = self.pending_output.lock();
            if pending.is_empty() {
                return;
            }
            std::mem::take(&mut *pending)
        };
        let mut term = self.term.lock();
        let mut processor = self.processor.lock();
        let mut sidecar = self.osc_sidecar.lock();
        let mut prompt_sidecar = self.prompt_sidecar.lock();
        let mut prompt_tracker = self.prompt_tracker.lock();

        let history_before = term.grid().history_size();
        sidecar.advance(&data);
        advance_with_prompt_marks(
            &mut *term,
            &mut *processor,
            &mut prompt_sidecar,
            &mut prompt_tracker,
            &data,
        );
        let history_after = term.grid().history_size();
        prompt_tracker.on_history_changed(
            history_before,
            history_after,
            term.grid().topmost_line().0,
        );
        self.content_generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Check if terminal has pending changes (and clear the flag).
    /// Used by PTY event loop for direct content pane notification.
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }

    /// Get the current content generation counter.
    pub fn content_generation(&self) -> u64 {
        self.content_generation.load(Ordering::Relaxed)
    }

    /// Send input to the PTY
    /// Automatically scrolls to bottom if scrolled into history
    pub fn send_input(&self, input: &str) {
        self.had_user_input.store(true, Ordering::Relaxed);
        self.scroll_to_bottom();
        self.transport.send_input(&self.terminal_id, input.as_bytes());
    }

    /// Send pasted text to the PTY, wrapping in bracketed paste sequences if the
    /// terminal application has enabled bracketed paste mode (DECSET 2004).
    /// This prevents shells from executing each line of a multi-line paste individually.
    pub fn send_paste(&self, text: &str) {
        self.had_user_input.store(true, Ordering::Relaxed);
        self.scroll_to_bottom();

        // Convert line endings: \r\n → \r, then \n → \r
        // Terminals send \r for Enter; shells in bracketed paste mode buffer these.
        let normalized = text.replace("\r\n", "\r").replace('\n', "\r");

        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        if bracketed {
            // Strip any embedded bracketed paste sequences to prevent escape injection
            let sanitized = normalized
                .replace("\x1b[200~", "")
                .replace("\x1b[201~", "");
            let mut buf = Vec::with_capacity(sanitized.len() + 12);
            buf.extend_from_slice(b"\x1b[200~");
            buf.extend_from_slice(sanitized.as_bytes());
            buf.extend_from_slice(b"\x1b[201~");
            self.transport.send_input(&self.terminal_id, &buf);
        } else {
            self.transport.send_input(&self.terminal_id, normalized.as_bytes());
        }
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
        self.start_selection(start_col, start_row, Side::Left);
        self.update_selection(end_col, end_row, Side::Right);
        self.end_selection();
    }

    /// Scroll to bottom (display_offset = 0)
    pub fn scroll_to_bottom(&self) {
        let mut term = self.term.lock();
        let current = term.grid().display_offset();
        if current > 0 {
            term.scroll_display(Scroll::Delta(-(current as i32)));
            self.content_generation.fetch_add(1, Ordering::Relaxed);
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
        {
            let mut rs = self.resize_state.lock();
            rs.size = new_size;
            rs.last_local_resize = std::time::Instant::now();
        }

        // Resize terminal grid immediately (independent mutex)
        let mut term = self.term.lock();
        let term_size = TermSize::new(new_size.cols as usize, new_size.rows as usize);
        term.resize(term_size);
        drop(term);

        self.content_generation.fetch_add(1, Ordering::Relaxed);

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
    ///
    /// Skips the resize if the client recently performed a local resize (within 200ms)
    /// to avoid redundant grid reflows from server echo during active resize drag.
    pub fn resize_grid_only(&self, cols: u16, rows: u16) {
        let rs = self.resize_state.lock();
        // Skip if we recently resized locally — the server is echoing back our own resize
        if rs.last_local_resize.elapsed().as_millis() < 200 {
            // Still accept if the size actually differs (e.g. server-initiated resize)
            if rs.size.cols == cols && rs.size.rows == rows {
                return;
            }
        }
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
        self.content_generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Mark the local side (origin) as resize authority. Process-global:
    /// any local keyboard/mouse input in any terminal claims authority for all
    /// terminals.
    pub fn claim_resize_local(&self) {
        claim_resize_authority_local();
    }

    /// Mark the remote side as resize authority. Called on the server when a
    /// remote client sends input to any terminal.
    pub fn claim_resize_remote(&self) {
        claim_resize_authority_remote();
    }

    /// Returns true if the local (origin) side currently has resize authority.
    /// The server's UI uses this to decide whether to push resize events to
    /// the PTY.
    pub fn is_resize_owner_local(&self) -> bool {
        is_resize_authority_local()
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

    /// Access the terminal content for rendering.
    ///
    /// Drains any pending output (enqueued by remote clients) before
    /// handing the content to the callback, so the rendered frame is
    /// always up-to-date.
    pub fn with_content<R>(&self, f: impl FnOnce(&Term<ZedEventListener>) -> R) -> R {
        self.drain_pending_output();
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
        self.content_generation.fetch_add(1, Ordering::Relaxed);
        // External scroll disengages the prompt-jump walker — the user's
        // implicit reference point has moved, so the next Above jump
        // should start over from the newest prompt.
        *self.prompt_jump_index.lock() = None;
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

    /// Take any pending OSC 52 clipboard writes. Called by the GPUI thread
    /// on each render; returns the texts to write to the system clipboard.
    pub fn take_pending_clipboard_writes(&self) -> Vec<String> {
        std::mem::take(&mut *self.pending_clipboard.lock())
    }

    /// Take any pending iTerm2-style `OSC 9 ; message` notifications. The
    /// GPUI thread drains these on each render to surface toasts or native
    /// desktop notifications for long-running commands that finished while
    /// the user was in another pane.
    pub fn take_pending_notifications(&self) -> Vec<String> {
        std::mem::take(&mut *self.pending_notifications.lock())
    }

    /// Push the active theme palette so the event listener can answer
    /// OSC 10/11/12/4 color queries with real theme colors. Called from the
    /// render loop on every frame; writes are cheap and uncontested.
    pub fn set_palette(&self, colors: okena_core::theme::ThemeColors) {
        *self.palette.lock() = Some(colors);
    }

    /// Return the OSC 8 hyperlink URI at the given visual cell, if any.
    /// `visual_row` is the on-screen row (0..screen_lines); scrolling is
    /// handled via `display_offset` so history cells work too.
    pub fn hyperlink_at(&self, col: usize, visual_row: i32) -> Option<String> {
        let term = self.term.lock();
        let display_offset = term.grid().display_offset() as i32;
        let buffer_row = visual_row - display_offset;
        let cell = &term.grid()[Point::new(Line(buffer_row), Column(col))];
        cell.hyperlink().map(|h| h.uri().to_owned())
    }

    /// Clear the bell notification flag (call when terminal receives focus)
    pub fn clear_bell(&self) {
        *self.has_bell.lock() = false;
    }

    /// Get the initial working directory for this terminal
    pub fn initial_cwd(&self) -> &str {
        &self.initial_cwd
    }

    /// Get the working directory most recently reported by the shell via
    /// `OSC 7 ; file://host/path`. Returns `None` until the shell has emitted
    /// at least one such sequence.
    pub fn reported_cwd(&self) -> Option<String> {
        self.reported_cwd.lock().clone()
    }

    /// Best known working directory for the shell running in this terminal.
    /// Prefers the shell-reported cwd (OSC 7) and falls back to the directory
    /// the PTY was originally spawned in. Use this when resolving relative
    /// paths, opening "new tab here", or syncing sidebar selection.
    pub fn current_cwd(&self) -> String {
        self.reported_cwd
            .lock()
            .clone()
            .unwrap_or_else(|| self.initial_cwd.clone())
    }

    /// Snapshot of the OSC 133 shell-integration marks currently tracked
    /// for this terminal, oldest first. Returns an empty Vec when the
    /// running shell has no OSC 133 support enabled.
    pub fn prompt_marks(&self) -> Vec<PromptMark> {
        self.prompt_tracker.lock().snapshot()
    }

    /// Scroll the viewport so the next older `OSC 133 ; A` prompt lands at
    /// visual row 0. The first call after any shell output lands on the
    /// most-recent prompt (even if it's already visible); each subsequent
    /// call walks one prompt further into history until there are none
    /// left, at which point `false` is returned.
    pub fn jump_to_prompt_above(&self) -> bool {
        self.jump_to_prompt(JumpDirection::Above)
    }

    /// Reverse of [`jump_to_prompt_above`]: walks one prompt forward toward
    /// the live bottom. Returns `false` when the walker is already sitting
    /// on the newest prompt or hasn't started walking yet.
    pub fn jump_to_prompt_below(&self) -> bool {
        self.jump_to_prompt(JumpDirection::Below)
    }

    fn jump_to_prompt(&self, direction: JumpDirection) -> bool {
        let marks = self.prompt_tracker.lock().snapshot();
        // Only `PromptStart` is a reliable "prompt begins here" marker.
        let prompts: Vec<&PromptMark> = marks
            .iter()
            .filter(|m| m.kind == PromptMarkKind::PromptStart)
            .collect();
        if prompts.is_empty() {
            return false;
        }

        // `prompt_jump_index` is a reverse index into `prompts`: 0 = newest,
        // 1 = one older, etc. `None` means "walker is not engaged; an
        // Above jump should land on the newest prompt". Storing a reverse
        // index keeps the walk scroll-invariant — scrolling rebases line
        // values on every mark, but the relative order and count don't
        // change.
        let new_index: usize = {
            let mut state = self.prompt_jump_index.lock();
            let next = match (direction, *state) {
                (JumpDirection::Above, None) => 0,
                (JumpDirection::Above, Some(n)) => {
                    if n + 1 >= prompts.len() {
                        return false;
                    }
                    n + 1
                }
                (JumpDirection::Below, Some(n)) if n > 0 => n - 1,
                (JumpDirection::Below, _) => return false,
            };
            *state = Some(next);
            next
        };

        let target = prompts[prompts.len() - 1 - new_index];
        let target_offset = (-target.line).max(0);

        // Scroll inline (bypassing self.scroll) so the jump walker state
        // isn't cleared — self.scroll() is reserved for externally-driven
        // scrolling which resets the walker.
        let mut term = self.term.lock();
        let current = term.grid().display_offset() as i32;
        let delta = target_offset - current;
        if delta != 0 {
            term.scroll_display(Scroll::Delta(delta));
            drop(term);
            *self.scroll_offset.lock() += delta;
            self.content_generation.fetch_add(1, Ordering::Relaxed);
        }
        true
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

    /// Returns true if the shell currently has a child process running.
    /// Performs a synchronous, low-overhead check (direct `/proc` read on Linux,
    /// `pgrep -P` fallback elsewhere) and is safe to call from UI event handlers.
    ///
    /// Note: `shell_pid` is expected to be the *real* shell pid, not a session
    /// proxy (dtach / tmux attach client). Session-backend resolution is done
    /// when the terminal is created (see `TerminalBackend::get_foreground_shell_pid`).
    pub fn has_running_child(&self) -> bool {
        match *self.shell_pid.lock() {
            Some(pid) => has_child_processes(pid),
            None => false,
        }
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
            let _display_offset = grid.display_offset() as i32;

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

                // Build byte-to-column mapping for converting byte offsets to grid columns.
                // Each char in line_text corresponds to exactly one grid column.
                let total_chars = line_text.chars().count();

                // Convert a byte offset to a column index
                let col_at_byte = |byte_offset: usize| -> usize {
                    line_text.char_indices()
                        .enumerate()
                        .find(|(_, (b, _))| *b == byte_offset)
                        .map(|(col, _)| col)
                        .unwrap_or(total_chars)
                };

                if let Some(ref regex) = regex {
                    // Regex search
                    for mat in regex.find_iter(&line_text) {
                        let col = col_at_byte(mat.start());
                        let end_col = col_at_byte(mat.end());
                        // Store absolute grid line (not display-relative)
                        matches.push((line, col, end_col - col));
                    }
                } else {
                    // Plain text search
                    let (search_text, query_text) = if case_sensitive {
                        (line_text.clone(), query.to_string())
                    } else {
                        (line_text.to_lowercase(), query.to_lowercase())
                    };

                    let query_char_len = query.chars().count();
                    let mut search_start = 0;
                    while let Some(pos) = search_text[search_start..].find(&query_text) {
                        let byte_pos = search_start + pos;
                        let col = col_at_byte(byte_pos);
                        // Store absolute grid line (not display-relative)
                        matches.push((line, col, query_char_len));
                        search_start = byte_pos + query_text.len();
                        if search_start >= search_text.len() {
                            break;
                        }
                    }
                }
            }
        });

        matches
    }

    /// Scan visible cells for OSC 8 hyperlinks.
    ///
    /// Returns one `DetectedLink` per contiguous run of cells sharing the same
    /// hyperlink id on the same visual row. Runs that share an id across rows
    /// (wrapped link labels) get the same `wrap_group`, so hover highlight
    /// covers both halves together.
    pub fn detect_hyperlinks(&self) -> Vec<DetectedLink> {
        let mut result = Vec::new();
        let mut id_to_group: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

        self.with_content(|term| {
            let grid = term.grid();
            let screen_lines = grid.screen_lines() as i32;
            let cols = grid.columns();
            let display_offset = grid.display_offset() as i32;

            for visual_row in 0..screen_lines {
                let buffer_line = visual_row - display_offset;
                let mut col = 0usize;
                while col < cols {
                    let cell = &grid[Point::new(Line(buffer_line), Column(col))];
                    let Some(hl) = cell.hyperlink() else {
                        col += 1;
                        continue;
                    };
                    let id = hl.id().to_owned();
                    let uri = hl.uri().to_owned();

                    let start_col = col;
                    col += 1;
                    while col < cols {
                        let next_cell = &grid[Point::new(Line(buffer_line), Column(col))];
                        match next_cell.hyperlink() {
                            Some(nh) if nh.id() == id => col += 1,
                            _ => break,
                        }
                    }
                    let len = col - start_col;

                    let next_group = id_to_group.len();
                    let link_group = *id_to_group.entry(id).or_insert(next_group);

                    result.push(DetectedLink {
                        line: visual_row,
                        col: start_col,
                        len,
                        text: uri,
                        file_line: None,
                        file_col: None,
                        is_url: true,
                        wrap_group: link_group,
                    });
                }
            }
        });

        result
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
        let mut next_wrap_group = 0usize;

        self.with_content(|term| {
            let grid = term.grid();
            let screen_lines = grid.screen_lines() as i32;
            let cols = grid.columns();
            let last_col = Column(cols - 1);
            let display_offset = grid.display_offset() as i32;

            // Helper: read a visual row from the grid as a String.
            let read_row = |vrow: i32| -> String {
                let buf = vrow - display_offset;
                let mut s = String::with_capacity(cols);
                for c in 0..cols {
                    s.push(grid[Point::new(Line(buf), Column(c))].c);
                }
                s
            };

            // Iterate over visual rows (0..screen_lines).
            // When scrolled, visual row 0 maps to buffer line (0 - display_offset).
            let mut visual_row = 0i32;
            while visual_row < screen_lines {
                let mut combined_text = String::new();
                // (visual_row, offset_in_combined, leading_spaces_stripped)
                let mut row_offsets: Vec<(i32, usize, usize)> = Vec::new();

                // Collect wrapped lines into one logical line
                loop {
                    let row_text = read_row(visual_row);

                    // Trim trailing spaces — URLs/paths never end with spaces,
                    // and this allows the regex to match across padded line breaks.
                    let rtrimmed = row_text.trim_end_matches(' ');

                    // For continuation rows, also strip leading spaces (TUI padding)
                    let (text_to_add, leading_stripped) = if combined_text.is_empty() {
                        (rtrimmed, 0usize)
                    } else {
                        let ltrimmed = rtrimmed.trim_start_matches(' ');
                        (ltrimmed, rtrimmed.len() - ltrimmed.len())
                    };

                    row_offsets.push((visual_row, combined_text.len(), leading_stripped));
                    combined_text.push_str(text_to_add);

                    let buffer_line = visual_row - display_offset;
                    let last_cell = &grid[Point::new(Line(buffer_line), last_col)];
                    let has_wrapline_flag = last_cell.flags.contains(Flags::WRAPLINE);

                    visual_row += 1;

                    // Only merge via terminal WRAPLINE flag.  TUI-managed
                    // wrapping (no WRAPLINE) is handled in Phase 2 below.
                    if !has_wrapline_flag || visual_row >= screen_lines {
                        break;
                    }
                }

                for mat in regex.find_iter(&combined_text) {
                    let raw = mat.as_str();
                    let trimmed = trim_url_trailing(raw);
                    if trimmed.is_empty() {
                        continue;
                    }

                    // Each regex match gets a unique wrap_group.
                    // Segments of a wrapped URL (same match, multiple rows) share it.
                    let wrap_group = next_wrap_group;
                    next_wrap_group += 1;

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
                        let (phys_row, row_start_offset, leading_stripped) = row_offsets[i];
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

                        let col_start = combined_text[row_start_offset..seg_start].chars().count() + leading_stripped;
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
                                wrap_group,
                            });
                        }
                    }
                }
            }

            // ── Phase 2: Extend URL matches at TUI-wrapped row boundaries ──
            //
            // Phase 1 only merges rows with the terminal WRAPLINE flag.  TUI
            // applications manage their own wrapping (no WRAPLINE), so a long
            // URL may be split across visual rows with only the first fragment
            // matched by the regex.
            //
            // Approach inspired by Kitty: for each URL that reaches the end of
            // visible content, strip leading whitespace from the next row and
            // consume URL-compatible chars.  No attempt to reverse-engineer TUI
            // decoration via common-prefix detection (too fragile).
            //
            // Guards against false positives:
            //  - URL must not start at col 0 (terminal would set WRAPLINE)
            //  - No alphabetic text before/after the URL (prose context)
            //  - Continuation must have alphanumeric chars (not just punctuation)
            //  - "Weak" continuations (no `/`) rejected if content has spaces
            //  - Continuation containing `://` means a new URL, not extension

            let phase1_len = matches.len();
            let mut idx = 0;
            while idx < phase1_len {
                let group = matches[idx].wrap_group;

                // Advance to the last segment of this wrap_group.
                let mut last_idx = idx;
                while last_idx + 1 < phase1_len
                    && matches[last_idx + 1].wrap_group == group
                {
                    last_idx += 1;
                }
                let next_idx = last_idx + 1;

                // Only extend URL matches (not file paths).
                if !matches[last_idx].is_url {
                    idx = next_idx;
                    continue;
                }

                // URL must start after col 0 — if the URL occupies the full
                // line without WRAPLINE, the lines are independent (the
                // terminal would have set WRAPLINE for a genuine wrap).
                let url_start_col = matches[idx].col;
                if url_start_col == 0 {
                    idx = next_idx;
                    continue;
                }

                // Skip rows with WRAPLINE (already handled by Phase 1).
                let m_line = matches[last_idx].line;
                let m_col = matches[last_idx].col;
                let m_len = matches[last_idx].len;
                let match_buf_line = m_line - display_offset;
                let match_last_cell =
                    &grid[Point::new(Line(match_buf_line), last_col)];
                if match_last_cell.flags.contains(Flags::WRAPLINE) {
                    idx = next_idx;
                    continue;
                }

                let match_row_text = read_row(m_line);
                let match_rtrimmed = match_row_text.trim_end();

                // URL must reach near the end of visible content.
                // TUIs may use a narrower layout than the terminal width.
                let trimmed_char_len = match_rtrimmed.chars().count();
                if m_col + m_len + 3 < trimmed_char_len {
                    idx = next_idx;
                    continue;
                }

                // No alphabetic text after the URL (prose context).
                let url_end_pos = m_col + m_len;
                let suffix_byte = match_rtrimmed
                    .char_indices()
                    .nth(url_end_pos)
                    .map_or(match_rtrimmed.len(), |(b, _)| b);
                if match_rtrimmed[suffix_byte..]
                    .chars()
                    .any(|c| c.is_alphabetic())
                {
                    idx = next_idx;
                    continue;
                }

                // ── Extension loop ──
                let mut extended_url = matches[last_idx].text.clone();
                let mut current_row = m_line;

                loop {
                    let next_row = current_row + 1;
                    if next_row >= screen_lines {
                        break;
                    }

                    let next_row_text = read_row(next_row);
                    let next_rtrimmed = next_row_text.trim_end();

                    // Strip leading whitespace (TUI indentation).
                    let content = next_rtrimmed.trim_start_matches(' ');
                    let indent = next_rtrimmed.len() - content.len();

                    if content.is_empty() {
                        break;
                    }

                    // Don't extend into a new URL scheme.
                    if content.starts_with("http://")
                        || content.starts_with("https://")
                        || content.starts_with("ftp://")
                        || content.starts_with("file://")
                        || content.starts_with("ssh://")
                        || content.starts_with("git://")
                    {
                        break;
                    }

                    // Take URL-compatible chars as extension.
                    let ext_char_len = content
                        .chars()
                        .take_while(|c| url_char(*c))
                        .count();
                    if ext_char_len == 0 {
                        break;
                    }
                    let ext_byte_len = content
                        .char_indices()
                        .nth(ext_char_len)
                        .map_or(content.len(), |(b, _)| b);
                    let ext_raw = &content[..ext_byte_len];

                    // Trim the FULL combined URL, not just the fragment,
                    // so balanced parens spanning the line break are
                    // handled correctly (e.g. `Rust_(pr` + `ogramming_language)`).
                    let candidate = format!("{}{}", extended_url, ext_raw);
                    let trimmed_full = trim_url_trailing(&candidate);
                    if trimmed_full.len() <= extended_url.len() {
                        break;
                    }
                    let ext_trimmed = &trimmed_full[extended_url.len()..];

                    // Must contain at least one alphanumeric character.
                    if !ext_trimmed.chars().any(|c| c.is_alphanumeric()) {
                        break;
                    }

                    // Pure alphabetic words (e.g. "remote", "next",
                    // "Press") are not URL continuations — URL path
                    // fragments always contain non-alpha chars (digits,
                    // `/`, `-`, `_`, `.`, etc.).
                    if ext_trimmed.chars().all(|c| c.is_alphabetic()) {
                        break;
                    }

                    // Remaining content has a URL scheme → new item.
                    let remaining = &content[ext_byte_len..];
                    if remaining.contains("://") {
                        break;
                    }

                    // "Weak" extension (no path separator `/`): only
                    // accept when the full content has no spaces.
                    // URLs never contain spaces; spaces mean prose.
                    // Exception: tokens with digits (UUIDs, hashes, IDs)
                    // are almost certainly URL content, not words.
                    if !ext_trimmed.contains('/')
                        && !ext_trimmed.chars().any(|c| c.is_ascii_digit())
                        && content.contains(' ')
                    {
                        break;
                    }

                    // Commit extension.
                    let ext_trimmed_len = ext_trimmed.len();
                    let ext_trimmed_chars = ext_trimmed.chars().count();
                    extended_url.push_str(ext_trimmed);

                    matches.push(DetectedLink {
                        line: next_row,
                        col: indent,
                        len: ext_trimmed_chars,
                        text: String::new(), // updated below
                        file_line: None,
                        file_col: None,
                        is_url: true,
                        wrap_group: group,
                    });

                    // If trim_url_trailing removed characters, the URL
                    // ended here (e.g. trailing `,`, `.`).
                    if ext_trimmed_len < ext_raw.len() {
                        break;
                    }

                    // Continue only if extension fills to near end of
                    // visible content on this row.
                    let next_trimmed_len =
                        next_rtrimmed.chars().count();
                    if indent + ext_char_len + 3 < next_trimmed_len {
                        break;
                    }
                    if !remaining.is_empty()
                        && remaining
                            .chars()
                            .any(|c| c.is_alphanumeric())
                    {
                        break;
                    }

                    current_row = next_row;
                }

                // Update text for all segments (original + extensions).
                if extended_url != matches[last_idx].text {
                    for m in matches.iter_mut() {
                        if m.wrap_group == group {
                            m.text.clone_from(&extended_url);
                        }
                    }
                }

                idx = next_idx;
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
            self.content_generation.fetch_add(1, Ordering::Relaxed);
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
        term.mode().intersects(TermMode::MOUSE_MODE)
    }

    /// Check if terminal is in application cursor keys mode (DECCKM)
    /// When enabled, arrow keys should send SS3 sequences (\x1bOA) instead of CSI (\x1b[A)
    /// This is used by applications like less, vim, htop, etc.
    pub fn is_app_cursor_mode(&self) -> bool {
        let term = self.term.lock();
        term.mode().contains(TermMode::APP_CURSOR)
    }

    /// Check if terminal is using the alternate screen buffer.
    /// TUI apps (vim, less, htop, Claude Code CLI) use alternate screen.
    pub fn is_alt_screen(&self) -> bool {
        let term = self.term.lock();
        term.mode().contains(TermMode::ALT_SCREEN)
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

    /// Cursor shape requested by the terminal application via DECSCUSR, if any.
    ///
    /// Returns `None` when the app has not overridden the shape (or has reset
    /// it with `\x1b[0 q`), so callers can fall back to the user setting.
    pub fn app_cursor_shape(&self) -> Option<AppCursorShape> {
        let term = self.term.lock();
        let style = term.cursor_style();
        match style.shape {
            VteCursorShape::HollowBlock => None,
            VteCursorShape::Block => Some(AppCursorShape::Block),
            VteCursorShape::Beam => Some(AppCursorShape::Bar),
            VteCursorShape::Underline => Some(AppCursorShape::Underline),
            VteCursorShape::Hidden => None,
        }
    }

    /// Cursor blinking flag from DECSCUSR, if the app has set a shape.
    ///
    /// Returns `None` when the app has not spoken (HollowBlock sentinel),
    /// so callers can fall back to the user's cursor_blink setting.
    pub fn app_cursor_blinking(&self) -> Option<bool> {
        let term = self.term.lock();
        let style = term.cursor_style();
        if style.shape == VteCursorShape::HollowBlock {
            None
        } else {
            Some(style.blinking)
        }
    }

    /// True if the active app has enabled focus event reporting (DEC mode 1004).
    pub fn wants_focus_events(&self) -> bool {
        let term = self.term.lock();
        term.mode().contains(TermMode::FOCUS_IN_OUT)
    }

    /// Send a focus-in (`\x1b[I`) or focus-out (`\x1b[O`) report to the PTY.
    /// Caller should gate on `wants_focus_events()`.
    pub fn send_focus(&self, focused: bool) {
        let bytes: &[u8] = if focused { b"\x1b[I" } else { b"\x1b[O" };
        self.send_bytes(bytes);
    }

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

/// Check if a process has any child processes.
///
/// On Linux, this reads `/proc/<pid>/task/*/children` directly — sub-millisecond,
/// safe to call synchronously from UI handlers (e.g. click / key-down).
/// On other Unix, falls back to `pgrep -P` (~5–20 ms fork+exec).
/// On non-Unix, always returns false.
#[cfg(target_os = "linux")]
pub fn has_child_processes(pid: u32) -> bool {
    let task_dir = format!("/proc/{}/task", pid);
    let Ok(entries) = std::fs::read_dir(&task_dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let Some(tid) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let path = format!("/proc/{}/task/{}/children", pid, tid);
        if let Ok(s) = std::fs::read_to_string(&path) {
            if !s.trim().is_empty() {
                return true;
            }
        }
    }
    false
}

#[cfg(all(unix, not(target_os = "linux")))]
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

    // Clear viewport, then clear scrollback history, then home cursor.
    // `\x1b[2J` alone scrolls the old viewport into history (alacritty's
    // `clear_viewport` calls `scroll_up`), so successive snapshots would stack
    // old content into the remote client's scrollback and the user would see
    // their output duplicated when scrolling up. `\x1b[3J` (ED 3 = erase saved
    // lines) drops the history alacritty just pushed, leaving a clean grid
    // before the snapshot body renders.
    buf.extend_from_slice(b"\x1b[2J\x1b[3J\x1b[H");

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

    /// Records every byte the sidecar writes back to the PTY so tests can
    /// assert on XTVERSION / DA / color responses.
    struct CapturingTransport {
        writes: Mutex<Vec<Vec<u8>>>,
    }

    impl CapturingTransport {
        fn new() -> Self {
            Self { writes: Mutex::new(Vec::new()) }
        }

        fn writes(&self) -> Vec<Vec<u8>> {
            self.writes.lock().clone()
        }
    }

    impl TerminalTransport for CapturingTransport {
        fn send_input(&self, _terminal_id: &str, data: &[u8]) {
            self.writes.lock().push(data.to_vec());
        }
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
    fn test_osc7_reports_cwd() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "test-id".to_string(),
            TerminalSize::default(),
            transport,
            "/tmp".to_string(),
        );

        assert_eq!(terminal.reported_cwd(), None);
        assert_eq!(terminal.current_cwd(), "/tmp");

        terminal.process_output(b"\x1b]7;file://myhost/home/matej/projects/okena\x1b\\");

        assert_eq!(
            terminal.reported_cwd().as_deref(),
            Some("/home/matej/projects/okena"),
        );
        assert_eq!(terminal.current_cwd(), "/home/matej/projects/okena");
    }

    #[test]
    fn test_osc7_percent_decoded() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport,
            "/tmp".into(),
        );

        terminal.process_output(b"\x1b]7;file:///home/user/My%20Projects/foo%20bar\x07");

        assert_eq!(
            terminal.reported_cwd().as_deref(),
            Some("/home/user/My Projects/foo bar"),
        );
    }

    #[test]
    fn test_osc7_empty_host() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport,
            "/tmp".into(),
        );

        terminal.process_output(b"\x1b]7;file:///home/user\x07");

        assert_eq!(terminal.reported_cwd().as_deref(), Some("/home/user"));
    }

    #[test]
    fn test_osc7_split_across_chunks() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport,
            "/tmp".into(),
        );

        terminal.process_output(b"\x1b]7;file:///home");
        assert_eq!(terminal.reported_cwd(), None);

        terminal.process_output(b"/user/proj\x07");
        assert_eq!(terminal.reported_cwd().as_deref(), Some("/home/user/proj"));
    }

    #[test]
    fn test_osc7_updates_on_cd() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport,
            "/tmp".into(),
        );

        terminal.process_output(b"\x1b]7;file:///a\x07");
        assert_eq!(terminal.reported_cwd().as_deref(), Some("/a"));

        terminal.process_output(b"\x1b]7;file:///b/c\x07");
        assert_eq!(terminal.reported_cwd().as_deref(), Some("/b/c"));
    }

    #[test]
    fn test_osc7_invalid_scheme_ignored() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport,
            "/tmp".into(),
        );

        terminal.process_output(b"\x1b]7;http://example/x\x07");
        assert_eq!(terminal.reported_cwd(), None);
    }

    #[test]
    fn test_osc133_prompt_start_captures_cursor_position() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport,
            "/tmp".into(),
        );

        // Two lines of output, then a prompt marker. After the newline
        // and carriage return the cursor sits at column 0 of line 2, and
        // that's where the prompt begins.
        terminal.process_output(b"hi\r\nok\r\n\x1b]133;A\x1b\\$ ");

        let marks = terminal.prompt_marks();
        assert_eq!(marks.len(), 1);
        let mark = marks[0];
        assert_eq!(mark.kind, PromptMarkKind::PromptStart);
        assert_eq!(mark.line, 2);
        assert_eq!(mark.column, 0);
    }

    #[test]
    fn test_osc133_all_four_kinds_captured_in_order() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport,
            "/tmp".into(),
        );

        // Full prompt lifecycle on one line: A (prompt) B (cmd input) C
        // (executing) D (done with exit code).
        terminal.process_output(
            b"\x1b]133;A\x1b\\$ \x1b]133;B\x1b\\ls\r\n\x1b]133;C\x1b\\output\r\n\x1b]133;D;0\x1b\\",
        );

        let marks = terminal.prompt_marks();
        assert_eq!(marks.len(), 4);
        assert_eq!(marks[0].kind, PromptMarkKind::PromptStart);
        assert_eq!(marks[1].kind, PromptMarkKind::CommandStart);
        assert_eq!(marks[2].kind, PromptMarkKind::CommandExecuted);
        assert_eq!(
            marks[3].kind,
            PromptMarkKind::CommandFinished { exit_code: Some(0) },
        );
    }

    #[test]
    fn test_osc133_d_parses_nonzero_exit_code() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport,
            "/tmp".into(),
        );

        terminal.process_output(b"\x1b]133;D;127\x1b\\");

        let marks = terminal.prompt_marks();
        assert_eq!(
            marks[0].kind,
            PromptMarkKind::CommandFinished { exit_code: Some(127) },
        );
    }

    #[test]
    fn test_osc133_d_without_exit_code_is_none() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport,
            "/tmp".into(),
        );

        terminal.process_output(b"\x1b]133;D\x1b\\");

        let marks = terminal.prompt_marks();
        assert_eq!(
            marks[0].kind,
            PromptMarkKind::CommandFinished { exit_code: None },
        );
    }

    #[test]
    fn test_osc133_ignores_unknown_kind() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport,
            "/tmp".into(),
        );

        // `E` is not a valid OSC 133 kind — must be dropped silently.
        terminal.process_output(b"\x1b]133;E\x1b\\");

        assert!(terminal.prompt_marks().is_empty());
    }

    #[test]
    fn test_osc133_split_across_chunks() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport,
            "/tmp".into(),
        );

        terminal.process_output(b"\x1b]133");
        assert!(terminal.prompt_marks().is_empty());
        terminal.process_output(b";A\x1b\\");

        let marks = terminal.prompt_marks();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].kind, PromptMarkKind::PromptStart);
    }

    #[test]
    fn test_osc133_marks_shift_when_content_scrolls() {
        // Small viewport so we can provoke scrollback growth without
        // flooding the test. 5 rows, 20 columns.
        let size = TerminalSize {
            cols: 20,
            rows: 5,
            cell_width: 8.0,
            cell_height: 16.0,
        };
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new("t".into(), size, transport, "/tmp".into());

        // Capture a prompt at the top of the viewport.
        terminal.process_output(b"\x1b]133;A\x1b\\$ ");
        assert_eq!(terminal.prompt_marks()[0].line, 0);

        // Push three lines of output — content scrolls, prompt should
        // still be tracked but at a lower line value (scrollback).
        terminal.process_output(b"\r\na\r\nb\r\nc\r\nd\r\ne");

        let marks = terminal.prompt_marks();
        assert_eq!(marks.len(), 1, "mark must survive scroll within cap");
        // After five linefeeds with a five-row viewport the original
        // prompt row is pushed one row into scrollback.
        assert!(
            marks[0].line < 0,
            "expected prompt to slide into scrollback, got {}",
            marks[0].line,
        );
    }

    #[test]
    fn test_jump_to_prompt_walks_through_history() {
        // Small viewport (5x20) so three prompts push older ones into
        // scrollback and jumping eventually lands in history.
        let size = TerminalSize {
            cols: 20,
            rows: 5,
            cell_width: 8.0,
            cell_height: 16.0,
        };
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new("t".into(), size, transport, "/tmp".into());

        // Three prompts with enough output between them to push the
        // oldest into scrollback.
        terminal.process_output(b"\x1b]133;A\x1b\\$ a\r\nout\r\nmore\r\n");
        terminal.process_output(b"\x1b]133;A\x1b\\$ b\r\nout\r\nmore\r\n");
        terminal.process_output(b"\x1b]133;A\x1b\\$ c\r\n");

        assert_eq!(terminal.prompt_marks().len(), 3);

        // Walk all the way back. Each press must succeed; at least one of
        // them must cross into scrollback.
        assert!(terminal.jump_to_prompt_above());
        assert!(terminal.jump_to_prompt_above());
        assert!(terminal.jump_to_prompt_above());
        assert!(
            terminal.display_offset() > 0,
            "after walking through all three prompts the display must be \
             scrolled into history, got offset {}",
            terminal.display_offset(),
        );

        // Fourth press has nothing older.
        assert!(!terminal.jump_to_prompt_above());
    }

    #[test]
    fn test_jump_to_prompt_above_stops_at_oldest() {
        let size = TerminalSize {
            cols: 20,
            rows: 5,
            cell_width: 8.0,
            cell_height: 16.0,
        };
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new("t".into(), size, transport, "/tmp".into());

        terminal.process_output(b"\x1b]133;A\x1b\\$ a\r\nout\r\n");
        terminal.process_output(b"\x1b]133;A\x1b\\$ b\r\n");

        // Two prompts → two Above presses succeed, third fails.
        assert!(terminal.jump_to_prompt_above()); // newest (index 0)
        assert!(terminal.jump_to_prompt_above()); // oldest (index 1)
        let before = terminal.display_offset();
        assert!(!terminal.jump_to_prompt_above()); // nothing older
        assert_eq!(terminal.display_offset(), before);
    }

    #[test]
    fn test_jump_to_prompt_below_reverses_walk() {
        let size = TerminalSize {
            cols: 20,
            rows: 5,
            cell_width: 8.0,
            cell_height: 16.0,
        };
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new("t".into(), size, transport, "/tmp".into());

        terminal.process_output(b"\x1b]133;A\x1b\\$ a\r\nout\r\nmore\r\n");
        terminal.process_output(b"\x1b]133;A\x1b\\$ b\r\nout\r\nmore\r\n");
        terminal.process_output(b"\x1b]133;A\x1b\\$ c\r\n");

        // Walk up to oldest prompt (3 presses: newest → middle → oldest).
        terminal.jump_to_prompt_above();
        terminal.jump_to_prompt_above();
        terminal.jump_to_prompt_above();
        let at_top = terminal.display_offset();

        // Step down once — must move strictly forward (smaller offset).
        assert!(terminal.jump_to_prompt_below());
        let step1 = terminal.display_offset();
        assert!(
            step1 < at_top,
            "below should reduce display offset ({step1} < {at_top})",
        );
    }

    #[test]
    fn test_jump_below_without_walker_is_noop() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport,
            "/tmp".into(),
        );

        terminal.process_output(b"\x1b]133;A\x1b\\");
        // No Above press yet — walker is disengaged, Below must no-op.
        assert!(!terminal.jump_to_prompt_below());
    }

    #[test]
    fn test_new_output_resets_walker() {
        let size = TerminalSize {
            cols: 20,
            rows: 5,
            cell_width: 8.0,
            cell_height: 16.0,
        };
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new("t".into(), size, transport, "/tmp".into());

        terminal.process_output(b"\x1b]133;A\x1b\\$ a\r\nout\r\n");
        terminal.process_output(b"\x1b]133;A\x1b\\$ b\r\n");

        // Engage the walker and step back.
        terminal.jump_to_prompt_above();
        terminal.jump_to_prompt_above();

        // New shell output must reset the walker — a subsequent Below
        // press has no walker to reverse, so it no-ops.
        terminal.process_output(b"fresh output\r\n");
        assert!(!terminal.jump_to_prompt_below());
    }

    #[test]
    fn test_jump_to_prompt_returns_false_without_marks() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport,
            "/tmp".into(),
        );

        assert!(!terminal.jump_to_prompt_above());
        assert!(!terminal.jump_to_prompt_below());
    }

    #[test]
    fn test_jump_to_prompt_ignores_non_prompt_kinds() {
        let size = TerminalSize {
            cols: 20,
            rows: 5,
            cell_width: 8.0,
            cell_height: 16.0,
        };
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new("t".into(), size, transport, "/tmp".into());

        // Only `C` and `D` kinds — jumping must still be a no-op because
        // PromptStart is the canonical "prompt begins here" signal.
        terminal.process_output(b"\x1b]133;C\x1b\\cmd\r\nout\r\n");
        terminal.process_output(b"\x1b]133;D;0\x1b\\");

        assert!(!terminal.jump_to_prompt_above());
    }

    #[test]
    fn test_osc133_ring_buffer_evicts_oldest() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport,
            "/tmp".into(),
        );

        // Drive 70 PromptStart marks through — ring capacity is 64, so
        // the 6 oldest must be evicted and the newest kept.
        for _ in 0..70 {
            terminal.process_output(b"\x1b]133;A\x1b\\");
        }

        let marks = terminal.prompt_marks();
        assert_eq!(marks.len(), 64);
        assert!(marks.iter().all(|m| m.kind == PromptMarkKind::PromptStart));
    }

    #[test]
    fn test_parse_osc133_kind() {
        assert_eq!(parse_osc133_kind(b'A', &[]), Some(PromptMarkKind::PromptStart));
        assert_eq!(parse_osc133_kind(b'B', &[]), Some(PromptMarkKind::CommandStart));
        assert_eq!(parse_osc133_kind(b'C', &[]), Some(PromptMarkKind::CommandExecuted));
        assert_eq!(
            parse_osc133_kind(b'D', &[b"42"]),
            Some(PromptMarkKind::CommandFinished { exit_code: Some(42) }),
        );
        // Non-numeric extra params mean "unknown exit".
        assert_eq!(
            parse_osc133_kind(b'D', &[b"aid=abc"]),
            Some(PromptMarkKind::CommandFinished { exit_code: None }),
        );
        assert_eq!(parse_osc133_kind(b'Z', &[]), None);
    }

    #[test]
    fn test_osc9_notification_collected() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport,
            "/tmp".into(),
        );

        terminal.process_output(b"\x1b]9;Build complete\x07");

        let pending = terminal.take_pending_notifications();
        assert_eq!(pending, vec!["Build complete".to_string()]);
        // Second drain is empty (consumed).
        assert!(terminal.take_pending_notifications().is_empty());
    }

    #[test]
    fn test_osc9_multiple_notifications_queued() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport,
            "/tmp".into(),
        );

        terminal.process_output(b"\x1b]9;first\x07\x1b]9;second\x07");

        assert_eq!(
            terminal.take_pending_notifications(),
            vec!["first".to_string(), "second".to_string()],
        );
    }

    #[test]
    fn test_osc9_empty_message_ignored() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport,
            "/tmp".into(),
        );

        // Empty body should not queue a blank toast.
        terminal.process_output(b"\x1b]9;\x07");
        terminal.process_output(b"\x1b]9;   \x07");

        assert!(terminal.take_pending_notifications().is_empty());
    }

    #[test]
    fn test_osc9_split_across_chunks() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport,
            "/tmp".into(),
        );

        terminal.process_output(b"\x1b]9;Long ");
        assert!(terminal.take_pending_notifications().is_empty());
        terminal.process_output(b"running job done\x07");

        assert_eq!(
            terminal.take_pending_notifications(),
            vec!["Long running job done".to_string()],
        );
    }

    #[test]
    fn test_osc9_st_terminator() {
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport,
            "/tmp".into(),
        );

        // ST-terminated form (ESC \) is equally valid.
        terminal.process_output(b"\x1b]9;hello\x1b\\");

        assert_eq!(
            terminal.take_pending_notifications(),
            vec!["hello".to_string()],
        );
    }

    #[test]
    fn test_xtversion_responds_with_okena_name() {
        set_app_version("0.20.0-test");

        let transport = Arc::new(CapturingTransport::new());
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport.clone(),
            "/tmp".into(),
        );

        // XTVERSION query: `CSI > q` with empty Ps.
        terminal.process_output(b"\x1b[>q");

        let writes = transport.writes();
        assert_eq!(writes.len(), 1, "expected exactly one PTY response");
        let body = std::str::from_utf8(&writes[0]).unwrap();
        // Response must be `DCS > | okena(<version>) ST` and start with ESC P.
        assert!(body.starts_with("\x1bP>|okena("), "got: {body:?}");
        assert!(body.ends_with("\x1b\\"), "got: {body:?}");
        // The version slot is filled from whatever was injected first; since
        // set_app_version uses OnceLock, we can't rely on the exact string
        // across tests. Assert that *some* non-empty version is reported.
        assert!(body.contains("okena("), "got: {body:?}");
        assert!(!body.contains("okena()"), "version must not be empty: {body:?}");
    }

    #[test]
    fn test_xtversion_ignores_nonzero_ps() {
        // `CSI > 1 q` is NOT XTVERSION — xterm uses it for unrelated
        // reporting modes. We must stay silent, otherwise we corrupt
        // whatever the real handler expects.
        set_app_version("0.20.0-test");

        let transport = Arc::new(CapturingTransport::new());
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport.clone(),
            "/tmp".into(),
        );

        terminal.process_output(b"\x1b[>1q");

        assert!(
            transport.writes().is_empty(),
            "non-zero Ps must not trigger a response: {:?}",
            transport.writes(),
        );
    }

    #[test]
    fn test_xtversion_ignores_unrelated_csi() {
        set_app_version("0.20.0-test");

        let transport = Arc::new(CapturingTransport::new());
        let terminal = Terminal::new(
            "t".into(),
            TerminalSize::default(),
            transport.clone(),
            "/tmp".into(),
        );

        // Cursor positioning and SGR must not trip the sidecar.
        terminal.process_output(b"\x1b[1;1H\x1b[31mhello\x1b[0m");

        assert!(transport.writes().is_empty());
    }

    #[test]
    fn test_xterm_cube_corners() {
        // First cube entry (16) is true black, last (231) is true white.
        assert_eq!(xterm_256_cube_rgb(16), (0, 0, 0));
        assert_eq!(xterm_256_cube_rgb(231), (255, 255, 255));
    }

    #[test]
    fn test_xterm_cube_axis_order() {
        // Pure blue axis: changing only the blue component.
        assert_eq!(xterm_256_cube_rgb(17), (0, 0, 95));
        assert_eq!(xterm_256_cube_rgb(18), (0, 0, 135));
        assert_eq!(xterm_256_cube_rgb(21), (0, 0, 255));

        // Pure green axis: next cube row.
        assert_eq!(xterm_256_cube_rgb(22), (0, 95, 0));
        assert_eq!(xterm_256_cube_rgb(28), (0, 135, 0));

        // Pure red axis: next cube plane.
        assert_eq!(xterm_256_cube_rgb(52), (95, 0, 0));
        assert_eq!(xterm_256_cube_rgb(88), (135, 0, 0));
    }

    #[test]
    fn test_xterm_cube_mixed() {
        // xterm's canonical value for 208 is the familiar "orange" (#ff8700).
        assert_eq!(xterm_256_cube_rgb(208), (255, 135, 0));
        // 196 = pure bright red (#ff0000).
        assert_eq!(xterm_256_cube_rgb(196), (255, 0, 0));
        // 226 = pure bright yellow (#ffff00).
        assert_eq!(xterm_256_cube_rgb(226), (255, 255, 0));
    }

    #[test]
    fn test_xterm_grayscale_endpoints() {
        // First grayscale step is 8 (just above black); last is 238.
        assert_eq!(xterm_256_grayscale_rgb(232), (8, 8, 8));
        assert_eq!(xterm_256_grayscale_rgb(255), (238, 238, 238));
    }

    #[test]
    fn test_xterm_grayscale_linear() {
        // Each step adds 10 to every channel.
        assert_eq!(xterm_256_grayscale_rgb(233), (18, 18, 18));
        assert_eq!(xterm_256_grayscale_rgb(244), (128, 128, 128));
    }

    #[test]
    fn test_parse_osc7_file_uri() {
        assert_eq!(
            parse_osc7_file_uri("file:///home/user").as_deref(),
            Some("/home/user"),
        );
        assert_eq!(
            parse_osc7_file_uri("file://host/home/user").as_deref(),
            Some("/home/user"),
        );
        assert_eq!(
            parse_osc7_file_uri("file:///path/with%20space").as_deref(),
            Some("/path/with space"),
        );
        assert_eq!(parse_osc7_file_uri("http://example/x"), None);
        assert_eq!(parse_osc7_file_uri("file://host-without-path"), None);
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

    // The resize authority is process-global; these tests share a mutex so
    // they don't observe each other's writes.
    static RESIZE_AUTH_TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    #[test]
    fn resize_owner_defaults_to_local() {
        let _g = RESIZE_AUTH_TEST_LOCK.lock();
        reset_resize_authority();
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, String::new());
        assert!(terminal.is_resize_owner_local());
    }

    #[test]
    fn resize_owner_transitions() {
        let _g = RESIZE_AUTH_TEST_LOCK.lock();
        reset_resize_authority();
        let transport = Arc::new(NullTransport);
        let terminal = Terminal::new("t".into(), TerminalSize::default(), transport, String::new());

        terminal.claim_resize_remote();
        assert!(!terminal.is_resize_owner_local());

        terminal.claim_resize_local();
        assert!(terminal.is_resize_owner_local());
    }

    #[test]
    fn resize_owner_is_process_global() {
        let _g = RESIZE_AUTH_TEST_LOCK.lock();
        reset_resize_authority();
        let transport = Arc::new(NullTransport);
        let term_a = Terminal::new("a".into(), TerminalSize::default(), transport.clone(), String::new());
        let term_b = Terminal::new("b".into(), TerminalSize::default(), transport, String::new());

        // Claiming remote on A flips authority for B as well.
        term_a.claim_resize_remote();
        assert!(!term_b.is_resize_owner_local());

        // Claiming local on B flips authority back for A.
        term_b.claim_resize_local();
        assert!(term_a.is_resize_owner_local());
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

    /// Helper: create a terminal and write text to it, returns detected URLs
    fn detect_urls_in(text: &str, cols: u16) -> Vec<DetectedLink> {
        let transport = Arc::new(NullTransport);
        let size = TerminalSize { cols, rows: 24, cell_width: 8.0, cell_height: 16.0 };
        let terminal = Terminal::new("test".into(), size, transport, "/tmp".into());
        terminal.process_output(text.as_bytes());
        terminal.detect_urls()
    }

    #[test]
    fn detect_url_wrapped_with_padding() {
        // TUI writes a URL with a decoration prefix, URL fills nearly the
        // whole row, then continues on next line with matching indentation.
        // No WRAPLINE flag — the TUI manages wrapping itself.
        // Row 1: "- https://claude.ai/code/sess_ABC" (33 chars)
        // Row 2: "  DEF123" + padding
        // cols=36 so row 1 is nearly full (33+3 >= 36).
        let links = detect_urls_in(
            "- https://claude.ai/code/sess_ABC\r\n  DEF123\r\n",
            36,
        );
        assert_eq!(links.len(), 2, "URL spans two rows: {:?}", links);
        assert_eq!(links[0].text, "https://claude.ai/code/sess_ABCDEF123");
        assert_eq!(links[0].col, 2);
        assert_eq!(links[1].text, "https://claude.ai/code/sess_ABCDEF123");
        assert_eq!(links[1].col, 2);
        assert_eq!(links[1].line, 1);
    }

    #[test]
    fn detect_url_wrapped_with_leading_padding() {
        // TUI adds leading spaces on the continuation line for alignment
        // Row 1: "  https://claude.ai/code/sess_ABC" (33 chars) + padding
        // Row 2: "  DEF123" + padding
        // cols=36 so row 1 is nearly full (33+3 >= 36).
        let links = detect_urls_in(
            "  https://claude.ai/code/sess_ABC\r\n  DEF123\r\n",
            36,
        );
        assert_eq!(links.len(), 2, "URL spans two rows: {:?}", links);
        assert_eq!(links[0].text, "https://claude.ai/code/sess_ABCDEF123");
        assert_eq!(links[0].col, 2); // starts after 2 spaces
        assert_eq!(links[1].text, "https://claude.ai/code/sess_ABCDEF123");
        assert_eq!(links[1].col, 2); // continuation also at col 2
        assert_eq!(links[1].line, 1);
    }

    #[test]
    fn detect_url_not_wrapped_when_next_line_more_indented() {
        // Next line has more leading spaces than the first line —
        // the extra indentation means it's NOT a URL continuation.
        // Reproduces: "   1. zkusí https://api.postmarkapp.com\n      (oficiální API)"
        let links = detect_urls_in(
            "   1. text https://api.postmarkapp.com\r\n      (next line)\r\n",
            50,
        );
        assert_eq!(links.len(), 1, "URL should NOT merge with next line: {:?}", links);
        assert_eq!(links[0].text, "https://api.postmarkapp.com");
    }

    #[test]
    fn detect_url_single_line_not_affected() {
        // Single-line URL should still work normally
        let links = detect_urls_in(
            "visit https://example.com/path here\r\n",
            80,
        );
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].text, "https://example.com/path");
        assert_eq!(links[0].col, 6);
        assert_eq!(links[0].line, 0);
    }

    #[test]
    fn detect_duplicate_urls_get_different_wrap_groups() {
        // Same URL on two separate lines should get different wrap_groups
        // so hovering one doesn't highlight the other.
        let links = detect_urls_in(
            "https://github.com/org/repo/pull/381\r\n\
             https://github.com/org/repo/pull/381\r\n",
            80,
        );
        assert_eq!(links.len(), 2, "Should detect two URLs: {:?}", links);
        assert_ne!(
            links[0].wrap_group, links[1].wrap_group,
            "Duplicate URLs must have different wrap_groups for independent hover"
        );
    }

    #[test]
    fn detect_duplicate_urls_separated_by_blank_line() {
        // Same URL separated by a blank line
        let links = detect_urls_in(
            "https://github.com/org/repo/pull/381\r\n\
             \r\n\
             https://github.com/org/repo/pull/381\r\n",
            80,
        );
        assert_eq!(links.len(), 2, "Should detect two URLs: {:?}", links);
        assert_ne!(
            links[0].wrap_group, links[1].wrap_group,
            "Duplicate URLs must have different wrap_groups"
        );
    }

    #[test]
    fn detect_duplicate_url_wrapped_then_whole() {
        // First URL wraps across two lines (TUI-style padding),
        // second URL appears whole on a later line.
        // This reproduces the real scenario from PR creation output.
        let url = "https://github.com/contember/webmaster/pull/381";
        let links = detect_urls_in(
            &format!(
                "Summary\r\n\
                 prefix {url}\r\n\
                 \r\n\
                 PR created:\r\n\
                 {url}\r\n"
            ),
            50,
        );
        let url_links: Vec<&DetectedLink> = links.iter()
            .filter(|l| l.text == url)
            .collect();
        // Wrapped URL produces 2 segments + standalone URL = 3 total
        assert!(url_links.len() >= 3, "Expected wrapped (2 segments) + standalone (1): {:?}", url_links);
        let wrapped_group = url_links[0].wrap_group;
        // All wrapped segments share the same group
        assert_eq!(url_links[0].wrap_group, url_links[1].wrap_group,
            "Wrapped segments should share wrap_group");
        // Standalone URL has a different group
        let standalone = url_links.last().unwrap();
        assert_ne!(wrapped_group, standalone.wrap_group,
            "Standalone URL must have different wrap_group than wrapped one");
    }

    #[test]
    fn detect_duplicate_url_after_colon_prefix() {
        // "PR created:" ends with ':' which is a url_char.
        // The next line starts with a URL. Visual wrap detection should NOT
        // merge them — or if it does, they must still get different wrap_groups.
        let url = "https://github.com/org/repo/pull/381";
        let links = detect_urls_in(
            &format!(
                "{url}\r\n\
                 \r\n\
                 PR created:\r\n\
                 {url}\r\n"
            ),
            80,
        );
        let url_links: Vec<&DetectedLink> = links.iter()
            .filter(|l| l.text == url)
            .collect();
        assert_eq!(url_links.len(), 2, "Should have exactly 2 URL matches: {:?}", url_links);
        assert_ne!(
            url_links[0].wrap_group, url_links[1].wrap_group,
            "URLs must have different wrap_groups even when preceded by colon"
        );
    }

    #[test]
    fn detect_url_not_wrapped_when_next_line_starts_with_word() {
        // "Press ENTER..." is natural language text, not a URL continuation.
        // The URL ends with alphanumeric chars and "Press" starts with one,
        // but the word-followed-by-space heuristic should prevent merging.
        let links = detect_urls_in(
            "Login at:\r\nhttps://www.npmjs.com/login?next=/login/cli/d907c402-4ad4-474c-a183-16ae52157acf\r\nPress ENTER to open in the browser...\r\n",
            100,
        );
        assert_eq!(links.len(), 1, "Should detect exactly one URL: {:?}", links);
        assert_eq!(
            links[0].text,
            "https://www.npmjs.com/login?next=/login/cli/d907c402-4ad4-474c-a183-16ae52157acf"
        );
    }

    #[test]
    fn detect_url_not_wrapped_when_next_line_word_after_wrapline() {
        // URL wraps via WRAPLINE (fills terminal width), then next line
        // after the wrap tail starts with a word — should not merge.
        let url = "https://www.npmjs.com/login?next=/login/cli/d907c402-4ad4-474c-a183-16ae52157acf";
        let links = detect_urls_in(
            &format!("{url}\r\nPress ENTER to open in the browser...\r\n"),
            60, // force URL to wrap via WRAPLINE
        );
        let url_links: Vec<&DetectedLink> = links.iter()
            .filter(|l| l.text == url)
            .collect();
        assert!(
            !url_links.is_empty(),
            "Should detect the URL: {:?}",
            links
        );
        // "Press" should NOT be part of any detected link
        assert!(
            links.iter().all(|l| !l.text.contains("Press")),
            "No link should contain 'Press': {:?}",
            links
        );
    }

    #[test]
    fn detect_url_not_merged_with_remote_prefix() {
        // Git push output: URL on a line that doesn't fill the terminal width.
        // The "remote:" on the next line must NOT be merged as a continuation.
        let links = detect_urls_in(
            "remote:       https://github.com/contember/dotaz/pull/new/fixes\r\nremote:\r\n",
            80,
        );
        assert_eq!(links.len(), 1, "Should detect exactly one URL: {:?}", links);
        assert_eq!(links[0].text, "https://github.com/contember/dotaz/pull/new/fixes");
    }

    #[test]
    fn detect_url_not_merged_with_label_suffix() {
        // Even when the URL line nearly fills the terminal, a continuation
        // ending with ':' (label pattern) must not be merged.
        let links = detect_urls_in(
            "https://github.com/contember/dotaz/pull/new/fixes\r\nremote:\r\n",
            52, // URL is 50 chars, nearly fills 52-col terminal
        );
        assert_eq!(links.len(), 1, "Label-like 'remote:' must not be merged: {:?}", links);
        assert_eq!(links[0].text, "https://github.com/contember/dotaz/pull/new/fixes");
    }

    #[test]
    fn detect_url_wrapped_with_trailing_text() {
        // URL wraps across two lines, continuation line has non-URL text after
        // the URL part (e.g. " — S3 bucket").  The first token of the
        // continuation contains '/' so it should still be recognised as a URL
        // continuation.
        let links = detect_urls_in(
            "    - #61 https://github.com/contember/npi-infrastru\r\n    cture/pull/61 \u{2014} S3 bucket\r\n",
            55,
        );
        let url_links: Vec<&DetectedLink> = links.iter()
            .filter(|l| l.text == "https://github.com/contember/npi-infrastructure/pull/61")
            .collect();
        assert!(
            !url_links.is_empty(),
            "Should detect the full wrapped URL: {:?}",
            links
        );
    }

    #[test]
    fn detect_url_wrapped_tui_narrow_layout() {
        // TUI uses a narrower layout than the terminal width.
        // URL doesn't reach the terminal edge but does reach the end
        // of the TUI's visible content.  Phase 2 should still extend.
        // Terminal is 55 cols, but TUI content only uses ~42 cols.
        let links = detect_urls_in(
            "\u{2514}  https://github.com/NPI-Cloud/npi-inf\r\n   rastructure/pull/64\r\n",
            55,
        );
        let url_links: Vec<&DetectedLink> = links.iter()
            .filter(|l| l.text == "https://github.com/NPI-Cloud/npi-infrastructure/pull/64")
            .collect();
        assert!(
            url_links.len() >= 2,
            "URL should span two rows even when TUI layout is narrower than terminal: {:?}",
            links
        );
    }

    #[test]
    fn detect_url_not_extended_by_list_marker() {
        // URL on its own line followed by a list item starting with "- ".
        // The "-" is a url_char but it's a list marker, not a URL
        // continuation.  Must not extend.
        let links = detect_urls_in(
            "  https://github.com/contember/dotaz/pull/2\r\n  - Format check passes\r\n",
            55,
        );
        assert_eq!(links.len(), 1, "Should not extend into list marker: {:?}", links);
        assert_eq!(links[0].text, "https://github.com/contember/dotaz/pull/2");
    }

    #[test]
    fn detect_url_extension_stops_after_trailing_trim() {
        // URL continuation ends with ")" which gets trimmed.  The "2." on
        // the following line must NOT be absorbed as another extension.
        // Simulates prose: "...npi-inf +\nrastructure/pull/65)\n2. https://..."
        let links = detect_urls_in(
            "  https://github.com/NPI-Cloud/npi-inf\r\n  rastructure/pull/65)\r\n  2. next item\r\n",
            42,
        );
        let url_links: Vec<&DetectedLink> = links.iter()
            .filter(|l| l.text.starts_with("https://github.com/NPI-Cloud/npi-inf"))
            .collect();
        // Should have 2 segments (line 0 + line 1), NOT 3
        assert_eq!(
            url_links.len(), 2,
            "Should not extend past trimmed ')' into '2.': {:?}",
            links
        );
        assert_eq!(
            url_links[0].text,
            "https://github.com/NPI-Cloud/npi-infrastructure/pull/65"
        );
    }

    #[test]
    fn detect_url_not_extended_into_numbered_list_item() {
        // Numbered list where each item has a URL.  The `2` from "2. https://..."
        // must NOT be absorbed as a continuation of the first URL.
        let links = detect_urls_in(
            "1. https://github.com/contember/dotaz/pull/2\r\n2. https://github.com/NPI-Cloud/npi-infrastr\r\n   ucture/pull/65\r\n",
            46,
        );
        // First URL should be exactly pull/2, not pull/22
        let first: Vec<&DetectedLink> = links.iter()
            .filter(|l| l.text == "https://github.com/contember/dotaz/pull/2")
            .collect();
        assert!(
            !first.is_empty(),
            "First URL should be pull/2, not absorb '2' from next line: {:?}",
            links
        );
        // Second URL should also be detected
        let second: Vec<&DetectedLink> = links.iter()
            .filter(|l| l.text.contains("npi-infrastructure/pull/65"))
            .collect();
        assert!(
            !second.is_empty(),
            "Second URL should be detected: {:?}",
            links
        );
    }

    #[test]
    fn detect_url_not_extended_by_prose_word() {
        // URL on a dash-list line, next line is also a dash-list item
        // with prose text.  "next" is a url_char word but must NOT be
        // absorbed as URL continuation.
        let links = detect_urls_in(
            "- https://github.com/contember/dotaz/pull/2\r\n- next item without URL\r\n",
            46,
        );
        assert_eq!(links.len(), 1, "Should not extend into 'next': {:?}", links);
        assert_eq!(links[0].text, "https://github.com/contember/dotaz/pull/2");
    }

    #[test]
    fn detect_url_uuid_continuation_with_trailing_prose() {
        // URL wraps mid-UUID, continuation line has prose after the UUID
        // fragment.  The UUID part (digits + hex letters + dashes) must
        // still be recognised as URL continuation despite spaces in the
        // line — digits distinguish it from a prose word.
        let links = detect_urls_in(
            "  http://localhost:19400/s/1f41d02d-6105-45fb-b3\r\n  b1-4b56ae4d869f \u{2014} take your time.\r\n",
            50,
        );
        let url_links: Vec<&DetectedLink> = links.iter()
            .filter(|l| l.text == "http://localhost:19400/s/1f41d02d-6105-45fb-b3b1-4b56ae4d869f")
            .collect();
        assert!(
            url_links.len() >= 2,
            "UUID continuation should be detected across wrapped lines: {:?}",
            links
        );
    }
}
