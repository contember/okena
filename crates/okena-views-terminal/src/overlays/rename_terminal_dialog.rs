//! Dialog for renaming a terminal when its header is hidden.

use crate::actions::Cancel;
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use okena_ui::button::{button, button_primary};
use okena_ui::input::input_container;
use okena_ui::modal::{modal_backdrop, modal_content};
use okena_ui::simple_input::{SimpleInput, SimpleInputState};
use okena_ui::theme::theme;
use okena_ui::tokens::{ui_text, ui_text_md, ui_text_xl};

#[derive(Clone)]
pub enum RenameTerminalDialogEvent {
    Close,
    Confirmed {
        project_id: String,
        terminal_id: String,
        new_name: String,
    },
}

impl okena_ui::overlay::CloseEvent for RenameTerminalDialogEvent {
    fn is_close(&self) -> bool {
        matches!(self, Self::Close | Self::Confirmed { .. })
    }
}

impl EventEmitter<RenameTerminalDialogEvent> for RenameTerminalDialog {}

pub struct RenameTerminalDialog {
    project_id: String,
    terminal_id: String,
    name_input: Entity<SimpleInputState>,
    show_empty_error: bool,
    focus_handle: FocusHandle,
    initialized: bool,
}

impl RenameTerminalDialog {
    pub fn new(
        project_id: String,
        terminal_id: String,
        current_name: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let name_input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Terminal name...")
                .default_value(current_name)
        });

        Self {
            project_id,
            terminal_id,
            name_input,
            show_empty_error: false,
            focus_handle: cx.focus_handle(),
            initialized: false,
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(RenameTerminalDialogEvent::Close);
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let new_name = self.name_input.read(cx).value().trim().to_string();
        if new_name.is_empty() {
            self.show_empty_error = true;
            cx.notify();
            return;
        }

        cx.emit(RenameTerminalDialogEvent::Confirmed {
            project_id: self.project_id.clone(),
            terminal_id: self.terminal_id.clone(),
            new_name,
        });
    }
}

okena_ui::impl_focusable!(RenameTerminalDialog);

impl Render for RenameTerminalDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();

        if !self.initialized {
            self.initialized = true;
            self.name_input.update(cx, |input, cx| {
                input.focus(window, cx);
            });
        }

        let name_input = self.name_input.clone();
        let input_focused = self.name_input.read(cx).focus_handle(cx).is_focused(window);

        modal_backdrop("rename-terminal-dialog-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("RenameTerminalDialog")
            .items_center()
            .on_action(cx.listener(|this, _: &Cancel, _, cx| {
                this.close(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key.as_str() == "enter" {
                    this.confirm(cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close(cx);
                }),
            )
            .child(
                modal_content("rename-terminal-dialog", &t)
                    .w(px(380.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .border_b_1()
                            .border_color(rgb(t.border))
                            .child(
                                h_flex()
                                    .gap(px(8.0))
                                    .child(
                                        svg()
                                            .path("icons/terminal.svg")
                                            .size(px(16.0))
                                            .text_color(rgb(t.border_active)),
                                    )
                                    .child(
                                        div()
                                            .text_size(ui_text_xl(cx))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(t.text_primary))
                                            .child("Rename Terminal"),
                                    ),
                            )
                            .child(
                                div()
                                    .id("close-rename-terminal-btn")
                                    .cursor_pointer()
                                    .size(px(24.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.0))
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .child(
                                        svg()
                                            .path("icons/close.svg")
                                            .size(px(14.0))
                                            .text_color(rgb(t.text_secondary)),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.close(cx);
                                    })),
                            ),
                    )
                    .child(
                        div().px(px(16.0)).py(px(12.0)).child(
                            input_container(&t, Some(input_focused))
                                .child(SimpleInput::new(&name_input).text_size(ui_text(13.0, cx))),
                        ),
                    )
                    .when(self.show_empty_error, |d| {
                        d.child(
                            div()
                                .px(px(16.0))
                                .pb(px(8.0))
                                .text_size(ui_text_md(cx))
                                .text_color(rgb(t.error))
                                .child("Terminal name cannot be empty"),
                        )
                    })
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(12.0))
                            .flex()
                            .justify_end()
                            .gap(px(8.0))
                            .border_t_1()
                            .border_color(rgb(t.border))
                            .child(
                                button("cancel-rename-terminal-btn", "Cancel", &t)
                                    .px(px(16.0))
                                    .py(px(8.0))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.close(cx);
                                    })),
                            )
                            .child(
                                button_primary("confirm-rename-terminal-btn", "Rename", &t)
                                    .px(px(16.0))
                                    .py(px(8.0))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.confirm(cx);
                                    })),
                            ),
                    ),
            )
    }
}
