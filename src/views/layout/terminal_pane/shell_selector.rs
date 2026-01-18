//! Shell selector dropdown component.
//!
//! An Entity with Render that displays current shell and allows switching.

use crate::terminal::shell_config::{available_shells, AvailableShell, ShellType};
use crate::theme::theme;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::tooltip::Tooltip;

/// Event emitted when shell is changed.
#[derive(Clone)]
pub enum ShellSelectorEvent {
    ShellChanged(ShellType),
}

impl EventEmitter<ShellSelectorEvent> for ShellSelector {}

/// Shell selector view for switching between shells.
pub struct ShellSelector {
    /// Current shell type
    current_shell: ShellType,
    /// Available shells
    available_shells: Vec<AvailableShell>,
    /// Whether dropdown is open
    dropdown_open: bool,
    /// Unique ID suffix for element IDs
    id_suffix: String,
}

impl ShellSelector {
    pub fn new(shell_type: ShellType, id_suffix: String, _cx: &mut Context<Self>) -> Self {
        Self {
            current_shell: shell_type,
            available_shells: available_shells(),
            dropdown_open: false,
            id_suffix,
        }
    }

    /// Get current shell type.
    pub fn current_shell(&self) -> &ShellType {
        &self.current_shell
    }

    /// Set current shell type (used when restoring from workspace state).
    pub fn set_shell(&mut self, shell_type: ShellType) {
        self.current_shell = shell_type;
    }

    /// Check if dropdown is open.
    pub fn is_open(&self) -> bool {
        self.dropdown_open
    }

    /// Close the dropdown.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.dropdown_open {
            self.dropdown_open = false;
            cx.notify();
        }
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

    /// Toggle dropdown visibility.
    fn toggle_dropdown(&mut self, cx: &mut Context<Self>) {
        self.dropdown_open = !self.dropdown_open;
        cx.notify();
    }

    /// Select a shell and emit event.
    fn select_shell(&mut self, shell_type: ShellType, cx: &mut Context<Self>) {
        self.dropdown_open = false;
        if self.current_shell != shell_type {
            self.current_shell = shell_type.clone();
            cx.emit(ShellSelectorEvent::ShellChanged(shell_type));
        }
        cx.notify();
    }

    /// Render the shell indicator button.
    fn render_indicator(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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
                cx.listener(|this, _, _window, cx| {
                    cx.stop_propagation();
                    this.toggle_dropdown(cx);
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

    /// Render the dropdown modal.
    fn render_dropdown(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        if !self.dropdown_open {
            return div().into_any_element();
        }

        let shells = self.available_shells.clone();
        let current_shell = self.current_shell.clone();

        div()
            .id("shell-modal-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000088))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    this.dropdown_open = false;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .id("shell-modal")
                    .w(px(200.0))
                    .bg(rgb(t.bg_secondary))
                    .border_1()
                    .border_color(rgb(t.border))
                    .rounded(px(8.0))
                    .shadow_lg()
                    .overflow_hidden()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
                            .border_b_1()
                            .border_color(rgb(t.border))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(t.text_primary))
                                    .child("Switch Shell"),
                            ),
                    )
                    .child(
                        div().py(px(2.0)).children(
                            shells
                                .into_iter()
                                .filter(|s| s.available)
                                .map(|shell| {
                                    let shell_type = shell.shell_type.clone();
                                    let is_current = shell_type == current_shell;
                                    let name = shell.name.clone();

                                    div()
                                        .id(format!(
                                            "shell-option-{}",
                                            name.replace(' ', "-").to_lowercase()
                                        ))
                                        .w_full()
                                        .px(px(12.0))
                                        .py(px(6.0))
                                        .cursor_pointer()
                                        .bg(if is_current {
                                            rgb(t.bg_hover)
                                        } else {
                                            rgb(t.bg_secondary)
                                        })
                                        .hover(|s| s.bg(rgb(t.bg_hover)))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _window, cx| {
                                                cx.stop_propagation();
                                                this.select_shell(shell_type.clone(), cx);
                                            }),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(8.0))
                                                .child(
                                                    div()
                                                        .text_size(px(12.0))
                                                        .text_color(rgb(t.text_primary))
                                                        .child(name),
                                                )
                                                .when(is_current, |el| {
                                                    el.child(
                                                        svg()
                                                            .path("icons/check.svg")
                                                            .size(px(12.0))
                                                            .text_color(rgb(t.success)),
                                                    )
                                                }),
                                        )
                                }),
                        ),
                    ),
            )
            .into_any_element()
    }
}

impl Render for ShellSelector {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .child(self.render_indicator(cx))
            .child(self.render_dropdown(cx))
    }
}
