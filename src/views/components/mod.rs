//! Reusable UI components.
//!
//! This module contains reusable components:
//! - Simple input field
//! - Path auto-complete input
//! - Modal backdrop and content builders
//! - Dropdown select component
//! - Rename state management
//! - Syntax highlighting utilities
//! - Virtualized code view

pub mod code_view;
pub mod dropdown;
pub mod list_overlay;
pub mod modal_backdrop;
pub mod path_autocomplete;
pub mod rename_state;
pub mod simple_input;
pub mod syntax;
pub mod ui_helpers;

pub use dropdown::{dropdown_button, dropdown_option, dropdown_overlay};
pub use list_overlay::{
    handle_list_overlay_key, substring_filter, ListOverlayAction, ListOverlayConfig,
    ListOverlayState,
};
pub use modal_backdrop::{modal_backdrop, modal_content, modal_header};
pub use ui_helpers::{badge, button, button_primary, code_block_container, context_menu_panel, input_container, keyboard_hint, keyboard_hints_footer, labeled_input, menu_item, menu_item_conditional, menu_item_disabled, menu_item_with_color, menu_separator, search_input_area, segmented_toggle, shell_indicator_chip};
pub use path_autocomplete::PathAutoCompleteState;
pub use rename_state::{
    cancel_rename, finish_rename, is_renaming, rename_input, start_rename, start_rename_with_blur,
    RenameState,
};
pub use simple_input::{SimpleInput, SimpleInputState};
pub use syntax::{highlight_content, load_syntax_set, HighlightedLine};
pub use code_view::{
    get_scrollbar_geometry, get_selected_text, start_scrollbar_drag, update_scrollbar_drag,
    ScrollbarDrag,
};
