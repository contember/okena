use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color, NamedColor};

use super::Terminal;
use super::event_listener::ZedEventListener;
use super::modes::TerminalModeState;

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

/// Which screen the snapshot bytes are replayed into.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SnapshotTarget {
    /// A terminal the snapshot owns outright: it selects the screen buffer,
    /// erases it and restores the remote's modes.
    OwnedScreen,
    /// A screen the caller already owns and restores itself: grid body only.
    HostScreen,
}

/// Serialize the visible terminal grid to ANSI escape sequences.
pub(super) fn grid_to_ansi(term: &Term<ZedEventListener>) -> Vec<u8> {
    write_grid(term, SnapshotTarget::OwnedScreen)
}

impl Terminal {
    /// Snapshot for a client that draws into a screen it already owns, such as
    /// a TUI inside the user's own terminal. Replay cannot leave the host's
    /// alternate screen, erase its scrollback, or install the remote's modes.
    pub fn render_snapshot_for_host_screen(&self) -> Vec<u8> {
        self.drain_pending_output();
        let term = self.term.lock();
        write_grid(&term, SnapshotTarget::HostScreen)
    }
}

fn write_grid(term: &Term<ZedEventListener>, target: SnapshotTarget) -> Vec<u8> {
    let owns_screen = target == SnapshotTarget::OwnedScreen;
    let grid = term.grid();
    let screen_lines = grid.screen_lines();
    let cols = grid.columns();
    let cursor = term.grid().cursor.point;
    let modes = TerminalModeState::from_term_mode(term.mode());

    // Generous pre-allocation
    let mut buf = Vec::with_capacity(screen_lines * cols * 4);

    if owns_screen {
        // Select the same screen buffer, then disable origin mode while drawing
        // so absolute cursor positions address the full viewport.
        modes.write_screen_selection(&mut buf);
        buf.extend_from_slice(b"\x1b[?6l");

        // Clear viewport, then clear scrollback history, then home cursor.
        // `\x1b[2J` alone scrolls the old viewport into history (alacritty's
        // `clear_viewport` calls `scroll_up`), so successive snapshots would
        // stack old content into the remote client's scrollback and the user
        // would see their output duplicated when scrolling up. `\x1b[3J` (ED 3
        // = erase saved lines) drops the history alacritty just pushed, leaving
        // a clean grid before the snapshot body renders.
        buf.extend_from_slice(b"\x1b[2J\x1b[3J\x1b[H");
    }

    let default_fg = Color::Named(NamedColor::Foreground);
    let default_bg = Color::Named(NamedColor::Background);

    let mut current = SgrState::default();

    for row in 0..screen_lines as i32 {
        // Position cursor at start of row
        write_csi_pos(&mut buf, row + 1, 1);

        if !owns_screen {
            // A host screen can be wider than this grid; drop the stale tail,
            // with default colours so the erase paints no background.
            buf.extend_from_slice(b"\x1b[0m\x1b[K");
            current = SgrState::default();
        }

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
                fg: if cell.fg == default_fg {
                    None
                } else {
                    Some(cell.fg)
                },
                bg: if cell.bg == default_bg {
                    None
                } else {
                    Some(cell.bg)
                },
            };

            if desired != current {
                emit_sgr(&mut buf, &desired);
                current = desired;
            }

            // Write the cell's glyph: base char, then the zero-width marks
            // stacked on it. The marks advance no column on the replaying end.
            let mut utf8_buf = [0u8; 4];
            let base = if cell.c == '\0' { ' ' } else { cell.c };
            buf.extend_from_slice(base.encode_utf8(&mut utf8_buf).as_bytes());
            for mark in cell.zerowidth().into_iter().flatten() {
                buf.extend_from_slice(mark.encode_utf8(&mut utf8_buf).as_bytes());
            }

            col_idx += 1;
        }
    }

    // Reset attributes
    buf.extend_from_slice(b"\x1b[0m");

    // Position cursor
    write_csi_pos(&mut buf, cursor.line.0 + 1, cursor.column.0 as i32 + 1);

    // Snapshots initialize fresh clients and repair stale modes on reconnect.
    if owns_screen {
        buf.extend_from_slice(&modes.to_ansi());
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
        Some(if is_fg {
            90 + (code - 8)
        } else {
            100 + (code - 8)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::terminal::tests::NullTransport;
    use crate::terminal::{Terminal, TerminalModeState, TerminalSize};

    fn terminal(id: &str) -> Terminal {
        Terminal::new(
            id.into(),
            TerminalSize::default(),
            Arc::new(NullTransport),
            "/tmp".into(),
        )
    }

    fn first_row(terminal: &Terminal) -> String {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Line, Point};

        terminal.with_content(|term| {
            let grid = term.grid();
            (0..grid.columns())
                .map(|col| grid[Point::new(Line(0), Column(col))].c)
                .collect::<String>()
                .trim_end()
                .to_string()
        })
    }

    #[test]
    fn host_screen_snapshot_omits_screen_and_mode_control() {
        let source = terminal("source");
        source.process_output(b"\x1b[?1002h\x1b[?1006hhello");

        let bytes = source.render_snapshot_for_host_screen();
        let text = String::from_utf8_lossy(&bytes);

        assert!(!text.contains("\x1b[?1049"), "selects a screen buffer");
        assert!(!text.contains("\x1b[3J"), "erases saved lines");
        assert!(!text.contains("\x1b[2J"), "erases the viewport");
        assert!(!text.contains("\x1b[?1002"), "replays mouse modes");
        assert!(!text.contains("\x1b[?6"), "changes origin mode");
        assert!(!text.contains("\x1b[="), "replays kitty keyboard flags");
        assert!(text.contains("hello"), "loses the grid body");

        let owned_bytes = source.render_snapshot();
        let owned = String::from_utf8_lossy(&owned_bytes);
        assert!(owned.contains("\x1b[?1049l"));
        assert!(owned.contains("\x1b[2J\x1b[3J\x1b[H"));
        assert!(owned.contains("\x1b[?1002h"));
        assert!(!owned.contains("\x1b[K"), "host-only erase leaked");
    }

    #[test]
    fn host_screen_snapshot_erases_the_stale_tail_of_each_row() {
        use alacritty_terminal::grid::Dimensions;

        let source = terminal("source");
        source.process_output(b"\x1b[31mhi");

        let bytes = source.render_snapshot_for_host_screen();
        let text = String::from_utf8_lossy(&bytes);
        let rows = source.with_content(|term| term.grid().screen_lines());

        assert_eq!(text.matches("\x1b[0m\x1b[K").count(), rows);
        assert!(text.contains("\x1b[1;1H\x1b[0m\x1b[K\x1b[0;31mhi"));
    }

    /// The owned path must stay byte-identical to what every other client
    /// already replays: framing around the same body, plus the row erase.
    #[test]
    fn the_targets_differ_only_by_framing_and_the_row_erase() {
        let source = terminal("source");
        source.process_output(b"\x1b[?1002hplain content\r\nsecond line");

        let host_bytes = source.render_snapshot_for_host_screen();
        let body = String::from_utf8_lossy(&host_bytes).replace("\x1b[0m\x1b[K", "");
        let modes =
            source.with_content(|term| TerminalModeState::from_term_mode(term.mode()).to_ansi());
        let owned_bytes = source.render_snapshot();

        assert_eq!(
            String::from_utf8_lossy(&owned_bytes),
            format!(
                "\x1b[?1049l\x1b[?6l\x1b[2J\x1b[3J\x1b[H{body}{}",
                String::from_utf8_lossy(&modes)
            )
        );
    }

    #[test]
    fn host_screen_snapshot_leaves_host_screen_and_modes_intact() {
        let source = terminal("source");
        source.process_output(b"remote output");

        let host = terminal("host");
        host.process_output(b"\x1b[?1049h\x1b[?1002h");
        host.process_output(&source.render_snapshot_for_host_screen());

        assert!(host.is_alt_screen(), "host left the alternate screen");
        assert!(host.is_mouse_mode(), "host lost mouse reporting");
        assert_eq!(first_row(&host), "remote output");
    }

    /// The mark of a decomposed grapheme lives beside `cell.c`, not in it.
    fn first_row_cells(terminal: &Terminal, count: usize) -> Vec<(char, Vec<char>)> {
        use alacritty_terminal::index::{Column, Line, Point};

        terminal.with_content(|term| {
            let grid = term.grid();
            (0..count)
                .map(|col| {
                    let cell = &grid[Point::new(Line(0), Column(col))];
                    (cell.c, cell.zerowidth().unwrap_or_default().to_vec())
                })
                .collect()
        })
    }

    #[test]
    fn a_snapshot_carries_combining_marks_and_their_columns() {
        let source = terminal("source");
        source.process_output("e\u{0301}x".as_bytes());
        assert_eq!(
            first_row_cells(&source, 2),
            vec![('e', vec!['\u{0301}']), ('x', Vec::new())],
            "alacritty stores the mark beside the base char"
        );

        let bytes = source.render_snapshot();
        assert!(
            String::from_utf8_lossy(&bytes).contains("e\u{0301}x"),
            "the snapshot dropped the combining mark"
        );

        let mirror = terminal("mirror");
        mirror.process_output(&bytes);
        assert_eq!(
            first_row_cells(&mirror, 2),
            vec![('e', vec!['\u{0301}']), ('x', Vec::new())],
            "the mirror lost the mark or shifted a column"
        );
    }

    #[test]
    fn a_snapshot_carries_a_mark_standing_over_a_blank_cell() {
        let source = terminal("source");
        source.process_output(" \u{0301}x".as_bytes());

        let mirror = terminal("mirror");
        mirror.process_output(&source.render_snapshot());

        assert_eq!(
            first_row_cells(&mirror, 2),
            vec![(' ', vec!['\u{0301}']), ('x', Vec::new())]
        );
    }

    #[test]
    fn owned_screen_snapshot_still_seizes_the_screen() {
        let source = terminal("source");
        source.process_output(b"remote output");

        let mirror = terminal("mirror");
        mirror.process_output(b"\x1b[?1049h\x1b[?1002h");
        mirror.process_output(&source.render_snapshot());

        assert!(!mirror.is_alt_screen());
        assert!(!mirror.is_mouse_mode());
        assert_eq!(first_row(&mirror), "remote output");
    }
}
