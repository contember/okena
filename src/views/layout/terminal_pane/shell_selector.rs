//! Shell selector button component.
//!
//! An Entity with Render that displays current shell and emits event to open selector.

use crate::terminal::shell_config::ShellType;
use crate::theme::theme;
use crate::views::components::shell_indicator_chip;
use gpui::*;
use gpui_component::tooltip::Tooltip;

/// Event emitted by ShellSelector.
#[derive(Clone)]
pub enum ShellSelectorEvent {
    /// Request to open shell selector overlay
    OpenSelector,
}

impl EventEmitter<ShellSelectorEvent> for ShellSelector {}

/// Shell selector button for switching between shells.
pub struct ShellSelector {
    /// Current shell type
    current_shell: ShellType,
    /// Unique ID suffix for element IDs
    id_suffix: String,
}

impl ShellSelector {
    pub fn new(shell_type: ShellType, id_suffix: String, _cx: &mut Context<Self>) -> Self {
        Self {
            current_shell: shell_type,
            id_suffix,
        }
    }

    /// Get current shell type.
    pub fn current_shell(&self) -> &ShellType {
        &self.current_shell
    }

    /// Close the dropdown (no-op now, kept for API compatibility).
    pub fn close(&mut self, _cx: &mut Context<Self>) {
        // No-op - dropdown is now in overlay
    }
}

impl Render for ShellSelector {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let shell_name = self.current_shell.short_display_name();

        shell_indicator_chip(format!("shell-indicator-{}", self.id_suffix), shell_name, &t)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_this, _, _window, cx| {
                    cx.stop_propagation();
                    cx.emit(ShellSelectorEvent::OpenSelector);
                }),
            )
            .tooltip(|_window, cx| Tooltip::new("Switch Shell").build(_window, cx))
    }
}
