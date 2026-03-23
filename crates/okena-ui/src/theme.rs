//! Theme helpers and re-exports.
//!
//! Re-exports core theme types from okena-core and provides GPUI-specific
//! color conversion utilities.

use gpui::*;

// Re-export core theme types
pub use okena_core::theme::{
    FolderColor, ThemeColors, ThemeInfo, ThemeMode, DARK_THEME, HIGH_CONTRAST_THEME, LIGHT_THEME,
    PASTEL_DARK_THEME,
};

/// Create an hsla color from a hex color with custom alpha.
pub fn with_alpha(hex: u32, alpha: f32) -> Hsla {
    let rgba = rgb(hex);
    Hsla::from(Rgba { a: alpha, ..rgba })
}

/// Global theme provider -- a function pointer that reads the current theme colors.
/// The host app registers this at startup; crate views call `theme()` to read colors.
pub struct GlobalThemeProvider(pub fn(&App) -> ThemeColors);

impl Global for GlobalThemeProvider {}

/// Get current theme colors.
/// Panics if `GlobalThemeProvider` has not been set by the host app.
pub fn theme(cx: &App) -> ThemeColors {
    (cx.global::<GlobalThemeProvider>().0)(cx)
}

/// Convert ANSI color to GPUI Hsla using theme colors.
/// GPUI-specific wrapper around `ThemeColors::ansi_to_argb`.
pub fn ansi_to_hsla(theme: &ThemeColors, color: &alacritty_terminal::vte::ansi::Color) -> Hsla {
    let argb = theme.ansi_to_argb(color);
    let r = ((argb >> 16) & 0xFF) as f32 / 255.0;
    let g = ((argb >> 8) & 0xFF) as f32 / 255.0;
    let b = (argb & 0xFF) as f32 / 255.0;
    Hsla::from(Rgba { r, g, b, a: 1.0 })
}
