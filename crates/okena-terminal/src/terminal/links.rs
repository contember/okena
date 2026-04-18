use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Flags;
use regex::Regex;
use std::sync::OnceLock;

use super::Terminal;
use super::types::DetectedLink;
use super::url_detect::{parse_path_line_col, trim_url_trailing};

impl Terminal {
    /// Scan visible cells for OSC 8 hyperlinks.
    ///
    /// Returns one `DetectedLink` per contiguous run of cells sharing the same
    /// hyperlink id on the same visual row. Runs that share an id across rows
    /// (wrapped link labels) get the same `wrap_group`, so hover highlight
    /// covers both halves together.
    pub fn detect_hyperlinks(&self) -> Vec<DetectedLink> {
        let mut result = Vec::new();
        let mut id_to_group: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

        self.with_content(|term| {
            let grid = term.grid();
            let screen_lines = grid.screen_lines() as i32;
            let cols = grid.columns();
            let display_offset = grid.display_offset() as i32;

            for visual_row in 0..screen_lines {
                let buffer_line = visual_row - display_offset;
                let mut col = 0usize;
                while col < cols {
                    let cell = &grid[Point::new(Line(buffer_line), Column(col))];
                    let Some(hl) = cell.hyperlink() else {
                        col += 1;
                        continue;
                    };
                    let id = hl.id().to_owned();
                    let uri = hl.uri().to_owned();

                    let start_col = col;
                    col += 1;
                    while col < cols {
                        let next_cell = &grid[Point::new(Line(buffer_line), Column(col))];
                        match next_cell.hyperlink() {
                            Some(nh) if nh.id() == id => col += 1,
                            _ => break,
                        }
                    }
                    let len = col - start_col;

                    let next_group = id_to_group.len();
                    let link_group = *id_to_group.entry(id).or_insert(next_group);

                    result.push(DetectedLink {
                        line: visual_row,
                        col: start_col,
                        len,
                        text: uri,
                        file_line: None,
                        file_col: None,
                        is_url: true,
                        wrap_group: link_group,
                    });
                }
            }
        });

        result
    }

    /// Detect URLs and file paths in the visible terminal content (Ghostty-style).
    ///
    /// Uses a single combined regex compiled once via OnceLock. Two branches:
    /// - URL: many schemes (http, https, ftp, ssh, git, mailto, etc.)
    /// - Path: explicit prefixes only (`/`, `~/`, `./`, `../`) with optional `:line:col`
    ///
    /// Returns a list of `DetectedLink` for each match. File paths are validated
    /// for existence by the caller (UrlDetector).
    #[allow(clippy::expect_used, reason = "literal regex, compilation checked by unit test")]
    pub fn detect_urls(&self) -> Vec<DetectedLink> {
        static LINK_REGEX: OnceLock<Regex> = OnceLock::new();
        let regex = LINK_REGEX.get_or_init(|| {
            // Combined regex: URL schemes | explicit file paths with optional :line:col
            // Path prefixes: /, ~/, ./, ../, or dotfile dirs like .github/
            Regex::new(
                r#"(?:(?:https?|ftp|file|ssh|git|mailto|tel|magnet|ipfs|gemini|gopher|news)://[^\s<>"'`{}\[\]|\\^]+|(?:~?/|(?:\./|\.\./)|\.[a-zA-Z][\w.-]*/)[^\s<>"'`{}\[\]|\\^()]+(?::(\d+)(?::(\d+))?)?)"#
            ).expect("link detection regex should compile")
        });

        // Characters that can appear in a URL (for continuation detection)
        let url_char = |c: char| -> bool {
            c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~' | ':' | '/' | '?' | '#' | '[' | ']' | '@' | '!' | '$' | '&' | '\'' | '(' | ')' | '*' | '+' | ',' | ';' | '=' | '%')
        };

        let mut matches = Vec::new();
        let mut next_wrap_group = 0usize;

        self.with_content(|term| {
            let grid = term.grid();
            let screen_lines = grid.screen_lines() as i32;
            let cols = grid.columns();
            let last_col = Column(cols - 1);
            let display_offset = grid.display_offset() as i32;

            // Helper: read a visual row from the grid as a String.
            let read_row = |vrow: i32| -> String {
                let buf = vrow - display_offset;
                let mut s = String::with_capacity(cols);
                for c in 0..cols {
                    s.push(grid[Point::new(Line(buf), Column(c))].c);
                }
                s
            };

            // Iterate over visual rows (0..screen_lines).
            // When scrolled, visual row 0 maps to buffer line (0 - display_offset).
            let mut visual_row = 0i32;
            while visual_row < screen_lines {
                let mut combined_text = String::new();
                // (visual_row, offset_in_combined, leading_spaces_stripped)
                let mut row_offsets: Vec<(i32, usize, usize)> = Vec::new();

                // Collect wrapped lines into one logical line
                loop {
                    let row_text = read_row(visual_row);

                    // Trim trailing spaces — URLs/paths never end with spaces,
                    // and this allows the regex to match across padded line breaks.
                    let rtrimmed = row_text.trim_end_matches(' ');

                    // For continuation rows, also strip leading spaces (TUI padding)
                    let (text_to_add, leading_stripped) = if combined_text.is_empty() {
                        (rtrimmed, 0usize)
                    } else {
                        let ltrimmed = rtrimmed.trim_start_matches(' ');
                        (ltrimmed, rtrimmed.len() - ltrimmed.len())
                    };

                    row_offsets.push((visual_row, combined_text.len(), leading_stripped));
                    combined_text.push_str(text_to_add);

                    let buffer_line = visual_row - display_offset;
                    let last_cell = &grid[Point::new(Line(buffer_line), last_col)];
                    let has_wrapline_flag = last_cell.flags.contains(Flags::WRAPLINE);

                    visual_row += 1;

                    // Only merge via terminal WRAPLINE flag.  TUI-managed
                    // wrapping (no WRAPLINE) is handled in Phase 2 below.
                    if !has_wrapline_flag || visual_row >= screen_lines {
                        break;
                    }
                }

                for mat in regex.find_iter(&combined_text) {
                    let raw = mat.as_str();
                    let trimmed = trim_url_trailing(raw);
                    if trimmed.is_empty() {
                        continue;
                    }

                    // Each regex match gets a unique wrap_group.
                    // Segments of a wrapped URL (same match, multiple rows) share it.
                    let wrap_group = next_wrap_group;
                    next_wrap_group += 1;

                    let match_start = mat.start();
                    let trimmed_end = match_start + trimmed.len();

                    // Determine if this is a URL or file path
                    let is_url = trimmed.contains("://");

                    // Parse :line:col from file paths
                    let (display_text, file_line, file_col) = if !is_url {
                        parse_path_line_col(trimmed)
                    } else {
                        (trimmed.to_string(), None, None)
                    };

                    // Map back to physical rows
                    for i in 0..row_offsets.len() {
                        let (phys_row, row_start_offset, leading_stripped) = row_offsets[i];
                        let row_end_offset = if i + 1 < row_offsets.len() {
                            row_offsets[i + 1].1
                        } else {
                            combined_text.len()
                        };

                        if trimmed_end <= row_start_offset || match_start >= row_end_offset {
                            continue;
                        }

                        let seg_start = match_start.max(row_start_offset);
                        let seg_end = trimmed_end.min(row_end_offset);

                        let col_start = combined_text[row_start_offset..seg_start].chars().count() + leading_stripped;
                        let len = combined_text[seg_start..seg_end].chars().count();

                        if len > 0 {
                            matches.push(DetectedLink {
                                line: phys_row,
                                col: col_start,
                                len,
                                text: display_text.clone(),
                                file_line,
                                file_col,
                                is_url,
                                wrap_group,
                            });
                        }
                    }
                }
            }

            // ── Phase 2: Extend URL matches at TUI-wrapped row boundaries ──
            //
            // Phase 1 only merges rows with the terminal WRAPLINE flag.  TUI
            // applications manage their own wrapping (no WRAPLINE), so a long
            // URL may be split across visual rows with only the first fragment
            // matched by the regex.
            //
            // Approach inspired by Kitty: for each URL that reaches the end of
            // visible content, strip leading whitespace from the next row and
            // consume URL-compatible chars.  No attempt to reverse-engineer TUI
            // decoration via common-prefix detection (too fragile).
            //
            // Guards against false positives:
            //  - URL must not start at col 0 (terminal would set WRAPLINE)
            //  - No alphabetic text before/after the URL (prose context)
            //  - Continuation must have alphanumeric chars (not just punctuation)
            //  - "Weak" continuations (no `/`) rejected if content has spaces
            //  - Continuation containing `://` means a new URL, not extension

            let phase1_len = matches.len();
            let mut idx = 0;
            while idx < phase1_len {
                let group = matches[idx].wrap_group;

                // Advance to the last segment of this wrap_group.
                let mut last_idx = idx;
                while last_idx + 1 < phase1_len
                    && matches[last_idx + 1].wrap_group == group
                {
                    last_idx += 1;
                }
                let next_idx = last_idx + 1;

                // Only extend URL matches (not file paths).
                if !matches[last_idx].is_url {
                    idx = next_idx;
                    continue;
                }

                // URL must start after col 0 — if the URL occupies the full
                // line without WRAPLINE, the lines are independent (the
                // terminal would have set WRAPLINE for a genuine wrap).
                let url_start_col = matches[idx].col;
                if url_start_col == 0 {
                    idx = next_idx;
                    continue;
                }

                // Skip rows with WRAPLINE (already handled by Phase 1).
                let m_line = matches[last_idx].line;
                let m_col = matches[last_idx].col;
                let m_len = matches[last_idx].len;
                let match_buf_line = m_line - display_offset;
                let match_last_cell =
                    &grid[Point::new(Line(match_buf_line), last_col)];
                if match_last_cell.flags.contains(Flags::WRAPLINE) {
                    idx = next_idx;
                    continue;
                }

                let match_row_text = read_row(m_line);
                let match_rtrimmed = match_row_text.trim_end();

                // URL must reach near the end of visible content.
                // TUIs may use a narrower layout than the terminal width.
                let trimmed_char_len = match_rtrimmed.chars().count();
                if m_col + m_len + 3 < trimmed_char_len {
                    idx = next_idx;
                    continue;
                }

                // No alphabetic text after the URL (prose context).
                let url_end_pos = m_col + m_len;
                let suffix_byte = match_rtrimmed
                    .char_indices()
                    .nth(url_end_pos)
                    .map_or(match_rtrimmed.len(), |(b, _)| b);
                if match_rtrimmed[suffix_byte..]
                    .chars()
                    .any(|c| c.is_alphabetic())
                {
                    idx = next_idx;
                    continue;
                }

                // ── Extension loop ──
                let mut extended_url = matches[last_idx].text.clone();
                let mut current_row = m_line;

                loop {
                    let next_row = current_row + 1;
                    if next_row >= screen_lines {
                        break;
                    }

                    let next_row_text = read_row(next_row);
                    let next_rtrimmed = next_row_text.trim_end();

                    // Strip leading whitespace (TUI indentation).
                    let content = next_rtrimmed.trim_start_matches(' ');
                    let indent = next_rtrimmed.len() - content.len();

                    if content.is_empty() {
                        break;
                    }

                    // Don't extend into a new URL scheme.
                    if content.starts_with("http://")
                        || content.starts_with("https://")
                        || content.starts_with("ftp://")
                        || content.starts_with("file://")
                        || content.starts_with("ssh://")
                        || content.starts_with("git://")
                    {
                        break;
                    }

                    // Take URL-compatible chars as extension.
                    let ext_char_len = content
                        .chars()
                        .take_while(|c| url_char(*c))
                        .count();
                    if ext_char_len == 0 {
                        break;
                    }
                    let ext_byte_len = content
                        .char_indices()
                        .nth(ext_char_len)
                        .map_or(content.len(), |(b, _)| b);
                    let ext_raw = &content[..ext_byte_len];

                    // Trim the FULL combined URL, not just the fragment,
                    // so balanced parens spanning the line break are
                    // handled correctly (e.g. `Rust_(pr` + `ogramming_language)`).
                    let candidate = format!("{}{}", extended_url, ext_raw);
                    let trimmed_full = trim_url_trailing(&candidate);
                    if trimmed_full.len() <= extended_url.len() {
                        break;
                    }
                    let ext_trimmed = &trimmed_full[extended_url.len()..];

                    // Must contain at least one alphanumeric character.
                    if !ext_trimmed.chars().any(|c| c.is_alphanumeric()) {
                        break;
                    }

                    // Pure alphabetic words (e.g. "remote", "next",
                    // "Press") are not URL continuations — URL path
                    // fragments always contain non-alpha chars (digits,
                    // `/`, `-`, `_`, `.`, etc.).
                    if ext_trimmed.chars().all(|c| c.is_alphabetic()) {
                        break;
                    }

                    // Remaining content has a URL scheme → new item.
                    let remaining = &content[ext_byte_len..];
                    if remaining.contains("://") {
                        break;
                    }

                    // "Weak" extension (no path separator `/`): only
                    // accept when the full content has no spaces.
                    // URLs never contain spaces; spaces mean prose.
                    // Exception: tokens with digits (UUIDs, hashes, IDs)
                    // are almost certainly URL content, not words.
                    if !ext_trimmed.contains('/')
                        && !ext_trimmed.chars().any(|c| c.is_ascii_digit())
                        && content.contains(' ')
                    {
                        break;
                    }

                    // Commit extension.
                    let ext_trimmed_len = ext_trimmed.len();
                    let ext_trimmed_chars = ext_trimmed.chars().count();
                    extended_url.push_str(ext_trimmed);

                    matches.push(DetectedLink {
                        line: next_row,
                        col: indent,
                        len: ext_trimmed_chars,
                        text: String::new(), // updated below
                        file_line: None,
                        file_col: None,
                        is_url: true,
                        wrap_group: group,
                    });

                    // If trim_url_trailing removed characters, the URL
                    // ended here (e.g. trailing `,`, `.`).
                    if ext_trimmed_len < ext_raw.len() {
                        break;
                    }

                    // Continue only if extension fills to near end of
                    // visible content on this row.
                    let next_trimmed_len =
                        next_rtrimmed.chars().count();
                    if indent + ext_char_len + 3 < next_trimmed_len {
                        break;
                    }
                    if !remaining.is_empty()
                        && remaining
                            .chars()
                            .any(|c| c.is_alphanumeric())
                    {
                        break;
                    }

                    current_row = next_row;
                }

                // Update text for all segments (original + extensions).
                if extended_url != matches[last_idx].text {
                    for m in matches.iter_mut() {
                        if m.wrap_group == group {
                            m.text.clone_from(&extended_url);
                        }
                    }
                }

                idx = next_idx;
            }
        });

        matches
    }
}
