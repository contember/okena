use crate::keybindings::Cancel;
use crate::remote::auth::AuthStore;
use crate::theme::theme;
use crate::views::components::{modal_backdrop, modal_content, modal_header};
use gpui::*;
use gpui::prelude::*;
use std::sync::Arc;

pub struct PairingDialog {
    focus_handle: FocusHandle,
    auth_store: Arc<AuthStore>,
    code: String,
    remaining_secs: u64,
    expired: bool,
}

pub enum PairingDialogEvent {
    Close,
}

impl EventEmitter<PairingDialogEvent> for PairingDialog {}

impl PairingDialog {
    pub fn new(auth_store: Arc<AuthStore>, cx: &mut Context<Self>) -> Self {
        let code = auth_store.generate_fresh_code();
        let remaining_secs = auth_store.code_remaining_secs();

        // Start countdown timer
        cx.spawn(async move |this: WeakEntity<PairingDialog>, cx| {
            loop {
                smol::Timer::after(std::time::Duration::from_secs(1)).await;
                let should_continue = this.update(cx, |this, cx| {
                    let remaining = this.auth_store.code_remaining_secs();
                    this.remaining_secs = remaining;
                    if remaining == 0 {
                        this.expired = true;
                    }
                    cx.notify();
                    true
                });
                if should_continue.is_err() {
                    break;
                }
            }
        })
        .detach();

        Self {
            focus_handle: cx.focus_handle(),
            auth_store,
            code,
            remaining_secs,
            expired: false,
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        self.auth_store.invalidate_code();
        cx.emit(PairingDialogEvent::Close);
    }

    fn generate_new_code(&mut self, cx: &mut Context<Self>) {
        self.code = self.auth_store.generate_fresh_code();
        self.remaining_secs = self.auth_store.code_remaining_secs();
        self.expired = false;
        cx.notify();
    }
}

impl Render for PairingDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();

        if !focus_handle.is_focused(window) {
            window.focus(&focus_handle, cx);
        }

        let code = self.code.clone();
        let code_for_copy = code.clone();
        let expired = self.expired;
        let remaining = self.remaining_secs;

        modal_backdrop("pairing-dialog-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("PairingDialog")
            .items_center()
            .on_action(cx.listener(|this, _: &Cancel, _, cx| {
                this.close(cx);
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.close(cx);
            }))
            .child(
                modal_content("pairing-dialog-modal", &t)
                    .w(px(400.0))
                    .child(modal_header(
                        "Pair Device",
                        Some("Enter this code on your client to connect"),
                        &t,
                        cx.listener(|this, _, _, cx| this.close(cx)),
                    ))
                    .child(
                        div()
                            .px(px(24.0))
                            .py(px(24.0))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(16.0))
                            // Code display
                            .when(!expired, |d| {
                                d.child(
                                    div()
                                        .text_size(px(32.0))
                                        .font_weight(FontWeight::BOLD)
                                        .font_family("JetBrains Mono")
                                        .text_color(rgb(t.term_yellow))
                                        .child(code.clone()),
                                )
                            })
                            .when(expired, |d| {
                                d.child(
                                    div()
                                        .text_size(px(18.0))
                                        .text_color(rgb(t.text_muted))
                                        .child("Code expired"),
                                )
                            })
                            // Countdown or generate button
                            .when(!expired, |d| {
                                d.child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(rgb(t.text_muted))
                                        .child(format!("Expires in {}s", remaining)),
                                )
                            })
                            // Buttons row
                            .child(
                                div()
                                    .flex()
                                    .gap(px(8.0))
                                    .when(!expired, |d| {
                                        d.child(
                                            div()
                                                .id("copy-code-btn")
                                                .cursor_pointer()
                                                .px(px(12.0))
                                                .py(px(6.0))
                                                .rounded(px(4.0))
                                                .bg(rgb(t.bg_secondary))
                                                .border_1()
                                                .border_color(rgb(t.border))
                                                .text_size(px(12.0))
                                                .text_color(rgb(t.text_primary))
                                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                                .child("Copy Code")
                                                .on_click(move |_, _window, cx| {
                                                    cx.write_to_clipboard(
                                                        ClipboardItem::new_string(code_for_copy.clone()),
                                                    );
                                                }),
                                        )
                                    })
                                    .when(expired, |d| {
                                        d.child(
                                            div()
                                                .id("generate-new-code-btn")
                                                .cursor_pointer()
                                                .px(px(12.0))
                                                .py(px(6.0))
                                                .rounded(px(4.0))
                                                .bg(rgb(t.term_cyan))
                                                .text_size(px(12.0))
                                                .text_color(rgb(t.bg_primary))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .hover(|s| s.opacity(0.9))
                                                .child("Generate New Code")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.generate_new_code(cx);
                                                })),
                                        )
                                    }),
                            ),
                    ),
            )
    }
}

impl_focusable!(PairingDialog);
