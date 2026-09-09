use alacritty_terminal::grid::{Dimensions, Grid};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::{Cell, Flags};
use regex::Regex;
use std::hash::{DefaultHasher, Hasher};
use std::sync::OnceLock;

use super::Terminal;
use super::search::GridText;
use super::types::DetectedLink;
use super::url_detect::{parse_path_line_col, trim_url_trailing};

/// What one viewer saw on its last `detect_urls_with` scan, so the next scan
/// re-runs the regex and wrap heuristics only over logical lines whose input
/// rows changed. Results are exact: a line's matches depend on nothing but
/// the rows recorded in `ScannedLine::last_read`.
#[derive(Default)]
pub struct UrlScanCache {
    cols: usize,
    /// Text of every visual row as of the last scan; buffers are reused.
    rows: Vec<GridText>,
    row_wrapline: Vec<bool>,
    row_hashes: Vec<u64>,
    lines: Vec<ScannedLine>,
    recomputed_lines: usize,
}

/// One logical line (a WRAPLINE chain) and the matches it produced.
#[derive(Clone)]
struct ScannedLine {
    start: usize,
    end: usize,
    /// Last visual row whose text influenced the result; phase 2 reads below `end`.
    last_read: usize,
    groups: usize,
    /// `wrap_group` is relative to this line's first group.
    phase1: Vec<DetectedLink>,
    phase2: Vec<DetectedLink>,
}

impl UrlScanCache {
    /// Refreshes the row texts and reports which visual rows differ from the
    /// last scan. A resize drops everything: rows are compared by position.
    fn read_rows(&mut self, grid: &Grid<Cell>) -> Vec<bool> {
        let screen_lines = grid.screen_lines();
        let cols = grid.columns();
        let display_offset = grid.display_offset() as i32;
        if self.cols != cols || self.row_hashes.len() != screen_lines {
            self.cols = cols;
            self.row_hashes.clear();
            self.lines.clear();
        }
        self.rows.resize_with(screen_lines, GridText::default);
        self.row_wrapline.resize(screen_lines, false);

        let mut hashes = Vec::with_capacity(screen_lines);
        let mut changed = Vec::with_capacity(screen_lines);
        for (visual_row, text) in self.rows.iter_mut().enumerate() {
            let row = &grid[Line(visual_row as i32 - display_offset)];
            text.rebuild(row, cols);
            let wrapline = row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
            self.row_wrapline[visual_row] = wrapline;

            // A row's columns follow from its text: a char's cell width is
            // fixed, so equal text under an unchanged `cols` maps identically.
            let mut hasher = DefaultHasher::new();
            hasher.write(text.text().as_bytes());
            hasher.write_u8(u8::from(wrapline));
            let hash = hasher.finish();
            changed.push(self.row_hashes.get(visual_row) != Some(&hash));
            hashes.push(hash);
        }
        self.row_hashes = hashes;
        changed
    }

    /// Splits the screen into logical lines, reuses every line whose rows
    /// `[start, last_read]` are unchanged, rescans the rest, and renumbers the
    /// wrap groups sequentially so the result matches a full scan.
    fn scan(&mut self, changed: &[bool]) -> Vec<DetectedLink> {
        let screen_lines = self.rows.len();
        let mut previous = std::mem::take(&mut self.lines).into_iter().peekable();
        let mut lines = Vec::new();
        let mut phase1 = Vec::new();
        let mut phase2 = Vec::new();
        let mut next_group = 0usize;
        self.recomputed_lines = 0;

        let mut start = 0;
        while start < screen_lines {
            let mut end = start;
            while self.row_wrapline[end] && end + 1 < screen_lines {
                end += 1;
            }

            while previous.peek().is_some_and(|line| line.start < start) {
                previous.next();
            }
            let cached = previous.next_if(|line| {
                line.start == start
                    && line.end == end
                    && !changed[start..=line.last_read]
                        .iter()
                        .any(|&changed| changed)
            });
            let line = match cached {
                Some(line) => line,
                None => {
                    self.recomputed_lines += 1;
                    scan_logical_line(&self.rows, &self.row_wrapline, start, end)
                }
            };

            for link in &line.phase1 {
                phase1.push(DetectedLink {
                    wrap_group: link.wrap_group + next_group,
                    ..link.clone()
                });
            }
            for link in &line.phase2 {
                phase2.push(DetectedLink {
                    wrap_group: link.wrap_group + next_group,
                    ..link.clone()
                });
            }
            next_group += line.groups;
            start = end + 1;
            lines.push(line);
        }

        self.lines = lines;
        phase1.extend(phase2);
        phase1
    }
}

impl Terminal {
    /// Scan visible cells for OSC 8 hyperlinks.
    ///
    /// Returns one `DetectedLink` per contiguous run of cells sharing the same
    /// hyperlink id on the same visual row. Runs that share an id across rows
    /// (wrapped link labels) get the same `wrap_group`, so hover highlight
    /// covers both halves together.
    pub fn detect_hyperlinks(&self) -> Vec<DetectedLink> {
        let mut result = Vec::new();
        let mut id_to_group: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

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
    pub fn detect_urls(&self) -> Vec<DetectedLink> {
        self.detect_urls_with(&mut UrlScanCache::default())
    }

    /// [`Self::detect_urls`] that rescans only the logical lines whose rows
    /// changed since `cache` was last used; the result is identical.
    pub fn detect_urls_with(&self, cache: &mut UrlScanCache) -> Vec<DetectedLink> {
        let changed = self.with_content(|term| cache.read_rows(term.grid()));
        cache.scan(&changed)
    }
}

#[allow(
    clippy::expect_used,
    reason = "literal regex, compilation checked by unit test"
)]
fn link_regex() -> &'static Regex {
    static LINK_REGEX: OnceLock<Regex> = OnceLock::new();
    LINK_REGEX.get_or_init(|| {
        // Combined regex: URL schemes | explicit file paths with optional :line:col
        // Path prefixes: /, ~/, ./, ../, or dotfile dirs like .github/
        Regex::new(
            r#"(?:(?:https?|ftp|file|ssh|git|mailto|tel|magnet|ipfs|gemini|gopher|news)://[^\s<>"'`{}\[\]|\\^]+|(?:~?/|(?:\./|\.\./)|\.[a-zA-Z][\w.-]*/)[^\s<>"'`{}\[\]|\\^()]+(?::(\d+)(?::(\d+))?)?)"#
        ).expect("link detection regex should compile")
    })
}

/// Characters that can appear in a URL (for continuation detection)
fn url_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '-' | '.'
                | '_'
                | '~'
                | ':'
                | '/'
                | '?'
                | '#'
                | '['
                | ']'
                | '@'
                | '!'
                | '$'
                | '&'
                | '\''
                | '('
                | ')'
                | '*'
                | '+'
                | ','
                | ';'
                | '='
                | '%'
        )
}

/// Runs the detection for the logical line at visual rows `start..=end`:
/// the regex over the joined rows, then the TUI-wrap extension of its URLs.
fn scan_logical_line(rows: &[GridText], wrapline: &[bool], start: usize, end: usize) -> ScannedLine {
    let regex = link_regex();
    let screen_lines = rows.len();
    let mut last_read = end;

    let mut combined_text = String::new();
    // (visual_row, offset_in_combined, bytes of leading padding stripped)
    let mut row_offsets: Vec<(usize, usize, usize)> = Vec::new();

    // Collect wrapped lines into one logical line
    for (visual_row, row) in rows.iter().enumerate().take(end + 1).skip(start) {
        // Trim trailing spaces — URLs/paths never end with spaces,
        // and this allows the regex to match across padded line breaks.
        let rtrimmed = row.text().trim_end_matches(' ');

        // For continuation rows, also strip leading spaces (TUI padding)
        let (text_to_add, leading_stripped) = if combined_text.is_empty() {
            (rtrimmed, 0usize)
        } else {
            let ltrimmed = rtrimmed.trim_start_matches(' ');
            (ltrimmed, rtrimmed.len() - ltrimmed.len())
        };

        row_offsets.push((visual_row, combined_text.len(), leading_stripped));
        combined_text.push_str(text_to_add);
    }

    let mut phase1: Vec<DetectedLink> = Vec::new();
    let mut phase1_rows: Vec<usize> = Vec::new();
    let mut groups = 0usize;

    for mat in regex.find_iter(&combined_text) {
        let raw = mat.as_str();
        let trimmed = trim_url_trailing(raw);
        if trimmed.is_empty() {
            continue;
        }

        // Each regex match gets a unique wrap_group.
        // Segments of a wrapped URL (same match, multiple rows) share it.
        let wrap_group = groups;
        groups += 1;

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

            // Back to the row's own bytes: the segment was copied verbatim out
            // of it, past the padding the join stripped.
            let row = &rows[phys_row];
            let byte_in_row = |offset: usize| offset - row_start_offset + leading_stripped;
            let col_start = row.col_at_byte(byte_in_row(seg_start));
            let len = row.col_at_byte(byte_in_row(seg_end)) - col_start;

            if len > 0 {
                phase1.push(DetectedLink {
                    line: phys_row as i32,
                    col: col_start,
                    len,
                    text: display_text.clone(),
                    file_line,
                    file_col,
                    is_url,
                    wrap_group,
                });
                phase1_rows.push(phys_row);
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

    let mut phase2: Vec<DetectedLink> = Vec::new();
    let phase1_len = phase1.len();
    let mut idx = 0;
    while idx < phase1_len {
        let group = phase1[idx].wrap_group;

        // Advance to the last segment of this wrap_group.
        let mut last_idx = idx;
        while last_idx + 1 < phase1_len && phase1[last_idx + 1].wrap_group == group {
            last_idx += 1;
        }
        let next_idx = last_idx + 1;

        // Only extend URL matches (not file paths).
        if !phase1[last_idx].is_url {
            idx = next_idx;
            continue;
        }

        // URL must start after col 0 — if the URL occupies the full
        // line without WRAPLINE, the lines are independent (the
        // terminal would have set WRAPLINE for a genuine wrap).
        let url_start_col = phase1[idx].col;
        if url_start_col == 0 {
            idx = next_idx;
            continue;
        }

        // Skip rows with WRAPLINE (already handled by Phase 1).
        let m_row = phase1_rows[last_idx];
        let m_col = phase1[last_idx].col;
        let m_len = phase1[last_idx].len;
        if wrapline[m_row] {
            idx = next_idx;
            continue;
        }

        let match_rtrimmed = rows[m_row].text().trim_end();

        // A mid-token wrap runs the URL into the layout edge, so the
        // URL is the last thing on its row.  Anything after it — a
        // dash, a bracket, prose — means the row had room left and the
        // break was a word break, not a wrap.
        if rows[m_row].col_at_byte(match_rtrimmed.len()) != m_col + m_len {
            idx = next_idx;
            continue;
        }

        // ── Extension loop ──
        let mut extended_url = phase1[last_idx].text.clone();
        let mut current_row = m_row;
        let mut url_end_col = m_col + m_len;

        loop {
            let next_row = current_row + 1;
            if next_row >= screen_lines {
                break;
            }
            last_read = last_read.max(next_row);

            let next_row_text = &rows[next_row];
            let next_rtrimmed = next_row_text.text().trim_end();

            // A mid-token wrap fills the row to the layout edge, so a
            // continuation can never be wider than the row it
            // continues.  A wider next row means the break was a word
            // break — the URL ended on its own line.  No slack: the
            // edge is exact, and slack is what let a 3-column-longer
            // continuation through.
            if next_row_text.col_at_byte(next_rtrimmed.len()) > url_end_col {
                break;
            }

            // Strip leading whitespace (TUI indentation).
            let content = next_rtrimmed.trim_start_matches(' ');
            let indent_bytes = next_rtrimmed.len() - content.len();
            let indent = next_row_text.col_at_byte(indent_bytes);

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

            // ── The continuation must read as a continuation of the
            // URL's last token.  A hard wrap breaks a token mid-way;
            // it never starts a new one. ──
            let first = content.chars().next();

            // An opening paren starts a bracketed token, so this is a
            // parenthetical following the URL, not the URL's tail.
            if first == Some('(') {
                break;
            }

            // A wholly numeric last segment (`/pull/567`, `/issues/42`)
            // can only continue with more digits, or with a delimiter
            // that starts the next segment.
            let last_segment = extended_url.rsplit('/').next().unwrap_or("");
            let continues_a_number =
                first.is_some_and(|c| c.is_ascii_digit() || matches!(c, '/' | '?' | '#'));
            if !last_segment.is_empty()
                && last_segment.bytes().all(|b| b.is_ascii_digit())
                && !continues_a_number
            {
                break;
            }

            // Take URL-compatible cells as extension; a cell's zero-width
            // marks ride along with the base char they stand on.
            let mut ext_byte_len = 0;
            for (offset, c) in content.char_indices() {
                if next_row_text.starts_cell(indent_bytes + offset) && !url_char(c) {
                    break;
                }
                ext_byte_len = offset + c.len_utf8();
            }
            if ext_byte_len == 0 {
                break;
            }
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
            let ext_trimmed_cols =
                next_row_text.col_at_byte(indent_bytes + ext_trimmed_len) - indent;
            extended_url.push_str(ext_trimmed);

            phase2.push(DetectedLink {
                line: next_row as i32,
                col: indent,
                len: ext_trimmed_cols,
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

            // Same rule one row down: the URL keeps going only if it
            // reached this row's edge too.
            if !remaining.is_empty() {
                break;
            }

            url_end_col = next_row_text.col_at_byte(indent_bytes + ext_byte_len);
            current_row = next_row;
        }

        // Update text for all segments (original + extensions).
        if extended_url != phase1[last_idx].text {
            for m in phase1.iter_mut().chain(phase2.iter_mut()) {
                if m.wrap_group == group {
                    m.text.clone_from(&extended_url);
                }
            }
        }

        idx = next_idx;
    }

    ScannedLine {
        start,
        end,
        last_read,
        groups,
        phase1,
        phase2,
    }
}

#[cfg(test)]
mod tests {
    use super::super::Terminal;
    use super::super::tests::NullTransport;
    use super::super::types::TerminalSize;
    use super::UrlScanCache;
    use std::sync::Arc;

    /// xorshift64*: deterministic, dependency-free.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next() % u64::try_from(n.max(1)).unwrap_or(1)).unwrap_or(0)
        }

        fn one_in(&mut self, n: usize) -> bool {
            self.below(n) == 0
        }

        fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
            items[self.below(items.len())]
        }
    }

    /// Fragments that exercise every branch of the regex and the wrap rules.
    const TOKENS: &[&str] = &[
        "https://github.com/org/repo/pull/567",
        "https://example.com/a/b/c?x=1#frag",
        "http://localhost:19400/s/1f41d02d-6105-45fb-b3b1-4b56ae4d869f",
        "https://www.npmjs.com/login?next=/login/cli/d907c402-4ad4",
        "ftp://files.example.org/x",
        "/usr/local/bin/tool",
        "./src/main.rs:12:3",
        "../lib/mod.rs",
        "~/notes.md",
        ".github/workflows/ci.yml",
        "remote:",
        "(docs/data-flow)",
        "\u{2014} S3 bucket",
        "Press ENTER",
        "- next item",
        "2. ",
        "word",
        "pull/",
        "567",
        "rastructure/pull/61",
        "http://",
        "://",
        ")",
        ",",
        ".",
        "  ",
        " ",
    ];

    /// Row pairs that split a URL the way a TUI does (no WRAPLINE).
    const TUI_WRAPS: &[(&str, &str)] = &[
        ("  https://github.com/contember/webmaster/pull/56", "  7"),
        ("- https://claude.ai/code/sess_ABC", "  DEF123"),
        (
            "  https://github.com/NPI-Cloud/npi-inf",
            "  rastructure/pull/65)",
        ),
        (
            "  http://localhost:19400/s/1f41d02d-6105-45fb-b3",
            "  b1-4b56ae4d869f \u{2014} take your time.",
        ),
        (
            "  https://github.com/contember/webmaster/tree/feat/brow",
            "  ser-sentry",
        ),
        (
            "  https://github.com/NPI-Cloud/npi-docs/pull/4",
            "  (docs/data-flow-findings \u{2192} main).",
        ),
    ];

    fn random_text(rng: &mut Rng) -> String {
        let mut text = String::new();
        for _ in 0..(1 + rng.below(5)) {
            text.push_str(rng.pick(TOKENS));
            if rng.one_in(2) {
                text.push(' ');
            }
        }
        text
    }

    fn write_at(terminal: &Terminal, row: usize, col: usize, text: &str) {
        terminal.process_output(format!("\x1b[{};{}H{}", row + 1, col + 1, text).as_bytes());
    }

    #[derive(Debug)]
    enum Edit {
        Write {
            row: usize,
            col: usize,
            text: String,
        },
        ClearRow(usize),
        TuiWrap {
            row: usize,
            pair: usize,
        },
        Newlines(usize),
        Scroll(i32),
        Resize {
            cols: u16,
            rows: u16,
        },
    }

    fn random_edit(
        rng: &mut Rng,
        rows: usize,
        cols: usize,
        matches: &[super::DetectedLink],
    ) -> Edit {
        match rng.below(12) {
            0 => Edit::ClearRow(rng.below(rows)),
            1 => Edit::TuiWrap {
                row: rng.below(rows.saturating_sub(1)),
                pair: rng.below(TUI_WRAPS.len()),
            },
            2 => Edit::Newlines(1 + rng.below(3)),
            3 => Edit::Scroll(i32::try_from(rng.below(7)).unwrap_or(0) - 3),
            4 if rng.one_in(3) => Edit::Resize {
                cols: u16::try_from(30 + rng.below(40)).unwrap_or(40),
                rows: u16::try_from(6 + rng.below(8)).unwrap_or(8),
            },
            // Edits next to an existing match: the boundaries the wrap rules watch.
            5..=8 if !matches.is_empty() => {
                let link = &matches[rng.below(matches.len())];
                let row = usize::try_from(link.line).unwrap_or(0);
                let (row, col) = match rng.below(4) {
                    0 => (row + 1, rng.below(4)),
                    1 => (row.saturating_sub(1), rng.below(cols)),
                    2 => (row, link.col + link.len),
                    _ => (row, link.col + rng.below(link.len.max(1))),
                };
                Edit::Write {
                    row: row.min(rows - 1),
                    col: col.min(cols - 1),
                    text: rng.pick(TOKENS).to_string(),
                }
            }
            _ => Edit::Write {
                row: rng.below(rows),
                col: rng.below(cols),
                text: random_text(rng),
            },
        }
    }

    fn apply(terminal: &Terminal, edit: &Edit, rows: usize) {
        match edit {
            Edit::Write { row, col, text } => write_at(terminal, *row, *col, text),
            Edit::ClearRow(row) => write_at(terminal, *row, 0, "\x1b[2K"),
            Edit::TuiWrap { row, pair } => {
                let (first, second) = TUI_WRAPS[*pair];
                write_at(terminal, *row, 0, &format!("\x1b[2K{first}"));
                write_at(terminal, *row + 1, 0, &format!("\x1b[2K{second}"));
            }
            Edit::Newlines(count) => {
                write_at(terminal, rows - 1, 0, &"\r\n".repeat(*count));
            }
            Edit::Scroll(delta) => terminal.scroll(*delta),
            Edit::Resize { cols, rows } => terminal.resize(TerminalSize {
                cols: *cols,
                rows: *rows,
                cell_width: 8.0,
                cell_height: 16.0,
            }),
        }
    }

    fn detected(text: &str, cols: u16) -> Vec<super::DetectedLink> {
        let terminal = Terminal::new(
            "links".into(),
            TerminalSize {
                cols,
                rows: 3,
                cell_width: 8.0,
                cell_height: 16.0,
            },
            Arc::new(NullTransport),
            "/tmp".into(),
        );
        terminal.process_output(text.as_bytes());
        terminal.detect_urls()
    }

    #[test]
    fn a_url_carrying_a_combining_mark_keeps_it_and_its_columns() {
        let links = detected("see https://example.com/cafe\u{0301}x rest", 60);

        assert_eq!(links.len(), 1);
        assert_eq!(links[0].text, "https://example.com/cafe\u{0301}x");
        assert_eq!(
            (links[0].col, links[0].len),
            (4, 25),
            "the mark rides on its base cell and takes no column"
        );
    }

    #[test]
    fn a_wide_char_before_a_path_shifts_it_by_both_of_its_columns() {
        let links = detected("\u{65e5}\u{672c} /usr/bin/x", 40);

        assert_eq!(links.len(), 1);
        assert_eq!(links[0].text, "/usr/bin/x");
        assert_eq!((links[0].col, links[0].len), (5, 10));
    }

    #[test]
    fn incremental_detection_matches_a_full_scan_after_every_edit() {
        let mut reused_lines = 0usize;
        let mut wrapped_scans = 0usize;
        for seed in 1..=6u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
            let mut size = TerminalSize {
                cols: u16::try_from(40 + rng.below(30)).unwrap_or(50),
                rows: u16::try_from(8 + rng.below(8)).unwrap_or(10),
                cell_width: 8.0,
                cell_height: 16.0,
            };
            let terminal =
                Terminal::new("links".into(), size, Arc::new(NullTransport), "/tmp".into());
            for row in 0..usize::from(size.rows) {
                write_at(&terminal, row, 0, &random_text(&mut rng));
            }

            let mut cache = UrlScanCache::default();
            let mut matches = terminal.detect_urls_with(&mut cache);
            assert_eq!(matches, terminal.detect_urls(), "seed {seed}: initial scan");

            for step in 0..300 {
                let edit = random_edit(
                    &mut rng,
                    usize::from(size.rows),
                    usize::from(size.cols),
                    &matches,
                );
                apply(&terminal, &edit, usize::from(size.rows));
                if let Edit::Resize { cols, rows } = edit {
                    size.cols = cols;
                    size.rows = rows;
                }

                matches = terminal.detect_urls_with(&mut cache);
                let full = terminal.detect_urls();
                assert_eq!(
                    matches, full,
                    "seed {seed}, step {step}, after {edit:?}: incremental scan differs"
                );
                reused_lines += cache.lines.len() - cache.recomputed_lines;
                let spans_rows = |link: &super::DetectedLink| {
                    full.iter()
                        .any(|other| other.wrap_group == link.wrap_group && other.line != link.line)
                };
                if full.iter().any(spans_rows) {
                    wrapped_scans += 1;
                }
            }
        }
        assert!(reused_lines > 0, "the cache never reused a logical line");
        assert!(wrapped_scans > 0, "no scan ever saw a URL spanning rows");
    }

    #[test]
    fn an_unchanged_screen_rescans_nothing() {
        let terminal = Terminal::new(
            "links".into(),
            TerminalSize {
                cols: 40,
                rows: 4,
                cell_width: 8.0,
                cell_height: 16.0,
            },
            Arc::new(NullTransport),
            "/tmp".into(),
        );
        terminal.process_output(b"see https://example.com/a\r\nand /usr/bin\r\n");
        let mut cache = UrlScanCache::default();
        let first = terminal.detect_urls_with(&mut cache);
        assert_eq!(cache.recomputed_lines, 4);

        let second = terminal.detect_urls_with(&mut cache);
        assert_eq!(cache.recomputed_lines, 0);
        assert_eq!(first, second);
    }

    #[test]
    fn an_edit_rescans_only_its_logical_line_and_the_urls_that_read_it() {
        let terminal = Terminal::new(
            "links".into(),
            TerminalSize {
                cols: 48,
                rows: 5,
                cell_width: 8.0,
                cell_height: 16.0,
            },
            Arc::new(NullTransport),
            "/tmp".into(),
        );
        // Row 0 holds a TUI-wrapped URL whose extension reads row 1.
        terminal.process_output(
            b"  https://github.com/contember/webmaster/pull/56\r\n  7\r\nplain\r\nplain\r\nplain",
        );
        let mut cache = UrlScanCache::default();
        let before = terminal.detect_urls_with(&mut cache);
        assert_eq!(
            before[0].text,
            "https://github.com/contember/webmaster/pull/567"
        );

        write_at(&terminal, 3, 0, "changed");
        terminal.detect_urls_with(&mut cache);
        assert_eq!(cache.recomputed_lines, 1, "only row 3 is rescanned");

        write_at(&terminal, 1, 0, "  8");
        let after = terminal.detect_urls_with(&mut cache);
        assert_eq!(
            cache.recomputed_lines, 2,
            "row 1 and the URL on row 0 that extends into it"
        );
        assert_eq!(
            after[0].text,
            "https://github.com/contember/webmaster/pull/568"
        );
        assert_eq!(after, terminal.detect_urls());
    }
}
