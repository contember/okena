//! Shared syntax highlighting utilities.
//!
//! Provides types and functions for syntax highlighting that can be used
//! across different viewers (file viewer, diff viewer, etc.).

use gpui::Rgba;
use std::path::Path;
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::Theme;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// Global cached syntax set with extended syntaxes (including TypeScript/TSX).
static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();

/// Global cached syntax highlighting theme.
static SYNTAX_THEME: OnceLock<Theme> = OnceLock::new();

/// Load a SyntaxSet with extended syntaxes including TypeScript/TSX.
/// Uses the two-face crate which provides many additional syntaxes.
pub fn load_syntax_set() -> SyntaxSet {
    SYNTAX_SET
        .get_or_init(|| two_face::syntax::extra_newlines())
        .clone()
}

/// Load the syntax highlighting theme (cached).
/// Uses Dracula theme from two-face for vibrant, modern syntax colors.
pub fn load_syntax_theme() -> &'static Theme {
    SYNTAX_THEME.get_or_init(|| {
        let theme_set = two_face::theme::extra();
        theme_set
            .get(two_face::theme::EmbeddedThemeName::Dracula)
            .clone()
    })
}

/// A pre-processed span with color and text ready for display.
#[derive(Clone)]
pub struct HighlightedSpan {
    pub color: Rgba,
    pub text: String,
}

/// A highlighted line with pre-processed spans.
#[derive(Clone)]
pub struct HighlightedLine {
    pub spans: Vec<HighlightedSpan>,
    /// Plain text content of the line (for selection/copy).
    pub plain_text: String,
}

/// Default text color for fallback.
pub fn default_text_color() -> Rgba {
    Rgba {
        r: 0.8,
        g: 0.8,
        b: 0.8,
        a: 1.0,
    }
}

/// Map file extension to syntax name for better coverage.
pub fn map_extension_to_syntax(ext: &str) -> Option<&'static str> {
    match ext.to_lowercase().as_str() {
        // TypeScript/JavaScript variants
        "ts" | "mts" | "cts" => Some("ts"),
        "tsx" => Some("tsx"),
        "jsx" => Some("tsx"), // JSX uses TypeScriptReact syntax (best JSX support)
        "mjs" | "cjs" => Some("js"),
        // Vue/Svelte - use HTML
        "vue" | "svelte" => Some("html"),
        // Config files
        "yml" | "yaml" => Some("yaml"),
        "json" | "jsonc" | "json5" => Some("json"),
        "toml" => Some("toml"),
        "ini" | "cfg" | "conf" => Some("ini"),
        // Shell scripts
        "sh" | "bash" | "zsh" | "fish" => Some("sh"),
        "ps1" | "psm1" | "psd1" => Some("ps1"),
        // Web
        "html" | "htm" | "xhtml" => Some("html"),
        "css" | "scss" | "sass" | "less" => Some("css"),
        "xml" | "svg" | "xsl" | "xslt" => Some("xml"),
        // Common languages
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
        "diff" | "patch" => Some("diff"),
        "dockerfile" => Some("dockerfile"),
        _ => None,
    }
}

/// Get syntax reference for a file path.
pub fn get_syntax_for_path<'a>(
    path: &Path,
    syntax_set: &'a SyntaxSet,
) -> &'a syntect::parsing::SyntaxReference {
    let ext = path.extension().and_then(|e| e.to_str());

    ext.and_then(|e| map_extension_to_syntax(e))
        .and_then(|mapped| syntax_set.find_syntax_by_extension(mapped))
        .or_else(|| ext.and_then(|e| syntax_set.find_syntax_by_extension(e)))
        .or_else(|| {
            // Try by filename for special files
            path.file_name().and_then(|n| n.to_str()).and_then(|name| {
                let name_lower = name.to_lowercase();
                match name_lower.as_str() {
                    "makefile" | "gnumakefile" => syntax_set.find_syntax_by_extension("makefile"),
                    "dockerfile" => syntax_set.find_syntax_by_extension("dockerfile"),
                    "cargo.toml" | "cargo.lock" | "pyproject.toml" => {
                        syntax_set.find_syntax_by_extension("toml")
                    }
                    "package.json" | "tsconfig.json" | "jsconfig.json" => {
                        syntax_set.find_syntax_by_extension("json")
                    }
                    ".gitignore" | ".dockerignore" | ".npmignore" => {
                        syntax_set.find_syntax_by_name("Git Ignore")
                    }
                    ".bashrc" | ".zshrc" | ".bash_profile" | ".profile" => {
                        syntax_set.find_syntax_by_extension("sh")
                    }
                    ".env" | ".env.local" | ".env.development" | ".env.production" => {
                        syntax_set.find_syntax_by_extension("sh")
                    }
                    _ => None,
                }
            })
        })
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text())
}

/// Highlight a single line of code.
pub fn highlight_line(
    content: &str,
    highlighter: &mut HighlightLines,
    syntax_set: &SyntaxSet,
) -> Vec<HighlightedSpan> {
    let default_color = default_text_color();

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
            color: default_color,
            text: content
                .trim_end_matches(&['\n', '\r'][..])
                .replace('\t', "    "),
        }],
    }
}

/// Highlight file content and return a vector of highlighted lines.
///
/// # Arguments
/// * `content` - The file content to highlight
/// * `path` - Path to the file (used for syntax detection)
/// * `syntax_set` - Syntect syntax set
/// * `max_lines` - Maximum number of lines to process (0 = unlimited)
pub fn highlight_content(
    content: &str,
    path: &Path,
    syntax_set: &SyntaxSet,
    max_lines: usize,
) -> Vec<HighlightedLine> {
    let syntax = get_syntax_for_path(path, syntax_set);
    let theme = load_syntax_theme();
    let mut highlighter = HighlightLines::new(syntax, theme);
    let default_color = default_text_color();

    let mut lines = Vec::new();
    let mut line_count = 0;

    for line in LinesWithEndings::from(content) {
        if max_lines > 0 && line_count >= max_lines {
            break;
        }

        let (display_spans, plain_text) = match highlighter.highlight_line(line, syntax_set) {
            Ok(spans) => {
                let mut merged: Vec<HighlightedSpan> = Vec::new();
                let mut plain = String::new();

                for (style, text) in spans {
                    let color = Rgba {
                        r: style.foreground.r as f32 / 255.0,
                        g: style.foreground.g as f32 / 255.0,
                        b: style.foreground.b as f32 / 255.0,
                        a: style.foreground.a as f32 / 255.0,
                    };

                    // Pre-process text: remove newlines, expand tabs
                    let processed = text
                        .trim_end_matches(&['\n', '\r'][..])
                        .replace('\t', "    ");

                    if processed.is_empty() {
                        continue;
                    }

                    plain.push_str(&processed);

                    // Try to merge with previous span if same color
                    if let Some(last) = merged.last_mut() {
                        if (last.color.r - color.r).abs() < 0.01
                            && (last.color.g - color.g).abs() < 0.01
                            && (last.color.b - color.b).abs() < 0.01
                        {
                            last.text.push_str(&processed);
                            continue;
                        }
                    }

                    merged.push(HighlightedSpan {
                        color,
                        text: processed,
                    });
                }

                (merged, plain)
            }
            Err(_) => {
                let text = line
                    .trim_end_matches(&['\n', '\r'][..])
                    .replace('\t', "    ");
                (
                    vec![HighlightedSpan {
                        color: default_color,
                        text: text.clone(),
                    }],
                    text,
                )
            }
        };

        lines.push(HighlightedLine {
            spans: display_spans,
            plain_text,
        });
        line_count += 1;
    }

    lines
}
