//! Dialog action button row (Cancel + Confirm).

use crate::button::{button, button_primary};
use crate::theme::ThemeColors;
use gpui::*;

/// Cancel + Confirm button row for dialogs.
///
/// Right-aligned with 8px gap. Returns a Div that the caller can wrap in padding.
pub fn dialog_actions<F1, F2>(
    cancel_label: impl Into<SharedString>,
    on_cancel: F1,
    confirm_label: impl Into<SharedString>,
    on_confirm: F2,
    t: &ThemeColors,
) -> Div
where
    F1: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    F2: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    div()
        .flex()
        .gap(px(8.0))
        .justify_end()
        .child(
            button("dialog-cancel-btn", cancel_label, t)
                .on_click(on_cancel),
        )
        .child(
            button_primary("dialog-confirm-btn", confirm_label, t)
                .on_click(on_confirm),
        )
}
