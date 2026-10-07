//! Settings → AI Proxy: route `claude` through the local daemon's account pool.

use crate::settings::settings_entity;
use crate::theme::theme;
use crate::ui::tokens::ui_text_ms;
use crate::views::components::simple_input::{SimpleInput, SimpleInputState};
use gpui::prelude::FluentBuilder;
use gpui::*;
use okena_core::ai_proxy::{AiAccount, AiProxyCommand, AiProxyState, AiUsage};
use okena_core::api::ActionRequest;
use okena_ui::button::{button, button_primary};
use okena_ui::empty_state::empty_state;
use std::time::Duration;

use super::SettingsPanel;
use super::components::*;

pub(super) enum AiProxyLoad {
    NotLoaded,
    Loading,
    Loaded(AiProxyState),
}

pub(super) struct AiProxyPanel {
    pub(super) load: AiProxyLoad,
    pub(super) busy: bool,
    label_input: Entity<SimpleInputState>,
    code_input: Entity<SimpleInputState>,
}

impl AiProxyPanel {
    pub(super) fn new(cx: &mut App) -> Self {
        Self {
            load: AiProxyLoad::NotLoaded,
            busy: false,
            label_input: cx.new(|cx| SimpleInputState::new(cx).placeholder("e.g. Work Max")),
            code_input: cx.new(|cx| {
                SimpleInputState::new(cx).placeholder("Paste the code shown after authorizing")
            }),
        }
    }
}

impl SettingsPanel {
    /// Send one command to the daemon's AI proxy and show the resulting state.
    fn ai_proxy_command(&mut self, request: AiProxyCommand, cx: &mut Context<Self>) {
        let Some(endpoint) = self.daemon_endpoint.clone() else {
            self.ai_proxy.load = AiProxyLoad::Loaded(AiProxyState {
                status: None,
                failure: Some("Local daemon is not connected".into()),
            });
            return;
        };
        self.ai_proxy.busy = true;
        if matches!(self.ai_proxy.load, AiProxyLoad::NotLoaded) {
            self.ai_proxy.load = AiProxyLoad::Loading;
        }
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    let value = crate::remote::local::post_action(
                        &endpoint,
                        &ActionRequest::AiProxy { request },
                        // Starting the proxy and finishing a login can take a while.
                        Duration::from_secs(75),
                    )?;
                    serde_json::from_value::<AiProxyState>(value)
                        .map_err(|error| format!("Invalid AI proxy response: {error}"))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.ai_proxy.busy = false;
                this.ai_proxy.load =
                    AiProxyLoad::Loaded(outcome.unwrap_or_else(|failure| AiProxyState {
                        status: None,
                        failure: Some(failure),
                    }));
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_ai_proxy(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let enabled = settings_entity(cx).read(cx).settings.ai_proxy_enabled;
        if enabled && matches!(self.ai_proxy.load, AiProxyLoad::NotLoaded) {
            self.ai_proxy_command(AiProxyCommand::Status, cx);
        }

        let toggle = settings_row_with_desc(
            "ai-proxy-enabled",
            "Route Claude Through the AI Proxy",
            "New terminals get ANTHROPIC_BASE_URL pointing at a local proxy that spreads \
             requests over the accounts below. Running terminals keep their environment.",
            &t,
            cx,
            false,
        )
        .child(
            toggle_switch("ai-proxy-enabled-toggle", enabled, &t).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    settings_entity(cx).update(cx, |state, cx| {
                        state.set_ai_proxy_enabled(!enabled, cx);
                    });
                    if !enabled {
                        this.ai_proxy_command(AiProxyCommand::Status, cx);
                    }
                }),
            ),
        );

        let content = div()
            .child(section_header("AI Proxy", &t, cx))
            .child(section_container(&t).child(toggle));
        if !enabled {
            return content;
        }

        let state = match &self.ai_proxy.load {
            AiProxyLoad::NotLoaded | AiProxyLoad::Loading => {
                return content.child(section_header("Accounts", &t, cx)).child(
                    section_container(&t)
                        .child(empty_state("Starting AI proxy...", &t, cx).py(px(16.0))),
                );
            }
            AiProxyLoad::Loaded(state) => state.clone(),
        };

        let busy = self.ai_proxy.busy;
        let mut accounts = section_container(&t);
        match &state.status {
            Some(status) if status.accounts.is_empty() => {
                accounts = accounts.child(
                    empty_state(
                        "No accounts yet. Until one is added, requests use each terminal's own Claude login.",
                        &t,
                        cx,
                    )
                    .py(px(16.0)),
                );
            }
            Some(status) => {
                let count = status.accounts.len();
                for (index, account) in status.accounts.iter().enumerate() {
                    accounts = accounts.child(self.render_ai_account(
                        account,
                        index + 1 < count,
                        busy,
                        cx,
                    ));
                }
            }
            None => {}
        }

        let mut content = content
            .child(section_header("Accounts", &t, cx))
            .child(accounts)
            .child(
                div().px(px(4.0)).pt(px(6.0)).flex().justify_end().child(
                    button(
                        "ai-proxy-refresh",
                        if busy { "Refreshing..." } else { "Refresh" },
                        &t,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.ai_proxy_command(AiProxyCommand::Status, cx);
                    })),
                ),
            );

        if let Some(failure) = &state.failure {
            content = content.child(
                div()
                    .px(px(4.0))
                    .pt(px(8.0))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.term_red))
                    .child(failure.clone()),
            );
        }

        let login = state
            .status
            .as_ref()
            .and_then(|status| status.logins.last().cloned());
        content.child(section_header("Add Account", &t, cx)).child(
            section_container(&t).child(match login {
                None => self.render_ai_login_start(busy, cx).into_any_element(),
                Some(login) => self
                    .render_ai_login_finish(login, busy, cx)
                    .into_any_element(),
            }),
        )
    }

    fn render_ai_account(
        &self,
        account: &AiAccount,
        has_border: bool,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as i64)
            .unwrap_or_default();
        let mut title = account.label.clone();
        if let Some(email) = &account.email {
            title = format!("{title} \u{2022} {email}");
        }
        if let Some(plan) = &account.plan {
            title = format!("{title} ({plan})");
        }
        let mut details = vec![
            usage_text("5h", account.five_hour.as_ref(), now),
            usage_text("7d", account.seven_day.as_ref(), now),
        ];
        if account.needs_login {
            details.insert(0, "Needs login".into());
        } else if !account.enabled {
            details.insert(0, "Disabled".into());
        } else if let Some(until) = account.cooldown_until_ms {
            details.insert(
                0,
                format!("Rate limited for {}", format_remaining(until - now)),
            );
        }
        details.push(format!("{} requests", account.requests));

        let id = account.id.clone();
        let enabled = account.enabled;
        let toggle_id = id.clone();
        let relogin_label = account.label.clone();
        settings_row_with_desc(
            format!("ai-account-{id}"),
            &title,
            &details.join(" \u{2022} "),
            &t,
            cx,
            has_border,
        )
        .child(
            div()
                .flex()
                .gap(px(6.0))
                .when(account.needs_login, |row| {
                    row.child(
                        button(format!("ai-account-relogin-{id}"), "Log In Again", &t).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.ai_proxy_command(
                                    AiProxyCommand::StartLogin {
                                        label: relogin_label.clone(),
                                    },
                                    cx,
                                );
                            }),
                        ),
                    )
                })
                .child(
                    toggle_switch(format!("ai-account-enabled-{id}"), enabled, &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            if !busy {
                                this.ai_proxy_command(
                                    AiProxyCommand::SetAccountEnabled {
                                        account_id: toggle_id.clone(),
                                        enabled: !enabled,
                                    },
                                    cx,
                                );
                            }
                        }),
                    ),
                )
                .child(
                    button(format!("ai-account-remove-{id}"), "Remove", &t).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.ai_proxy_command(
                                AiProxyCommand::RemoveAccount {
                                    account_id: id.clone(),
                                },
                                cx,
                            );
                        },
                    )),
                ),
        )
    }

    fn render_ai_login_start(&self, busy: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let label_input = self.ai_proxy.label_input.clone();
        settings_input_row(
            "ai-login-start",
            "Account Name",
            "A name to tell the account apart. Logging in opens claude.ai in your browser.",
            &t,
            cx,
            false,
        )
        .child(
            div()
                .flex()
                .gap(px(6.0))
                .child(
                    input_box(&t)
                        .flex_1()
                        .child(SimpleInput::new(&self.ai_proxy.label_input)),
                )
                .child(
                    button_primary("ai-login-start-button", "Log In", &t).on_click(cx.listener(
                        move |this, _, _, cx| {
                            let label = label_input.read(cx).value().trim().to_owned();
                            if label.is_empty() || busy {
                                return;
                            }
                            this.ai_proxy_command(AiProxyCommand::StartLogin { label }, cx);
                        },
                    )),
                ),
        )
    }

    fn render_ai_login_finish(
        &self,
        login: okena_core::ai_proxy::AiPendingLogin,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);
        let url = login.authorize_url.clone();
        let copy_url = login.authorize_url.clone();
        let finish_id = login.id.clone();
        let cancel_id = login.id.clone();
        let code_input = self.ai_proxy.code_input.clone();
        settings_input_row(
            "ai-login-finish",
            &format!("Log in \u{201c}{}\u{201d}", login.label),
            "Open the link, sign in with the account you want to add, then paste the code it shows.",
            &t,
            cx,
            false,
        )
        .child(
            div()
                .flex()
                .gap(px(6.0))
                .child(
                    button_primary("ai-login-open", "Open Login Page", &t)
                        .on_click(move |_, _, _| okena_core::process::open_url(&url)),
                )
                .child(
                    button("ai-login-copy", "Copy Link", &t).on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy_url.clone()));
                    }),
                ),
        )
        .child(
            div()
                .flex()
                .gap(px(6.0))
                .child(input_box(&t).flex_1().child(SimpleInput::new(&self.ai_proxy.code_input)))
                .child(
                    button_primary(
                        "ai-login-finish-button",
                        if busy { "Finishing..." } else { "Finish" },
                        &t,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let code = code_input.read(cx).value().trim().to_owned();
                        if code.is_empty() || busy {
                            return;
                        }
                        this.ai_proxy.code_input.update(cx, |input, cx| input.set_value("", cx));
                        this.ai_proxy.label_input.update(cx, |input, cx| input.set_value("", cx));
                        this.ai_proxy_command(
                            AiProxyCommand::FinishLogin {
                                login_id: finish_id.clone(),
                                code,
                            },
                            cx,
                        );
                    })),
                )
                .child(button("ai-login-cancel", "Cancel", &t).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.ai_proxy_command(
                            AiProxyCommand::CancelLogin {
                                login_id: cancel_id.clone(),
                            },
                            cx,
                        );
                    },
                ))),
        )
        .when_some(login.error, |row, error| {
            row.child(
                div()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.term_red))
                    .child(error),
            )
        })
    }
}

fn usage_text(window: &str, usage: Option<&AiUsage>, now: i64) -> String {
    match usage {
        None => format!("{window} unknown"),
        Some(usage) => match usage.resets_at_ms.filter(|reset| *reset > now) {
            Some(reset) => format!(
                "{window} {:.0}% (resets in {})",
                usage.utilization,
                format_remaining(reset - now)
            ),
            None => format!("{window} {:.0}%", usage.utilization),
        },
    }
}

fn format_remaining(ms: i64) -> String {
    let minutes = (ms / 60_000).max(1);
    if minutes < 60 {
        format!("{minutes}m")
    } else if minutes < 48 * 60 {
        format!("{}h {}m", minutes / 60, minutes % 60)
    } else {
        format!("{}d", minutes / (24 * 60))
    }
}
