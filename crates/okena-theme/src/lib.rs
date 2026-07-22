#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

// Re-export core theme types (source of truth is okena-core)
pub use okena_core::theme::{
    DARK_THEME, FolderColor, HIGH_CONTRAST_THEME, LIGHT_THEME, PASTEL_DARK_THEME, ThemeColors,
    ThemeInfo, ThemeMode,
};

mod app_theme;
pub mod custom;
#[cfg(feature = "gpui")]
mod gpui_helpers;

pub use app_theme::AppTheme;
#[cfg(feature = "gpui")]
pub use app_theme::{GlobalTheme, theme_entity};
pub use custom::{CustomThemeColors, CustomThemeConfig, get_themes_dir, load_custom_themes};
#[cfg(feature = "gpui")]
pub use gpui_helpers::{GlobalThemeProvider, ansi_to_hsla, theme, with_alpha};
