//! Syntax highlighting for the diff viewer.

use super::types::{DiffDisplayFile, DisplayLine, HighlightedSpan};
use okena_git::{DiffLineType, FileDiff};
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
    let mut lines = Vec::new();
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

    for hunk in &file.hunks {
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

            lines.push(DisplayLine {
                line_type: line.line_type,
                old_line_num: line.old_line_num,
                new_line_num: line.new_line_num,
                spans,
                plain_text,
            });
        }
    }

    log::debug!("[process_file] total: {:?}, display lines: {}, file: {}", t_total.elapsed(), lines.len(), path);
    DiffDisplayFile { lines }
}
