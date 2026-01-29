//! Design tokens for consistent UI spacing and sizing.
//!
//! This module defines named constants for common UI values to ensure
//! consistency across the application and make global adjustments easier.

use gpui::px;

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

/// Large text (13px) - emphasized content
pub const TEXT_LG: gpui::Pixels = px(13.0);

/// Extra large text (14px) - headings, modal titles
pub const TEXT_XL: gpui::Pixels = px(14.0);

// =============================================================================
// Border radius
// =============================================================================

/// Small radius (2px) - subtle rounding
pub const RADIUS_SM: gpui::Pixels = px(2.0);

/// Medium radius (3px) - badges, small elements
pub const RADIUS_MD: gpui::Pixels = px(3.0);

/// Standard radius (4px) - buttons, inputs, cards
pub const RADIUS_STD: gpui::Pixels = px(4.0);

/// Large radius (8px) - modals, dialogs
pub const RADIUS_LG: gpui::Pixels = px(8.0);

// =============================================================================
// Icon sizes
// =============================================================================

/// Small icon (10px) - inline icons, chevrons
pub const ICON_SM: gpui::Pixels = px(10.0);

/// Medium icon (12px) - list icons
pub const ICON_MD: gpui::Pixels = px(12.0);

/// Standard icon (14px) - menu icons, buttons
pub const ICON_STD: gpui::Pixels = px(14.0);

/// Large icon (16px) - header icons
pub const ICON_LG: gpui::Pixels = px(16.0);

// =============================================================================
// Component heights
// =============================================================================

/// Compact height (18px) - chips, small indicators
pub const HEIGHT_CHIP: gpui::Pixels = px(18.0);

/// Button height (24px) - icon buttons
pub const HEIGHT_BUTTON_SM: gpui::Pixels = px(24.0);

// =============================================================================
// Widths
// =============================================================================

/// Minimum context menu width
pub const WIDTH_CONTEXT_MENU: gpui::Pixels = px(140.0);

/// Minimum project context menu width
pub const WIDTH_PROJECT_MENU: gpui::Pixels = px(160.0);

/// Standard modal width (small)
pub const WIDTH_MODAL_SM: gpui::Pixels = px(200.0);

/// Standard modal width (medium)
pub const WIDTH_MODAL_MD: gpui::Pixels = px(450.0);
