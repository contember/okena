//! Theme type definitions
//!
//! Contains ThemeMode, FolderColor, and ThemeInfo types.

use gpui::*;

pub use okena_core::theme::{FolderColor, ThemeInfo, ThemeMode};

/// Create an hsla color from a hex color with custom alpha
pub fn with_alpha(hex: u32, alpha: f32) -> Hsla {
    let rgba = rgb(hex);
    Hsla::from(Rgba { a: alpha, ..rgba })
}
