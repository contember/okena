//! Plain-text export of a line range, for the native "Select text" mode.
//!
//! The native views show a frozen copy of the scrollback in a platform text
//! view (native handles, loupe, edit menu), which needs the text as one
//! string. Lines use the absolute addressing of `render.rs`.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::Flags;

use crate::client::terminal_holder::{MobileEventListener, RenderTrackSnapshot};

/// Upper bound on lines one export reads (the views ask for less).
pub const MAX_EXPORT_LINES: i64 = 20_000;

/// Text of a line range plus where a given line landed in it.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct RangeText {
    pub text: String,
    /// Number of `\n` before the logical line that holds the anchor line.
    pub anchor_newlines: u32,
}

/// The text of absolute lines `start..=end`, clamped to the readable range
/// (at most [`MAX_EXPORT_LINES`], keeping the newest).
///
/// Soft-wrapped rows (`WRAPLINE`) are joined into one line, trailing blanks
/// of each line are trimmed, wide-character spacers are skipped and
/// combining marks kept. Blank lines at the end are dropped. `anchor` is an
/// absolute line whose position the caller wants (the viewport top), see
/// [`RangeText::anchor_newlines`].
pub(crate) fn range_text(
    term: &Term<MobileEventListener>,
    track: RenderTrackSnapshot,
    start: i64,
    end: i64,
    anchor: i64,
) -> RangeText {
    let grid = term.grid();
    let history = grid.history_size() as i64;
    let first = track.screen_top - history;
    let last = track.screen_top + grid.screen_lines() as i64 - 1;
    let end = end.min(last);
    let mut start = start.max(first).max(end - MAX_EXPORT_LINES + 1);
    let cols = grid.columns();
    let spacer = Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER;
    let wrapped_at = |line: i64| {
        let row = &grid[Line((line - track.screen_top) as i32)];
        cols > 0 && row[Column(cols - 1)].flags.contains(Flags::WRAPLINE)
    };
    // Begin at a logical line, not inside a soft-wrapped one.
    while start > first && start < end && wrapped_at(start - 1) {
        start += 1;
    }

    let mut out = RangeText::default();
    let mut newlines = 0u32;
    let mut line_start = 0usize;
    let mut line = start;
    while line <= end {
        if line == anchor {
            out.anchor_newlines = newlines;
        }
        let row = &grid[Line((line - track.screen_top) as i32)];
        for c in 0..cols {
            let cell = &row[Column(c)];
            if cell.flags.intersects(spacer) {
                continue;
            }
            out.text.push(if cell.c == '\0' { ' ' } else { cell.c });
            if let Some(marks) = cell.zerowidth() {
                out.text.extend(marks.iter());
            }
        }
        if !wrapped_at(line) || line == end {
            let trimmed = out.text[line_start..].trim_end_matches(' ').len();
            out.text.truncate(line_start + trimmed);
            if line != end {
                out.text.push('\n');
                newlines += 1;
                line_start = out.text.len();
            }
        }
        line += 1;
    }
    if anchor > end {
        out.anchor_newlines = newlines;
    }
    let kept = out.text.trim_end_matches(['\n', ' ']).len();
    out.text.truncate(kept);
    let total = out.text.matches('\n').count() as u32;
    out.anchor_newlines = out.anchor_newlines.min(total);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::terminal_holder::TerminalHolder;

    fn export(holder: &TerminalHolder, start: i64, end: i64, anchor: i64) -> RangeText {
        holder.with_term(|term, track| range_text(term, track, start, end, anchor))
    }

    fn screen_top(holder: &TerminalHolder) -> i64 {
        holder.with_term(|_, track| track.screen_top)
    }

    #[test]
    fn joins_soft_wraps_and_trims_trailing_blanks() {
        let holder = TerminalHolder::new(10, 6);
        holder.process_output(b"short   \r\n0123456789abcdef\r\nnext");
        let top = screen_top(&holder);
        let out = export(&holder, top, top + 5, top + 2);
        assert_eq!(out.text, "short\n0123456789abcdef\nnext");
        // Row 2 is the wrapped tail of the second line.
        assert_eq!(out.anchor_newlines, 1);
    }

    #[test]
    fn starts_at_a_logical_line() {
        let holder = TerminalHolder::new(10, 6);
        holder.process_output(b"0123456789abcdef\r\nnext");
        let top = screen_top(&holder);
        // Row 1 continues row 0: start at the next logical line.
        let out = export(&holder, top + 1, top + 5, top + 2);
        assert_eq!(out.text, "next");
        assert_eq!(out.anchor_newlines, 0);
    }

    #[test]
    fn skips_wide_spacers_and_keeps_combining_marks() {
        let holder = TerminalHolder::new(10, 3);
        holder.process_output("a中b e\u{0301}".as_bytes());
        let top = screen_top(&holder);
        assert_eq!(export(&holder, top, top, top).text, "a中b e\u{0301}");
    }

    #[test]
    fn clamps_to_the_readable_range_and_covers_history() {
        let holder = TerminalHolder::new(10, 3);
        for i in 0..8 {
            holder.process_output(format!("L{i}\r\n").as_bytes());
        }
        let top = screen_top(&holder);
        let out = export(&holder, -100, top + 100, top);
        assert_eq!(out.text, "L0\nL1\nL2\nL3\nL4\nL5\nL6\nL7");
        assert_eq!(out.anchor_newlines, 6);
        // A window into history.
        let out = export(&holder, 2, 4, 3);
        assert_eq!(out.text, "L2\nL3\nL4");
        assert_eq!(out.anchor_newlines, 1);
    }

    #[test]
    fn caps_the_number_of_lines() {
        let holder = TerminalHolder::with_history(10, 3, 30_000);
        let big: String = (0..(MAX_EXPORT_LINES + 50))
            .map(|i| format!("{i}\r\n"))
            .collect();
        holder.process_output(big.as_bytes());
        let top = screen_top(&holder);
        let out = export(&holder, i64::MIN / 2, top + 2, top);
        // The newest lines up to the blank cursor row, which is trimmed.
        assert_eq!(out.text.lines().count() as i64, MAX_EXPORT_LINES - 1);
        assert_eq!(out.text.lines().next(), Some("51"));
    }
}
