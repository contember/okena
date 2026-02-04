//! Theme color definitions
//!
//! Contains ThemeColors struct and all built-in theme color constants.

use gpui::*;

use super::types::FolderColor;

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
    pub term_background_unfocused: u32,

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

    // Button colors
    pub button_primary_bg: u32,
    pub button_primary_fg: u32,
    pub button_primary_hover: u32,

    // Folder colors (8 distinct colors for project folders)
    pub folder_default: u32,
    pub folder_red: u32,
    pub folder_orange: u32,
    pub folder_yellow: u32,
    pub folder_green: u32,
    pub folder_blue: u32,
    pub folder_purple: u32,
    pub folder_pink: u32,

    // Diff colors
    pub diff_added_bg: u32,
    pub diff_removed_bg: u32,
    pub diff_added_fg: u32,
    pub diff_removed_fg: u32,
    pub diff_hunk_header_bg: u32,
    pub diff_hunk_header_fg: u32,
}

/// Dark theme (VSCode-like)
pub const DARK_THEME: ThemeColors = ThemeColors {
    // Background colors
    bg_primary: 0x1e1e1e,
    bg_secondary: 0x252526,
    bg_header: 0x323233,
    bg_selection: 0x264f78,
    bg_hover: 0x2a2d2e,

    // Border colors - subtle but visible borders for clean separation
    border: 0x3c3c3c,
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
    term_background_unfocused: 0x252526,

    // UI element colors
    cursor: 0xaeafad,
    scrollbar: 0x5a5a5a,
    scrollbar_hover: 0x7a7a7a,

    // Status colors
    success: 0x4ec9b0,
    warning: 0xdcdcaa,
    error: 0xf44747,

    // Button colors
    button_primary_bg: 0x007acc,
    button_primary_fg: 0xffffff,
    button_primary_hover: 0x005a9e,

    // Folder colors (distinct colors for project folders)
    folder_default: 0x8a9199,  // Gray/steel - neutral default
    folder_red: 0xe06c75,
    folder_orange: 0xd19a66,
    folder_yellow: 0xe5c07b,
    folder_green: 0x98c379,
    folder_blue: 0x61afef,     // Vibrant blue
    folder_purple: 0xc678dd,
    folder_pink: 0xe06c9f,

    // Diff colors
    diff_added_bg: 0x1e3a1e,      // Dark green background
    diff_removed_bg: 0x3a1e1e,    // Dark red background
    diff_added_fg: 0x4ec9b0,      // Green for + indicator
    diff_removed_fg: 0xf14c4c,    // Red for - indicator
    diff_hunk_header_bg: 0x2d3748, // Blue-gray background
    diff_hunk_header_fg: 0x569cd6, // Blue text
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
    term_background_unfocused: 0xf3f3f3,

    // UI element colors
    cursor: 0x000000,
    scrollbar: 0xc1c1c1,
    scrollbar_hover: 0xa0a0a0,

    // Status colors
    success: 0x008000,
    warning: 0x795e26,
    error: 0xa31515,

    // Button colors
    button_primary_bg: 0x007acc,
    button_primary_fg: 0xffffff,
    button_primary_hover: 0x005a9e,

    // Folder colors (distinct colors for project folders)
    folder_default: 0x6a737d,  // Gray - neutral default
    folder_red: 0xd73a49,
    folder_orange: 0xe36209,
    folder_yellow: 0xb08800,
    folder_green: 0x22863a,
    folder_blue: 0x0366d6,     // Vibrant blue
    folder_purple: 0x6f42c1,
    folder_pink: 0xdb2777,

    // Diff colors
    diff_added_bg: 0xdafbe1,      // Light green background
    diff_removed_bg: 0xffebe9,    // Light red background
    diff_added_fg: 0x22863a,      // Green for + indicator
    diff_removed_fg: 0xd73a49,    // Red for - indicator
    diff_hunk_header_bg: 0xe1e4e8, // Gray background
    diff_hunk_header_fg: 0x0366d6, // Blue text
};

/// Pastel Dark theme (Ghostty Builtin Pastel Dark)
/// UI elements use lighter colors, terminal uses dark background
pub const PASTEL_DARK_THEME: ThemeColors = ThemeColors {
    // Background colors - slightly lighter than terminal for UI elements
    bg_primary: 0x1a1a1a,
    bg_secondary: 0x222222,
    bg_header: 0x282828,
    bg_selection: 0x363983,
    bg_hover: 0x303030,

    // Border colors - subtle but visible
    border: 0x404040,
    border_active: 0x96cbfe,
    border_focused: 0x4a4a4a,
    border_bell: 0xffa560,

    // Text colors
    text_primary: 0xeeeeee,
    text_secondary: 0x999999,
    text_muted: 0x666666,

    // Selection colors
    selection_bg: 0x363983,
    selection_fg: 0xf2f2f2,

    // Search highlight colors
    search_match_bg: 0x613214,
    search_current_bg: 0xffa560,

    // Terminal colors (Ghostty Builtin Pastel Dark)
    term_black: 0x4f4f4f,
    term_red: 0xff6c60,
    term_green: 0xa8ff60,
    term_yellow: 0xffffb6,
    term_blue: 0x96cbfe,
    term_magenta: 0xff73fd,
    term_cyan: 0xc6c5fe,
    term_white: 0xeeeeee,
    term_bright_black: 0x7c7c7c,
    term_bright_red: 0xffb6b0,
    term_bright_green: 0xceffac,
    term_bright_yellow: 0xffffcc,
    term_bright_blue: 0xb5dcff,
    term_bright_magenta: 0xff9cfe,
    term_bright_cyan: 0xdfdffe,
    term_bright_white: 0xffffff,
    term_foreground: 0xbbbbbb,
    term_background: 0x000000, // Terminal stays dark
    term_background_unfocused: 0x111111,

    // UI element colors
    cursor: 0xffa560,
    scrollbar: 0x3a3a3a,
    scrollbar_hover: 0x555555,

    // Status colors
    success: 0xa8ff60,
    warning: 0xffffb6,
    error: 0xff6c60,

    // Button colors - dark background with electric blue text
    button_primary_bg: 0x1e3a5f,
    button_primary_fg: 0x4db8ff,
    button_primary_hover: 0x0d1520,

    // Folder colors (distinct colors for project folders)
    folder_default: 0xa9b1d6,  // Lavender gray - neutral default
    folder_red: 0xf7768e,
    folder_orange: 0xff9e64,
    folder_yellow: 0xe0af68,
    folder_green: 0x9ece6a,
    folder_blue: 0x7dcfff,     // Bright cyan-blue
    folder_purple: 0xbb9af7,
    folder_pink: 0xf472b6,

    // Diff colors
    diff_added_bg: 0x1a2e1a,      // Dark green background
    diff_removed_bg: 0x2e1a1a,    // Dark red background
    diff_added_fg: 0xa8ff60,      // Pastel green for + indicator
    diff_removed_fg: 0xff6c60,    // Pastel red for - indicator
    diff_hunk_header_bg: 0x282828, // Dark gray background
    diff_hunk_header_fg: 0x96cbfe, // Pastel blue text
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
    term_background_unfocused: 0x111111,

    // UI element colors
    cursor: 0xffffff,
    scrollbar: 0x808080,
    scrollbar_hover: 0xa0a0a0,

    // Status colors
    success: 0x00ff00,
    warning: 0xffff00,
    error: 0xff0000,

    // Button colors
    button_primary_bg: 0x0066cc,
    button_primary_fg: 0xffffff,
    button_primary_hover: 0x0055aa,

    // Folder colors (distinct colors for project folders)
    folder_default: 0xcccccc,  // Silver/gray - neutral default
    folder_red: 0xff5555,
    folder_orange: 0xffaa00,
    folder_yellow: 0xffff00,
    folder_green: 0x55ff55,
    folder_blue: 0x55aaff,     // Bright blue
    folder_purple: 0xff55ff,
    folder_pink: 0xff77aa,

    // Diff colors
    diff_added_bg: 0x003300,      // Dark green background
    diff_removed_bg: 0x330000,    // Dark red background
    diff_added_fg: 0x00ff00,      // Bright green for + indicator
    diff_removed_fg: 0xff0000,    // Bright red for - indicator
    diff_hunk_header_bg: 0x001133, // Dark blue background
    diff_hunk_header_fg: 0x00aaff, // Bright blue text
};

impl ThemeColors {
    /// Get RGB tuple from a hex color
    fn hex_to_rgb(hex: u32) -> (u8, u8, u8) {
        (
            ((hex >> 16) & 0xFF) as u8,
            ((hex >> 8) & 0xFF) as u8,
            (hex & 0xFF) as u8,
        )
    }

    /// Get the actual color value for a folder color option
    pub fn get_folder_color(&self, color: FolderColor) -> u32 {
        match color {
            FolderColor::Default => self.folder_default,
            FolderColor::Red => self.folder_red,
            FolderColor::Orange => self.folder_orange,
            FolderColor::Yellow => self.folder_yellow,
            FolderColor::Green => self.folder_green,
            FolderColor::Blue => self.folder_blue,
            FolderColor::Purple => self.folder_purple,
            FolderColor::Pink => self.folder_pink,
        }
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
