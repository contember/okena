//! Okena UI component library.
//!
//! Reusable UI components, design tokens, and theme helpers for the Okena terminal.

use gpui::{AnyView, StyleRefinement, Styled};

/// Wrap a view with `cached(size_full())` on platforms where it helps.
///
/// On macOS, GPUI calls `window.refresh()` frequently (hover tracking, input
/// modality switches), which forces all cached views to miss. The miss path
/// uses `layout_as_root()` — an independent taffy tree that doubles layout
/// cost. On Linux/Windows, refreshes are rare so caches hit most frames.
pub fn cached_on_non_macos(view: AnyView) -> AnyView {
    if cfg!(target_os = "macos") {
        view
    } else {
        view.cached(StyleRefinement::default().size_full())
    }
}

pub mod badge;
pub mod button;
pub mod chip;
pub mod simple_input;
pub mod text_utils;
pub mod click_detector;
pub mod color_utils;
pub mod header_buttons;
pub mod code_block;
pub mod color_dot;
pub mod context_menu_backdrop;
pub mod dialog_actions;
pub mod dropdown;
pub mod empty_state;
pub mod file_icon;
mod focusable;
pub mod expand;
pub mod icon_action_button;
pub mod icon_button;
pub mod input;
pub mod list_row;
pub mod menu;
pub mod modal;
pub mod overlay;
pub mod popover;
pub mod rename_state;
pub mod selectable_list;
pub mod settings;
pub mod theme;
pub mod title_subtitle;
pub mod toggle;
pub mod tokens;
