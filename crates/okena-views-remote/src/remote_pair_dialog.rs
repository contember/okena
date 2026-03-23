//! Re-pair dialog for existing remote connections.

use crate::Cancel;
use okena_ui::dialog_actions::dialog_actions;
use okena_ui::input::{input_container, labeled_input};
use okena_ui::modal::{modal_backdrop, modal_content, modal_header};
use okena_ui::simple_input::{SimpleInput, SimpleInputState};
use okena_ui::theme::theme;
use gpui::prelude::*;
use gpui::*;

pub struct RemotePairDialog {
    connection_id: String,
    connection_name: String,
    focus_handle: FocusHandle,
    code_input: Entity<SimpleInputState>,
    initial_focus_done: bool,
}

pub enum RemotePairDialogEvent {
    Close,
    Pair { connection_id: String, code: String },
}

impl okena_ui::overlay::CloseEvent for RemotePairDialogEvent {
    fn is_close(&self) -> bool { matches!(self, Self::Close) }
}

impl EventEmitter<RemotePairDialogEvent> for RemotePairDialog {}

impl RemotePairDialog {
    pub fn new(
        connection_id: String,
        connection_name: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let code_input =
            cx.new(|cx| SimpleInputState::new(cx).placeholder("Pairing code from remote..."));

        Self {
            connection_id,
            connection_name,
            focus_handle: cx.focus_handle(),
            code_input,
            initial_focus_done: false,
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(RemotePairDialogEvent::Close);
    }

    fn pair(&self, cx: &mut Context<Self>) {
        let code = self.code_input.read(cx).value().to_string();
        if code.is_empty() {
            return;
        }
        cx.emit(RemotePairDialogEvent::Pair {
            connection_id: self.connection_id.clone(),
            code,
        });
    }
}

impl Render for RemotePairDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();

        if !self.initial_focus_done {
            self.initial_focus_done = true;
            self.code_input.update(cx, |input, cx| {
                input.focus(window, cx);
            });
        }

        modal_backdrop("remote-pair-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("RemotePairDialog")
            .items_center()
            .on_action(cx.listener(|this, _: &Cancel, _, cx| {
                this.close(cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close(cx);
                }),
            )
            .child(
                modal_content("remote-pair-modal", &t)
                    .w(px(400.0))
                    .child(modal_header(
                        &format!("Pair \"{}\"", self.connection_name),
                        None::<&str>,
                        &t,
                        cx.listener(|this, _, _, cx| this.close(cx)),
                    ))
                    .child(
                        div()
                            .p(px(16.0))
                            .flex()
                            .flex_col()
                            .gap(px(12.0))
                            .child(
                                labeled_input("Pairing Code:", &t).child(
                                    input_container(&t, None).child(
                                        SimpleInput::new(&self.code_input).text_size(px(12.0)),
                                    ),
                                ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(rgb(t.text_muted))
                                    .child(
                                        "Enter the pairing code shown on the remote machine's status bar",
                                    ),
                            )
                            .child(
                                dialog_actions(
                                    "Cancel",
                                    cx.listener(|this, _, _window, cx| { this.close(cx); }),
                                    "Pair",
                                    cx.listener(|this, _, _window, cx| { this.pair(cx); }),
                                    &t,
                                ),
                            ),
                    ),
            )
    }
}

okena_ui::impl_focusable!(RemotePairDialog);
