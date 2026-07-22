//! Theme helpers — re-exported from okena-theme.
pub use okena_theme::{
    DARK_THEME,
    // Core types (via okena-theme which re-exports from okena-core)
    FolderColor,
    GlobalThemeProvider,
    HIGH_CONTRAST_THEME,
    LIGHT_THEME,
    PASTEL_DARK_THEME,
    ThemeColors,
    ThemeInfo,
    ThemeMode,
    ansi_to_hsla,
    theme,
    // GPUI helpers
    with_alpha,
};
