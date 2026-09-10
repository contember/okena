//! Tasks settings — connecting okena to a task manager.
//!
//! The credential lives on the daemon, which is the only thing that talks to a
//! provider, so everything here goes through an action rather than touching a
//! key directly. The panel never sees a stored key: it can set one and clear
//! one, and reads back only whether a provider is connected and as whom.

use super::SettingsPanel;
use super::components::{section_container, section_header, settings_input_row};
use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_md, ui_text_ms};
use crate::views::components::SimpleInput;
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::ActionRequest;
use okena_core::tasks::{TaskAuthState, TaskAuthStatusResponse, TaskProviderStatus};

impl SettingsPanel {
    /// Read every provider's auth state. Cheap and local — the daemon answers
    /// from the stored credential without calling out.
    pub(super) fn refresh_task_providers(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.action_client.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client
                    .post_action(ActionRequest::TasksAuthStatus)
                    .and_then(|v| v.ok_or_else(|| "Missing auth status".to_string()))
                    .and_then(|v| {
                        serde_json::from_value::<TaskAuthStatusResponse>(v)
                            .map_err(|e| format!("Unexpected auth status: {e}"))
                    })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(status) => this.tasks_status = Some(status),
                        Err(e) => this.tasks_error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Store a key for `provider`, after the daemon verifies it.
    fn connect_provider(&mut self, provider: String, cx: &mut Context<Self>) {
        if self.tasks_busy {
            return;
        }
        let key = self.tasks_api_key_input.read(cx).value().trim().to_string();
        if key.is_empty() {
            self.tasks_error = Some("Paste an API key first.".into());
            cx.notify();
            return;
        }
        let Some(client) = self.action_client.clone() else {
            self.tasks_error = Some("The local daemon is unavailable.".into());
            cx.notify();
            return;
        };
        self.tasks_busy = true;
        self.tasks_error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client.post_action(ActionRequest::TasksConnectApiKey {
                    provider,
                    api_key: key,
                })
            })
            .await;

            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.tasks_busy = false;
                    match result {
                        Ok(_) => {
                            // Clear it on success: the panel has no reason to
                            // keep a credential on screen once it is stored.
                            this.tasks_api_key_input
                                .update(cx, |i, cx| i.set_value("", cx));
                            this.refresh_task_providers(cx);
                        }
                        Err(e) => this.tasks_error = Some(e),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn disconnect_provider(&mut self, provider: String, cx: &mut Context<Self>) {
        let Some(client) = self.action_client.clone() else {
            return;
        };
        self.tasks_busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                client.post_action(ActionRequest::TasksDisconnect { provider })
            })
            .await;
            cx.update(|cx| {
                let _ = this.update(cx, |this, cx| {
                    this.tasks_busy = false;
                    if let Err(e) = result {
                        this.tasks_error = Some(e);
                    }
                    this.refresh_task_providers(cx);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn render_provider(&self, p: &TaskProviderStatus, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let (color, status) = match &p.auth {
            TaskAuthState::Connected { account } => (
                t.success,
                match account {
                    Some(a) => format!("Connected as {a}"),
                    None => "Connected".to_string(),
                },
            ),
            // Distinct from disconnected: the key is there but the provider
            // rejected it, so the fix is a new key rather than a first one.
            TaskAuthState::Expired => (t.error, "Key rejected — reconnect".to_string()),
            TaskAuthState::Disconnected => (t.text_muted, "Not connected".to_string()),
            TaskAuthState::Unknown => (t.text_muted, "Unknown".to_string()),
        };
        let connected = matches!(p.auth, TaskAuthState::Connected { .. });
        let id = p.provider.clone();
        let busy = self.tasks_busy;

        v_flex()
            .gap(px(8.0))
            .px(px(12.0))
            .py(px(10.0))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        v_flex()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(ui_text(13.0, cx))
                                    .text_color(rgb(t.text_primary))
                                    .child(p.display_name.clone()),
                            )
                            .child(
                                div()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .bg(with_alpha(color, 0.15))
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(color))
                                    .child(status),
                            ),
                    )
                    .when(connected, |d| {
                        let id = id.clone();
                        d.child(
                            div()
                                .id(SharedString::from(format!("tasks-disconnect-{id}")))
                                .cursor_pointer()
                                .px(px(10.0))
                                .py(px(4.0))
                                .rounded(px(4.0))
                                .hover(|s| s.bg(with_alpha(t.error, 0.1)))
                                .text_size(ui_text_ms(cx))
                                .text_color(rgb(t.error))
                                .child("Disconnect")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _window, cx| {
                                        this.disconnect_provider(id.clone(), cx);
                                    }),
                                ),
                        )
                    }),
            )
            // The key field is only offered where it would do something: a
            // connected provider needs a disconnect, not a second key.
            .when(!connected, |d| {
                let id = id.clone();
                d.child(
                    h_flex()
                        .gap(px(8.0))
                        .items_center()
                        .child(
                            okena_ui::input::input_container(&t, None)
                                .flex_1()
                                .min_w_0()
                                .px(px(8.0))
                                .py(px(5.0))
                                .child(
                                    SimpleInput::new(&self.tasks_api_key_input)
                                        .text_size(ui_text(13.0, cx)),
                                ),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("tasks-connect-{id}")))
                                .cursor_pointer()
                                .flex_shrink_0()
                                .px(px(12.0))
                                .py(px(5.0))
                                .rounded(px(4.0))
                                .bg(rgb(t.button_primary_bg))
                                .hover(|s| s.bg(rgb(t.button_primary_hover)))
                                .text_size(ui_text_md(cx))
                                .text_color(rgb(t.button_primary_fg))
                                .child(if busy { "Verifying…" } else { "Connect" })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _window, cx| {
                                        this.connect_provider(id.clone(), cx);
                                    }),
                                ),
                        ),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_tasks(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);
        // Read on first paint of the page rather than when the panel opens, so
        // a user who never visits Tasks makes no call at all.
        if self.tasks_status.is_none() && self.action_client.is_some() && !self.tasks_busy {
            self.refresh_task_providers(cx);
        }

        let providers = self
            .tasks_status
            .as_ref()
            .map(|s| s.providers.clone())
            .unwrap_or_default();

        let mut body = v_flex()
            .child(section_header("Task manager", &t, cx))
            .child(settings_input_row(
                "tasks-intro",
                "Provider",
                "okena reads your assigned work from a task manager. The key is \
                 verified once and then stored by the daemon — the app never \
                 holds it, and never sends it anywhere else.",
                &t,
                cx,
                false,
            ));

        let mut container = section_container(&t);
        if providers.is_empty() {
            container = container.child(
                div()
                    .px(px(12.0))
                    .py(px(10.0))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(if self.action_client.is_some() {
                        "Loading providers…"
                    } else {
                        "The local daemon is unavailable, so providers cannot be configured."
                    }),
            );
        }
        for p in &providers {
            container = container.child(self.render_provider(p, cx));
        }
        body = body.child(container);

        if let Some(err) = self.tasks_error.clone() {
            body = body.child(
                div()
                    .mx(px(12.0))
                    .mt(px(8.0))
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(4.0))
                    .bg(with_alpha(t.error, 0.1))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.error))
                    .child(err),
            );
        }
        body
    }
}
