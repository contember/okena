//! Reusable UI utilities and components.
//!
//! This module contains shared utilities that can be used across
//! different views in the application.

mod click_detector;
pub mod tokens;

pub use click_detector::ClickDetector;
pub use okena_ui::color_utils::tint_color;
