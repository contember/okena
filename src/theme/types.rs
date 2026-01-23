//! Theme type definitions
//!
//! Contains ThemeMode, FolderColor, and ThemeInfo types.

use gpui::*;
use serde::{Deserialize, Serialize};

/// Create an hsla color from a hex color with custom alpha
pub fn with_alpha(hex: u32, alpha: f32) -> Hsla {
    let rgba = rgb(hex);
    Hsla::from(Rgba { a: alpha, ..rgba })
}

/// Theme mode preference
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Light,
    Dark,
    PastelDark,
    HighContrast,
    #[default]
    Auto,
    /// Custom theme loaded from configuration
    Custom,
}

/// Folder color options for projects
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FolderColor {
    #[default]
    Default,
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Purple,
    Pink,
}

impl FolderColor {
    /// Get all folder color variants for UI
    pub fn all() -> &'static [FolderColor] {
        &[
            FolderColor::Default,
            FolderColor::Red,
            FolderColor::Orange,
            FolderColor::Yellow,
            FolderColor::Green,
            FolderColor::Blue,
            FolderColor::Purple,
            FolderColor::Pink,
        ]
    }
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
                id: "pastel-dark".to_string(),
                name: "Pastel Dark".to_string(),
                description: "Soft pastel colors on dark background".to_string(),
                is_dark: true,
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
