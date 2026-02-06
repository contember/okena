//! Syntax highlighting for the diff viewer.

use super::types::{DiffDisplayFile, DisplayLine, HighlightedSpan};
use crate::git::{get_file_contents_for_diff, DiffLineType, DiffMode, FileDiff};
use gpui::Rgba;
use std::collections::HashMap;
use std::path::Path;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// Map file extension to syntax name.
pub fn map_extension_to_syntax(ext: &str) -> Option<&'static str> {
    match ext.to_lowercase().as_str() {
        "ts" | "mts" | "cts" => Some("ts"),
        "tsx" => Some("tsx"),
        "jsx" => Some("tsx"), // JSX uses TypeScriptReact syntax (best JSX support)
        "mjs" | "cjs" => Some("js"),
        "vue" | "svelte" => Some("html"),
        "yml" | "yaml" => Some("yaml"),
        "json" | "jsonc" | "json5" => Some("json"),
        "toml" => Some("toml"),
        "ini" | "cfg" | "conf" => Some("ini"),
        "sh" | "bash" | "zsh" | "fish" => Some("sh"),
        "ps1" | "psm1" | "psd1" => Some("ps1"),
        "html" | "htm" | "xhtml" => Some("html"),
        "css" | "scss" | "sass" | "less" => Some("css"),
        "xml" | "svg" | "xsl" | "xslt" => Some("xml"),
        "py" | "pyw" | "pyi" => Some("py"),
        "rb" | "erb" | "rake" => Some("rb"),
        "rs" => Some("rs"),
        "go" => Some("go"),
        "c" | "h" => Some("c"),
        "cpp" | "cc" | "cxx" | "hpp" | "hxx" | "hh" => Some("cpp"),
        "java" => Some("java"),
        "kt" | "kts" => Some("kt"),
        "swift" => Some("swift"),
        "cs" => Some("cs"),
        "php" => Some("php"),
        "pl" | "pm" => Some("pl"),
        "lua" => Some("lua"),
        "sql" => Some("sql"),
        "md" | "markdown" => Some("md"),
        "tex" | "latex" => Some("tex"),
        _ => None,
    }
}

/// Get syntax for a file path.
pub fn get_syntax_for_path<'a>(
    path: &str,
    syntax_set: &'a SyntaxSet,
) -> &'a syntect::parsing::SyntaxReference {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str());

    ext.and_then(|e| map_extension_to_syntax(e))
        .and_then(|mapped| syntax_set.find_syntax_by_extension(mapped))
        .or_else(|| ext.and_then(|e| syntax_set.find_syntax_by_extension(e)))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text())
}

/// Default text color for unhighlighted content.
fn default_color() -> Rgba {
    Rgba {
        r: 0.8,
        g: 0.8,
        b: 0.8,
        a: 1.0,
    }
}

/// Highlight a line of code and return spans.
fn highlight_line_to_spans(
    content: &str,
    highlighter: &mut HighlightLines,
    syntax_set: &SyntaxSet,
) -> Vec<HighlightedSpan> {
    match highlighter.highlight_line(content, syntax_set) {
        Ok(spans) => {
            let mut result = Vec::new();
            for (style, text) in spans {
                let color = Rgba {
                    r: style.foreground.r as f32 / 255.0,
                    g: style.foreground.g as f32 / 255.0,
                    b: style.foreground.b as f32 / 255.0,
                    a: style.foreground.a as f32 / 255.0,
                };
                // Strip newlines and expand tabs
                let processed = text
                    .trim_end_matches(&['\n', '\r'][..])
                    .replace('\t', "    ");
                if !processed.is_empty() {
                    result.push(HighlightedSpan {
                        color,
                        text: processed,
                    });
                }
            }
            result
        }
        Err(_) => vec![HighlightedSpan {
            color: default_color(),
            text: content
                .trim_end_matches(&['\n', '\r'][..])
                .replace('\t', "    "),
        }],
    }
}

/// Pre-highlight an entire file and return a map of line number -> spans.
/// Line numbers are 1-based to match git diff line numbers.
fn highlight_full_file(
    content: &str,
    syntax: &syntect::parsing::SyntaxReference,
    theme: &syntect::highlighting::Theme,
    syntax_set: &SyntaxSet,
) -> HashMap<usize, Vec<HighlightedSpan>> {
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut result = HashMap::new();

    // Use LinesWithEndings to preserve newlines - syntect needs them for proper state tracking
    for (idx, line) in LinesWithEndings::from(content).enumerate() {
        let line_num = idx + 1; // 1-based line numbers
        let spans = highlight_line_to_spans(line, &mut highlighter, syntax_set);
        result.insert(line_num, spans);
    }

    result
}

/// Create a fallback span for content without highlighting.
fn fallback_spans(content: &str) -> Vec<HighlightedSpan> {
    vec![HighlightedSpan {
        color: default_color(),
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
    theme_set: &ThemeSet,
    repo_path: &Path,
    diff_mode: DiffMode,
) -> DiffDisplayFile {
    let mut lines = Vec::new();
    let path = file.display_name();

    // Get syntax highlighter for this file
    let syntax = get_syntax_for_path(path, syntax_set);
    let theme = &theme_set.themes["base16-ocean.dark"];

    // Fetch and pre-highlight the full file content for both old and new versions.
    // This ensures correct syntax state for all hunks, even those starting mid-file.
    let (old_content, new_content) = get_file_contents_for_diff(repo_path, path, diff_mode);

    let old_highlighted = match old_content.as_ref() {
        Some(content) => highlight_full_file(content, syntax, theme, syntax_set),
        None => HashMap::new(),
    };

    let new_highlighted = match new_content.as_ref() {
        Some(content) => highlight_full_file(content, syntax, theme, syntax_set),
        None => HashMap::new(),
    };

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
                            .unwrap_or_else(|| fallback_spans(&line.content))
                    }
                    DiffLineType::Added => {
                        // Added lines come from the new version
                        line.new_line_num
                            .and_then(|num| new_highlighted.get(&num).cloned())
                            .unwrap_or_else(|| fallback_spans(&line.content))
                    }
                    DiffLineType::Context => {
                        // Context lines exist in both - prefer new version
                        line.new_line_num
                            .and_then(|num| new_highlighted.get(&num).cloned())
                            .or_else(|| {
                                line.old_line_num
                                    .and_then(|num| old_highlighted.get(&num).cloned())
                            })
                            .unwrap_or_else(|| fallback_spans(&line.content))
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

    DiffDisplayFile { lines }
}
