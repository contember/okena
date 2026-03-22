//! Shared header action buttons for terminal panes and tab groups.
//!
//! Re-exports from okena-ui and provides a keybinding-aware wrapper.

pub use okena_ui::header_buttons::{ButtonSize, HeaderAction};

use crate::keybindings;
use crate::theme::ThemeColors;
use gpui::*;

/// Renders a header button base element with keybinding-aware tooltips.
/// The caller should attach `.on_click()` to handle the action.
pub fn header_button_base(
    action: HeaderAction,
    id_suffix: &str,
    size: ButtonSize,
    t: &ThemeColors,
    tooltip_override: Option<&'static str>,
) -> Stateful<Div> {
    let gpui_action = gpui_action_for(action);
    okena_ui::header_buttons::header_button_base(action, id_suffix, size, t, tooltip_override, gpui_action)
}

/// Returns the corresponding GPUI action for keybinding display in tooltips.
fn gpui_action_for(action: HeaderAction) -> Option<Box<dyn Action>> {
    match action {
        HeaderAction::SplitVertical => Some(Box::new(keybindings::SplitVertical)),
        HeaderAction::SplitHorizontal => Some(Box::new(keybindings::SplitHorizontal)),
        HeaderAction::AddTab => Some(Box::new(keybindings::AddTab)),
        HeaderAction::Minimize => Some(Box::new(keybindings::MinimizeTerminal)),
        HeaderAction::Fullscreen => Some(Box::new(keybindings::ToggleFullscreen)),
        HeaderAction::Close => Some(Box::new(keybindings::CloseTerminal)),
        HeaderAction::ExportBuffer | HeaderAction::Detach
        | HeaderAction::ZoomPrev | HeaderAction::ZoomNext | HeaderAction::ExitZoom => None,
    }
}
