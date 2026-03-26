//! Syntax highlighting for the diff viewer.

use super::types::{DiffDisplayFile, DisplayItem, DisplayLine, ExpanderRow, HighlightedSpan};
use okena_git::{DiffLineType, FileDiff};
use okena_git::diff::DiffHunk;
use okena_files::syntax::{
    default_text_color, get_syntax_for_path, highlight_line, load_syntax_theme,
};
use gpui::Rgba;
use std::collections::HashMap;
use syntect::easy::HighlightLines;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// Pre-highlight an entire file and return a map of line number -> spans.
/// Line numbers are 1-based to match git diff line numbers.
fn highlight_full_file(
    content: &str,
    syntax: &syntect::parsing::SyntaxReference,
    theme: &syntect::highlighting::Theme,
    syntax_set: &SyntaxSet,
    is_dark: bool,
) -> HashMap<usize, Vec<HighlightedSpan>> {
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut result = HashMap::new();

    // Use LinesWithEndings to preserve newlines - syntect needs them for proper state tracking
    for (idx, line) in LinesWithEndings::from(content).enumerate() {
        let line_num = idx + 1; // 1-based line numbers
        let spans = highlight_line(line, &mut highlighter, syntax_set, is_dark);
        result.insert(line_num, spans);
    }

    result
}

/// Create a fallback span for content without highlighting.
fn fallback_spans(content: &str, is_dark: bool) -> Vec<HighlightedSpan> {
    vec![HighlightedSpan {
        color: default_text_color(is_dark),
        text: content.replace('\t', "    "),
    }]
}

/// Process a single file into display format with syntax highlighting.
///
/// This function pre-highlights the full old and new file versions to ensure
/// correct syntax highlighting even for hunks that start mid-file (e.g., inside
/// a function, string literal, or JSX expression).
pub fn process_file(
    file: &FileDiff,
    max_line_num: &mut usize,
    syntax_set: &SyntaxSet,
    old_content: Option<String>,
    new_content: Option<String>,
    is_dark: bool,
) -> DiffDisplayFile {
    let t_total = std::time::Instant::now();
    let path = file.display_name();

    // Get syntax highlighter for this file
    let syntax = get_syntax_for_path(std::path::Path::new(path), syntax_set);
    let theme = load_syntax_theme(is_dark);

    let t1 = std::time::Instant::now();
    let old_highlighted = match old_content.as_ref() {
        Some(content) => highlight_full_file(content, syntax, theme, syntax_set, is_dark),
        None => HashMap::new(),
    };
    log::debug!("[process_file] highlight old: {:?}, lines: {}", t1.elapsed(), old_highlighted.len());

    let t2 = std::time::Instant::now();
    let new_highlighted = match new_content.as_ref() {
        Some(content) => highlight_full_file(content, syntax, theme, syntax_set, is_dark),
        None => HashMap::new(),
    };
    log::debug!("[process_file] highlight new: {:?}, lines: {}", t2.elapsed(), new_highlighted.len());

    let old_line_count = old_content.as_ref().map(|c| c.lines().count()).unwrap_or(0);
    let new_line_count = new_content.as_ref().map(|c| c.lines().count()).unwrap_or(0);

    // Build display lines from hunks, tracking positions for expander insertion
    let mut hunk_items: Vec<Vec<DisplayItem>> = Vec::new();

    for hunk in &file.hunks {
        let mut items = Vec::new();
        for line in &hunk.lines {
            if let Some(num) = line.old_line_num {
                *max_line_num = (*max_line_num).max(num);
            }
            if let Some(num) = line.new_line_num {
                *max_line_num = (*max_line_num).max(num);
            }

            // For header lines, use special styling
            let (spans, plain_text) = if line.line_type == DiffLineType::Header {
                (
                    vec![HighlightedSpan {
                        color: Rgba {
                            r: 0.5,
                            g: 0.6,
                            b: 0.8,
                            a: 1.0,
                        },
                        text: line.content.clone(),
                    }],
                    line.content.clone(),
                )
            } else {
                let plain = line.content.replace('\t', "    ");

                // Look up pre-highlighted spans based on line type and line number
                let spans = match line.line_type {
                    DiffLineType::Removed => {
                        // Removed lines come from the old version
                        line.old_line_num
                            .and_then(|num| old_highlighted.get(&num).cloned())
                            .unwrap_or_else(|| fallback_spans(&line.content, is_dark))
                    }
                    DiffLineType::Added => {
                        // Added lines come from the new version
                        line.new_line_num
                            .and_then(|num| new_highlighted.get(&num).cloned())
                            .unwrap_or_else(|| fallback_spans(&line.content, is_dark))
                    }
                    DiffLineType::Context => {
                        // Context lines exist in both - prefer new version
                        line.new_line_num
                            .and_then(|num| new_highlighted.get(&num).cloned())
                            .or_else(|| {
                                line.old_line_num
                                    .and_then(|num| old_highlighted.get(&num).cloned())
                            })
                            .unwrap_or_else(|| fallback_spans(&line.content, is_dark))
                    }
                    DiffLineType::Header => unreachable!(), // Handled above
                };

                (spans, plain)
            };

            items.push(DisplayItem::Line(DisplayLine {
                line_type: line.line_type,
                old_line_num: line.old_line_num,
                new_line_num: line.new_line_num,
                spans,
                plain_text,
            }));
        }
        hunk_items.push(items);
    }

    // Pre-compute per-hunk last line numbers (before draining)
    let hunk_last_lines: Vec<(usize, usize)> = file.hunks.iter().enumerate()
        .map(|(idx, hunk)| last_line_nums(&hunk_items[idx], hunk))
        .collect();

    // Build the final items list with expanders inserted at gaps
    let mut items: Vec<DisplayItem> = Vec::new();

    for (hunk_idx, hunk) in file.hunks.iter().enumerate() {
        if hunk_idx == 0 {
            // Expander before first hunk (if it doesn't start at line 1)
            let first_old = hunk.old_start;
            let first_new = hunk.new_start;
            // Both sides must have a real gap (>= 1 hidden line)
            if first_old > 1 && first_new > 1 {
                items.push(DisplayItem::Expander(ExpanderRow {
                    old_range: (1, first_old - 1),
                    new_range: (1, first_new - 1),
                }));
            }
        } else {
            // Expander between hunks: gap from end of previous hunk to start of this one
            let (prev_old_end, prev_new_end) = hunk_last_lines[hunk_idx - 1];

            let old_gap_start = prev_old_end + 1;
            let old_gap_end = hunk.old_start.saturating_sub(1);
            let new_gap_start = prev_new_end + 1;
            let new_gap_end = hunk.new_start.saturating_sub(1);

            // Only insert if both sides have a valid positive gap
            if old_gap_start <= old_gap_end && new_gap_start <= new_gap_end {
                items.push(DisplayItem::Expander(ExpanderRow {
                    old_range: (old_gap_start, old_gap_end),
                    new_range: (new_gap_start, new_gap_end),
                }));
            }
        }

        // Add this hunk's items
        items.extend(hunk_items[hunk_idx].drain(..));
    }

    // Expander after last hunk (if file continues beyond)
    if let Some(&(last_old, last_new)) = hunk_last_lines.last() {
        *max_line_num = (*max_line_num).max(old_line_count).max(new_line_count);

        if last_old < old_line_count && last_new < new_line_count {
            items.push(DisplayItem::Expander(ExpanderRow {
                old_range: (last_old + 1, old_line_count),
                new_range: (last_new + 1, new_line_count),
            }));
        }
    }

    log::debug!("[process_file] total: {:?}, display items: {}, file: {}", t_total.elapsed(), items.len(), path);
    DiffDisplayFile {
        items,
        old_highlighted,
        new_highlighted,
        old_line_count,
        new_line_count,
    }
}

/// Find the last old and new line numbers from a hunk's display items.
/// Falls back to hunk start if no line numbers found.
fn last_line_nums(items: &[DisplayItem], hunk: &DiffHunk) -> (usize, usize) {
    let mut last_old = hunk.old_start;
    let mut last_new = hunk.new_start;
    for item in items.iter().rev() {
        if let DisplayItem::Line(line) = item {
            if line.line_type == DiffLineType::Header {
                continue;
            }
            if let Some(n) = line.old_line_num {
                last_old = last_old.max(n);
            }
            if let Some(n) = line.new_line_num {
                last_new = last_new.max(n);
            }
            break;
        }
    }
    (last_old, last_new)
}
