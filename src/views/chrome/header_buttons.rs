//! Shared header action buttons for terminal panes and tab groups.
//!
//! This module provides reusable button definitions to ensure consistency
//! between regular terminal mode and tab group mode.

use crate::theme::ThemeColors;
use gpui::*;
use gpui_component::tooltip::Tooltip;

/// All available header actions for terminal panes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderAction {
    SplitVertical,
    SplitHorizontal,
    AddTab,
    Minimize,
    ExportBuffer,
    Fullscreen,
    Detach,
    Close,
}

impl HeaderAction {
    /// Returns the icon path for this action.
    pub fn icon(&self) -> &'static str {
        match self {
            HeaderAction::SplitVertical => "icons/split-vertical.svg",
            HeaderAction::SplitHorizontal => "icons/split-horizontal.svg",
            HeaderAction::AddTab => "icons/tabs.svg",
            HeaderAction::Minimize => "icons/minimize.svg",
            HeaderAction::ExportBuffer => "icons/copy.svg",
            HeaderAction::Fullscreen => "icons/fullscreen.svg",
            HeaderAction::Detach => "icons/detach.svg",
            HeaderAction::Close => "icons/close.svg",
        }
    }

    /// Returns the default tooltip text for this action.
    pub fn tooltip(&self) -> &'static str {
        match self {
            HeaderAction::SplitVertical => "Split Vertical",
            HeaderAction::SplitHorizontal => "Split Horizontal",
            HeaderAction::AddTab => "Add Tab",
            HeaderAction::Minimize => "Minimize",
            HeaderAction::ExportBuffer => "Export Buffer to File",
            HeaderAction::Fullscreen => "Fullscreen",
            HeaderAction::Detach => "Detach to Window",
            HeaderAction::Close => "Close",
        }
    }

    /// Returns true if this is the close action (for red hover styling).
    pub fn is_close(&self) -> bool {
        matches!(self, HeaderAction::Close)
    }

    /// Returns the element ID prefix for this action.
    pub fn id_prefix(&self) -> &'static str {
        match self {
            HeaderAction::SplitVertical => "split-vertical-btn",
            HeaderAction::SplitHorizontal => "split-horizontal-btn",
            HeaderAction::AddTab => "add-tab-btn",
            HeaderAction::Minimize => "minimize-btn",
            HeaderAction::ExportBuffer => "export-buffer-btn",
            HeaderAction::Fullscreen => "fullscreen-btn",
            HeaderAction::Detach => "detach-btn",
            HeaderAction::Close => "close-btn",
        }
    }
}

/// Button size configuration.
#[derive(Clone, Copy)]
pub struct ButtonSize {
    pub button: f32,
    pub icon: f32,
}

impl ButtonSize {
    /// Regular size for terminal pane headers (22px button, 14px icon).
    pub const REGULAR: Self = Self {
        button: 22.0,
        icon: 14.0,
    };

    /// Compact size for tab group headers (20px button, 12px icon).
    pub const COMPACT: Self = Self {
        button: 20.0,
        icon: 12.0,
    };
}

/// Renders a header button base element without click handler.
/// The caller should attach `.on_click()` to handle the action.
///
/// # Arguments
/// * `action` - The action this button represents
/// * `id_suffix` - Unique suffix for the element ID
/// * `size` - Button and icon size configuration
/// * `t` - Theme reference
/// * `tooltip_override` - Optional tooltip text override (e.g., "Close Tab" instead of "Close")
pub fn header_button_base(
    action: HeaderAction,
    id_suffix: &str,
    size: ButtonSize,
    t: &ThemeColors,
    tooltip_override: Option<&'static str>,
) -> Stateful<Div> {
    let tooltip_text = tooltip_override.unwrap_or_else(|| action.tooltip());
    let bg_hover = t.bg_hover;

    let base = div()
        .id(format!("{}-{}", action.id_prefix(), id_suffix))
        .cursor_pointer()
        .w(px(size.button))
        .h(px(size.button))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
        })
        .child(
            svg()
                .path(action.icon())
                .size(px(size.icon))
                .text_color(rgb(t.text_secondary)),
        )
        .tooltip(move |_window, cx| Tooltip::new(tooltip_text).build(_window, cx));

    // Apply hover style - red for close, normal for others
    if action.is_close() {
        base.hover(|s| s.bg(rgba(0xf14c4c99)))
    } else {
        base.hover(move |s| s.bg(rgb(bg_hover)))
    }
}
