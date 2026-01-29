//! Reusable UI components.
//!
//! This module contains reusable components:
//! - Simple input field
//! - Path auto-complete input
//! - Modal backdrop and content builders
//! - Dropdown select component
//! - Rename state management

pub mod dropdown;
pub mod list_overlay;
pub mod modal_backdrop;
pub mod path_autocomplete;
pub mod rename_state;
pub mod simple_input;
pub mod ui_helpers;

pub use dropdown::{dropdown_button, dropdown_option, dropdown_overlay};
pub use list_overlay::{
    handle_list_overlay_key, substring_filter, FilterResult, ListOverlayAction, ListOverlayConfig,
    ListOverlayState,
};
pub use modal_backdrop::{modal_backdrop, modal_content, modal_header};
pub use ui_helpers::{badge, kbd, keyboard_hint, keyboard_hints_footer, menu_item, menu_item_disabled, menu_item_with_color, search_input_area, segmented_toggle, shell_indicator_chip};
pub use path_autocomplete::PathAutoCompleteState;
pub use rename_state::{
    cancel_rename, finish_rename, is_renaming, rename_input, start_rename, start_rename_with_blur,
    RenameState,
};
pub use simple_input::{SimpleInput, SimpleInputState};
