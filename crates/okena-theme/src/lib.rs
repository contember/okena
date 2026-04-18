#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

// Re-export core theme types (source of truth is okena-core)
pub use okena_core::theme::{
    ThemeColors, ThemeInfo, ThemeMode, FolderColor,
    DARK_THEME, LIGHT_THEME, PASTEL_DARK_THEME, HIGH_CONTRAST_THEME,
};

pub mod custom;
mod gpui_helpers;
mod app_theme;

pub use gpui_helpers::{with_alpha, ansi_to_hsla, GlobalThemeProvider, theme};
pub use app_theme::{AppTheme, GlobalTheme, theme_entity};
pub use custom::{CustomThemeConfig, CustomThemeColors, get_themes_dir, load_custom_themes};
