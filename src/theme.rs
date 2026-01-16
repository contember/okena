use gpui::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Theme mode preference
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Light,
    Dark,
    HighContrast,
    #[default]
    Auto,
    /// Custom theme loaded from configuration
    Custom,
}

/// Available built-in themes
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub is_dark: bool,
}

impl ThemeInfo {
    #[allow(dead_code)]
    pub fn builtin_themes() -> Vec<ThemeInfo> {
        vec![
            ThemeInfo {
                id: "dark".to_string(),
                name: "Dark".to_string(),
                description: "Default dark theme (VSCode-like)".to_string(),
                is_dark: true,
            },
            ThemeInfo {
                id: "light".to_string(),
                name: "Light".to_string(),
                description: "Clean light theme".to_string(),
                is_dark: false,
            },
            ThemeInfo {
                id: "high-contrast".to_string(),
                name: "High Contrast".to_string(),
                description: "High contrast for better visibility".to_string(),
                is_dark: true,
            },
            ThemeInfo {
                id: "auto".to_string(),
                name: "Auto".to_string(),
                description: "Follow system appearance".to_string(),
                is_dark: true, // Default to dark
            },
        ]
    }
}

/// Theme colors - all UI colors in one struct
#[derive(Clone, Copy, Debug)]
pub struct ThemeColors {
    // Background colors
    pub bg_primary: u32,
    pub bg_secondary: u32,
    pub bg_header: u32,
    pub bg_selection: u32,
    pub bg_hover: u32,

    // Border colors
    pub border: u32,
    pub border_active: u32,
    pub border_focused: u32,
    pub border_bell: u32,

    // Text colors
    pub text_primary: u32,
    pub text_secondary: u32,
    pub text_muted: u32,

    // Selection colors
    pub selection_bg: u32,
    pub selection_fg: u32,

    // Search highlight colors
    pub search_match_bg: u32,
    pub search_current_bg: u32,

    // Terminal colors (ANSI)
    pub term_black: u32,
    pub term_red: u32,
    pub term_green: u32,
    pub term_yellow: u32,
    pub term_blue: u32,
    pub term_magenta: u32,
    pub term_cyan: u32,
    pub term_white: u32,
    pub term_bright_black: u32,
    pub term_bright_red: u32,
    pub term_bright_green: u32,
    pub term_bright_yellow: u32,
    pub term_bright_blue: u32,
    pub term_bright_magenta: u32,
    pub term_bright_cyan: u32,
    pub term_bright_white: u32,
    pub term_foreground: u32,
    pub term_background: u32,

    // UI element colors
    pub cursor: u32,
    #[allow(dead_code)] // Reserved for future scrollbar UI
    pub scrollbar: u32,
    #[allow(dead_code)] // Reserved for future scrollbar UI
    pub scrollbar_hover: u32,

    // Status colors
    #[allow(dead_code)] // Reserved for future status indicators
    pub success: u32,
    #[allow(dead_code)] // Reserved for future status indicators
    pub warning: u32,
    #[allow(dead_code)] // Reserved for future status indicators
    pub error: u32,
}

/// Dark theme (VSCode-like)
pub const DARK_THEME: ThemeColors = ThemeColors {
    // Background colors
    bg_primary: 0x1e1e1e,
    bg_secondary: 0x252526,
    bg_header: 0x323233,
    bg_selection: 0x264f78,
    bg_hover: 0x2a2d2e,

    // Border colors
    border: 0x252526,
    border_active: 0x007acc,
    border_focused: 0x569cd6,
    border_bell: 0xe69500,

    // Text colors
    text_primary: 0xcccccc,
    text_secondary: 0x808080,
    text_muted: 0x6a6a6a,

    // Selection colors
    selection_bg: 0x264f78,
    selection_fg: 0xffffff,

    // Search highlight colors
    search_match_bg: 0x613214,    // Dark orange/brown for matches
    search_current_bg: 0xa45a00, // Brighter orange for current match

    // Terminal colors
    term_black: 0x000000,
    term_red: 0xcd3131,
    term_green: 0x0dbc79,
    term_yellow: 0xe5e510,
    term_blue: 0x2472c8,
    term_magenta: 0xbc3fbc,
    term_cyan: 0x11a8cd,
    term_white: 0xe5e5e5,
    term_bright_black: 0x666666,
    term_bright_red: 0xf14c4c,
    term_bright_green: 0x23d18b,
    term_bright_yellow: 0xf5f543,
    term_bright_blue: 0x3b8eea,
    term_bright_magenta: 0xd670d6,
    term_bright_cyan: 0x29b8db,
    term_bright_white: 0xffffff,
    term_foreground: 0xcccccc,
    term_background: 0x1e1e1e,

    // UI element colors
    cursor: 0xaeafad,
    scrollbar: 0x5a5a5a,
    scrollbar_hover: 0x7a7a7a,

    // Status colors
    success: 0x4ec9b0,
    warning: 0xdcdcaa,
    error: 0xf44747,
};

/// Light theme (VSCode Light-like)
pub const LIGHT_THEME: ThemeColors = ThemeColors {
    // Background colors
    bg_primary: 0xffffff,
    bg_secondary: 0xf3f3f3,
    bg_header: 0xe8e8e8,
    bg_selection: 0xadd6ff,
    bg_hover: 0xe8e8e8,

    // Border colors
    border: 0xe5e5e5,
    border_active: 0x007acc,
    border_focused: 0x0078d4,
    border_bell: 0xe69500,

    // Text colors
    text_primary: 0x333333,
    text_secondary: 0x6e6e6e,
    text_muted: 0xa0a0a0,

    // Selection colors
    selection_bg: 0xadd6ff,
    selection_fg: 0x000000,

    // Search highlight colors
    search_match_bg: 0xffd700,    // Yellow for matches
    search_current_bg: 0xff8c00, // Orange for current match

    // Terminal colors (light theme ANSI)
    term_black: 0x000000,
    term_red: 0xcd3131,
    term_green: 0x00bc00,
    term_yellow: 0x949800,
    term_blue: 0x0451a5,
    term_magenta: 0xbc05bc,
    term_cyan: 0x0598bc,
    term_white: 0x555555,
    term_bright_black: 0x666666,
    term_bright_red: 0xcd3131,
    term_bright_green: 0x14ce14,
    term_bright_yellow: 0xb5ba00,
    term_bright_blue: 0x0451a5,
    term_bright_magenta: 0xbc05bc,
    term_bright_cyan: 0x0598bc,
    term_bright_white: 0xa5a5a5,
    term_foreground: 0x333333,
    term_background: 0xffffff,

    // UI element colors
    cursor: 0x000000,
    scrollbar: 0xc1c1c1,
    scrollbar_hover: 0xa0a0a0,

    // Status colors
    success: 0x008000,
    warning: 0x795e26,
    error: 0xa31515,
};

/// High Contrast theme for accessibility
pub const HIGH_CONTRAST_THEME: ThemeColors = ThemeColors {
    // Background colors - pure black for maximum contrast
    bg_primary: 0x000000,
    bg_secondary: 0x0a0a0a,
    bg_header: 0x111111,
    bg_selection: 0x0066cc,
    bg_hover: 0x1a1a1a,

    // Border colors - high visibility
    border: 0x6fc3df,
    border_active: 0x00aaff,
    border_focused: 0xffff00,
    border_bell: 0xff6600,

    // Text colors - pure white and bright colors
    text_primary: 0xffffff,
    text_secondary: 0xe0e0e0,
    text_muted: 0xb0b0b0,

    // Selection colors
    selection_bg: 0x0066cc,
    selection_fg: 0xffffff,

    // Search highlight colors
    search_match_bg: 0xff6600,
    search_current_bg: 0xffff00,

    // Terminal colors (high contrast ANSI)
    term_black: 0x000000,
    term_red: 0xff0000,
    term_green: 0x00ff00,
    term_yellow: 0xffff00,
    term_blue: 0x0080ff,
    term_magenta: 0xff00ff,
    term_cyan: 0x00ffff,
    term_white: 0xffffff,
    term_bright_black: 0x808080,
    term_bright_red: 0xff6666,
    term_bright_green: 0x66ff66,
    term_bright_yellow: 0xffff66,
    term_bright_blue: 0x66b3ff,
    term_bright_magenta: 0xff66ff,
    term_bright_cyan: 0x66ffff,
    term_bright_white: 0xffffff,
    term_foreground: 0xffffff,
    term_background: 0x000000,

    // UI element colors
    cursor: 0xffffff,
    scrollbar: 0x808080,
    scrollbar_hover: 0xa0a0a0,

    // Status colors
    success: 0x00ff00,
    warning: 0xffff00,
    error: 0xff0000,
};

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

    /// Check if current mode is dark
    #[allow(dead_code)]
    pub fn is_dark(&self) -> bool {
        match self.mode {
            ThemeMode::Dark | ThemeMode::HighContrast => true,
            ThemeMode::Light => false,
            ThemeMode::Custom => self.custom_colors.map(|_| true).unwrap_or(true), // Assume dark for custom
            ThemeMode::Auto => self.system_is_dark,
        }
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

impl ThemeColors {
    /// Get RGB tuple from a hex color
    fn hex_to_rgb(hex: u32) -> (u8, u8, u8) {
        (
            ((hex >> 16) & 0xFF) as u8,
            ((hex >> 8) & 0xFF) as u8,
            (hex & 0xFF) as u8,
        )
    }

    /// Get terminal color by ANSI name
    pub fn get_term_color(&self, named: &alacritty_terminal::vte::ansi::NamedColor) -> u32 {
        use alacritty_terminal::vte::ansi::NamedColor;
        match named {
            NamedColor::Black => self.term_black,
            NamedColor::Red => self.term_red,
            NamedColor::Green => self.term_green,
            NamedColor::Yellow => self.term_yellow,
            NamedColor::Blue => self.term_blue,
            NamedColor::Magenta => self.term_magenta,
            NamedColor::Cyan => self.term_cyan,
            NamedColor::White => self.term_white,
            NamedColor::BrightBlack => self.term_bright_black,
            NamedColor::BrightRed => self.term_bright_red,
            NamedColor::BrightGreen => self.term_bright_green,
            NamedColor::BrightYellow => self.term_bright_yellow,
            NamedColor::BrightBlue => self.term_bright_blue,
            NamedColor::BrightMagenta => self.term_bright_magenta,
            NamedColor::BrightCyan => self.term_bright_cyan,
            NamedColor::BrightWhite => self.term_bright_white,
            NamedColor::Foreground => self.term_foreground,
            NamedColor::Background => self.term_background,
            NamedColor::Cursor => self.cursor,
            _ => self.term_foreground,
        }
    }

    /// Convert ANSI color to Hsla using theme colors
    pub fn ansi_to_hsla(&self, color: &alacritty_terminal::vte::ansi::Color) -> Hsla {
        use alacritty_terminal::vte::ansi::Color;
        use alacritty_terminal::vte::ansi::NamedColor;

        match color {
            Color::Named(named) => {
                let hex = self.get_term_color(named);
                let (r, g, b) = Self::hex_to_rgb(hex);
                Hsla::from(Rgba {
                    r: r as f32 / 255.0,
                    g: g as f32 / 255.0,
                    b: b as f32 / 255.0,
                    a: 1.0,
                })
            }
            Color::Spec(rgb_color) => Hsla::from(Rgba {
                r: rgb_color.r as f32 / 255.0,
                g: rgb_color.g as f32 / 255.0,
                b: rgb_color.b as f32 / 255.0,
                a: 1.0,
            }),
            Color::Indexed(idx) => {
                let idx = *idx as usize;
                if idx < 16 {
                    let named = match idx {
                        0 => NamedColor::Black,
                        1 => NamedColor::Red,
                        2 => NamedColor::Green,
                        3 => NamedColor::Yellow,
                        4 => NamedColor::Blue,
                        5 => NamedColor::Magenta,
                        6 => NamedColor::Cyan,
                        7 => NamedColor::White,
                        8 => NamedColor::BrightBlack,
                        9 => NamedColor::BrightRed,
                        10 => NamedColor::BrightGreen,
                        11 => NamedColor::BrightYellow,
                        12 => NamedColor::BrightBlue,
                        13 => NamedColor::BrightMagenta,
                        14 => NamedColor::BrightCyan,
                        15 => NamedColor::BrightWhite,
                        _ => NamedColor::Foreground,
                    };
                    self.ansi_to_hsla(&Color::Named(named))
                } else if idx < 232 {
                    let idx = idx - 16;
                    let r = (idx / 36) * 51;
                    let g = ((idx / 6) % 6) * 51;
                    let b = (idx % 6) * 51;
                    Hsla::from(Rgba {
                        r: r as f32 / 255.0,
                        g: g as f32 / 255.0,
                        b: b as f32 / 255.0,
                        a: 1.0,
                    })
                } else {
                    let gray = (idx - 232) * 10 + 8;
                    Hsla::from(Rgba {
                        r: gray as f32 / 255.0,
                        g: gray as f32 / 255.0,
                        b: gray as f32 / 255.0,
                        a: 1.0,
                    })
                }
            }
        }
    }

}

// =============================================================================
// Custom Theme Configuration Support
// =============================================================================

/// Custom theme configuration file format
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CustomThemeConfig {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub is_dark: bool,
    pub colors: CustomThemeColors,
}

/// Serializable theme colors with hex string format
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CustomThemeColors {
    // Background colors
    #[serde(default = "default_bg_primary")]
    pub bg_primary: String,
    #[serde(default = "default_bg_secondary")]
    pub bg_secondary: String,
    #[serde(default = "default_bg_header")]
    pub bg_header: String,
    #[serde(default = "default_bg_selection")]
    pub bg_selection: String,
    #[serde(default = "default_bg_hover")]
    pub bg_hover: String,

    // Border colors
    #[serde(default = "default_border")]
    pub border: String,
    #[serde(default = "default_border_active")]
    pub border_active: String,
    #[serde(default = "default_border_focused")]
    pub border_focused: String,
    #[serde(default = "default_border_bell")]
    pub border_bell: String,

    // Text colors
    #[serde(default = "default_text_primary")]
    pub text_primary: String,
    #[serde(default = "default_text_secondary")]
    pub text_secondary: String,
    #[serde(default = "default_text_muted")]
    pub text_muted: String,

    // Selection colors
    #[serde(default = "default_selection_bg")]
    pub selection_bg: String,
    #[serde(default = "default_selection_fg")]
    pub selection_fg: String,

    // Search highlight colors
    #[serde(default = "default_search_match_bg")]
    pub search_match_bg: String,
    #[serde(default = "default_search_current_bg")]
    pub search_current_bg: String,

    // Terminal colors
    #[serde(default = "default_term_black")]
    pub term_black: String,
    #[serde(default = "default_term_red")]
    pub term_red: String,
    #[serde(default = "default_term_green")]
    pub term_green: String,
    #[serde(default = "default_term_yellow")]
    pub term_yellow: String,
    #[serde(default = "default_term_blue")]
    pub term_blue: String,
    #[serde(default = "default_term_magenta")]
    pub term_magenta: String,
    #[serde(default = "default_term_cyan")]
    pub term_cyan: String,
    #[serde(default = "default_term_white")]
    pub term_white: String,
    #[serde(default = "default_term_bright_black")]
    pub term_bright_black: String,
    #[serde(default = "default_term_bright_red")]
    pub term_bright_red: String,
    #[serde(default = "default_term_bright_green")]
    pub term_bright_green: String,
    #[serde(default = "default_term_bright_yellow")]
    pub term_bright_yellow: String,
    #[serde(default = "default_term_bright_blue")]
    pub term_bright_blue: String,
    #[serde(default = "default_term_bright_magenta")]
    pub term_bright_magenta: String,
    #[serde(default = "default_term_bright_cyan")]
    pub term_bright_cyan: String,
    #[serde(default = "default_term_bright_white")]
    pub term_bright_white: String,
    #[serde(default = "default_term_foreground")]
    pub term_foreground: String,
    #[serde(default = "default_term_background")]
    pub term_background: String,

    // UI element colors
    #[serde(default = "default_cursor")]
    pub cursor: String,
    #[serde(default = "default_scrollbar")]
    pub scrollbar: String,
    #[serde(default = "default_scrollbar_hover")]
    pub scrollbar_hover: String,

    // Status colors
    #[serde(default = "default_success")]
    pub success: String,
    #[serde(default = "default_warning")]
    pub warning: String,
    #[serde(default = "default_error")]
    pub error: String,
}

// Default color functions for serde (based on dark theme)
fn default_bg_primary() -> String { "#1e1e1e".to_string() }
fn default_bg_secondary() -> String { "#252526".to_string() }
fn default_bg_header() -> String { "#323233".to_string() }
fn default_bg_selection() -> String { "#264f78".to_string() }
fn default_bg_hover() -> String { "#2a2d2e".to_string() }
fn default_border() -> String { "#252526".to_string() }
fn default_border_active() -> String { "#007acc".to_string() }
fn default_border_focused() -> String { "#569cd6".to_string() }
fn default_border_bell() -> String { "#e69500".to_string() }
fn default_text_primary() -> String { "#cccccc".to_string() }
fn default_text_secondary() -> String { "#808080".to_string() }
fn default_text_muted() -> String { "#6a6a6a".to_string() }
fn default_selection_bg() -> String { "#264f78".to_string() }
fn default_selection_fg() -> String { "#ffffff".to_string() }
fn default_search_match_bg() -> String { "#613214".to_string() }
fn default_search_current_bg() -> String { "#a45a00".to_string() }
fn default_term_black() -> String { "#000000".to_string() }
fn default_term_red() -> String { "#cd3131".to_string() }
fn default_term_green() -> String { "#0dbc79".to_string() }
fn default_term_yellow() -> String { "#e5e510".to_string() }
fn default_term_blue() -> String { "#2472c8".to_string() }
fn default_term_magenta() -> String { "#bc3fbc".to_string() }
fn default_term_cyan() -> String { "#11a8cd".to_string() }
fn default_term_white() -> String { "#e5e5e5".to_string() }
fn default_term_bright_black() -> String { "#666666".to_string() }
fn default_term_bright_red() -> String { "#f14c4c".to_string() }
fn default_term_bright_green() -> String { "#23d18b".to_string() }
fn default_term_bright_yellow() -> String { "#f5f543".to_string() }
fn default_term_bright_blue() -> String { "#3b8eea".to_string() }
fn default_term_bright_magenta() -> String { "#d670d6".to_string() }
fn default_term_bright_cyan() -> String { "#29b8db".to_string() }
fn default_term_bright_white() -> String { "#ffffff".to_string() }
fn default_term_foreground() -> String { "#cccccc".to_string() }
fn default_term_background() -> String { "#1e1e1e".to_string() }
fn default_cursor() -> String { "#aeafad".to_string() }
fn default_scrollbar() -> String { "#5a5a5a".to_string() }
fn default_scrollbar_hover() -> String { "#7a7a7a".to_string() }
fn default_success() -> String { "#4ec9b0".to_string() }
fn default_warning() -> String { "#dcdcaa".to_string() }
fn default_error() -> String { "#f44747".to_string() }

impl CustomThemeColors {
    /// Parse a hex color string (e.g., "#1e1e1e" or "1e1e1e") to u32
    fn parse_hex(s: &str) -> u32 {
        let s = s.trim_start_matches('#');
        u32::from_str_radix(s, 16).unwrap_or(0)
    }

    /// Convert to ThemeColors
    pub fn to_theme_colors(&self) -> ThemeColors {
        ThemeColors {
            bg_primary: Self::parse_hex(&self.bg_primary),
            bg_secondary: Self::parse_hex(&self.bg_secondary),
            bg_header: Self::parse_hex(&self.bg_header),
            bg_selection: Self::parse_hex(&self.bg_selection),
            bg_hover: Self::parse_hex(&self.bg_hover),
            border: Self::parse_hex(&self.border),
            border_active: Self::parse_hex(&self.border_active),
            border_focused: Self::parse_hex(&self.border_focused),
            border_bell: Self::parse_hex(&self.border_bell),
            text_primary: Self::parse_hex(&self.text_primary),
            text_secondary: Self::parse_hex(&self.text_secondary),
            text_muted: Self::parse_hex(&self.text_muted),
            selection_bg: Self::parse_hex(&self.selection_bg),
            selection_fg: Self::parse_hex(&self.selection_fg),
            search_match_bg: Self::parse_hex(&self.search_match_bg),
            search_current_bg: Self::parse_hex(&self.search_current_bg),
            term_black: Self::parse_hex(&self.term_black),
            term_red: Self::parse_hex(&self.term_red),
            term_green: Self::parse_hex(&self.term_green),
            term_yellow: Self::parse_hex(&self.term_yellow),
            term_blue: Self::parse_hex(&self.term_blue),
            term_magenta: Self::parse_hex(&self.term_magenta),
            term_cyan: Self::parse_hex(&self.term_cyan),
            term_white: Self::parse_hex(&self.term_white),
            term_bright_black: Self::parse_hex(&self.term_bright_black),
            term_bright_red: Self::parse_hex(&self.term_bright_red),
            term_bright_green: Self::parse_hex(&self.term_bright_green),
            term_bright_yellow: Self::parse_hex(&self.term_bright_yellow),
            term_bright_blue: Self::parse_hex(&self.term_bright_blue),
            term_bright_magenta: Self::parse_hex(&self.term_bright_magenta),
            term_bright_cyan: Self::parse_hex(&self.term_bright_cyan),
            term_bright_white: Self::parse_hex(&self.term_bright_white),
            term_foreground: Self::parse_hex(&self.term_foreground),
            term_background: Self::parse_hex(&self.term_background),
            cursor: Self::parse_hex(&self.cursor),
            scrollbar: Self::parse_hex(&self.scrollbar),
            scrollbar_hover: Self::parse_hex(&self.scrollbar_hover),
            success: Self::parse_hex(&self.success),
            warning: Self::parse_hex(&self.warning),
            error: Self::parse_hex(&self.error),
        }
    }
}

/// Get path to custom themes directory
pub fn get_themes_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("term-manager-rs")
        .join("themes")
}

/// Load custom themes from the themes directory
pub fn load_custom_themes() -> Vec<(ThemeInfo, ThemeColors)> {
    let themes_dir = get_themes_dir();
    let mut custom_themes = Vec::new();

    if !themes_dir.exists() {
        // Create themes directory and example theme
        if let Err(e) = std::fs::create_dir_all(&themes_dir) {
            log::warn!("Failed to create themes directory: {}", e);
            return custom_themes;
        }

        // Write an example custom theme file
        let example_theme = CustomThemeConfig {
            name: "My Custom Theme".to_string(),
            description: "An example custom theme - modify colors as desired".to_string(),
            is_dark: true,
            colors: CustomThemeColors {
                bg_primary: "#1a1a2e".to_string(),
                bg_secondary: "#16213e".to_string(),
                bg_header: "#0f3460".to_string(),
                bg_selection: "#e94560".to_string(),
                bg_hover: "#1f2b4a".to_string(),
                border: "#0f3460".to_string(),
                border_active: "#e94560".to_string(),
                border_focused: "#e94560".to_string(),
                border_bell: "#f39c12".to_string(),
                text_primary: "#eaeaea".to_string(),
                text_secondary: "#a0a0a0".to_string(),
                text_muted: "#707070".to_string(),
                selection_bg: "#e94560".to_string(),
                selection_fg: "#ffffff".to_string(),
                search_match_bg: "#f39c12".to_string(),
                search_current_bg: "#e94560".to_string(),
                term_black: "#000000".to_string(),
                term_red: "#e94560".to_string(),
                term_green: "#0dbc79".to_string(),
                term_yellow: "#f5f543".to_string(),
                term_blue: "#0f3460".to_string(),
                term_magenta: "#bc3fbc".to_string(),
                term_cyan: "#11a8cd".to_string(),
                term_white: "#e5e5e5".to_string(),
                term_bright_black: "#666666".to_string(),
                term_bright_red: "#ff6b6b".to_string(),
                term_bright_green: "#23d18b".to_string(),
                term_bright_yellow: "#f5f543".to_string(),
                term_bright_blue: "#3b8eea".to_string(),
                term_bright_magenta: "#d670d6".to_string(),
                term_bright_cyan: "#29b8db".to_string(),
                term_bright_white: "#ffffff".to_string(),
                term_foreground: "#eaeaea".to_string(),
                term_background: "#1a1a2e".to_string(),
                cursor: "#e94560".to_string(),
                scrollbar: "#5a5a5a".to_string(),
                scrollbar_hover: "#7a7a7a".to_string(),
                success: "#4ec9b0".to_string(),
                warning: "#f39c12".to_string(),
                error: "#e94560".to_string(),
            },
        };

        let example_path = themes_dir.join("example-theme.json");
        if let Ok(content) = serde_json::to_string_pretty(&example_theme) {
            let _ = std::fs::write(&example_path, content);
        }
    }

    // Load all JSON files from themes directory
    if let Ok(entries) = std::fs::read_dir(&themes_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "json") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(config) = serde_json::from_str::<CustomThemeConfig>(&content) {
                        let theme_id = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("custom")
                            .to_string();

                        let info = ThemeInfo {
                            id: format!("custom:{}", theme_id),
                            name: config.name.clone(),
                            description: config.description.clone(),
                            is_dark: config.is_dark,
                        };
                        let colors = config.colors.to_theme_colors();
                        custom_themes.push((info, colors));
                    }
                }
            }
        }
    }

    custom_themes
}
