//! Custom theme configuration support
//!
//! Allows loading custom themes from JSON files in the themes directory.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::{ThemeColors, ThemeInfo};

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
    #[serde(default = "default_border_idle")]
    pub border_idle: String,

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
    #[serde(default = "default_term_background_unfocused")]
    pub term_background_unfocused: String,

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

    // Button colors
    #[serde(default = "default_button_primary_bg")]
    pub button_primary_bg: String,
    #[serde(default = "default_button_primary_fg")]
    pub button_primary_fg: String,
    #[serde(default = "default_button_primary_hover")]
    pub button_primary_hover: String,

    // Folder colors
    #[serde(default = "default_folder_default")]
    pub folder_default: String,
    #[serde(default = "default_folder_red")]
    pub folder_red: String,
    #[serde(default = "default_folder_orange")]
    pub folder_orange: String,
    #[serde(default = "default_folder_yellow")]
    pub folder_yellow: String,
    #[serde(default = "default_folder_lime")]
    pub folder_lime: String,
    #[serde(default = "default_folder_green")]
    pub folder_green: String,
    #[serde(default = "default_folder_teal")]
    pub folder_teal: String,
    #[serde(default = "default_folder_cyan")]
    pub folder_cyan: String,
    #[serde(default = "default_folder_blue")]
    pub folder_blue: String,
    #[serde(default = "default_folder_indigo")]
    pub folder_indigo: String,
    #[serde(default = "default_folder_purple")]
    pub folder_purple: String,
    #[serde(default = "default_folder_pink")]
    pub folder_pink: String,

    // Status bar metric colors
    #[serde(default = "default_metric_normal")]
    pub metric_normal: String,
    #[serde(default = "default_metric_warning")]
    pub metric_warning: String,
    #[serde(default = "default_metric_critical")]
    pub metric_critical: String,

    // Diff colors
    #[serde(default = "default_diff_added_bg")]
    pub diff_added_bg: String,
    #[serde(default = "default_diff_removed_bg")]
    pub diff_removed_bg: String,
    #[serde(default = "default_diff_added_fg")]
    pub diff_added_fg: String,
    #[serde(default = "default_diff_removed_fg")]
    pub diff_removed_fg: String,
    #[serde(default = "default_diff_hunk_header_bg")]
    pub diff_hunk_header_bg: String,
    #[serde(default = "default_diff_hunk_header_fg")]
    pub diff_hunk_header_fg: String,
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
fn default_border_idle() -> String { "#e5a100".to_string() }
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
fn default_term_background_unfocused() -> String { "#252526".to_string() }
fn default_cursor() -> String { "#aeafad".to_string() }
fn default_scrollbar() -> String { "#5a5a5a".to_string() }
fn default_scrollbar_hover() -> String { "#7a7a7a".to_string() }
fn default_success() -> String { "#4ec9b0".to_string() }
fn default_warning() -> String { "#dcdcaa".to_string() }
fn default_error() -> String { "#f44747".to_string() }
fn default_button_primary_bg() -> String { "#007acc".to_string() }
fn default_button_primary_fg() -> String { "#ffffff".to_string() }
fn default_button_primary_hover() -> String { "#005a9e".to_string() }
fn default_folder_default() -> String { "#8a9199".to_string() }
fn default_folder_red() -> String { "#e06c75".to_string() }
fn default_folder_orange() -> String { "#d19a66".to_string() }
fn default_folder_yellow() -> String { "#e5c07b".to_string() }
fn default_folder_lime() -> String { "#a3d955".to_string() }
fn default_folder_green() -> String { "#98c379".to_string() }
fn default_folder_teal() -> String { "#2fbda0".to_string() }
fn default_folder_cyan() -> String { "#56d7e5".to_string() }
fn default_folder_blue() -> String { "#61afef".to_string() }
fn default_folder_indigo() -> String { "#818cf8".to_string() }
fn default_folder_purple() -> String { "#c678dd".to_string() }
fn default_folder_pink() -> String { "#e06c9f".to_string() }
fn default_metric_normal() -> String { "#0dbc79".to_string() }
fn default_metric_warning() -> String { "#e5e510".to_string() }
fn default_metric_critical() -> String { "#cd3131".to_string() }
fn default_diff_added_bg() -> String { "#1e3a1e".to_string() }
fn default_diff_removed_bg() -> String { "#3a1e1e".to_string() }
fn default_diff_added_fg() -> String { "#4ec9b0".to_string() }
fn default_diff_removed_fg() -> String { "#f14c4c".to_string() }
fn default_diff_hunk_header_bg() -> String { "#2d3748".to_string() }
fn default_diff_hunk_header_fg() -> String { "#569cd6".to_string() }

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
            border_idle: Self::parse_hex(&self.border_idle),
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
            term_background_unfocused: Self::parse_hex(&self.term_background_unfocused),
            cursor: Self::parse_hex(&self.cursor),
            scrollbar: Self::parse_hex(&self.scrollbar),
            scrollbar_hover: Self::parse_hex(&self.scrollbar_hover),
            success: Self::parse_hex(&self.success),
            warning: Self::parse_hex(&self.warning),
            error: Self::parse_hex(&self.error),
            button_primary_bg: Self::parse_hex(&self.button_primary_bg),
            button_primary_fg: Self::parse_hex(&self.button_primary_fg),
            button_primary_hover: Self::parse_hex(&self.button_primary_hover),
            folder_default: Self::parse_hex(&self.folder_default),
            folder_red: Self::parse_hex(&self.folder_red),
            folder_orange: Self::parse_hex(&self.folder_orange),
            folder_yellow: Self::parse_hex(&self.folder_yellow),
            folder_lime: Self::parse_hex(&self.folder_lime),
            folder_green: Self::parse_hex(&self.folder_green),
            folder_teal: Self::parse_hex(&self.folder_teal),
            folder_cyan: Self::parse_hex(&self.folder_cyan),
            folder_blue: Self::parse_hex(&self.folder_blue),
            folder_indigo: Self::parse_hex(&self.folder_indigo),
            folder_purple: Self::parse_hex(&self.folder_purple),
            folder_pink: Self::parse_hex(&self.folder_pink),
            metric_normal: Self::parse_hex(&self.metric_normal),
            metric_warning: Self::parse_hex(&self.metric_warning),
            metric_critical: Self::parse_hex(&self.metric_critical),
            diff_added_bg: Self::parse_hex(&self.diff_added_bg),
            diff_removed_bg: Self::parse_hex(&self.diff_removed_bg),
            diff_added_fg: Self::parse_hex(&self.diff_added_fg),
            diff_removed_fg: Self::parse_hex(&self.diff_removed_fg),
            diff_hunk_header_bg: Self::parse_hex(&self.diff_hunk_header_bg),
            diff_hunk_header_fg: Self::parse_hex(&self.diff_hunk_header_fg),
        }
    }
}

/// Get path to custom themes directory
pub fn get_themes_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("okena")
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
                bg_primary: "#1a1a1a".to_string(),
                bg_secondary: "#222222".to_string(),
                bg_header: "#282828".to_string(),
                bg_selection: "#363983".to_string(),
                bg_hover: "#303030".to_string(),
                border: "#3a3a3a".to_string(),
                border_active: "#96cbfe".to_string(),
                border_focused: "#96cbfe".to_string(),
                border_bell: "#ffa560".to_string(),
                border_idle: "#e5a100".to_string(),
                text_primary: "#eeeeee".to_string(),
                text_secondary: "#999999".to_string(),
                text_muted: "#666666".to_string(),
                selection_bg: "#363983".to_string(),
                selection_fg: "#f2f2f2".to_string(),
                search_match_bg: "#613214".to_string(),
                search_current_bg: "#ffa560".to_string(),
                term_black: "#4f4f4f".to_string(),
                term_red: "#ff6c60".to_string(),
                term_green: "#a8ff60".to_string(),
                term_yellow: "#ffffb6".to_string(),
                term_blue: "#96cbfe".to_string(),
                term_magenta: "#ff73fd".to_string(),
                term_cyan: "#c6c5fe".to_string(),
                term_white: "#eeeeee".to_string(),
                term_bright_black: "#7c7c7c".to_string(),
                term_bright_red: "#ffb6b0".to_string(),
                term_bright_green: "#ceffac".to_string(),
                term_bright_yellow: "#ffffcc".to_string(),
                term_bright_blue: "#b5dcff".to_string(),
                term_bright_magenta: "#ff9cfe".to_string(),
                term_bright_cyan: "#dfdffe".to_string(),
                term_bright_white: "#ffffff".to_string(),
                term_foreground: "#bbbbbb".to_string(),
                term_background: "#000000".to_string(),
                term_background_unfocused: "#111111".to_string(),
                cursor: "#ffa560".to_string(),
                scrollbar: "#3a3a3a".to_string(),
                scrollbar_hover: "#555555".to_string(),
                success: "#a8ff60".to_string(),
                warning: "#ffffb6".to_string(),
                error: "#ff6c60".to_string(),
                button_primary_bg: "#1e3a5f".to_string(),
                button_primary_fg: "#4db8ff".to_string(),
                button_primary_hover: "#0d1520".to_string(),
                folder_default: "#a9b1d6".to_string(),
                folder_red: "#f7768e".to_string(),
                folder_orange: "#ff9e64".to_string(),
                folder_yellow: "#e0af68".to_string(),
                folder_lime: "#b8e655".to_string(),
                folder_green: "#9ece6a".to_string(),
                folder_teal: "#2ac3a2".to_string(),
                folder_cyan: "#67e8f9".to_string(),
                folder_blue: "#7dcfff".to_string(),
                folder_indigo: "#7f7ff5".to_string(),
                folder_purple: "#bb9af7".to_string(),
                folder_pink: "#f472b6".to_string(),
                metric_normal: "#999999".to_string(),
                metric_warning: "#eeeeee".to_string(),
                metric_critical: "#ff6c60".to_string(),
                diff_added_bg: "#1a2e1a".to_string(),
                diff_removed_bg: "#2e1a1a".to_string(),
                diff_added_fg: "#a8ff60".to_string(),
                diff_removed_fg: "#ff6c60".to_string(),
                diff_hunk_header_bg: "#282828".to_string(),
                diff_hunk_header_fg: "#96cbfe".to_string(),
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
