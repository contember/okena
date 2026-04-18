use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use regex::Regex;

use super::Terminal;

impl Terminal {
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
}
