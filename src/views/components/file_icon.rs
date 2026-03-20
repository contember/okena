//! File type icon badges — small colored letter indicators for common file types.
//!
//! Renders a rounded badge with a 1-2 character label. The background is the
//! accent color at low opacity, so badges look good on both light and dark themes.

use gpui::*;

/// Style for a file type icon.
struct FileIconStyle {
    /// Accent color (used for text; background is this color at 15% opacity).
    color: u32,
    /// 1-2 character label.
    label: &'static str,
}

/// Return the icon style for a filename based on its extension.
fn icon_style_for(filename: &str) -> FileIconStyle {
    let ext = filename.rsplit('.').next().unwrap_or("");
    match ext.to_ascii_lowercase().as_str() {
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
        // Shell
        "sh" | "bash" | "zsh" | "fish" => FileIconStyle { color: 0x4eaa25, label: "$" },
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
        // Lock files
        "lock" => FileIconStyle { color: 0x808080, label: "lk" },
        // Config / env
        "env" | "ini" | "cfg" => FileIconStyle { color: 0x808080, label: "cf" },
        // Images
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "bmp" => {
            FileIconStyle { color: 0x4eaa25, label: "im" }
        }
        // Default — no icon
        _ => FileIconStyle { color: 0, label: "" },
    }
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
    if lower == "cargo.toml" {
        return Some(FileIconStyle { color: 0xf0813a, label: "cg" });
    }
    if lower == ".gitignore" || lower == ".gitattributes" {
        return Some(FileIconStyle { color: 0xe44d26, label: "gi" });
    }
    None
}

/// Build an RGBA color from a u32 RGB color and an alpha value (0.0–1.0).
fn color_with_alpha(color: u32, alpha: f32) -> Rgba {
    let r = ((color >> 16) & 0xff) as f32 / 255.0;
    let g = ((color >> 8) & 0xff) as f32 / 255.0;
    let b = (color & 0xff) as f32 / 255.0;
    rgba(r, g, b, alpha)
}

/// Render a small file-type icon badge for the given filename.
///
/// Returns a 16×16 rounded div with a tinted background and a bold letter label.
/// Unknown file types get an invisible spacer of the same size to keep alignment.
pub fn render_file_icon(filename: &str) -> Div {
    let style = icon_style_for_special(filename).unwrap_or_else(|| icon_style_for(filename));

    if style.label.is_empty() {
        return div().size(px(16.0)).flex_shrink_0();
    }

    div()
        .size(px(16.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.0))
        .bg(color_with_alpha(style.color, 0.15))
        .child(
            div()
                .text_size(px(8.5))
                .line_height(px(16.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(style.color))
                .child(style.label),
        )
}
