use alacritty_terminal::grid::{Dimensions, Row};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use regex::Regex;

use super::Terminal;

/// A grid row as searchable text, carrying the column every byte of it sits in.
/// A cell contributes its base char then its zero-width marks, and a wide char's
/// spacer contributes nothing — so a byte offset out of a regex or a substring
/// search converts back to the exact column the match starts and ends at.
#[derive(Default)]
pub(super) struct GridText {
    text: String,
    /// One entry per byte of `text`; anything at or past its end is `width`.
    col_by_byte: Vec<usize>,
    width: usize,
}

impl GridText {
    /// Refills from `row`, reusing the buffers.
    pub(super) fn rebuild(&mut self, row: &Row<Cell>, cols: usize) {
        self.text.clear();
        self.col_by_byte.clear();
        self.width = cols;
        for col in 0..cols {
            let cell = &row[Column(col)];
            // The spacer's column belongs to the wide char standing over it.
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            self.text.push(cell.c);
            for mark in cell.zerowidth().unwrap_or_default() {
                self.text.push(*mark);
            }
            self.col_by_byte.resize(self.text.len(), col);
        }
    }

    pub(super) fn text(&self) -> &str {
        &self.text
    }

    /// The column of the cell `byte` belongs to. The end of the text answers
    /// with the row width, so `col_at_byte(end) - col_at_byte(start)` is the
    /// column span of `start..end` whatever those bytes hold.
    pub(super) fn col_at_byte(&self, byte: usize) -> usize {
        self.col_by_byte.get(byte).copied().unwrap_or(self.width)
    }

    /// Whether `byte` opens a cell — false inside a char and for its marks.
    pub(super) fn starts_cell(&self, byte: usize) -> bool {
        match byte.checked_sub(1) {
            None => !self.text.is_empty(),
            Some(previous) => self.col_by_byte.get(byte) != self.col_by_byte.get(previous),
        }
    }

    /// Refills from `source` lowercased char by char, each keeping its column.
    /// Lowercasing the built string instead would move byte offsets off their
    /// columns, because a lowercased char need not keep its byte length.
    pub(super) fn lowercase_from(&mut self, source: &GridText) {
        self.text.clear();
        self.col_by_byte.clear();
        self.width = source.width;
        for (byte, c) in source.text.char_indices() {
            let col = source.col_at_byte(byte);
            self.text.extend(c.to_lowercase());
            self.col_by_byte.resize(self.text.len(), col);
        }
    }
}

impl Terminal {
    /// Search the terminal grid for occurrences of a query string
    /// Returns a list of (line, col, column span) for each match
    /// Supports case-sensitive and regex search, and searches through scrollback buffer
    pub fn search_grid(
        &self,
        query: &str,
        case_sensitive: bool,
        is_regex: bool,
    ) -> Vec<(i32, usize, usize)> {
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

        // Char by char, the rule `GridText::lowercase_from` uses on the screen:
        // `str::to_lowercase` would also apply Greek final-sigma context.
        let needle: String = if case_sensitive {
            query.to_string()
        } else {
            query.chars().flat_map(char::to_lowercase).collect()
        };

        let mut matches = Vec::new();

        self.with_content(|term| {
            let grid = term.grid();
            let screen_lines = grid.screen_lines() as i32;
            let history_size = grid.history_size() as i32;
            let cols = grid.columns();

            let mut row_text = GridText::default();
            let mut lowered = GridText::default();

            // Search from top of history to bottom of screen
            // Line numbers: negative = history, 0..screen_lines = visible
            for line in (-history_size)..screen_lines {
                row_text.rebuild(&grid[Line(line)], cols);

                if let Some(ref regex) = regex {
                    for mat in regex.find_iter(row_text.text()) {
                        let col = row_text.col_at_byte(mat.start());
                        // Store absolute grid line (not display-relative)
                        matches.push((line, col, row_text.col_at_byte(mat.end()) - col));
                    }
                } else {
                    let haystack = if case_sensitive {
                        &row_text
                    } else {
                        lowered.lowercase_from(&row_text);
                        &lowered
                    };

                    let mut from = 0;
                    while let Some(offset) = haystack.text()[from..].find(&needle) {
                        let start = from + offset;
                        let end = start + needle.len();
                        let col = haystack.col_at_byte(start);
                        matches.push((line, col, haystack.col_at_byte(end) - col));
                        from = end;
                    }
                }
            }
        });

        matches
    }
}

#[cfg(test)]
mod tests {
    use super::super::Terminal;
    use super::super::tests::NullTransport;
    use super::super::types::TerminalSize;
    use std::sync::Arc;

    fn terminal_showing(text: &str) -> Terminal {
        let terminal = Terminal::new(
            "search".into(),
            TerminalSize {
                cols: 20,
                rows: 3,
                cell_width: 8.0,
                cell_height: 16.0,
            },
            Arc::new(NullTransport),
            "/tmp".into(),
        );
        terminal.process_output(text.as_bytes());
        terminal
    }

    #[test]
    fn plain_ascii_search_reports_every_occurrence_and_its_columns() {
        let terminal = terminal_showing("ab cab\r\nxx ab");

        assert_eq!(
            terminal.search_grid("ab", true, false),
            vec![(0, 0, 2), (0, 4, 2), (1, 3, 2)]
        );
        assert_eq!(terminal.search_grid("AB", true, false), Vec::new());
        assert_eq!(
            terminal.search_grid("AB", false, false),
            vec![(0, 0, 2), (0, 4, 2), (1, 3, 2)]
        );
        assert_eq!(terminal.search_grid("c.b", true, true), vec![(0, 3, 3)]);
    }

    #[test]
    fn a_decomposed_grapheme_is_found_at_the_column_it_paints_on() {
        let terminal = terminal_showing("abe\u{0301}f");

        assert_eq!(
            terminal.search_grid("e\u{0301}", true, false),
            vec![(0, 2, 1)]
        );
        assert_eq!(
            terminal.search_grid("e\u{0301}f", true, false),
            vec![(0, 2, 2)],
            "the mark must not consume a column of its own"
        );
        assert_eq!(
            terminal.search_grid("f", true, false),
            vec![(0, 3, 1)],
            "the cell after the mark keeps its column"
        );
    }

    #[test]
    fn a_wide_char_owns_both_of_its_columns() {
        let terminal = terminal_showing("\u{65e5}\u{672c}x");

        assert_eq!(
            terminal.search_grid("\u{672c}", true, false),
            vec![(0, 2, 2)],
            "a wide char starts at its own column and spans two"
        );
        assert_eq!(
            terminal.search_grid("\u{65e5}\u{672c}", true, false),
            vec![(0, 0, 4)],
            "the spacer cell must not break the two wide chars apart"
        );
        assert_eq!(terminal.search_grid("x", true, false), vec![(0, 4, 1)]);
    }

    #[test]
    fn a_case_insensitive_match_keeps_its_column_when_lowercasing_grows_the_text() {
        // 'İ' lowercases to two chars, so a lowercased line's byte offsets no
        // longer land on the original's char boundaries.
        let terminal = terminal_showing("\u{0130}x");

        assert_eq!(terminal.search_grid("X", false, false), vec![(0, 1, 1)]);
    }

    #[test]
    fn a_match_in_the_scrollback_reports_a_negative_line() {
        let terminal = terminal_showing("needle\r\n\r\n\r\n\r\nlast");

        assert_eq!(
            terminal.search_grid("needle", true, false),
            vec![(-2, 0, 6)]
        );
    }
}
