//! Theme color definitions
//!
//! Re-exports ThemeColors and built-in theme constants from okena-core.
//! Provides GPUI-specific `ansi_to_hsla` conversion.

use gpui::*;

pub use okena_core::theme::{
    ThemeColors, DARK_THEME, HIGH_CONTRAST_THEME, LIGHT_THEME, PASTEL_DARK_THEME,
};

/// Convert ANSI color to GPUI Hsla using theme colors.
/// GPUI-specific wrapper around `ThemeColors::ansi_to_argb`.
pub fn ansi_to_hsla(theme: &ThemeColors, color: &alacritty_terminal::vte::ansi::Color) -> Hsla {
    let argb = theme.ansi_to_argb(color);
    let r = ((argb >> 16) & 0xFF) as f32 / 255.0;
    let g = ((argb >> 8) & 0xFF) as f32 / 255.0;
    let b = (argb & 0xFF) as f32 / 255.0;
    Hsla::from(Rgba { r, g, b, a: 1.0 })
}
