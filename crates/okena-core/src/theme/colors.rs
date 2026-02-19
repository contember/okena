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
    pub border_idle: u32,

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

    // Folder colors (12 distinct colors for project folders)
    pub folder_default: u32,
    pub folder_red: u32,
    pub folder_orange: u32,
    pub folder_yellow: u32,
    pub folder_lime: u32,
    pub folder_green: u32,
    pub folder_teal: u32,
    pub folder_cyan: u32,
    pub folder_blue: u32,
    pub folder_indigo: u32,
    pub folder_purple: u32,
    pub folder_pink: u32,

    // Status bar metric colors (CPU/MEM severity)
    pub metric_normal: u32,
    pub metric_warning: u32,
    pub metric_critical: u32,

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
    bg_primary: 0x1e1e1e,
    bg_secondary: 0x252526,
    bg_header: 0x323233,
    bg_selection: 0x264f78,
    bg_hover: 0x2a2d2e,
    border: 0x3c3c3c,
    border_active: 0x007acc,
    border_focused: 0x569cd6,
    border_bell: 0xe69500,
    border_idle: 0xe5a100,
    text_primary: 0xcccccc,
    text_secondary: 0x808080,
    text_muted: 0x6a6a6a,
    selection_bg: 0x264f78,
    selection_fg: 0xffffff,
    search_match_bg: 0x613214,
    search_current_bg: 0xa45a00,
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
    cursor: 0xaeafad,
    scrollbar: 0x5a5a5a,
    scrollbar_hover: 0x7a7a7a,
    success: 0x4ec9b0,
    warning: 0xdcdcaa,
    error: 0xf44747,
    button_primary_bg: 0x007acc,
    button_primary_fg: 0xffffff,
    button_primary_hover: 0x005a9e,
    folder_default: 0x8a9199,
    folder_red: 0xe06c75,
    folder_orange: 0xd19a66,
    folder_yellow: 0xe5c07b,
    folder_lime: 0xa3d955,
    folder_green: 0x98c379,
    folder_teal: 0x2fbda0,
    folder_cyan: 0x56d7e5,
    folder_blue: 0x61afef,
    folder_indigo: 0x818cf8,
    folder_purple: 0xc678dd,
    folder_pink: 0xe06c9f,
    metric_normal: 0x0dbc79,   // term_green
    metric_warning: 0xe5e510,  // term_yellow
    metric_critical: 0xcd3131, // term_red
    diff_added_bg: 0x2ea043,
    diff_removed_bg: 0xf85149,
    diff_added_fg: 0x3fb950,
    diff_removed_fg: 0xf85149,
    diff_hunk_header_bg: 0x1d2d3e,
    diff_hunk_header_fg: 0x79b8ff,
};

/// Light theme (VSCode Light-like)
pub const LIGHT_THEME: ThemeColors = ThemeColors {
    bg_primary: 0xffffff,
    bg_secondary: 0xf3f3f3,
    bg_header: 0xe8e8e8,
    bg_selection: 0xadd6ff,
    bg_hover: 0xe8e8e8,
    border: 0xe5e5e5,
    border_active: 0x007acc,
    border_focused: 0x0078d4,
    border_bell: 0xe69500,
    border_idle: 0xb38600,
    text_primary: 0x333333,
    text_secondary: 0x6e6e6e,
    text_muted: 0xa0a0a0,
    selection_bg: 0xadd6ff,
    selection_fg: 0x000000,
    search_match_bg: 0xffd700,
    search_current_bg: 0xff8c00,
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
    cursor: 0x000000,
    scrollbar: 0xc1c1c1,
    scrollbar_hover: 0xa0a0a0,
    success: 0x008000,
    warning: 0x795e26,
    error: 0xa31515,
    button_primary_bg: 0x007acc,
    button_primary_fg: 0xffffff,
    button_primary_hover: 0x005a9e,
    folder_default: 0x6a737d,
    folder_red: 0xd73a49,
    folder_orange: 0xe36209,
    folder_yellow: 0xb08800,
    folder_lime: 0x65a30d,
    folder_green: 0x22863a,
    folder_teal: 0x0d9488,
    folder_cyan: 0x0891b2,
    folder_blue: 0x0366d6,
    folder_indigo: 0x4f46e5,
    folder_purple: 0x6f42c1,
    folder_pink: 0xdb2777,
    metric_normal: 0x00bc00,   // term_green
    metric_warning: 0x949800,  // term_yellow
    metric_critical: 0xcd3131, // term_red
    diff_added_bg: 0xdafbe1,
    diff_removed_bg: 0xffebe9,
    diff_added_fg: 0x22863a,
    diff_removed_fg: 0xd73a49,
    diff_hunk_header_bg: 0xe1e4e8,
    diff_hunk_header_fg: 0x0366d6,
};

/// Pastel Dark theme (Ghostty Builtin Pastel Dark)
pub const PASTEL_DARK_THEME: ThemeColors = ThemeColors {
    bg_primary: 0x1a1a1a,
    bg_secondary: 0x222222,
    bg_header: 0x282828,
    bg_selection: 0x3a3a3a,
    bg_hover: 0x303030,
    border: 0x404040,
    border_active: 0x96cbfe,
    border_focused: 0x4a4a4a,
    border_bell: 0xffa560,
    border_idle: 0xe5a100,
    text_primary: 0xeeeeee,
    text_secondary: 0x999999,
    text_muted: 0x666666,
    selection_bg: 0x363983,
    selection_fg: 0xf2f2f2,
    search_match_bg: 0x613214,
    search_current_bg: 0xffa560,
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
    term_background: 0x000000,
    term_background_unfocused: 0x1a1a1a,
    cursor: 0xffa560,
    scrollbar: 0x3a3a3a,
    scrollbar_hover: 0x555555,
    success: 0xa8ff60,
    warning: 0xffffb6,
    error: 0xff6c60,
    button_primary_bg: 0x1e3a5f,
    button_primary_fg: 0x4db8ff,
    button_primary_hover: 0x0d1520,
    folder_default: 0xa9b1d6,
    folder_red: 0xf7768e,
    folder_orange: 0xff9e64,
    folder_yellow: 0xe0af68,
    folder_lime: 0xb8e655,
    folder_green: 0x9ece6a,
    folder_teal: 0x2ac3a2,
    folder_cyan: 0x67e8f9,
    folder_blue: 0x7dcfff,
    folder_indigo: 0x7f7ff5,
    folder_purple: 0xbb9af7,
    folder_pink: 0xf472b6,
    metric_normal: 0x999999,   // text_secondary (subtle)
    metric_warning: 0xeeeeee,  // text_primary
    metric_critical: 0xff6c60, // term_red
    diff_added_bg: 0x2ea043,
    diff_removed_bg: 0xf85149,
    diff_added_fg: 0x56d364,
    diff_removed_fg: 0xf85149,
    diff_hunk_header_bg: 0x1c2333,
    diff_hunk_header_fg: 0x79b8ff,
};

/// High Contrast theme for accessibility
pub const HIGH_CONTRAST_THEME: ThemeColors = ThemeColors {
    bg_primary: 0x000000,
    bg_secondary: 0x0a0a0a,
    bg_header: 0x111111,
    bg_selection: 0x0066cc,
    bg_hover: 0x1a1a1a,
    border: 0x6fc3df,
    border_active: 0x00aaff,
    border_focused: 0xffff00,
    border_bell: 0xff6600,
    border_idle: 0xffaa00,
    text_primary: 0xffffff,
    text_secondary: 0xe0e0e0,
    text_muted: 0xb0b0b0,
    selection_bg: 0x0066cc,
    selection_fg: 0xffffff,
    search_match_bg: 0xff6600,
    search_current_bg: 0xffff00,
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
    cursor: 0xffffff,
    scrollbar: 0x808080,
    scrollbar_hover: 0xa0a0a0,
    success: 0x00ff00,
    warning: 0xffff00,
    error: 0xff0000,
    button_primary_bg: 0x0066cc,
    button_primary_fg: 0xffffff,
    button_primary_hover: 0x0055aa,
    folder_default: 0xcccccc,
    folder_red: 0xff5555,
    folder_orange: 0xffaa00,
    folder_yellow: 0xffff00,
    folder_lime: 0x88ff00,
    folder_green: 0x55ff55,
    folder_teal: 0x00e5cc,
    folder_cyan: 0x55e5ff,
    folder_blue: 0x55aaff,
    folder_indigo: 0x8888ff,
    folder_purple: 0xff55ff,
    folder_pink: 0xff77aa,
    metric_normal: 0x00ff00,   // term_green
    metric_warning: 0xffff00,  // term_yellow
    metric_critical: 0xff0000, // term_red
    diff_added_bg: 0x003300,
    diff_removed_bg: 0x330000,
    diff_added_fg: 0x00ff00,
    diff_removed_fg: 0xff0000,
    diff_hunk_header_bg: 0x001133,
    diff_hunk_header_fg: 0x00aaff,
};

impl ThemeColors {
    /// Determine if this is a dark theme based on background luminance.
    pub fn is_dark(&self) -> bool {
        let (r, g, b) = Self::hex_to_rgb(self.bg_primary);
        // Relative luminance approximation
        let luminance = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
        luminance < 128.0
    }

    /// Get RGB tuple from a hex color
    pub fn hex_to_rgb(hex: u32) -> (u8, u8, u8) {
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
            FolderColor::Lime => self.folder_lime,
            FolderColor::Green => self.folder_green,
            FolderColor::Teal => self.folder_teal,
            FolderColor::Cyan => self.folder_cyan,
            FolderColor::Blue => self.folder_blue,
            FolderColor::Indigo => self.folder_indigo,
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

    /// Convert ANSI color to packed ARGB u32 (0xAARRGGBB).
    /// Framework-agnostic version of color conversion.
    pub fn ansi_to_argb(&self, color: &alacritty_terminal::vte::ansi::Color) -> u32 {
        use alacritty_terminal::vte::ansi::{Color, NamedColor};

        match color {
            Color::Named(named) => {
                let hex = self.get_term_color(named);
                0xFF000000 | hex
            }
            Color::Spec(rgb) => {
                0xFF000000 | ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | (rgb.b as u32)
            }
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
                    0xFF000000 | self.get_term_color(&named)
                } else if idx < 232 {
                    let i = idx - 16;
                    let r = (i / 36) * 51;
                    let g = ((i / 6) % 6) * 51;
                    let b = (i % 6) * 51;
                    0xFF000000 | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
                } else {
                    let gray = ((idx - 232) * 10 + 8) as u32;
                    0xFF000000 | (gray << 16) | (gray << 8) | gray
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};

    #[test]
    fn ansi_to_argb_named_colors() {
        let t = &DARK_THEME;
        // Check all 16 named colors resolve to the theme's term_* values
        assert_eq!(t.ansi_to_argb(&Color::Named(NamedColor::Black)), 0xFF000000 | t.term_black);
        assert_eq!(t.ansi_to_argb(&Color::Named(NamedColor::Red)), 0xFF000000 | t.term_red);
        assert_eq!(t.ansi_to_argb(&Color::Named(NamedColor::Green)), 0xFF000000 | t.term_green);
        assert_eq!(t.ansi_to_argb(&Color::Named(NamedColor::Yellow)), 0xFF000000 | t.term_yellow);
        assert_eq!(t.ansi_to_argb(&Color::Named(NamedColor::Blue)), 0xFF000000 | t.term_blue);
        assert_eq!(t.ansi_to_argb(&Color::Named(NamedColor::Magenta)), 0xFF000000 | t.term_magenta);
        assert_eq!(t.ansi_to_argb(&Color::Named(NamedColor::Cyan)), 0xFF000000 | t.term_cyan);
        assert_eq!(t.ansi_to_argb(&Color::Named(NamedColor::White)), 0xFF000000 | t.term_white);
        assert_eq!(t.ansi_to_argb(&Color::Named(NamedColor::BrightBlack)), 0xFF000000 | t.term_bright_black);
        assert_eq!(t.ansi_to_argb(&Color::Named(NamedColor::BrightRed)), 0xFF000000 | t.term_bright_red);
        assert_eq!(t.ansi_to_argb(&Color::Named(NamedColor::BrightGreen)), 0xFF000000 | t.term_bright_green);
        assert_eq!(t.ansi_to_argb(&Color::Named(NamedColor::BrightYellow)), 0xFF000000 | t.term_bright_yellow);
        assert_eq!(t.ansi_to_argb(&Color::Named(NamedColor::BrightBlue)), 0xFF000000 | t.term_bright_blue);
        assert_eq!(t.ansi_to_argb(&Color::Named(NamedColor::BrightMagenta)), 0xFF000000 | t.term_bright_magenta);
        assert_eq!(t.ansi_to_argb(&Color::Named(NamedColor::BrightCyan)), 0xFF000000 | t.term_bright_cyan);
        assert_eq!(t.ansi_to_argb(&Color::Named(NamedColor::BrightWhite)), 0xFF000000 | t.term_bright_white);
    }

    #[test]
    fn ansi_to_argb_spec_rgb() {
        let t = &DARK_THEME;
        let color = Color::Spec(Rgb { r: 0xAB, g: 0xCD, b: 0xEF });
        assert_eq!(t.ansi_to_argb(&color), 0xFFABCDEF);
    }

    #[test]
    fn ansi_to_argb_indexed_cube() {
        let t = &DARK_THEME;
        // Index 16 = first color cube entry (0,0,0)
        assert_eq!(t.ansi_to_argb(&Color::Indexed(16)), 0xFF000000);
        // Index 21 = (0,0,5) -> (0,0,255)
        assert_eq!(t.ansi_to_argb(&Color::Indexed(21)), 0xFF0000FF);
        // Index 196 = (5,0,0) -> (255,0,0)
        assert_eq!(t.ansi_to_argb(&Color::Indexed(196)), 0xFFFF0000);
        // Index 46 = (0,5,0) -> (0,255,0)
        assert_eq!(t.ansi_to_argb(&Color::Indexed(46)), 0xFF00FF00);
        // Index 231 = (5,5,5) -> (255,255,255)
        assert_eq!(t.ansi_to_argb(&Color::Indexed(231)), 0xFFFFFFFF);
    }

    #[test]
    fn ansi_to_argb_indexed_grayscale() {
        let t = &DARK_THEME;
        // Index 232 = first grayscale = (0*10+8) = 8
        assert_eq!(t.ansi_to_argb(&Color::Indexed(232)), 0xFF080808);
        // Index 255 = last grayscale = (23*10+8) = 238
        assert_eq!(t.ansi_to_argb(&Color::Indexed(255)), 0xFFEEEEEE);
    }

    #[test]
    fn ansi_to_argb_indexed_first_16() {
        let t = &DARK_THEME;
        // Index 0-15 should map to the same values as the named colors
        assert_eq!(
            t.ansi_to_argb(&Color::Indexed(0)),
            t.ansi_to_argb(&Color::Named(NamedColor::Black))
        );
        assert_eq!(
            t.ansi_to_argb(&Color::Indexed(9)),
            t.ansi_to_argb(&Color::Named(NamedColor::BrightRed))
        );
    }
}
