use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::Term;
use std::sync::atomic::Ordering;

use super::Terminal;
use super::event_listener::ZedEventListener;

impl Terminal {
    /// Scroll to bottom (display_offset = 0)
    pub fn scroll_to_bottom(&self) {
        let mut term = self.term.lock();
        let current = term.grid().display_offset();
        if current > 0 {
            term.scroll_display(Scroll::Delta(-(current as i32)));
            self.content_generation.fetch_add(1, Ordering::Relaxed);
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
        f(&term)
    }

    /// The visible screen as text: trailing blanks and empty last lines cut.
    pub fn visible_text(&self) -> String {
        self.with_content(screen_text)
    }

    /// The last `count` rows of scrollback and screen as text, so content
    /// taller than the pane reads in full. Trailing empty rows are cut first.
    pub fn tail_text(&self, count: usize) -> String {
        self.with_content(|term| tail_text(term, count))
    }

    /// Scroll the terminal
    pub fn scroll(&self, delta: i32) {
        let mut term = self.term.lock();
        term.scroll_display(Scroll::Delta(delta));
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
        let current = term.grid().display_offset();
        let delta = offset as i32 - current as i32;
        if delta != 0 {
            term.scroll_display(Scroll::Delta(delta));
            self.content_generation.fetch_add(1, Ordering::Relaxed);
        }
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
}

/// alacritty keeps a cell's zero-width marks beside `cell.c`, so the base char
/// alone loses the accent of decomposed text.
fn screen_text<T: EventListener>(term: &Term<T>) -> String {
    let screen_lines = term.grid().screen_lines();
    let mut lines: Vec<String> = (0..screen_lines as i32)
        .map(|row| row_text(term, Line(row)))
        .collect();

    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }

    lines.join("\n")
}

/// The last `count` rows of scrollback plus screen, ending at the last
/// non-empty row.
fn tail_text<T: EventListener>(term: &Term<T>, count: usize) -> String {
    let top = term.grid().topmost_line();
    let mut row = term.grid().bottommost_line();
    let mut lines = Vec::new();
    while row >= top && lines.len() < count {
        let line = row_text(term, row);
        if !(lines.is_empty() && line.is_empty()) {
            lines.push(line);
        }
        row -= 1;
    }
    lines.reverse();
    lines.join("\n")
}

/// One grid row as text, trailing blanks cut. Negative lines are scrollback.
fn row_text<T: EventListener>(term: &Term<T>, row: Line) -> String {
    let grid = term.grid();
    let cols = grid.columns();
    let mut line = String::with_capacity(cols);
    for col in 0..cols {
        let cell = &grid[Point::new(row, Column(col))];
        line.push(cell.c);
        line.extend(cell.zerowidth().into_iter().flatten());
    }
    line.truncate(line.trim_end().len());
    line
}

#[cfg(test)]
mod tests {
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

    fn screen(output: &str) -> String {
        screen_text(&term_with(output))
    }

    #[test]
    fn a_tail_read_reaches_rows_scrolled_off_the_screen() {
        // Six rows through a three-row screen: 1-3 sit in scrollback.
        let term = term_with("1\r\n2\r\n3\r\n4\r\n5\r\n6");
        assert_eq!(screen_text(&term), "4\n5\n6");
        assert_eq!(tail_text(&term, 5), "2\n3\n4\n5\n6");
        // Asking for more than exists returns everything.
        assert_eq!(tail_text(&term, 100), "1\n2\n3\n4\n5\n6");
    }

    #[test]
    fn a_tail_read_counts_from_the_last_non_empty_row() {
        // The cursor sits on an empty row below the output.
        let term = term_with("1\r\n2\r\n3\r\n4\r\n");
        assert_eq!(tail_text(&term, 2), "3\n4");
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
