//! Reusable UI utilities and components.
//!
//! This module contains shared utilities that can be used across
//! different views in the application.

mod click_detector;
pub mod tokens;

pub use click_detector::ClickDetector;
pub use okena_files::selection::*;

/// Blend `tint` into `base` RGB color at the given ratio (0.0 = pure base, 1.0 = pure tint).
pub fn tint_color(base: u32, tint: u32, amount: f32) -> u32 {
    let lerp = |b: u32, t: u32| (b as f32 + (t as f32 - b as f32) * amount) as u32;
    let r = lerp((base >> 16) & 0xFF, (tint >> 16) & 0xFF);
    let g = lerp((base >> 8) & 0xFF, (tint >> 8) & 0xFF);
    let b = lerp(base & 0xFF, tint & 0xFF);
    (r << 16) | (g << 8) | b
}
