//! Theme module
//!
//! Provides theming support including built-in themes, custom themes, and color management.

mod colors;
mod custom;
mod types;

// Re-export public API
pub use colors::{
    ThemeColors, DARK_THEME, HIGH_CONTRAST_THEME, LIGHT_THEME, PASTEL_DARK_THEME,
};
#[allow(unused_imports)]
pub use custom::{
    CustomThemeColors, CustomThemeConfig, get_themes_dir, load_custom_themes,
};
pub use types::{FolderColor, ThemeInfo, ThemeMode, with_alpha};

use gpui::*;

/// Global theme state
pub struct AppTheme {
    pub mode: ThemeMode,
    pub colors: ThemeColors,
    system_is_dark: bool,
    /// Custom theme colors (when mode is Custom)
    custom_colors: Option<ThemeColors>,
    /// Preview colors for live preview (temporarily overrides colors)
    preview_colors: Option<ThemeColors>,
}

impl AppTheme {
    pub fn new(mode: ThemeMode, system_is_dark: bool) -> Self {
        let colors = Self::colors_for_mode(mode, system_is_dark, None);
        Self {
            mode,
            colors,
            system_is_dark,
            custom_colors: None,
            preview_colors: None,
        }
    }

    fn colors_for_mode(mode: ThemeMode, system_is_dark: bool, custom: Option<ThemeColors>) -> ThemeColors {
        match mode {
            ThemeMode::Dark => DARK_THEME,
            ThemeMode::Light => LIGHT_THEME,
            ThemeMode::PastelDark => PASTEL_DARK_THEME,
            ThemeMode::HighContrast => HIGH_CONTRAST_THEME,
            ThemeMode::Custom => custom.unwrap_or(DARK_THEME),
            ThemeMode::Auto => {
                if system_is_dark {
                    DARK_THEME
                } else {
                    LIGHT_THEME
                }
            }
        }
    }

    pub fn set_mode(&mut self, mode: ThemeMode) {
        self.mode = mode;
        self.update_colors();
    }

    pub fn set_system_appearance(&mut self, is_dark: bool) {
        self.system_is_dark = is_dark;
        if self.mode == ThemeMode::Auto {
            self.update_colors();
        }
    }

    /// Set custom theme colors
    pub fn set_custom_colors(&mut self, colors: ThemeColors) {
        self.custom_colors = Some(colors);
        if self.mode == ThemeMode::Custom {
            self.update_colors();
        }
    }

    /// Set preview colors temporarily (for live preview)
    pub fn set_preview(&mut self, mode: ThemeMode) {
        self.preview_colors = Some(Self::colors_for_mode(mode, self.system_is_dark, self.custom_colors));
    }

    /// Set preview colors directly (for custom themes)
    pub fn set_preview_colors(&mut self, colors: ThemeColors) {
        self.preview_colors = Some(colors);
    }

    /// Clear preview and restore actual theme
    pub fn clear_preview(&mut self) {
        self.preview_colors = None;
    }

    /// Get the current display colors (preview if set, otherwise actual)
    pub fn display_colors(&self) -> ThemeColors {
        self.preview_colors.unwrap_or(self.colors)
    }

    fn update_colors(&mut self) {
        self.colors = Self::colors_for_mode(self.mode, self.system_is_dark, self.custom_colors);
    }

}

/// Wrapper for global theme entity
pub struct GlobalTheme(pub Entity<AppTheme>);

impl Global for GlobalTheme {}

/// Get the current theme colors from the global theme entity (uses preview if active)
pub fn theme(cx: &App) -> ThemeColors {
    cx.global::<GlobalTheme>().0.read(cx).display_colors()
}

/// Get the theme entity for observation
pub fn theme_entity(cx: &App) -> Entity<AppTheme> {
    cx.global::<GlobalTheme>().0.clone()
}
