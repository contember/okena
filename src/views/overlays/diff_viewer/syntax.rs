//! Syntax highlighting for the diff viewer.

use super::types::{DiffDisplayFile, DisplayLine, HighlightedSpan};
use crate::git::{DiffLineType, FileDiff};
use gpui::Rgba;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

/// Map file extension to syntax name.
pub fn map_extension_to_syntax(ext: &str) -> Option<&'static str> {
    match ext.to_lowercase().as_str() {
        "ts" | "tsx" | "mts" | "cts" => Some("js"),
        "jsx" | "mjs" | "cjs" => Some("js"),
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

/// Highlight a line of code.
pub fn highlight_line(
    content: &str,
    highlighter: &mut HighlightLines,
    syntax_set: &SyntaxSet,
) -> Vec<HighlightedSpan> {
    let default_color = Rgba {
        r: 0.8,
        g: 0.8,
        b: 0.8,
        a: 1.0,
    };

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
                let processed = text.replace('\t', "    ");
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
            color: default_color,
            text: content.replace('\t', "    "),
        }],
    }
}

/// Process a single file into display format with syntax highlighting.
pub fn process_file(
    file: &FileDiff,
    max_line_num: &mut usize,
    syntax_set: &SyntaxSet,
    theme_set: &ThemeSet,
) -> DiffDisplayFile {
    let mut lines = Vec::new();
    let path = file.display_name();

    // Get syntax highlighter for this file
    let syntax = get_syntax_for_path(path, syntax_set);
    let theme = &theme_set.themes["base16-ocean.dark"];

    for hunk in &file.hunks {
        // Create fresh highlighters for each hunk since hunks may be from
        // different parts of the file with different syntax states.
        // We use separate highlighters for added and removed lines because
        // they come from different versions of the file and have different contexts.
        // Context lines use the "added" (new file) highlighter.
        let mut highlighter_added = HighlightLines::new(syntax, theme);
        let mut highlighter_removed = HighlightLines::new(syntax, theme);

        for line in &hunk.lines {
            if let Some(num) = line.old_line_num {
                *max_line_num = (*max_line_num).max(num);
            }
            if let Some(num) = line.new_line_num {
                *max_line_num = (*max_line_num).max(num);
            }

            // For header lines, don't highlight
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
                // Use separate highlighters for added vs removed lines to maintain
                // proper syntax state for each version of the file
                let highlighter = match line.line_type {
                    DiffLineType::Removed => &mut highlighter_removed,
                    _ => &mut highlighter_added, // Added and Context use the "new" file highlighter
                };
                let spans = highlight_line(&line.content, highlighter, syntax_set);
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

    DiffDisplayFile {
        path: path.to_string(),
        added: file.lines_added,
        removed: file.lines_removed,
        lines,
        is_binary: file.is_binary,
        is_new: file.old_path.is_none(),
        is_deleted: file.new_path.is_none(),
    }
}
