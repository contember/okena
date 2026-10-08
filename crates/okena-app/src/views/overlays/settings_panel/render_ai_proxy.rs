//! Settings → AI Proxy: route `claude` through the local daemon's account pool,
//! and the routing policies that choose which accounts a project may use.

use crate::settings::settings_entity;
use crate::theme::theme;
use crate::ui::tokens::ui_text_ms;
use crate::views::components::simple_input::{SimpleInput, SimpleInputState};
use gpui::prelude::FluentBuilder;
use gpui::*;
use okena_core::ai_proxy::{
    AiAccount, AiAccountRule, AiProxyCommand, AiProxyState, AiProxyStatus, AiRoutingPolicy, AiUsage,
};
use okena_core::api::ActionRequest;
use okena_transport::client::LOCAL_DAEMON_CONNECTION_ID;
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

    pub(super) fn render_ai_proxy(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.selected_project_id.clone() {
            Some(project_id) => self.render_ai_project(&project_id, cx).into_any_element(),
            None => self.render_ai_pool(cx).into_any_element(),
        }
    }

    fn render_ai_pool(&mut self, cx: &mut Context<Self>) -> Div {
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

        if let Some(status) = state
            .status
            .as_ref()
            .filter(|status| !status.accounts.is_empty())
        {
            content = content
                .child(section_header("Default Policy", &t, cx))
                .child(section_note(
                    "Accounts that terminals of projects without their own policy may use.",
                    &t,
                    cx,
                ))
                .child(self.render_policy_editor(
                    PolicyTarget::Default,
                    &status.routing.default,
                    status,
                    busy,
                    cx,
                ))
                .child(section_header("Project Policies", &t, cx))
                .child(self.render_project_policies(status, busy, cx));
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

    /// Settings → (project) → AI Proxy: the project's routing policy.
    fn render_ai_project(&mut self, project_id: &str, cx: &mut Context<Self>) -> Div {
        let t = theme(cx);
        let mut content = div().child(section_header("AI Proxy Policy", &t, cx));
        let (project_name, root_id) = {
            let workspace = self.workspace.read(cx);
            let Some(project) = workspace.project(project_id) else {
                return content;
            };
            let root_id = project.root_project_id().to_owned();
            let root_name = workspace
                .project(&root_id)
                .map_or_else(|| project.name.clone(), |root| root.name.clone());
            (root_name, root_id)
        };
        let Some(daemon_id) = local_daemon_project_id(&root_id).map(str::to_owned) else {
            return content.child(section_note(
                "This project runs on another host, which has its own AI proxy and policies.",
                &t,
                cx,
            ));
        };
        if !settings_entity(cx).read(cx).settings.ai_proxy_enabled {
            return content.child(section_note(
                "The AI proxy is off. Turn it on in the User settings under AI Proxy.",
                &t,
                cx,
            ));
        }
        if matches!(self.ai_proxy.load, AiProxyLoad::NotLoaded) {
            self.ai_proxy_command(AiProxyCommand::Status, cx);
        }
        let state = match &self.ai_proxy.load {
            AiProxyLoad::Loaded(state) => state.clone(),
            AiProxyLoad::NotLoaded | AiProxyLoad::Loading => {
                return content.child(
                    section_container(&t)
                        .child(empty_state("Starting AI proxy...", &t, cx).py(px(16.0))),
                );
            }
        };
        if root_id != project_id {
            content = content.child(section_note(
                format!("Worktree terminals use the policy of \u{201c}{project_name}\u{201d}."),
                &t,
                cx,
            ));
        }
        if let Some(failure) = &state.failure {
            content = content.child(
                div()
                    .px(px(4.0))
                    .pb(px(8.0))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.term_red))
                    .child(failure.clone()),
            );
        }
        let Some(status) = state.status else {
            return content;
        };
        if status.accounts.is_empty() {
            return content.child(section_note(
                "Add accounts in the User settings under AI Proxy first.",
                &t,
                cx,
            ));
        }

        let busy = self.ai_proxy.busy;
        let own_policy = status.routing.projects.get(&daemon_id).cloned();
        let segments = [
            Segment {
                id: "default".into(),
                label: "Default",
                selected: own_policy.is_none(),
            },
            Segment {
                id: "custom".into(),
                label: "Custom",
                selected: own_policy.is_some(),
            },
        ];
        let this = cx.entity().downgrade();
        let has_own_policy = own_policy.is_some();
        let default_policy = status.routing.default.clone();
        let mode_project_id = daemon_id.clone();
        let mode = settings_row_with_desc(
            "ai-project-policy-mode",
            "Accounts",
            "Which pool accounts Claude in this project's terminals may use.",
            &t,
            cx,
            false,
        )
        .child(segmented_control(
            "ai-project-policy-mode",
            &segments,
            &t,
            cx,
            move |index, _, cx| {
                let custom = index == 1;
                if busy || custom == has_own_policy {
                    return;
                }
                // A new custom policy starts as a copy of the default one.
                let policy = custom.then(|| default_policy.clone());
                let command = AiProxyCommand::SetProjectPolicy {
                    project_id: mode_project_id.clone(),
                    policy,
                };
                let _ = this.update(cx, |this, cx| this.ai_proxy_command(command, cx));
            },
        ));
        content = content.child(section_container(&t).child(mode));
        match own_policy {
            None => content.child(section_note(
                format!(
                    "Uses the default policy: {}.",
                    policy_summary(&status.routing.default, &status.accounts)
                ),
                &t,
                cx,
            )),
            Some(policy) => content.child(self.render_policy_editor(
                PolicyTarget::Project(daemon_id),
                &policy,
                &status,
                busy,
                cx,
            )),
        }
    }

    /// One row per account, plus the rule for accounts added later. Every edit
    /// sends the whole policy.
    fn render_policy_editor(
        &self,
        target: PolicyTarget,
        policy: &AiRoutingPolicy,
        status: &AiProxyStatus,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = theme(cx);
        let this = cx.entity().downgrade();
        let apply = move |policy: AiRoutingPolicy, cx: &mut App| {
            if busy {
                return;
            }
            let command = target.command(policy);
            let _ = this.update(cx, |this, cx| this.ai_proxy_command(command, cx));
        };
        // Listing every current account keeps their rules when `others` changes:
        // the "New Accounts" row then affects only accounts added later.
        let mut explicit = policy.clone();
        for account in &status.accounts {
            explicit
                .accounts
                .insert(account.id.clone(), policy.rule(&account.id));
        }

        let mut editor = section_container(&t);
        for account in &status.accounts {
            let rule = policy.rule(&account.id);
            let rules = [
                (AiAccountRule::Prefer, "Prefer"),
                (AiAccountRule::Allow, "Allow"),
                (AiAccountRule::Forbid, "Never"),
            ];
            let segments: Vec<Segment<'_>> = rules
                .iter()
                .map(|(option, label)| Segment {
                    id: format!("{option:?}").into(),
                    label,
                    selected: *option == rule,
                })
                .collect();
            let apply = apply.clone();
            let explicit = explicit.clone();
            let account_id = account.id.clone();
            editor = editor.child(
                settings_row(
                    format!("ai-policy-account-{}", account.id),
                    &account.label,
                    &t,
                    cx,
                    true,
                )
                .child(segmented_control(
                    &format!("ai-policy-account-{}", account.id),
                    &segments,
                    &t,
                    cx,
                    move |index, _, cx| {
                        let mut policy = explicit.clone();
                        policy.accounts.insert(account_id.clone(), rules[index].0);
                        apply(policy, cx);
                    },
                )),
            );
        }

        let others = [
            (AiAccountRule::Allow, "Allow"),
            (AiAccountRule::Forbid, "Never"),
        ];
        let segments: Vec<Segment<'_>> = others
            .iter()
            .map(|(option, label)| Segment {
                id: format!("{option:?}").into(),
                label,
                selected: *option == policy.others,
            })
            .collect();
        editor.child(
            settings_row_with_desc(
                "ai-policy-others",
                "New Accounts",
                "Accounts added later",
                &t,
                cx,
                false,
            )
            .child(segmented_control(
                "ai-policy-others",
                &segments,
                &t,
                cx,
                move |index, _, cx| {
                    let mut policy = explicit.clone();
                    policy.others = others[index].0;
                    apply(policy, cx);
                },
            )),
        )
    }

    fn render_project_policies(
        &self,
        status: &AiProxyStatus,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = theme(cx);
        if status.routing.projects.is_empty() {
            return section_note(
                "No project has its own policy. Pick a project at the top of Settings to give it one.",
                &t,
                cx,
            );
        }
        let count = status.routing.projects.len();
        let mut list = section_container(&t);
        for (index, (daemon_id, policy)) in status.routing.projects.iter().enumerate() {
            let mirrored_id = format!("remote:{LOCAL_DAEMON_CONNECTION_ID}:{daemon_id}");
            let name = self
                .workspace
                .read(cx)
                .project(&mirrored_id)
                .map(|project| project.name.clone());
            let remove_id = daemon_id.clone();
            let row = settings_row_with_desc(
                format!("ai-project-policy-{daemon_id}"),
                name.as_deref().unwrap_or("Deleted project"),
                &policy_summary(policy, &status.accounts),
                &t,
                cx,
                index + 1 < count,
            )
            .child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .when(name.is_some(), |row| {
                        row.child(
                            button(format!("ai-project-policy-edit-{daemon_id}"), "Edit", &t)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.select_project(Some(mirrored_id.clone()), cx);
                                })),
                        )
                    })
                    .child(
                        button(
                            format!("ai-project-policy-remove-{daemon_id}"),
                            "Remove",
                            &t,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !busy {
                                this.ai_proxy_command(
                                    AiProxyCommand::SetProjectPolicy {
                                        project_id: remove_id.clone(),
                                        policy: None,
                                    },
                                    cx,
                                );
                            }
                        })),
                    ),
            );
            list = list.child(row);
        }
        list
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

/// The policy a [`SettingsPanel::render_policy_editor`] edits.
#[derive(Clone)]
enum PolicyTarget {
    Default,
    /// The local daemon's project id.
    Project(String),
}

impl PolicyTarget {
    fn command(&self, policy: AiRoutingPolicy) -> AiProxyCommand {
        match self {
            Self::Default => AiProxyCommand::SetDefaultPolicy { policy },
            Self::Project(project_id) => AiProxyCommand::SetProjectPolicy {
                project_id: project_id.clone(),
                policy: Some(policy),
            },
        }
    }
}

/// The local daemon's id of a mirrored project; `None` for a project of
/// another host, whose terminals use that host's proxy.
fn local_daemon_project_id(project_id: &str) -> Option<&str> {
    match project_id.strip_prefix("remote:") {
        None => Some(project_id),
        Some(rest) => rest
            .strip_prefix(LOCAL_DAEMON_CONNECTION_ID)?
            .strip_prefix(':'),
    }
}

/// One line that says which accounts a policy lets a project use.
fn policy_summary(policy: &AiRoutingPolicy, accounts: &[AiAccount]) -> String {
    let labels = |keep: &dyn Fn(AiAccountRule) -> bool| -> Vec<&str> {
        accounts
            .iter()
            .filter(|account| keep(policy.rule(&account.id)))
            .map(|account| account.label.as_str())
            .collect()
    };
    let permitted = labels(&|rule| rule != AiAccountRule::Forbid);
    let forbidden = labels(&|rule| rule == AiAccountRule::Forbid);
    let preferred = labels(&|rule| rule == AiAccountRule::Prefer);
    // With new accounts forbidden, the permitted ones are the whole list;
    // otherwise everything not forbidden is permitted, later accounts included.
    let mut parts = vec![match policy.others {
        AiAccountRule::Forbid if permitted.is_empty() => "No account".to_owned(),
        AiAccountRule::Forbid => format!("Only {}", permitted.join(", ")),
        _ if forbidden.is_empty() => "Any account".to_owned(),
        _ => format!("Never {}", forbidden.join(", ")),
    }];
    if !preferred.is_empty() {
        parts.push(format!("prefers {}", preferred.join(", ")));
    }
    parts.join(" \u{2022} ")
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

#[cfg(test)]
mod tests {
    use super::{LOCAL_DAEMON_CONNECTION_ID, local_daemon_project_id, policy_summary};
    use okena_core::ai_proxy::{AiAccount, AiAccountRule, AiRoutingPolicy};
    use std::collections::BTreeMap;

    fn account(id: &str) -> AiAccount {
        AiAccount {
            id: id.into(),
            label: id.to_uppercase(),
            email: None,
            plan: None,
            enabled: true,
            needs_login: false,
            cooldown_until_ms: None,
            five_hour: None,
            seven_day: None,
            requests: 0,
            last_used_ms: None,
        }
    }

    fn policy(others: AiAccountRule, rules: &[(&str, AiAccountRule)]) -> AiRoutingPolicy {
        AiRoutingPolicy {
            others,
            accounts: rules
                .iter()
                .map(|(id, rule)| (id.to_string(), *rule))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    #[test]
    fn summary_names_what_a_project_may_use() {
        use AiAccountRule::*;
        let accounts = [account("a"), account("b"), account("c")];
        let summary = |policy: AiRoutingPolicy| policy_summary(&policy, &accounts);
        assert_eq!(summary(AiRoutingPolicy::default()), "Any account");
        assert_eq!(summary(policy(Forbid, &[("a", Allow)])), "Only A");
        assert_eq!(
            summary(policy(Allow, &[("b", Prefer)])),
            "Any account \u{2022} prefers B"
        );
        assert_eq!(summary(policy(Allow, &[("c", Forbid)])), "Never C");
        assert_eq!(summary(policy(Forbid, &[])), "No account");
    }

    #[test]
    fn only_local_daemon_projects_have_a_daemon_id() {
        assert_eq!(
            local_daemon_project_id(&format!("remote:{LOCAL_DAEMON_CONNECTION_ID}:abc")),
            Some("abc")
        );
        assert_eq!(local_daemon_project_id("abc"), Some("abc"));
        assert_eq!(local_daemon_project_id("remote:laptop:abc"), None);
    }
}
