//! Theme helpers — re-exported from okena-theme.
pub use okena_theme::{
    // Core types (via okena-theme which re-exports from okena-core)
    FolderColor, ThemeColors, ThemeInfo, ThemeMode,
    DARK_THEME, HIGH_CONTRAST_THEME, LIGHT_THEME, PASTEL_DARK_THEME,
    // GPUI helpers
    with_alpha, ansi_to_hsla, GlobalThemeProvider, theme,
};
