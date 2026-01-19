//! Shell selector overlay for switching terminal shells.
//!
//! Displays available shells in a modal dialog for selection.

use crate::terminal::shell_config::{available_shells, AvailableShell, ShellType};
use crate::theme::theme;
use crate::views::components::{modal_backdrop, modal_content, modal_header};
use gpui::*;
use gpui::prelude::*;

/// Shell selector overlay for choosing a shell.
pub struct ShellSelectorOverlay {
    focus_handle: FocusHandle,
    available_shells: Vec<AvailableShell>,
    current_shell: ShellType,
    selected_index: usize,
    /// Context: which terminal this is for (project_id, terminal_id)
    context: Option<(String, String)>,
}

impl ShellSelectorOverlay {
    pub fn new(current_shell: ShellType, context: Option<(String, String)>, cx: &mut Context<Self>) -> Self {
        let shells: Vec<_> = available_shells().into_iter().filter(|s| s.available).collect();
        let selected_index = shells
            .iter()
            .position(|s| s.shell_type == current_shell)
            .unwrap_or(0);

        Self {
            focus_handle: cx.focus_handle(),
            available_shells: shells,
            current_shell,
            selected_index,
            context,
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(ShellSelectorOverlayEvent::Close);
    }

    fn select_shell(&mut self, shell_type: ShellType, cx: &mut Context<Self>) {
        cx.emit(ShellSelectorOverlayEvent::ShellSelected {
            shell_type,
            context: self.context.clone(),
        });
    }

    fn select_current(&mut self, cx: &mut Context<Self>) {
        if let Some(shell) = self.available_shells.get(self.selected_index) {
            self.select_shell(shell.shell_type.clone(), cx);
        }
    }
}

#[derive(Clone)]
pub enum ShellSelectorOverlayEvent {
    Close,
    ShellSelected {
        shell_type: ShellType,
        context: Option<(String, String)>,
    },
}

impl EventEmitter<ShellSelectorOverlayEvent> for ShellSelectorOverlay {}

impl Render for ShellSelectorOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let shells = self.available_shells.clone();
        let current_shell = self.current_shell.clone();
        let selected_index = self.selected_index;

        window.focus(&focus_handle, cx);

        modal_backdrop("shell-selector-overlay-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("ShellSelectorOverlay")
            .items_center()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => this.close(cx),
                    "up" => {
                        if this.selected_index > 0 {
                            this.selected_index -= 1;
                            cx.notify();
                        }
                    }
                    "down" => {
                        if this.selected_index < this.available_shells.len().saturating_sub(1) {
                            this.selected_index += 1;
                            cx.notify();
                        }
                    }
                    "enter" => this.select_current(cx),
                    _ => {}
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _window, cx| {
                this.close(cx);
            }))
            .child(
                modal_content("shell-selector-overlay-modal", &t)
                    .w(px(280.0))
                    .child(modal_header(
                        "Switch Shell",
                        Some("Select shell for this terminal"),
                        &t,
                        cx.listener(|this, _, _window, cx| this.close(cx)),
                    ))
                    .child(
                        div()
                            .id("shell-selector-list")
                            .py(px(4.0))
                            .max_h(px(300.0))
                            .overflow_y_scroll()
                            .children(shells.into_iter().enumerate().map(|(i, shell)| {
                                let is_current = shell.shell_type == current_shell;
                                let is_selected = i == selected_index;
                                let shell_type = shell.shell_type.clone();
                                let name = shell.name.clone();

                                div()
                                    .id(ElementId::Name(format!("shell-opt-{}", i).into()))
                                    .w_full()
                                    .px(px(12.0))
                                    .py(px(8.0))
                                    .cursor_pointer()
                                    .when(is_selected, |d| d.bg(rgb(t.bg_hover)))
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
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_size(px(13.0))
                                                    .text_color(rgb(t.text_primary))
                                                    .child(name),
                                            )
                                            .when(is_current, |d| {
                                                d.child(
                                                    div()
                                                        .text_size(px(12.0))
                                                        .text_color(rgb(t.success))
                                                        .child("✓"),
                                                )
                                            }),
                                    )
                            })),
                    ),
            )
    }
}

impl_focusable!(ShellSelectorOverlay);
