//! Remote connection dialog overlay.
//!
//! Allows users to configure and connect to a remote Okena server
//! by entering host, port, and pairing code. The dialog performs
//! health check + pairing inline before emitting a Connected event,
//! so the caller only receives fully-validated connections.

use crate::keybindings::Cancel;
use crate::remote_client::config::RemoteConnectionConfig;
use crate::remote_client::manager::RemoteConnectionManager;
use crate::theme::theme;
use crate::views::components::{
    button, button_primary, input_container, labeled_input, modal_backdrop, modal_content,
    modal_header, SimpleInput, SimpleInputState,
};
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
}

#[derive(Clone)]
enum ConnectionDialogStatus {
    Idle,
    Testing,
    TestSuccess(String),
    TestFailed(String),
    Connecting,
    ConnectFailed(String),
}

impl ConnectionDialogStatus {
    fn is_busy(&self) -> bool {
        matches!(self, Self::Testing | Self::Connecting)
    }
}

pub enum RemoteConnectDialogEvent {
    Close,
    Connected {
        config: RemoteConnectionConfig,
    },
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
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        if !self.status.is_busy() {
            cx.emit(RemoteConnectDialogEvent::Close);
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
            let result = runtime
                .spawn(async move {
                    let client = reqwest::Client::new();
                    let url = format!("http://{}:{}/health", host, port_num);
                    client
                        .get(&url)
                        .timeout(std::time::Duration::from_secs(5))
                        .send()
                        .await
                })
                .await;

            let status = match result {
                Ok(Ok(resp)) if resp.status().is_success() => {
                    let body = resp.text().await.unwrap_or_default();
                    let version = serde_json::from_str::<serde_json::Value>(&body)
                        .ok()
                        .and_then(|v| v.get("version").and_then(|v| v.as_str()).map(String::from))
                        .unwrap_or_else(|| "unknown".to_string());
                    ConnectionDialogStatus::TestSuccess(version)
                }
                Ok(Ok(resp)) => {
                    ConnectionDialogStatus::TestFailed(format!("HTTP {}", resp.status()))
                }
                Ok(Err(e)) => ConnectionDialogStatus::TestFailed(format!("{}", e)),
                Err(e) => ConnectionDialogStatus::TestFailed(format!("{}", e)),
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
        };

        let runtime = self.runtime(cx);
        let base_url = format!("http://{}:{}", host, port);

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            // Step 1: Health check
            let health_result = runtime
                .spawn({
                    let base_url = base_url.clone();
                    async move {
                        let client = reqwest::Client::new();
                        client
                            .get(format!("{}/health", base_url))
                            .timeout(std::time::Duration::from_secs(5))
                            .send()
                            .await
                    }
                })
                .await;

            match health_result {
                Ok(Ok(resp)) if resp.status().is_success() => {}
                Ok(Ok(resp)) => {
                    let msg = format!("Server returned HTTP {}", resp.status());
                    let _ = this.update(cx, |this, cx| {
                        this.status = ConnectionDialogStatus::ConnectFailed(msg);
                        cx.notify();
                    });
                    return;
                }
                Ok(Err(e)) => {
                    let msg = format!("Cannot reach server: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.status = ConnectionDialogStatus::ConnectFailed(msg);
                        cx.notify();
                    });
                    return;
                }
                Err(e) => {
                    let msg = format!("Internal error: {}", e);
                    let _ = this.update(cx, |this, cx| {
                        this.status = ConnectionDialogStatus::ConnectFailed(msg);
                        cx.notify();
                    });
                    return;
                }
            }

            // Step 2: Pair with code
            let pair_result = runtime
                .spawn({
                    let base_url = base_url.clone();
                    let code = code.clone();
                    async move {
                        let client = reqwest::Client::new();
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
                            let _ = this.update(cx, |_this, cx| {
                                cx.emit(RemoteConnectDialogEvent::Connected { config });
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
                        format!("Invalid pairing code")
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
            ConnectionDialogStatus::Idle => div().into_any_element(),
            ConnectionDialogStatus::Testing => div()
                .text_size(px(11.0))
                .text_color(rgb(t.text_secondary))
                .child("Testing connection...")
                .into_any_element(),
            ConnectionDialogStatus::TestSuccess(version) => div()
                .text_size(px(11.0))
                .text_color(rgb(t.term_green))
                .child(format!("Server reachable (v{})", version))
                .into_any_element(),
            ConnectionDialogStatus::TestFailed(err) => div()
                .text_size(px(11.0))
                .text_color(rgb(t.term_red))
                .child(format!("Failed: {}", err))
                .into_any_element(),
            ConnectionDialogStatus::Connecting => div()
                .text_size(px(11.0))
                .text_color(rgb(t.text_secondary))
                .child("Connecting...")
                .into_any_element(),
            ConnectionDialogStatus::ConnectFailed(err) => div()
                .text_size(px(11.0))
                .text_color(rgb(t.term_red))
                .child(format!("Failed: {}", err))
                .into_any_element(),
        };

        let connect_label = if matches!(self.status, ConnectionDialogStatus::Connecting) {
            "Connecting..."
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
                        cx.listener(|this, _, _, cx| this.close(cx)),
                    ))
                    .child(
                        div()
                            .p(px(16.0))
                            .flex()
                            .flex_col()
                            .gap(px(12.0))
                            // Name input
                            .child(
                                labeled_input("Name:", &t).child(
                                    input_container(&t, None).child(
                                        SimpleInput::new(&self.name_input).text_size(px(12.0)),
                                    ),
                                ),
                            )
                            // Host input
                            .child(
                                labeled_input("Host:", &t).child(
                                    input_container(&t, None).child(
                                        SimpleInput::new(&self.host_input).text_size(px(12.0)),
                                    ),
                                ),
                            )
                            // Port input
                            .child(
                                labeled_input("Port:", &t).child(
                                    input_container(&t, None).child(
                                        SimpleInput::new(&self.port_input).text_size(px(12.0)),
                                    ),
                                ),
                            )
                            // Test connection button + status
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
                            // Pairing code input
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
                            // Action buttons
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
                                                    this.connect(cx);
                                                }))
                                            })
                                            .when(is_busy, |el| el.opacity(0.5)),
                                    ),
                            ),
                    ),
            )
    }
}

impl_focusable!(RemoteConnectDialog);
