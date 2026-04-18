use alacritty_terminal::event::{Event as TermEvent, EventListener};
use parking_lot::Mutex;
use std::sync::Arc;

use super::transport::TerminalTransport;

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
