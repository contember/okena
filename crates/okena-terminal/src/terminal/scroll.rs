use alacritty_terminal::grid::{Dimensions, Scroll};
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
