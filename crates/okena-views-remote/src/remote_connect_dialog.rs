//! Remote connection dialog overlay.

use crate::Cancel;
use okena_transport::client::tls::format_fingerprint;
use okena_transport::client::RemoteConnectionConfig;
use okena_remote_client::RemoteConnectionManager;
use okena_ui::button::{button, button_primary};
use okena_ui::input::{input_container, labeled_input};
use okena_ui::modal::{modal_backdrop, modal_content, modal_header};
use okena_ui::simple_input::{SimpleInput, SimpleInputState};
use okena_ui::theme::theme;
use okena_ui::tokens::{ui_text_ms, ui_text_md, ui_text_sm};
use gpui::prelude::*;
use gpui::*;
use std::sync::Arc;

pub struct RemoteConnectDialog {
    remote_manager: Entity<RemoteConnectionManager>,
    focus_handle: FocusHandle,
    name_input: Entity<SimpleInputState>,
    host_input: Entity<SimpleInputState>,
    port_input: Entity<SimpleInputState>,
    code_input: Entity<SimpleInputState>,
    status: ConnectionDialogStatus,
    initial_focus_done: bool,
    /// Config (with token + captured pin) held back during fingerprint
    /// verification — emitted as Connected only once the user confirms.
    pending_config: Option<RemoteConnectionConfig>,
}

#[derive(Clone)]
enum ConnectionDialogStatus {
    Idle,
    Testing,
    TestSuccess(String),
    TestFailed(String),
    Connecting,
    ConnectFailed(String),
    /// Paired over TLS; awaiting the user to verify the captured cert
    /// fingerprint matches the host before the pin is trusted/saved.
    VerifyFingerprint(String),
}

impl ConnectionDialogStatus {
    fn is_busy(&self) -> bool {
        matches!(self, Self::Testing | Self::Connecting)
    }
}

/// Result of auto-detecting how to reach a server.
struct Detected {
    tls: bool,
    base_url: String,
    version: Option<String>,
}

/// Probe `/health` over TLS first, then plain http (auto mode). Prefers the
/// secure scheme; falls back so existing plain-http servers still connect.
/// `observed` captures the TLS cert fingerprint when the TLS probe succeeds.
async fn detect_scheme(
    runtime: &Arc<tokio::runtime::Runtime>,
    host: String,
    port: u16,
    observed: okena_transport::client::tls::ObservedFingerprint,
) -> Result<Detected, String> {
    for tls in [true, false] {
        let host = host.clone();
        let observed = observed.clone();
        let probe = runtime
            .spawn(async move {
                let client =
                    okena_transport::client::tls::build_reqwest_client(tls, None, observed);
                let scheme = if tls { "https" } else { "http" };
                let base_url = format!("{}://{}:{}", scheme, host, port);
                let resp = client
                    .get(format!("{}/health", base_url))
                    .timeout(std::time::Duration::from_secs(5))
                    .send()
                    .await;
                (base_url, resp)
            })
            .await;

        if let Ok((base_url, Ok(resp))) = probe
            && resp.status().is_success()
        {
            let version = resp
                .text()
                .await
                .ok()
                .and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok())
                .and_then(|v| v.get("version").and_then(|v| v.as_str()).map(String::from));
            return Ok(Detected {
                tls,
                base_url,
                version,
            });
        }
    }
    Err("Cannot reach server over TLS or plain http".to_string())
}

pub enum RemoteConnectDialogEvent {
    Close,
    Connected {
        config: RemoteConnectionConfig,
    },
}

impl okena_ui::overlay::CloseEvent for RemoteConnectDialogEvent {
    fn is_close(&self) -> bool { matches!(self, Self::Close) }
}

impl EventEmitter<RemoteConnectDialogEvent> for RemoteConnectDialog {}

impl RemoteConnectDialog {
    pub fn new(remote_manager: Entity<RemoteConnectionManager>, cx: &mut Context<Self>) -> Self {
        let name_input = cx.new(|cx| SimpleInputState::new(cx).placeholder("Connection name..."));
        let host_input = cx.new(|cx| SimpleInputState::new(cx).placeholder("hostname or IP..."));
        let port_input = cx.new(|cx| {
            let mut s = SimpleInputState::new(cx);
            s.set_value("19100", cx);
            s.placeholder("19100")
        });
        let code_input =
            cx.new(|cx| SimpleInputState::new(cx).placeholder("Pairing code from remote..."));

        Self {
            remote_manager,
            focus_handle: cx.focus_handle(),
            name_input,
            host_input,
            port_input,
            code_input,
            status: ConnectionDialogStatus::Idle,
            initial_focus_done: false,
            pending_config: None,
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        if !self.status.is_busy() {
            cx.emit(RemoteConnectDialogEvent::Close);
        }
    }

    /// User confirmed the verified fingerprint — emit the held-back config so the
    /// connection (with its now-trusted pin) is saved and connected.
    fn confirm_fingerprint(&mut self, cx: &mut Context<Self>) {
        if let Some(config) = self.pending_config.take() {
            cx.emit(RemoteConnectDialogEvent::Connected { config });
        }
    }

    fn runtime(&self, cx: &Context<Self>) -> Arc<tokio::runtime::Runtime> {
        self.remote_manager.read(cx).runtime()
    }

    fn test_connection(&mut self, cx: &mut Context<Self>) {
        let host = self.host_input.read(cx).value().to_string();
        let port = self.port_input.read(cx).value().to_string();

        if host.is_empty() {
            self.status = ConnectionDialogStatus::TestFailed("Host is required".to_string());
            cx.notify();
            return;
        }

        self.status = ConnectionDialogStatus::Testing;
        cx.notify();

        let port_num: u16 = port.parse().unwrap_or(19100);
        let runtime = self.runtime(cx);

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let observed = okena_transport::client::tls::new_observed();
            let status = match detect_scheme(&runtime, host, port_num, observed).await {
                Ok(d) => {
                    let version = d.version.unwrap_or_else(|| "unknown".to_string());
                    let label = if d.tls {
                        format!("{version}, TLS")
                    } else {
                        format!("{version}, plaintext")
                    };
                    ConnectionDialogStatus::TestSuccess(label)
                }
                Err(e) => ConnectionDialogStatus::TestFailed(e),
            };

            let _ = this.update(cx, |this, cx| {
                this.status = status;
                cx.notify();
            });
        })
        .detach();
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        let name = self.name_input.read(cx).value().to_string();
        let host = self.host_input.read(cx).value().to_string();
        let port_str = self.port_input.read(cx).value().to_string();
        let code = self.code_input.read(cx).value().to_string();

        if host.is_empty() || code.is_empty() {
            self.status = ConnectionDialogStatus::ConnectFailed(
                "Host and pairing code are required".to_string(),
            );
            cx.notify();
            return;
        }

        let port: u16 = port_str.parse().unwrap_or(19100);
        let name = if name.is_empty() {
            format!("{}:{}", host, port)
        } else {
            name
        };

        self.status = ConnectionDialogStatus::Connecting;
        cx.notify();

        let config = RemoteConnectionConfig {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            host: host.clone(),
            port,
            saved_token: None,
            token_obtained_at: None,
            tls: false, // set by auto-detection below
            pinned_cert_sha256: None,
        };

        let runtime = self.runtime(cx);
        // TOFU: no pin yet on a brand-new connection. The verifier records the
        // observed cert fingerprint into this slot during the TLS handshake so we
        // can pin it onto the config before emitting Connected.
        let observed = okena_transport::client::tls::new_observed();

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let mut config = config;

            // Auto-detect the scheme (TLS first, plain-http fallback) and adopt it.
            let detected =
                match detect_scheme(&runtime, host.clone(), port, observed.clone()).await {
                    Ok(d) => d,
                    Err(e) => {
                        let _ = this.update(cx, |this, cx| {
                            this.status = ConnectionDialogStatus::ConnectFailed(e);
                            cx.notify();
                        });
                        return;
                    }
                };
            let use_tls = detected.tls;
            let base_url = detected.base_url;
            config.tls = use_tls;

            let pair_result = runtime
                .spawn({
                    let base_url = base_url.clone();
                    let code = code.clone();
                    let observed = observed.clone();
                    async move {
                        let client = okena_transport::client::tls::build_reqwest_client(
                            use_tls, None, observed,
                        );
                        let pair_body = serde_json::json!({ "code": code });
                        client
                            .post(format!("{}/v1/pair", base_url))
                            .json(&pair_body)
                            .timeout(std::time::Duration::from_secs(10))
                            .send()
                            .await
                    }
                })
                .await;

            #[derive(serde::Deserialize)]
            struct PairResp {
                token: String,
                #[allow(dead_code)]
                expires_in: u64,
            }

            match pair_result {
                Ok(Ok(resp)) if resp.status().is_success() => {
                    match resp.json::<PairResp>().await {
                        Ok(pair_resp) => {
                            let mut config = config;
                            config.saved_token = Some(pair_resp.token);
                            // Pin the cert fingerprint captured during the TLS
                            // handshake (TOFU).
                            let captured = if config.tls {
                                let fp = observed.lock().ok().and_then(|g| g.clone());
                                config.pinned_cert_sha256 = fp.clone();
                                fp
                            } else {
                                None
                            };
                            let _ = this.update(cx, |this, cx| match captured {
                                // TLS: hold the config back and ask the user to
                                // verify the fingerprint against the host before we
                                // trust the pin and persist the connection.
                                Some(fp) => {
                                    this.pending_config = Some(config);
                                    this.status =
                                        ConnectionDialogStatus::VerifyFingerprint(fp);
                                    cx.notify();
                                }
                                // Plain http (or no cert captured): nothing to
                                // verify, connect immediately as before.
                                None => {
                                    cx.emit(RemoteConnectDialogEvent::Connected { config });
                                }
                            });
                        }
                        Err(e) => {
                            let msg = format!("Invalid pair response: {}", e);
                            let _ = this.update(cx, |this, cx| {
                                this.status = ConnectionDialogStatus::ConnectFailed(msg);
                                cx.notify();
                            });
                        }
                    }
                }
                Ok(Ok(resp)) => {
                    let status_code = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    let msg = if status_code.as_u16() == 401 || status_code.as_u16() == 400 {
                        "Invalid pairing code".to_string()
                    } else {
                        format!("Pairing failed: HTTP {} - {}", status_code, body)
                    };
                    let _ = this.update(cx, |this, cx| {
                        this.status = ConnectionDialogStatus::ConnectFailed(msg);
                        cx.notify();
                    });
                }
                Ok(Err(e)) => {
                    let msg = format!("Pairing request failed: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.status = ConnectionDialogStatus::ConnectFailed(msg);
                        cx.notify();
                    });
                }
                Err(e) => {
                    let msg = format!("Internal error: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.status = ConnectionDialogStatus::ConnectFailed(msg);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }
}

impl Render for RemoteConnectDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let is_busy = self.status.is_busy();

        if !self.initial_focus_done {
            self.initial_focus_done = true;
            self.name_input.update(cx, |input, cx| {
                input.focus(window, cx);
            });
        }

        let status_element = match &self.status {
            // The fingerprint verification UI is a full-width block rendered
            // below, not an inline status next to the Test Connection button.
            ConnectionDialogStatus::Idle | ConnectionDialogStatus::VerifyFingerprint(_) => {
                div().into_any_element()
            }
            ConnectionDialogStatus::Testing => div()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_secondary))
                .child("Testing connection...")
                .into_any_element(),
            ConnectionDialogStatus::TestSuccess(version) => div()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.term_green))
                .child(format!("Server reachable (v{})", version))
                .into_any_element(),
            ConnectionDialogStatus::TestFailed(err) => div()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.term_red))
                .child(format!("Failed: {}", err))
                .into_any_element(),
            ConnectionDialogStatus::Connecting => div()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.text_secondary))
                .child("Connecting...")
                .into_any_element(),
            ConnectionDialogStatus::ConnectFailed(err) => div()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.term_red))
                .child(format!("Failed: {}", err))
                .into_any_element(),
        };

        // Full-width fingerprint verification block, shown after a TLS pair.
        // Kept out of the Test Connection row so the long hex can wrap freely
        // instead of overflowing the modal.
        let verify_element = match &self.status {
            ConnectionDialogStatus::VerifyFingerprint(fp) => Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_secondary))
                            .child("Paired. Verify this certificate fingerprint matches the one shown on the host (Settings → Remote Server), then confirm:"),
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
                            .child(format_fingerprint(fp)),
                    ),
            ),
            _ => None,
        };

        let verifying = matches!(self.status, ConnectionDialogStatus::VerifyFingerprint(_));

        let connect_label = if matches!(self.status, ConnectionDialogStatus::Connecting) {
            "Connecting..."
        } else if verifying {
            "Confirm & Connect"
        } else {
            "Connect"
        };

        modal_backdrop("remote-connect-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("RemoteConnectDialog")
            .items_center()
            .on_action(cx.listener(|this, _: &Cancel, _, cx| {
                this.close(cx);
            }))
            .when(!is_busy, |el| {
                el.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.close(cx);
                    }),
                )
            })
            .child(
                modal_content("remote-connect-modal", &t)
                    .w(px(450.0))
                    .child(modal_header(
                        "Connect to Remote Okena",
                        None::<&str>,
                        &t,
                        cx,
                        cx.listener(|this, _, _, cx| this.close(cx)),
                    ))
                    .child(
                        div()
                            .p(px(16.0))
                            .flex()
                            .flex_col()
                            .gap(px(12.0))
                            .child(
                                labeled_input("Name:", &t).child(
                                    input_container(&t, None).child(
                                        SimpleInput::new(&self.name_input).text_size(ui_text_md(cx)),
                                    ),
                                ),
                            )
                            .child(
                                labeled_input("Host:", &t).child(
                                    input_container(&t, None).child(
                                        SimpleInput::new(&self.host_input).text_size(ui_text_md(cx)),
                                    ),
                                ),
                            )
                            .child(
                                labeled_input("Port:", &t).child(
                                    input_container(&t, None).child(
                                        SimpleInput::new(&self.port_input).text_size(ui_text_md(cx)),
                                    ),
                                ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(
                                        button("test-connection-btn", "Test Connection", &t)
                                            .when(!is_busy, |el| {
                                                el.on_click(cx.listener(|this, _, _window, cx| {
                                                    this.test_connection(cx);
                                                }))
                                            })
                                            .when(is_busy, |el| el.opacity(0.5)),
                                    )
                                    .child(status_element),
                            )
                            .child(
                                labeled_input("Pairing Code:", &t).child(
                                    input_container(&t, None).child(
                                        SimpleInput::new(&self.code_input).text_size(ui_text_md(cx)),
                                    ),
                                ),
                            )
                            .child(
                                div()
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(
                                        "Enter the pairing code shown on the remote machine's status bar",
                                    ),
                            )
                            .children(verify_element)
                            .child(
                                div()
                                    .flex()
                                    .gap(px(8.0))
                                    .justify_end()
                                    .child(
                                        button("cancel-connect-btn", "Cancel", &t)
                                            .when(!is_busy, |el| {
                                                el.on_click(
                                                    cx.listener(|this, _, _window, cx| {
                                                        this.close(cx);
                                                    }),
                                                )
                                            })
                                            .when(is_busy, |el| el.opacity(0.5)),
                                    )
                                    .child(
                                        button_primary("confirm-connect-btn", connect_label, &t)
                                            .when(!is_busy, |el| {
                                                el.on_click(cx.listener(|this, _, _window, cx| {
                                                    // After a TLS pair the same primary
                                                    // button confirms the verified pin;
                                                    // otherwise it kicks off connect.
                                                    if matches!(
                                                        this.status,
                                                        ConnectionDialogStatus::VerifyFingerprint(_)
                                                    ) {
                                                        this.confirm_fingerprint(cx);
                                                    } else {
                                                        this.connect(cx);
                                                    }
                                                }))
                                            })
                                            .when(is_busy, |el| el.opacity(0.5)),
                                    ),
                            ),
                    ),
            )
    }
}

okena_ui::impl_focusable!(RemoteConnectDialog);
