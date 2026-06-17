//! Theme module — re-exports from okena-theme crate.

// Re-export everything from okena-theme
#[allow(unused_imports)]
pub use okena_theme::{
    ThemeColors, ThemeInfo, ThemeMode, FolderColor,
    DARK_THEME, LIGHT_THEME, PASTEL_DARK_THEME, HIGH_CONTRAST_THEME,
    with_alpha, ansi_to_hsla,
    AppTheme, GlobalTheme, theme_entity,
    CustomThemeConfig, CustomThemeColors, get_themes_dir, load_custom_themes,
};

use gpui::*;

/// Get the current theme colors from the global theme entity (uses preview if active).
/// This is the desktop app's theme() — reads from GlobalTheme entity directly.
/// Different from okena_theme::theme() which uses GlobalThemeProvider function pointer.
pub fn theme(cx: &App) -> ThemeColors {
    cx.global::<GlobalTheme>().0.read(cx).display_colors()
}
