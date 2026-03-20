//! File type icon — colored file silhouette with a text label overlay.
//!
//! Renders the classic "page with folded corner" file icon shape, tinted by
//! file type, with a 1-2 character uppercase label positioned at the bottom.

use crate::theme::ThemeColors;
use gpui::*;

/// Style for a file type icon.
struct FileIconStyle {
    /// Tint color for the file shape.
    color: u32,
    /// 1-2 character label shown on the icon.
    label: &'static str,
}

/// Return the icon style for a filename based on its extension.
fn icon_style_for(filename: &str) -> Option<FileIconStyle> {
    let ext = filename.rsplit('.').next().unwrap_or("");
    let style = match ext.to_ascii_lowercase().as_str() {
        // Rust
        "rs" => FileIconStyle { color: 0xf0813a, label: "rs" },
        // JavaScript
        "js" | "mjs" | "cjs" => FileIconStyle { color: 0xd4b830, label: "js" },
        // TypeScript
        "ts" => FileIconStyle { color: 0x3178c6, label: "ts" },
        "tsx" => FileIconStyle { color: 0x3178c6, label: "tx" },
        "jsx" => FileIconStyle { color: 0x51b5d0, label: "jx" },
        // CSS / SCSS
        "css" => FileIconStyle { color: 0x569cd6, label: "cs" },
        "scss" | "sass" => FileIconStyle { color: 0xcd6799, label: "sc" },
        "less" => FileIconStyle { color: 0x569cd6, label: "le" },
        // HTML
        "html" | "htm" => FileIconStyle { color: 0xe44d26, label: "h" },
        // JSON
        "json" | "jsonc" => FileIconStyle { color: 0xcba63c, label: "{}" },
        // YAML / TOML
        "yaml" | "yml" => FileIconStyle { color: 0xc678dd, label: "ym" },
        "toml" => FileIconStyle { color: 0x6dbfb0, label: "tm" },
        // Markdown
        "md" | "mdx" => FileIconStyle { color: 0x569cd6, label: "md" },
        // Python
        "py" => FileIconStyle { color: 0x4b8bbe, label: "py" },
        // Go
        "go" => FileIconStyle { color: 0x00add8, label: "go" },
        // C / C++ / C#
        "c" => FileIconStyle { color: 0x6295cb, label: "c" },
        "cpp" | "cc" | "cxx" => FileIconStyle { color: 0x6295cb, label: "c+" },
        "h" | "hpp" => FileIconStyle { color: 0x6295cb, label: ".h" },
        "cs" => FileIconStyle { color: 0x9b4dca, label: "c#" },
        // Java / Kotlin
        "java" => FileIconStyle { color: 0xe76f00, label: "jv" },
        "kt" | "kts" => FileIconStyle { color: 0x7f52ff, label: "kt" },
        // Ruby
        "rb" => FileIconStyle { color: 0xcc342d, label: "rb" },
        // PHP
        "php" => FileIconStyle { color: 0x8892be, label: "ph" },
        // Shell / PowerShell
        "sh" | "bash" | "zsh" | "fish" => FileIconStyle { color: 0x4eaa25, label: "$" },
        "ps1" | "psm1" | "psd1" => FileIconStyle { color: 0x5391fe, label: "ps" },
        // SQL
        "sql" => FileIconStyle { color: 0xe38c00, label: "sq" },
        // XML / SVG
        "xml" => FileIconStyle { color: 0xe44d26, label: "<>" },
        "svg" => FileIconStyle { color: 0xffb13b, label: "sv" },
        // Lua
        "lua" => FileIconStyle { color: 0x306998, label: "lu" },
        // Swift
        "swift" => FileIconStyle { color: 0xf05138, label: "sw" },
        // Dart
        "dart" => FileIconStyle { color: 0x0175c2, label: "dt" },
        // Zig
        "zig" => FileIconStyle { color: 0xf7a41d, label: "zg" },
        // R
        "r" | "rmd" => FileIconStyle { color: 0x276dc3, label: "R" },
        // Elixir / Erlang
        "ex" | "exs" => FileIconStyle { color: 0x6e4a7e, label: "ex" },
        "erl" => FileIconStyle { color: 0xa90533, label: "er" },
        // Nix
        "nix" => FileIconStyle { color: 0x7ebae4, label: "nx" },
        // Haskell
        "hs" => FileIconStyle { color: 0x5e5086, label: "hs" },
        // Lock files
        "lock" => FileIconStyle { color: 0x808080, label: "lk" },
        // Config / env
        "env" | "ini" | "cfg" | "conf" | "desktop" | "service" => {
            FileIconStyle { color: 0x808080, label: "cf" }
        }
        // Plain text / log
        "txt" | "text" | "log" => FileIconStyle { color: 0x808080, label: "tx" },
        // Archives
        "gz" | "tar" | "zip" | "bz2" | "xz" | "7z" | "rar" => {
            FileIconStyle { color: 0x808080, label: "ar" }
        }
        // Images
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "bmp" => {
            FileIconStyle { color: 0x4eaa25, label: "im" }
        }
        // Fonts
        "ttf" | "otf" | "woff" | "woff2" => FileIconStyle { color: 0x808080, label: "ft" },
        // Default — no match
        _ => return None,
    };
    Some(style)
}

/// Check for special filenames (case-insensitive) before falling back to extension.
fn icon_style_for_special(filename: &str) -> Option<FileIconStyle> {
    let lower = filename.to_ascii_lowercase();
    if lower == "dockerfile" || lower.starts_with("dockerfile.") {
        return Some(FileIconStyle { color: 0x2496ed, label: "dk" });
    }
    if lower == "makefile" || lower == "gnumakefile" {
        return Some(FileIconStyle { color: 0x4eaa25, label: "mk" });
    }
    if lower == "cargo.toml" || lower == "cargo.lock" {
        return Some(FileIconStyle { color: 0xf0813a, label: "cg" });
    }
    if lower == "package.json" || lower == "package-lock.json" {
        return Some(FileIconStyle { color: 0xcc3534, label: "np" });
    }
    if lower == ".gitignore" || lower == ".gitattributes" || lower == ".gitmodules" {
        return Some(FileIconStyle { color: 0xe44d26, label: "gi" });
    }
    if lower == "license" || lower == "licence" || lower.starts_with("license.") || lower.starts_with("licence.") {
        return Some(FileIconStyle { color: 0xcba63c, label: "li" });
    }
    if lower == "readme" || lower.starts_with("readme.") {
        return Some(FileIconStyle { color: 0x569cd6, label: "rm" });
    }
    None
}

/// Render a file-type icon for the given filename.
///
/// Shows the classic file silhouette (page with folded corner) tinted by type,
/// with a small uppercase label overlaid at the bottom center.
/// Unknown types use `text_muted` color with no label.
pub fn file_icon(filename: &str, t: &ThemeColors) -> Div {
    let style = icon_style_for_special(filename).or_else(|| icon_style_for(filename));

    let (color, label) = match &style {
        Some(s) => (s.color, s.label),
        None => (t.text_muted, ""),
    };

    let container = div()
        .relative()
        .size(px(16.0))
        .flex_shrink_0()
        .child(
            svg()
                .path("icons/file-filled.svg")
                .size(px(16.0))
                .text_color(rgb(color)),
        );

    if label.is_empty() {
        container
    } else {
        container.child(
            div()
                .absolute()
                .bottom(px(0.0))
                .left(px(0.0))
                .w(px(16.0))
                .flex()
                .justify_center()
                .child(
                    div()
                        .text_size(px(7.0))
                        .line_height(px(9.0))
                        .font_weight(FontWeight::EXTRA_BOLD)
                        .text_color(rgb(0xffffff))
                        .child(label.to_ascii_uppercase()),
                ),
        )
    }
}
