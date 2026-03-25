//! Design tokens for consistent UI spacing and sizing.
//!
//! This module defines named constants for common UI values to ensure
//! consistency across the application and make global adjustments easier.

use gpui::{px, App, Global};

// =============================================================================
// Global UI font size provider
// =============================================================================

/// Global function pointer that reads the current ui_font_size from the host app's settings.
/// The host app registers this at startup; crate views call `ui_text_*(cx)` to get scaled sizes.
pub struct GlobalUiFontSize(pub fn(&App) -> f32);

impl Global for GlobalUiFontSize {}

fn get_ui_font_size(cx: &App) -> f32 {
    cx.try_global::<GlobalUiFontSize>()
        .map(|g| (g.0)(cx))
        .unwrap_or(DEFAULT_UI_FONT_SIZE)
}

// =============================================================================
// Spacing (padding, margin, gap)
// =============================================================================

/// Extra small spacing (4px) - tight gaps, small padding
pub const SPACE_XS: gpui::Pixels = px(4.0);

/// Small spacing (6px) - compact padding
pub const SPACE_SM: gpui::Pixels = px(6.0);

/// Medium spacing (8px) - standard gaps
pub const SPACE_MD: gpui::Pixels = px(8.0);

/// Large spacing (12px) - section padding, larger gaps
pub const SPACE_LG: gpui::Pixels = px(12.0);

/// Extra large spacing (16px) - modal/dialog padding
pub const SPACE_XL: gpui::Pixels = px(16.0);

// =============================================================================
// Text sizes
// =============================================================================

/// Extra small text (9px) - badges, tags
pub const TEXT_XS: gpui::Pixels = px(9.0);

/// Small text (10px) - secondary labels, hints
pub const TEXT_SM: gpui::Pixels = px(10.0);

/// Medium-small text (11px) - compact UI, button labels
pub const TEXT_MS: gpui::Pixels = px(11.0);

/// Medium text (12px) - default body text, menu items
pub const TEXT_MD: gpui::Pixels = px(12.0);

/// Extra large text (14px) - headings, modal titles
pub const TEXT_XL: gpui::Pixels = px(14.0);

// =============================================================================
// Scaled text sizes (relative to ui_font_size setting)
// =============================================================================

const DEFAULT_UI_FONT_SIZE: f32 = 13.0;

fn ui_scale(ui_font_size: f32) -> f32 {
    ui_font_size / DEFAULT_UI_FONT_SIZE
}

pub fn ui_text_xs(cx: &App) -> gpui::Pixels {
    px(9.0 * ui_scale(get_ui_font_size(cx)))
}

pub fn ui_text_sm(cx: &App) -> gpui::Pixels {
    px(10.0 * ui_scale(get_ui_font_size(cx)))
}

pub fn ui_text_ms(cx: &App) -> gpui::Pixels {
    px(11.0 * ui_scale(get_ui_font_size(cx)))
}

pub fn ui_text_md(cx: &App) -> gpui::Pixels {
    px(12.0 * ui_scale(get_ui_font_size(cx)))
}

pub fn ui_text_xl(cx: &App) -> gpui::Pixels {
    px(14.0 * ui_scale(get_ui_font_size(cx)))
}

pub fn ui_text(default_px: f32, cx: &App) -> gpui::Pixels {
    px(default_px * ui_scale(get_ui_font_size(cx)))
}

// =============================================================================
// Border radius
// =============================================================================

/// Medium radius (3px) - badges, small elements
pub const RADIUS_MD: gpui::Pixels = px(3.0);

/// Standard radius (4px) - buttons, inputs, cards
pub const RADIUS_STD: gpui::Pixels = px(4.0);

// =============================================================================
// Icon sizes
// =============================================================================

/// Small icon (10px) - inline icons, chevrons
pub const ICON_SM: gpui::Pixels = px(10.0);

/// Standard icon (14px) - menu icons, buttons
pub const ICON_STD: gpui::Pixels = px(14.0);

// =============================================================================
// Component heights
// =============================================================================

/// Compact height (18px) - chips, small indicators
pub const HEIGHT_CHIP: gpui::Pixels = px(18.0);
