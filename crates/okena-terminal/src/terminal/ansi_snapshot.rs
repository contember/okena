use alacritty_terminal::grid::{Dimensions, GridCell, Row};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color, NamedColor};

use super::Terminal;
use super::event_listener::ZedEventListener;
use super::modes::TerminalModeState;
use super::scrollback::HistoryView;

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

/// Lines of scrollback an owned snapshot carries, so a client that streams a
/// terminal again (the phone switching back to it) keeps its recent history.
const SNAPSHOT_HISTORY_LINES: usize = 200;

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
pub(super) fn grid_to_ansi(view: &HistoryView<'_, ZedEventListener>) -> Vec<u8> {
    write_grid(view, SnapshotTarget::OwnedScreen)
}

impl Terminal {
    /// Snapshot for a client that draws into a screen it already owns, such as
    /// a TUI inside the user's own terminal. Replay cannot leave the host's
    /// alternate screen, erase its scrollback, or install the remote's modes.
    pub fn render_snapshot_for_host_screen(&self) -> Vec<u8> {
        self.with_history(|view| write_grid(view, SnapshotTarget::HostScreen))
    }
}

fn write_grid(view: &HistoryView<'_, ZedEventListener>, target: SnapshotTarget) -> Vec<u8> {
    let owns_screen = target == SnapshotTarget::OwnedScreen;
    let term = view.term();
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
        // a clean grid before the snapshot body renders. The SGR reset comes
        // first: the erase and every line scrolled in take the current colours,
        // which may be left over from the client's earlier output.
        buf.extend_from_slice(b"\x1b[0m\x1b[2J\x1b[3J\x1b[H");
    }

    let mut current = SgrState::default();

    if owns_screen && !term.mode().contains(TermMode::ALT_SCREEN) {
        write_history(&mut buf, view, &mut current);
    }

    for row in 0..screen_lines as i32 {
        // Position cursor at start of row
        write_csi_pos(&mut buf, row + 1, 1);

        if !owns_screen {
            // A host screen can be wider than this grid; drop the stale tail,
            // with default colours so the erase paints no background.
            buf.extend_from_slice(b"\x1b[0m\x1b[K");
            current = SgrState::default();
        }

        write_row(&mut buf, &grid[Line(row)], &mut current, cols);
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

/// Replay the newest [`SNAPSHOT_HISTORY_LINES`] of scrollback as lines that
/// scroll off the top, leaving them in the client's history above a blank
/// screen for the grid body to draw on.
fn write_history<L>(buf: &mut Vec<u8>, view: &HistoryView<'_, L>, current: &mut SgrState) {
    let lines = view.history_size().min(SNAPSHOT_HISTORY_LINES);
    if lines == 0 {
        return;
    }
    let cols = view.columns();
    // The line feeds must scroll the whole screen: a scroll region the client
    // kept from earlier output would hold the lines back from its history.
    // Autowrap continues soft-wrapped rows; the mode restore at the end of the
    // snapshot puts the source's setting back.
    buf.extend_from_slice(b"\x1b[r\x1b[?7h");
    let mut continues_wrap = false;
    for back in (1..=lines).rev() {
        let row = view.row(Line(-(back as i32)));
        // The newest history row always ends in a line feed: the line-feed
        // count below assumes the cursor stands below the last history row.
        let wraps = back > 1 && row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
        let end = if wraps {
            // A wide char that did not fit left a spacer in the last cell;
            // without it the client wraps the same way and sets its own.
            let last = &row[Column(cols - 1)];
            if last.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
                cols - 1
            } else {
                cols
            }
        } else {
            // The blank tail costs bytes and nothing else.
            let end = (0..cols)
                .rposition(|col| !row[Column(col)].is_empty())
                .map_or(0, |last| last + 1);
            let first_bg = row[Column(0)].bg;
            if continues_wrap && (end == 0 || first_bg != Color::Named(NamedColor::Background)) {
                // The wrap into this row happens on its first glyph, and a wrap
                // at the bottom scrolls in a line with that glyph's background:
                // write the whole row so no cell keeps it by mistake.
                cols
            } else {
                end
            }
        };
        write_row(buf, &row, current, end);
        continues_wrap = wraps;
        if wraps {
            continue;
        }
        // Lines scrolled in take the current background, so end it first.
        if *current != SgrState::default() {
            buf.extend_from_slice(b"\x1b[0m");
            *current = SgrState::default();
        }
        buf.extend_from_slice(b"\r\n");
    }
    // At most `screen_lines - 1` rows are still on screen; scroll them off.
    for _ in 1..view.screen_lines() {
        buf.push(b'\n');
    }
}

/// Write the first `end` cells of a row, switching SGR only where it changes.
fn write_row(buf: &mut Vec<u8>, row: &Row<Cell>, current: &mut SgrState, end: usize) {
    let default_fg = Color::Named(NamedColor::Foreground);
    let default_bg = Color::Named(NamedColor::Background);

    for col_idx in 0..end {
        let cell = &row[Column(col_idx)];

        // Skip wide char spacer cells
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
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

        if desired != *current {
            emit_sgr(buf, &desired);
            *current = desired;
        }

        // Write the cell's glyph: base char, then the zero-width marks
        // stacked on it. The marks advance no column on the replaying end.
        let mut utf8_buf = [0u8; 4];
        // A tab cell is blank: replaying `\t` would jump to the next tab stop
        // and shift the rest of the row.
        let base = if cell.c == '\0' || cell.c == '\t' {
            ' '
        } else {
            cell.c
        };
        buf.extend_from_slice(base.encode_utf8(&mut utf8_buf).as_bytes());
        for mark in cell.zerowidth().into_iter().flatten() {
            buf.extend_from_slice(mark.encode_utf8(&mut utf8_buf).as_bytes());
        }
    }
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

    use super::SNAPSHOT_HISTORY_LINES;

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
        assert!(owned.contains("\x1b[0m\x1b[2J\x1b[3J\x1b[H"));
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

    /// Both targets share the same grid body. Only the owned snapshot restores
    /// screen/mode framing and the title; the host path adds row erasure.
    #[test]
    fn the_targets_differ_only_by_framing_title_and_row_erase() {
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
                "\x1b[?1049l\x1b[?6l\x1b[0m\x1b[2J\x1b[3J\x1b[H{body}{}\x1b]2;\x1b\\",
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

    #[test]
    fn owned_snapshot_restores_idle_title_without_new_output() {
        let source = terminal("source");
        source.process_output("\x1b]2;Agent — idle\x07text".as_bytes());
        assert_eq!(source.title().as_deref(), Some("Agent — idle"));
        let mirror = terminal("mirror");
        mirror.process_output(&source.render_snapshot());
        assert_eq!(mirror.title().as_deref(), Some("Agent — idle"));
        assert_eq!(first_row(&mirror), "text");
    }

    #[test]
    fn owned_snapshot_clears_stale_title_after_source_reset() {
        let source = terminal("source");
        let mirror = terminal("mirror");
        mirror.process_output(b"\x1b]2;stale\x07");
        mirror.process_output(&source.render_snapshot());
        assert_eq!(mirror.title().unwrap_or_default(), "");
    }

    /// Glyph and colours of every history row, oldest first.
    fn history_cells(
        terminal: &Terminal,
    ) -> Vec<Vec<(char, alacritty_terminal::vte::ansi::Color)>> {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Line, Point};

        terminal.with_content(|term| {
            let grid = term.grid();
            (1..=grid.history_size() as i32)
                .rev()
                .map(|back| {
                    (0..grid.columns())
                        .map(|col| {
                            let cell = &grid[Point::new(Line(-back), Column(col))];
                            (cell.c, cell.bg)
                        })
                        .collect()
                })
                .collect()
        })
    }

    /// Glyph and background of every screen row.
    fn screen_cells(terminal: &Terminal) -> Vec<Vec<(char, alacritty_terminal::vte::ansi::Color)>> {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Line, Point};

        terminal.with_content(|term| {
            let grid = term.grid();
            (0..grid.screen_lines() as i32)
                .map(|row| {
                    (0..grid.columns())
                        .map(|col| {
                            let cell = &grid[Point::new(Line(row), Column(col))];
                            (cell.c, cell.bg)
                        })
                        .collect()
                })
                .collect()
        })
    }

    /// Which history rows, oldest first, continue on the next row.
    fn history_wraps(terminal: &Terminal) -> Vec<bool> {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Line, Point};
        use alacritty_terminal::term::cell::Flags;

        terminal.with_content(|term| {
            let grid = term.grid();
            let last = Column(grid.columns() - 1);
            (1..=grid.history_size() as i32)
                .rev()
                .map(|back| {
                    grid[Point::new(Line(-back), last)]
                        .flags
                        .contains(Flags::WRAPLINE)
                })
                .collect()
        })
    }

    fn screen_text(terminal: &Terminal) -> Vec<String> {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Line, Point};

        terminal.with_content(|term| {
            let grid = term.grid();
            (0..grid.screen_lines() as i32)
                .map(|row| {
                    (0..grid.columns())
                        .map(|col| grid[Point::new(Line(row), Column(col))].c)
                        .collect::<String>()
                        .trim_end()
                        .to_string()
                })
                .collect()
        })
    }

    fn print_lines(terminal: &Terminal, count: usize) {
        for i in 0..count {
            terminal.process_output(format!("line {i}\r\n").as_bytes());
        }
    }

    #[test]
    fn an_owned_snapshot_replaces_stale_history_with_the_newest_lines() {
        let source = terminal("source");
        print_lines(&source, 300);
        let mirror = terminal("mirror");
        // What the client kept from streaming the terminal earlier.
        mirror.process_output(b"stale 1\r\nstale 2\r\n");
        print_lines(&mirror, 40);

        mirror.process_output(&source.render_snapshot());

        let source_history = history_cells(&source);
        assert!(source_history.len() > SNAPSHOT_HISTORY_LINES);
        assert_eq!(
            history_cells(&mirror),
            source_history[source_history.len() - SNAPSHOT_HISTORY_LINES..]
        );
        assert_eq!(screen_text(&mirror), screen_text(&source));
    }

    #[test]
    fn an_owned_snapshot_carries_a_short_history_whole() {
        let source = terminal("source");
        print_lines(&source, 30);
        let mirror = terminal("mirror");

        mirror.process_output(&source.render_snapshot());

        assert_eq!(history_cells(&mirror), history_cells(&source));
        assert_eq!(screen_text(&mirror), screen_text(&source));
    }

    #[test]
    fn history_colours_stay_on_their_own_rows() {
        let source = terminal("source");
        // Enough rows before it that the blue row's line feed scrolls when it
        // is replayed, then plain rows after it.
        print_lines(&source, 40);
        source.process_output(b"\x1b[44mblue\x1b[K\r\n");
        source.process_output(b"\x1b[0m");
        print_lines(&source, 40);
        let mirror = terminal("mirror");
        // A colour left over from what the client streamed before.
        mirror.process_output(b"\x1b[41mstreamed in red");

        mirror.process_output(&source.render_snapshot());

        assert_eq!(history_cells(&mirror), history_cells(&source));
        assert_eq!(screen_cells(&mirror), screen_cells(&source));
    }

    #[test]
    fn soft_wrapped_history_rows_stay_wrapped() {
        use alacritty_terminal::grid::Dimensions;

        let source = terminal("source");
        let cols = source.with_content(|term| term.grid().columns());
        print_lines(&source, 10);
        // A long line, and one whose continuation is only blanks.
        source.process_output(format!("{}\r\n", "w".repeat(cols + 20)).as_bytes());
        source.process_output(format!("{}{}\r\n", "b".repeat(cols), " ".repeat(5)).as_bytes());
        // A wrap whose continuation starts on a coloured background.
        source.process_output(format!("{}\x1b[42mg\x1b[0m\r\n", "c".repeat(cols)).as_bytes());
        print_lines(&source, 40);
        assert!(history_wraps(&source).iter().filter(|&&w| w).count() >= 3);
        let mirror = terminal("mirror");

        mirror.process_output(&source.render_snapshot());

        assert_eq!(history_wraps(&mirror), history_wraps(&source));
        assert_eq!(history_cells(&mirror), history_cells(&source));
        assert_eq!(screen_text(&mirror), screen_text(&source));
    }

    #[test]
    fn tab_cells_keep_the_rest_of_the_row_in_place() {
        let source = terminal("source");
        source.process_output(b"\tindented\r\nx\ty\tz\r\n");
        print_lines(&source, 40);
        source.process_output(b"\ton screen");
        let mirror = terminal("mirror");

        mirror.process_output(&source.render_snapshot());

        let blank_tabs = |rows: Vec<Vec<(char, alacritty_terminal::vte::ansi::Color)>>| {
            rows.into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|(c, bg)| (if c == '\t' { ' ' } else { c }, bg))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(history_cells(&mirror), blank_tabs(history_cells(&source)));
        assert_eq!(screen_cells(&mirror), blank_tabs(screen_cells(&source)));
    }

    #[test]
    fn a_wide_char_that_did_not_fit_wraps_the_same_way() {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Line, Point};
        use alacritty_terminal::term::cell::Flags;

        let leading_spacers = |terminal: &Terminal| {
            terminal.with_content(|term| {
                let grid = term.grid();
                let last = Column(grid.columns() - 1);
                (1..=grid.history_size() as i32)
                    .filter(|back| {
                        grid[Point::new(Line(-back), last)]
                            .flags
                            .contains(Flags::LEADING_WIDE_CHAR_SPACER)
                    })
                    .count()
            })
        };
        let source = terminal("source");
        let cols = source.with_content(|term| term.grid().columns());
        let wrapped_wide = format!("{}\x1b[45m\u{6f22}\x1b[0m\r\n", "x".repeat(cols - 1));
        source.process_output(wrapped_wide.as_bytes());
        print_lines(&source, 40);
        // The same on the visible grid, where rows are drawn in place.
        source.process_output(wrapped_wide.as_bytes());
        assert_eq!(leading_spacers(&source), 1);
        let mirror = terminal("mirror");

        mirror.process_output(&source.render_snapshot());

        assert_eq!(leading_spacers(&mirror), 1);
        assert_eq!(history_wraps(&mirror), history_wraps(&source));
        assert_eq!(history_cells(&mirror), history_cells(&source));
        assert_eq!(screen_cells(&mirror), screen_cells(&source));
    }

    #[test]
    fn a_stale_scroll_region_on_the_client_does_not_hold_history_back() {
        let source = terminal("source");
        print_lines(&source, 60);
        let mirror = terminal("mirror");
        mirror.process_output(b"\x1b[3;6r");

        mirror.process_output(&source.render_snapshot());

        assert_eq!(history_cells(&mirror), history_cells(&source));
        assert_eq!(screen_text(&mirror), screen_text(&source));
    }

    #[test]
    fn an_alternate_screen_snapshot_carries_no_history() {
        let source = terminal("source");
        print_lines(&source, 60);
        source.process_output(b"\x1b[?1049h\x1b[Hfull screen app");

        let bytes = source.render_snapshot();

        assert!(!String::from_utf8_lossy(&bytes).contains("line 1"));
        let mirror = terminal("mirror");
        mirror.process_output(&bytes);
        assert_eq!(first_row(&mirror), "full screen app");
    }

    #[test]
    fn host_snapshot_preserves_the_host_title() {
        let source = terminal("source");
        source.process_output(b"\x1b]2;remote\x07text");
        let host = terminal("host");
        host.process_output(b"\x1b]2;local\x07");
        host.process_output(&source.render_snapshot_for_host_screen());
        assert_eq!(host.title().as_deref(), Some("local"));
    }
}
