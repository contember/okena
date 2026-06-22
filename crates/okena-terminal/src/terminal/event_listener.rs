use alacritty_terminal::event::{Event as TermEvent, EventListener};
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::transport::TerminalTransport;
use super::types::{ClipboardReadResponder, ResizeState};

/// The two OSC 52 clipboard queues the listener shares with `Terminal`.
///
/// Both are `Arc`-shared with the owning `Terminal` (which holds the same
/// `Arc`s as separate fields) and pushed here during `process_output`, then
/// drained on the GPUI thread. They are grouped into one struct so the
/// listener constructor stays compact as more shared state is added.
pub(super) struct ClipboardQueues {
    /// Pending OSC 52 clipboard writes to be picked up by the GPUI thread.
    pub writes: Arc<Mutex<Vec<String>>>,
    /// Pending OSC 52 clipboard *read* requests (`OSC 52 ; c ; ?`), each
    /// carrying a formatter that turns clipboard text into the PTY reply.
    /// Drained on the GPUI thread, where the system clipboard and the opt-in
    /// setting gating reads are reachable.
    pub reads: Arc<Mutex<Vec<ClipboardReadResponder>>>,
}

/// The current terminal state the listener reads to answer terminal queries.
///
/// Both fields are `Arc`-shared with the owning `Terminal` (which holds the
/// same `Arc`s) and read here during `process_output` to compose replies:
/// `palette` answers OSC 10/11/12/4 color queries, `resize_state` answers the
/// `CSI 14 t` (XTWINOPS) text-area-size-in-pixels query. They are grouped into
/// one struct so the listener constructor stays compact as more shared state
/// is added.
pub(super) struct CurrentState {
    /// Current theme palette, pushed from the GPUI thread on each render.
    /// Used to answer OSC 10/11/12/4 color queries from apps.
    pub palette: Arc<Mutex<Option<okena_core::theme::ThemeColors>>>,
    /// Current terminal size, kept up to date by `resize`. Read to answer the
    /// `CSI 14 t` XTWINOPS query (report text-area size in pixels).
    pub resize_state: Arc<Mutex<ResizeState>>,
}

/// Event listener for alacritty_terminal that captures title changes, bell, and PTY write requests
pub struct ZedEventListener {
    /// Shared title storage - OSC 0/1/2 sequences update this
    title: Arc<Mutex<Option<String>>>,
    /// Sticky bell flag for the UI (red border / sidebar dot), cleared on focus.
    has_bell: Arc<Mutex<bool>>,
    /// One-shot "the bell rang since the last drain" edge, consumed by the PTY
    /// event loop to fire a desktop notification exactly once per bell rather
    /// than on every batch while `has_bell` stays set.
    bell_pending: Arc<AtomicBool>,
    /// Pending OSC 52 clipboard write/read queues, shared with `Terminal`.
    clipboard: ClipboardQueues,
    /// Current state the listener reads to answer terminal queries (theme
    /// palette for color queries, size for the XTWINOPS size query).
    state: CurrentState,
    /// Transport for writing responses back to the terminal
    transport: Arc<dyn TerminalTransport>,
    /// Terminal ID for PTY write operations
    terminal_id: String,
}

impl ZedEventListener {
    pub(super) fn new(
        title: Arc<Mutex<Option<String>>>,
        has_bell: Arc<Mutex<bool>>,
        bell_pending: Arc<AtomicBool>,
        clipboard: ClipboardQueues,
        state: CurrentState,
        transport: Arc<dyn TerminalTransport>,
        terminal_id: String,
    ) -> Self {
        Self {
            title,
            has_bell,
            bell_pending,
            clipboard,
            state,
            transport,
            terminal_id,
        }
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

        let palette = self.state.palette.lock();
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
pub(super) fn xterm_256_cube_rgb(index: usize) -> (u8, u8, u8) {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let n = index - 16;
    (LEVELS[n / 36], LEVELS[(n / 6) % 6], LEVELS[n % 6])
}

/// xterm 24-step grayscale ramp for palette indices 232..=255. The levels
/// start at 8 and step by 10 (8, 18, ..., 238), skipping true black and
/// true white — apps that need those use cube indices 16 and 231.
pub(super) fn xterm_256_grayscale_rgb(index: usize) -> (u8, u8, u8) {
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
                self.bell_pending.store(true, Ordering::Relaxed);
            }
            TermEvent::ClipboardStore(_, text) => {
                self.clipboard.writes.lock().push(text);
            }
            TermEvent::ClipboardLoad(_ty, formatter) => {
                // An app asked to READ the clipboard (`OSC 52 ; c ; ?`). We
                // can't read the system clipboard here (no `cx` on this
                // thread), so queue the formatter; the GPUI thread drains it
                // later, gated behind the opt-in `allow_clipboard_read`
                // setting. The `ClipboardType` is ignored — primary-selection
                // requests are answered from the regular clipboard like the
                // rest, which matches what most terminals do.
                self.clipboard.reads.lock().push(formatter);
            }
            TermEvent::ColorRequest(index, response_fn) => {
                if let Some((r, g, b)) = self.resolve_color(index) {
                    let reply =
                        response_fn(alacritty_terminal::vte::ansi::Rgb { r, g, b });
                    self.transport.send_input(&self.terminal_id, reply.as_bytes());
                }
            }
            TermEvent::TextAreaSizeRequest(formatter) => {
                // Answer `CSI 14 t` (report text-area size in pixels). alacritty
                // hands us the formatter; we supply the current geometry. Cell
                // dims are f32 pixels in TerminalSize; round to the nearest whole
                // pixel (min 1) for the u16 WindowSize.
                let size = self.state.resize_state.lock().size;
                let window_size = alacritty_terminal::event::WindowSize {
                    num_lines: size.rows,
                    num_cols: size.cols,
                    cell_width: (size.cell_width.round() as u16).max(1),
                    cell_height: (size.cell_height.round() as u16).max(1),
                };
                let reply = formatter(window_size);
                self.transport.send_input(&self.terminal_id, reply.as_bytes());
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
