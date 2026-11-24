use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions, Row};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::{Cell, Flags};
use std::sync::atomic::Ordering;

use super::Terminal;
use super::event_listener::ZedEventListener;
use super::scrollback::{HistoryView, wraps};

impl Terminal {
    /// Scroll to bottom (display_offset = 0)
    pub fn scroll_to_bottom(&self) {
        let mut term = self.term.lock();
        let mut scrollback = self.scrollback.lock();
        if scrollback.display_offset(&term) > 0 {
            scrollback.set_display_offset(&mut term, 0);
            self.content_generation.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Access the terminal content for rendering.
    ///
    /// Drains any pending output (enqueued by remote clients) before
    /// handing the content to the callback, so the rendered frame is
    /// always up-to-date. Only the grid: history older than the live rows
    /// is in [`Terminal::with_history`].
    pub fn with_content<R>(&self, f: impl FnOnce(&Term<ZedEventListener>) -> R) -> R {
        self.drain_pending_output();
        let term = self.term.lock();
        f(&term)
    }

    /// Access the whole history, grid and compressed archive, after draining
    /// pending output like [`Terminal::with_content`]. Right after a column
    /// change archived rows may still read clipped or padded; see
    /// [`Terminal::with_settled_history`] and
    /// [`Terminal::settle_scrolled_history`].
    pub fn with_history<R>(&self, f: impl FnOnce(&HistoryView<'_, ZedEventListener>) -> R) -> R {
        self.history_view(false, f)
    }

    /// [`Terminal::with_history`] with every archived row laid out at the
    /// current width, for reads whose text must be whole: copy, search,
    /// export, snapshots.
    pub fn with_settled_history<R>(
        &self,
        f: impl FnOnce(&HistoryView<'_, ZedEventListener>) -> R,
    ) -> R {
        self.history_view(true, f)
    }

    fn history_view<R>(
        &self,
        settle: bool,
        f: impl FnOnce(&HistoryView<'_, ZedEventListener>) -> R,
    ) -> R {
        self.drain_pending_output();
        let term = self.term.lock();
        let mut scrollback = self.scrollback.lock();
        if settle && scrollback.settle(&term, true) {
            self.content_generation.fetch_add(1, Ordering::Relaxed);
        }
        f(&HistoryView::new(&term, &scrollback))
    }

    /// For the renderer, before it reads the content generation: reflow the
    /// archive once the columns have held for a moment while the viewport
    /// shows archived rows. Returns `true` while that reflow still waits, so
    /// the caller paints again.
    pub fn settle_scrolled_history(&self) -> bool {
        let term = self.term.lock();
        let mut scrollback = self.scrollback.lock();
        if scrollback.archive_offset == 0 || !scrollback.reflow_pending() {
            return false;
        }
        if scrollback.settle(&term, false) {
            self.content_generation.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        true
    }

    /// The visible screen as text: trailing blanks and empty last lines cut.
    pub fn visible_text(&self) -> String {
        self.with_content(screen_text)
    }

    /// The last `count` rows of scrollback and screen as text, so content
    /// taller than the pane reads in full. Trailing empty rows are cut first.
    pub fn tail_text(&self, count: usize) -> String {
        self.with_settled_history(|view| tail_text(view, count))
    }

    /// The whole history and screen as text, oldest first. Archived rows keep
    /// the width they were archived with; soft-wrapped rows join into one line.
    pub fn history_text(&self) -> String {
        self.with_settled_history(|view| {
            let mut text = String::new();
            let mut joined = false;
            view.for_each_full_row(|row| {
                if !joined && !text.is_empty() {
                    text.push('\n');
                }
                let line = cells_text(row, 0..row.len());
                joined = wraps(row);
                if joined {
                    text.push_str(&line);
                } else {
                    text.push_str(line.trim_end());
                }
            });
            while text.ends_with('\n') {
                text.pop();
            }
            text
        })
    }

    /// Scroll the terminal
    pub fn scroll(&self, delta: i32) {
        let mut term = self.term.lock();
        let mut scrollback = self.scrollback.lock();
        let current = scrollback.display_offset(&term) as i64;
        let target = (current + i64::from(delta)).max(0) as usize;
        scrollback.set_display_offset(&mut term, target);
        drop(scrollback);
        drop(term);
        *self.scroll_offset.lock() += delta;
        self.content_generation.fetch_add(1, Ordering::Relaxed);
        // External scroll disengages the prompt-jump walker — the user's
        // implicit reference point has moved, so the next Above jump
        // should start over from the newest prompt.
        *self.prompt_jump_index.lock() = None;
        *self.failed_jump_index.lock() = None;
    }

    /// Scroll up by lines
    pub fn scroll_up(&self, lines: i32) {
        self.scroll(lines);
    }

    /// Scroll down by lines
    pub fn scroll_down(&self, lines: i32) {
        self.scroll(-lines);
    }

    /// Scroll to a specific position (0 = bottom, positive = towards top)
    pub fn scroll_to(&self, offset: usize) {
        let mut term = self.term.lock();
        let mut scrollback = self.scrollback.lock();
        if scrollback.display_offset(&term) != offset {
            scrollback.set_display_offset(&mut term, offset);
            self.content_generation.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Get the number of screen lines
    pub fn screen_lines(&self) -> usize {
        self.with_content(|term| term.grid().screen_lines())
    }

    /// Get scroll information for scrollbar rendering
    /// Returns (total_lines, visible_lines, scroll_offset)
    /// total_lines: Total number of lines in history (grid and archive) + screen
    /// visible_lines: Number of lines visible on screen
    /// scroll_offset: Current scroll position (0 = bottom, positive = scrolled up)
    pub fn scroll_info(&self) -> (usize, usize, i32) {
        let term = self.term.lock();
        let scrollback = self.scrollback.lock();
        let visible_lines = term.grid().screen_lines();
        let total_lines = visible_lines + scrollback.history_size(&term);
        let display_offset = scrollback.display_offset(&term);
        (total_lines, visible_lines, display_offset as i32)
    }

    /// Get the current display offset (how many lines scrolled from bottom)
    pub fn display_offset(&self) -> usize {
        let term = self.term.lock();
        let scrollback = self.scrollback.lock();
        scrollback.display_offset(&term)
    }
}

/// alacritty keeps a cell's zero-width marks beside `cell.c`, so the base char
/// alone loses the accent of decomposed text.
fn screen_text<T: EventListener>(term: &Term<T>) -> String {
    let screen_lines = term.grid().screen_lines();
    let mut lines: Vec<String> = (0..screen_lines as i32)
        .map(|row| line_text(&term.grid()[Line(row)]))
        .collect();

    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }

    lines.join("\n")
}

/// The last `count` rows of scrollback plus screen, ending at the last
/// non-empty row.
fn tail_text<L>(view: &HistoryView<'_, L>, count: usize) -> String {
    let top = view.topmost_line();
    let mut row = view.bottommost_line();
    let mut lines = Vec::new();
    while row >= top && lines.len() < count {
        let line = line_text(&view.row(row));
        if !(lines.is_empty() && line.is_empty()) {
            lines.push(line);
        }
        row -= 1;
    }
    lines.reverse();
    lines.join("\n")
}

/// One row as text, every cell's char with its zero-width marks, trailing
/// blanks cut.
fn line_text(row: &Row<Cell>) -> String {
    let mut line = String::with_capacity(row.len());
    for col in 0..row.len() {
        let cell = &row[Column(col)];
        line.push(cell.c);
        line.extend(cell.zerowidth().into_iter().flatten());
    }
    line.truncate(line.trim_end().len());
    line
}

/// The text of `cols` in `row`: base chars with their zero-width marks, wide
/// char spacers skipped.
pub(super) fn cells_text(row: &Row<Cell>, cols: std::ops::Range<usize>) -> String {
    let mut text = String::with_capacity(cols.len());
    for col in cols {
        let cell = &row[Column(col)];
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        text.push(cell.c);
        text.extend(cell.zerowidth().into_iter().flatten());
    }
    text
}

#[cfg(test)]
mod tests {
    use super::super::scrollback::{HistoryView, Scrollback};
    use super::{screen_text, tail_text};
    use alacritty_terminal::event::{Event, EventListener};
    use alacritty_terminal::term::test::TermSize;
    use alacritty_terminal::term::{Config, Term};
    use alacritty_terminal::vte::ansi::Processor;

    struct NoopListener;

    impl EventListener for NoopListener {
        fn send_event(&self, _event: Event) {}
    }

    fn term_with(output: &str) -> Term<NoopListener> {
        let mut term = Term::new(Config::default(), &TermSize::new(10, 3), NoopListener);
        let mut processor: Processor = Processor::new();
        processor.advance(&mut term, output.as_bytes());
        term
    }

    /// A tail read over the grid alone, nothing archived.
    fn tail(term: &Term<NoopListener>, count: usize) -> String {
        tail_text(&HistoryView::new(term, &Scrollback::new(1000, 1000)), count)
    }

    fn screen(output: &str) -> String {
        screen_text(&term_with(output))
    }

    #[test]
    fn a_tail_read_reaches_rows_scrolled_off_the_screen() {
        // Six rows through a three-row screen: 1-3 sit in scrollback.
        let term = term_with("1\r\n2\r\n3\r\n4\r\n5\r\n6");
        assert_eq!(screen_text(&term), "4\n5\n6");
        assert_eq!(tail(&term, 5), "2\n3\n4\n5\n6");
        // Asking for more than exists returns everything.
        assert_eq!(tail(&term, 100), "1\n2\n3\n4\n5\n6");
    }

    #[test]
    fn a_tail_read_counts_from_the_last_non_empty_row() {
        // The cursor sits on an empty row below the output.
        let term = term_with("1\r\n2\r\n3\r\n4\r\n");
        assert_eq!(tail(&term, 2), "3\n4");
    }

    #[test]
    fn a_screen_read_keeps_a_combining_mark_with_its_base_char() {
        assert_eq!(screen("e\u{0301}x"), "e\u{0301}x");
    }

    #[test]
    fn a_screen_read_keeps_a_mark_standing_over_a_blank_cell() {
        assert_eq!(screen(" \u{0301}x"), " \u{0301}x");
    }
}
