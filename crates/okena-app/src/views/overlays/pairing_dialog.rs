use crate::keybindings::Cancel;
use crate::theme::theme;
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_sm};
use crate::views::components::{modal_backdrop, modal_content, modal_header};
use gpui::prelude::*;
use gpui::*;
use okena_transport::client::tls::format_fingerprint;
use std::time::Instant;

#[derive(Clone)]
pub struct PairingEndpoint {
    pub host: String,
    pub port: u16,
    pub token: String,
}

pub struct PairingDialog {
    focus_handle: FocusHandle,
    endpoint: Option<PairingEndpoint>,
    code: String,
    code_created_at: Instant,
    remaining_secs: u64,
    expired: bool,
    error: Option<String>,
}

pub enum PairingDialogEvent {
    Close,
}

impl EventEmitter<PairingDialogEvent> for PairingDialog {}

impl PairingDialog {
    pub fn new(endpoint: Option<PairingEndpoint>, cx: &mut Context<Self>) -> Self {
        // Start countdown timer
        cx.spawn(async move |this: WeakEntity<PairingDialog>, cx| {
            loop {
                smol::Timer::after(std::time::Duration::from_secs(1)).await;
                let should_continue = this.update(cx, |this, cx| {
                    if !this.code.is_empty() {
                        this.remaining_secs = seconds_remaining(this.code_created_at);
                        if this.remaining_secs == 0 {
                            this.expired = true;
                        }
                        cx.notify();
                    }
                    true
                });
                if should_continue.is_err() {
                    break;
                }
            }
        })
        .detach();

        let dialog = Self {
            focus_handle: cx.focus_handle(),
            endpoint,
            code: String::new(),
            code_created_at: Instant::now(),
            remaining_secs: 0,
            expired: false,
            error: None,
        };

        dialog.request_new_code(cx);
        dialog
    }

    fn request_new_code(&self, cx: &mut Context<Self>) {
        let Some(endpoint) = self.endpoint.clone() else {
            cx.spawn(async move |this: WeakEntity<PairingDialog>, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.code.clear();
                    this.remaining_secs = 0;
                    this.expired = true;
                    this.error = Some("Local daemon connection is not ready".to_string());
                    cx.notify();
                });
            })
            .detach();
            return;
        };

        cx.spawn(async move |this: WeakEntity<PairingDialog>, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    okena_remote_server::local::request_pair_code(
                        &endpoint.host,
                        endpoint.port,
                        &endpoint.token,
                    )
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(code) => {
                        this.code = code.code;
                        this.code_created_at = Instant::now();
                        this.remaining_secs = code.expires_in;
                        this.expired = false;
                        this.error = None;
                    }
                    Err(e) => {
                        this.code.clear();
                        this.remaining_secs = 0;
                        this.expired = true;
                        this.error = Some(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn invalidate_code(&self, cx: &mut Context<Self>) {
        let Some(endpoint) = self.endpoint.clone() else {
            return;
        };

        cx.spawn(async move |_this: WeakEntity<PairingDialog>, cx| {
            cx.background_executor()
                .spawn(async move {
                    okena_remote_server::local::invalidate_pair_code(
                        &endpoint.host,
                        endpoint.port,
                        &endpoint.token,
                    );
                })
                .await;
        })
        .detach();
    }

    fn clear_current_code(&mut self) {
        self.code.clear();
        self.remaining_secs = 0;
        self.expired = false;
        self.error = None;
    }

    fn close(&self, cx: &mut Context<Self>) {
        self.invalidate_code(cx);
        cx.emit(PairingDialogEvent::Close);
    }

    fn generate_new_code(&mut self, cx: &mut Context<Self>) {
        self.clear_current_code();
        self.request_new_code(cx);
        cx.notify();
    }
}

fn seconds_remaining(created_at: Instant) -> u64 {
    60u64.saturating_sub(created_at.elapsed().as_secs())
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
        let error = self.error.clone();
        let loading = code.is_empty() && error.is_none() && !expired;
        // TLS cert fingerprint, shown so the host can read it out for the client
        // to verify during pairing. `None` when the server runs without TLS.
        let fingerprint =
            crate::remote::tls::read_fingerprint(&crate::workspace::persistence::config_dir());

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
                        cx,
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
                            .when(loading, |d| {
                                d.child(
                                    div()
                                        .text_size(ui_text_md(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child("Generating code..."),
                                )
                            })
                            .when_some(error, |d, error| {
                                d.child(
                                    div()
                                        .text_size(ui_text_md(cx))
                                        .text_color(rgb(t.term_red))
                                        .child(error),
                                )
                            })
                            // Code display
                            .when(!expired && !code.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_size(ui_text(32.0, cx))
                                        .font_weight(FontWeight::BOLD)
                                        .font_family("JetBrains Mono")
                                        .text_color(rgb(t.term_yellow))
                                        .child(code.clone()),
                                )
                            })
                            .when(expired, |d| {
                                d.child(
                                    div()
                                        .text_size(ui_text(18.0, cx))
                                        .text_color(rgb(t.text_muted))
                                        .child(if code.is_empty() { "No code available" } else { "Code expired" }),
                                )
                            })
                            // Countdown or generate button
                            .when(!expired && !code.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_size(ui_text_md(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child(format!("Expires in {}s", remaining)),
                                )
                            })
                            // TLS certificate fingerprint (when serving over TLS)
                            .when_some(fingerprint, |d, fp| {
                                d.child(
                                    div()
                                        .w_full()
                                        .flex()
                                        .flex_col()
                                        .gap(px(4.0))
                                        .child(
                                            div()
                                                .text_size(ui_text_sm(cx))
                                                .text_color(rgb(t.text_muted))
                                                .child("TLS certificate fingerprint — verify it matches the one shown on the client:"),
                                        )
                                        .child(
                                            div()
                                                .w_full()
                                                .bg(rgb(t.bg_secondary))
                                                .border_1()
                                                .border_color(rgb(t.border))
                                                .rounded(px(4.0))
                                                .px(px(8.0))
                                                .py(px(6.0))
                                                .text_size(ui_text_sm(cx))
                                                .text_color(rgb(t.text_primary))
                                                .child(format_fingerprint(&fp)),
                                        ),
                                )
                            })
                            // Buttons row
                            .child(
                                div()
                                    .flex()
                                    .gap(px(8.0))
                                    .when(!expired && !code_for_copy.is_empty(), |d| {
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
                                                .text_size(ui_text_md(cx))
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
                                                .text_size(ui_text_md(cx))
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
