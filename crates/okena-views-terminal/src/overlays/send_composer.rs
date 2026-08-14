//! Annotate a terminal selection and paste it back into that same terminal.
//!
//! The usual subject is an agent's own output: quote the part you mean, add a
//! note, and it lands in the agent's prompt unsent so you can still edit it.

use crate::actions::Cancel;
use gpui::prelude::*;
use gpui::*;
use okena_ui::button::{button, button_primary};
use okena_ui::simple_input::{SimpleInput, SimpleInputState};
use okena_ui::theme::theme;
use okena_ui::tokens::*;

/// Lines of the quoted selection shown in the preview before it is elided.
const PREVIEW_LINES: usize = 6;

pub enum SendComposerEvent {
    Close,
    Send {
        terminal_id: String,
        quoted: String,
        note: String,
    },
}

pub struct SendComposer {
    terminal_id: String,
    /// Selection snapshot taken when the composer opened — the pane keeps
    /// running underneath, and the selection can be cleared by a stray click.
    quoted: String,
    position: Point<Pixels>,
    note_input: Entity<SimpleInputState>,
    focus_handle: FocusHandle,
}

impl SendComposer {
    pub fn new(
        terminal_id: String,
        quoted: String,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> Self {
        let note_input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .multiline()
                .submit_on_enter()
                .placeholder("Add a note…")
        });
        Self {
            terminal_id,
            quoted,
            position,
            note_input,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn close(&self, cx: &mut Context<Self>) {
        cx.emit(SendComposerEvent::Close);
    }

    pub fn send(&mut self, cx: &mut Context<Self>) {
        let note = self.note_input.read(cx).value().to_string();
        cx.emit(SendComposerEvent::Send {
            terminal_id: self.terminal_id.clone(),
            quoted: self.quoted.clone(),
            note,
        });
    }

    /// Set the note text (used by tests and future prefill paths).
    pub fn set_note(&mut self, note: impl Into<String>, cx: &mut Context<Self>) {
        self.note_input
            .update(cx, |input, cx| input.set_value(note.into(), cx));
    }

    /// First few lines of the quote, with a count of what is hidden.
    fn preview(&self) -> (String, Option<String>) {
        let lines: Vec<&str> = self.quoted.lines().collect();
        if lines.len() <= PREVIEW_LINES {
            return (self.quoted.clone(), None);
        }
        let shown = lines[..PREVIEW_LINES].join("\n");
        let hidden = lines.len() - PREVIEW_LINES;
        (shown, Some(format!("+{} more lines", hidden)))
    }
}

impl EventEmitter<SendComposerEvent> for SendComposer {}

impl Render for SendComposer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let position = self.position;
        let (preview, elided) = self.preview();

        // The note is the point of the overlay — focus it, not the panel.
        let note_focus = self.note_input.read(cx).focus_handle(cx);
        if !note_focus.is_focused(window) {
            window.focus(&note_focus, cx);
        }

        div()
            .track_focus(&self.focus_handle)
            .key_context("SendComposer")
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            // Plain Enter submits; the input keeps Shift+Enter for line breaks
            // (see SimpleInputState::submit_on_enter).
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                if event.keystroke.key == "enter" && !event.keystroke.modifiers.shift {
                    this.send(cx);
                }
            }))
            .absolute()
            .inset_0()
            .id("send-composer-backdrop")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    this.close(cx);
                }),
            )
            // Right-click too: the backdrop covers the whole window, so without
            // this a right-click would open a context menu behind it.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _window, cx| {
                    this.close(cx);
                }),
            )
            .child(deferred(
                anchored().position(position).snap_to_window().child(
                    div()
                        .id("send-composer")
                        .w(px(420.0))
                        .bg(rgb(t.bg_primary))
                        .border_1()
                        .border_color(rgb(t.border))
                        .rounded(px(8.0))
                        .shadow_xl()
                        .p(SPACE_LG)
                        .flex()
                        .flex_col()
                        .gap(SPACE_MD)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_scroll_wheel(|_, _, cx| {
                            cx.stop_propagation();
                        })
                        // Quoted selection
                        .child(
                            div()
                                .bg(rgb(t.bg_secondary))
                                .border_l_2()
                                .border_color(rgb(t.border_active))
                                .rounded(RADIUS_STD)
                                .px(SPACE_MD)
                                .py(SPACE_SM)
                                .text_size(TEXT_SM)
                                .text_color(rgb(t.text_secondary))
                                .font_family("monospace")
                                .child(preview)
                                .when_some(elided, |d, label| {
                                    d.child(
                                        div()
                                            .pt(SPACE_XS)
                                            .text_size(TEXT_XS)
                                            .text_color(rgb(t.text_muted))
                                            .child(label),
                                    )
                                }),
                        )
                        // Note
                        .child(
                            div()
                                .bg(rgb(t.bg_secondary))
                                .border_1()
                                .border_color(rgb(t.border_focused))
                                .rounded(RADIUS_STD)
                                .child(SimpleInput::new(&self.note_input).text_size(TEXT_MD)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(SPACE_MD)
                                .child(
                                    div()
                                        .text_size(TEXT_XS)
                                        .text_color(rgb(t.text_muted))
                                        .child("Shift+Enter for a new line"),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap(SPACE_MD)
                                        .child(
                                            button("send-composer-cancel", "Cancel", &t).on_click(
                                                cx.listener(|this, _, _window, cx| this.close(cx)),
                                            ),
                                        )
                                        .child(
                                            button_primary("send-composer-send", "Send", &t)
                                                .on_click(cx.listener(|this, _, _window, cx| {
                                                    this.send(cx)
                                                })),
                                        ),
                                ),
                        ),
                ),
            ))
    }
}

impl Focusable for SendComposer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
