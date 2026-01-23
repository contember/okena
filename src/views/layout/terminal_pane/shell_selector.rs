//! Shell selector button component.
//!
//! An Entity with Render that displays current shell and emits event to open selector.

use crate::terminal::shell_config::ShellType;
use crate::theme::theme;
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

    /// Get display name for the current shell type.
    fn get_display_name(&self) -> &'static str {
        match &self.current_shell {
            ShellType::Default => "Default",
            #[cfg(windows)]
            ShellType::Cmd => "CMD",
            #[cfg(windows)]
            ShellType::PowerShell { core } => {
                if *core { "pwsh" } else { "PS" }
            }
            #[cfg(windows)]
            ShellType::Wsl { .. } => "WSL",
            ShellType::Custom { .. } => "Custom",
        }
    }
}

impl Render for ShellSelector {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let shell_name = self.get_display_name();

        div()
            .id(format!("shell-indicator-{}", self.id_suffix))
            .cursor_pointer()
            .px(px(6.0))
            .h(px(18.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .bg(rgb(t.bg_secondary))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_this, _, _window, cx| {
                    cx.stop_propagation();
                    cx.emit(ShellSelectorEvent::OpenSelector);
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgb(t.text_secondary))
                            .child(shell_name),
                    )
                    .child(
                        svg()
                            .path("icons/chevron-down.svg")
                            .size(px(10.0))
                            .text_color(rgb(t.text_secondary)),
                    ),
            )
            .tooltip(|_window, cx| Tooltip::new("Switch Shell").build(_window, cx))
    }
}
