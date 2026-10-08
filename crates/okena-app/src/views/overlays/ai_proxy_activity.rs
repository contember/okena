//! AI proxy activity overlay (`ShowAiProxyActivity`): which pool accounts
//! answer `claude` requests, requests per minute, and the latest failovers and
//! errors. Polls the local daemon's AI proxy while open.

use crate::keybindings::Cancel;
use crate::remote::local::DaemonEndpoint;
use crate::settings::settings_entity;
use crate::theme::theme;
use crate::ui::tokens::{ui_text, ui_text_ms, ui_text_sm};
use crate::views::overlays::settings_panel::{format_remaining, send_ai_proxy_command};
use crate::workspace::state::Workspace;
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::ai_proxy::{
    AI_ACTIVITY_MINUTES, AiAccount, AiActivity, AiActivityEvent, AiActivityKind, AiProxyCommand,
    AiProxyState, AiProxyStatus, AiRouteActivity,
};
use okena_core::theme::ThemeColors;
use okena_transport::client::{LOCAL_DAEMON_CONNECTION_ID, make_prefixed_id};
use okena_ui::badge::keyboard_hints_footer;
use okena_ui::metrics::{SparklineStyle, sparkline};
use okena_ui::modal::{modal_backdrop, modal_content, modal_header};
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const HOUR_MS: i64 = 60 * 60 * 1000;

const SPARKLINE: SparklineStyle = SparklineStyle {
    slots: AI_ACTIVITY_MINUTES,
    height: px(16.0),
    bar_width: px(2.0),
    gap: px(1.0),
};

pub enum AiProxyActivityEvent {
    Close,
}

impl okena_ui::overlay::CloseEvent for AiProxyActivityEvent {
    fn is_close(&self) -> bool {
        matches!(self, Self::Close)
    }
}

pub struct AiProxyActivity {
    focus_handle: FocusHandle,
    workspace: Entity<Workspace>,
    daemon_endpoint: Option<DaemonEndpoint>,
    /// `None` until the first answer arrives.
    state: Option<AiProxyState>,
}

impl AiProxyActivity {
    pub fn new(
        workspace: Entity<Workspace>,
        daemon_endpoint: Option<DaemonEndpoint>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                let Ok(endpoint) = this.update(cx, |this, cx| this.endpoint_to_poll(cx)) else {
                    break;
                };
                if let Some(endpoint) = endpoint {
                    let state = cx
                        .background_executor()
                        .spawn(
                            async move { send_ai_proxy_command(&endpoint, AiProxyCommand::Status) },
                        )
                        .await;
                    let shown = this.update(cx, |this, cx| {
                        this.state = Some(state);
                        cx.notify();
                    });
                    if shown.is_err() {
                        break;
                    }
                }
                smol::Timer::after(POLL_INTERVAL).await;
            }
        })
        .detach();

        Self {
            focus_handle: cx.focus_handle(),
            workspace,
            daemon_endpoint,
            state: None,
        }
    }

    /// Like the Settings page, the overlay does not start a proxy that new
    /// terminals are not routed through.
    fn endpoint_to_poll(&self, cx: &App) -> Option<DaemonEndpoint> {
        if !settings_entity(cx).read(cx).settings.ai_proxy_enabled {
            return None;
        }
        self.daemon_endpoint.clone()
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(AiProxyActivityEvent::Close);
    }

    /// The local daemon's project name for an event, as the sidebar shows it.
    fn project_label(&self, project_id: Option<&str>, cx: &App) -> String {
        let Some(project_id) = project_id else {
            return "No project".to_owned();
        };
        self.workspace
            .read(cx)
            .project(&make_prefixed_id(LOCAL_DAEMON_CONNECTION_ID, project_id))
            .map_or_else(
                || "Deleted project".to_owned(),
                |project| project.name.clone(),
            )
    }
}

impl EventEmitter<AiProxyActivityEvent> for AiProxyActivity {}

impl Render for AiProxyActivity {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        if !focus_handle.is_focused(window) {
            window.focus(&focus_handle, cx);
        }
        let now = now_ms();
        let status = self.state.as_ref().and_then(|state| state.status.as_ref());
        let subtitle = status.map(|status| summary(&status.activity, now));

        let body = if self.daemon_endpoint.is_none() {
            note("The local daemon is not connected.", &t, cx).into_any_element()
        } else if !settings_entity(cx).read(cx).settings.ai_proxy_enabled {
            note(
                "The AI proxy is off. Turn it on in Settings under AI Proxy.",
                &t,
                cx,
            )
            .into_any_element()
        } else {
            match &self.state {
                None => note("Loading...", &t, cx).into_any_element(),
                Some(state) => self.render_state(state, now, &t, cx).into_any_element(),
            }
        };

        modal_backdrop("ai-proxy-activity-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("AiProxyActivity")
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| this.close(cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| this.close(cx)),
            )
            .child(
                modal_content("ai-proxy-activity-modal", &t)
                    .w(px(820.0))
                    .max_h(px(640.0))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(modal_header(
                        "AI Proxy Activity",
                        subtitle,
                        &t,
                        cx,
                        cx.listener(|this, _, _window, cx| this.close(cx)),
                    ))
                    .child(body)
                    .child(keyboard_hints_footer(&[("Esc", "close")], &t)),
            )
    }
}

impl AiProxyActivity {
    fn render_state(
        &self,
        state: &AiProxyState,
        now: i64,
        t: &ThemeColors,
        cx: &App,
    ) -> impl IntoElement {
        let mut content = v_flex().flex_1().min_h_0();
        if let Some(failure) = &state.failure {
            content = content.child(
                div()
                    .px(px(16.0))
                    .pt(px(10.0))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.error))
                    .child(failure.clone()),
            );
        }
        let Some(status) = &state.status else {
            return content;
        };
        content
            .child(section_title("Accounts", t, cx))
            .child(self.render_routes(status, now, t, cx))
            .child(section_title("Recent Events", t, cx))
            .child(self.render_events(status, now, t, cx))
    }

    fn render_routes(
        &self,
        status: &AiProxyStatus,
        now: i64,
        t: &ThemeColors,
        cx: &App,
    ) -> impl IntoElement {
        let rows = route_rows(status);
        let peak = rows
            .iter()
            .flat_map(|row| row.route.per_minute.iter())
            .copied()
            .max()
            .unwrap_or(0)
            .max(1) as f32;
        let mut table = v_flex().px(px(16.0)).child(
            table_row(t, cx)
                .text_color(rgb(t.text_muted))
                .child(div().flex_1().child("Account"))
                .child(div().w(px(150.0)).child("State"))
                .child(number_cell("Last min"))
                .child(number_cell("Last hour"))
                .child(number_cell("Errors"))
                .child(number_cell("429s"))
                .child(div().w(SPARKLINE.width()).child("Per minute")),
        );
        if rows.is_empty() {
            return table.child(note("No accounts and no requests yet.", t, cx));
        }
        for row in rows {
            let route = &row.route;
            let (state, state_color) = match row.account {
                Some(account) => account_state(account, now, t),
                None => ("\u{2014}".to_owned(), t.text_muted),
            };
            let values: Vec<f32> = route
                .per_minute
                .iter()
                .map(|count| *count as f32 / peak)
                .collect();
            table = table.child(
                table_row(t, cx)
                    .text_color(rgb(t.text_secondary))
                    .child(
                        div()
                            .flex_1()
                            .overflow_x_hidden()
                            .whitespace_nowrap()
                            .text_color(rgb(t.text_primary))
                            .child(row.label),
                    )
                    .child(
                        div()
                            .w(px(150.0))
                            .whitespace_nowrap()
                            .text_color(rgb(state_color))
                            .child(state),
                    )
                    .child(number_cell(last_minute(route).to_string()))
                    .child(number_cell(last_hour(route).to_string()))
                    .child(
                        number_cell(route.errors.to_string())
                            .when(route.errors > 0, |cell| cell.text_color(rgb(t.error))),
                    )
                    .child(
                        number_cell(route.rate_limited.to_string())
                            .when(route.rate_limited > 0, |cell| {
                                cell.text_color(rgb(t.warning))
                            }),
                    )
                    .child(sparkline(&values, SPARKLINE, t.term_blue, t)),
            );
        }
        table
    }

    fn render_events(
        &self,
        status: &AiProxyStatus,
        now: i64,
        t: &ThemeColors,
        cx: &App,
    ) -> impl IntoElement {
        let events = &status.activity.events;
        let mut list = v_flex()
            .id("ai-proxy-activity-events")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(16.0))
            .pb(px(8.0));
        if events.is_empty() {
            return list.child(note("No failovers or errors.", t, cx));
        }
        for event in events {
            list = list.child(self.render_event(event, &status.accounts, now, t, cx));
        }
        list
    }

    fn render_event(
        &self,
        event: &AiActivityEvent,
        accounts: &[AiAccount],
        now: i64,
        t: &ThemeColors,
        cx: &App,
    ) -> impl IntoElement {
        let color = match event.kind {
            AiActivityKind::RateLimited { .. } | AiActivityKind::Unauthorized { .. } => t.warning,
            _ => t.error,
        };
        table_row(t, cx)
            .items_start()
            .child(
                div()
                    .w(px(80.0))
                    .flex_shrink_0()
                    .text_color(rgb(t.text_muted))
                    .child(format_ago(now - event.at_ms)),
            )
            .child(
                div()
                    .w(px(150.0))
                    .flex_shrink_0()
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_color(rgb(t.text_secondary))
                    .child(self.project_label(event.project_id.as_deref(), cx)),
            )
            .child(
                div()
                    .flex_1()
                    .whitespace_normal()
                    .text_color(rgb(color))
                    .child(event_text(&event.kind, accounts)),
            )
            .when(event.count > 1, |row| {
                row.child(
                    div()
                        .flex_shrink_0()
                        .text_color(rgb(t.text_muted))
                        .child(format!("\u{d7}{}", event.count)),
                )
            })
    }
}

/// A route with the account it belongs to; `account: None` is the own login,
/// or an account removed within the last hour.
struct RouteRow<'a> {
    label: String,
    account: Option<&'a AiAccount>,
    route: AiRouteActivity,
}

/// Pool accounts in pool order (idle ones too), then routes of removed accounts,
/// then the terminals' own login.
fn route_rows(status: &AiProxyStatus) -> Vec<RouteRow<'_>> {
    let routes = &status.activity.routes;
    let route_of = |account_id: Option<&str>| {
        routes
            .iter()
            .find(|route| route.account_id.as_deref() == account_id)
            .cloned()
    };
    let mut rows: Vec<RouteRow> = status
        .accounts
        .iter()
        .map(|account| RouteRow {
            label: account_label(Some(&account.id), &status.accounts),
            account: Some(account),
            route: route_of(Some(&account.id)).unwrap_or_else(|| AiRouteActivity {
                account_id: Some(account.id.clone()),
                per_minute: vec![0; AI_ACTIVITY_MINUTES],
                errors: 0,
                rate_limited: 0,
            }),
        })
        .collect();
    for route in routes {
        let removed = route
            .account_id
            .as_ref()
            .is_some_and(|id| !status.accounts.iter().any(|account| &account.id == id));
        if removed {
            rows.push(RouteRow {
                label: account_label(route.account_id.as_deref(), &status.accounts),
                account: None,
                route: route.clone(),
            });
        }
    }
    if let Some(route) = route_of(None) {
        rows.push(RouteRow {
            label: account_label(None, &status.accounts),
            account: None,
            route,
        });
    }
    rows
}

fn account_state(account: &AiAccount, now: i64, t: &ThemeColors) -> (String, u32) {
    if account.needs_login {
        return ("Needs login".to_owned(), t.error);
    }
    if !account.enabled {
        return ("Disabled".to_owned(), t.text_muted);
    }
    if let Some(until) = account.cooldown_until_ms.filter(|until| *until > now) {
        return (
            format!("Rate limited {}", format_remaining(until - now)),
            t.warning,
        );
    }
    match &account.five_hour {
        Some(usage) => (
            format!("Ready \u{b7} 5h {:.0}%", usage.utilization),
            t.success,
        ),
        None => ("Ready".to_owned(), t.success),
    }
}

fn last_minute(route: &AiRouteActivity) -> u32 {
    route.per_minute.last().copied().unwrap_or(0)
}

fn last_hour(route: &AiRouteActivity) -> u32 {
    route.per_minute.iter().sum()
}

/// Header subtitle: totals over every route, and the period they cover.
fn summary(activity: &AiActivity, now: i64) -> String {
    let minute: u32 = activity.routes.iter().map(last_minute).sum();
    let hour: u32 = activity.routes.iter().map(last_hour).sum();
    let period = if now - activity.since_ms < HOUR_MS {
        format!(
            "since the proxy started {}",
            format_ago(now - activity.since_ms)
        )
    } else {
        "in the last hour".to_owned()
    };
    format!(
        "{minute} requests in the last minute \u{b7} {hour} {period} \u{b7} updates every {}s",
        POLL_INTERVAL.as_secs()
    )
}

fn account_label(account_id: Option<&str>, accounts: &[AiAccount]) -> String {
    let Some(account_id) = account_id else {
        return "Terminal's own login".to_owned();
    };
    accounts
        .iter()
        .find(|account| account.id == account_id)
        .map_or_else(
            || "Removed account".to_owned(),
            |account| account.label.clone(),
        )
}

fn event_text(kind: &AiActivityKind, accounts: &[AiAccount]) -> String {
    match kind {
        AiActivityKind::RateLimited { account_id } => format!(
            "{} rate limited, moved to the next account",
            account_label(Some(account_id), accounts)
        ),
        AiActivityKind::Unauthorized { account_id } => format!(
            "{} token rejected",
            account_label(Some(account_id), accounts)
        ),
        AiActivityKind::AllRateLimited => "Every permitted account is rate limited".to_owned(),
        AiActivityKind::NoPermittedAccount => {
            "The policy permits no enabled, logged-in account".to_owned()
        }
        AiActivityKind::Failed {
            account_id,
            status,
            message,
        } => {
            let reason = match (status, message) {
                (Some(status), Some(message)) => format!("{status} {message}"),
                (Some(status), None) => status.to_string(),
                (None, Some(message)) => message.clone(),
                (None, None) => "failed".to_owned(),
            };
            format!(
                "{}: {reason}",
                account_label(account_id.as_deref(), accounts)
            )
        }
    }
}

fn format_ago(ms: i64) -> String {
    if ms < 60_000 {
        "just now".to_owned()
    } else {
        format!("{} ago", format_remaining(ms))
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

fn table_row(t: &ThemeColors, cx: &App) -> Div {
    h_flex()
        .gap(px(12.0))
        .py(px(5.0))
        .items_center()
        .border_b_1()
        .border_color(rgb(t.border))
        .text_size(ui_text_ms(cx))
}

fn number_cell(text: impl Into<SharedString>) -> Div {
    div()
        .w(px(62.0))
        .flex_shrink_0()
        .flex()
        .justify_end()
        .child(text.into())
}

fn section_title(title: &'static str, t: &ThemeColors, cx: &App) -> impl IntoElement {
    div()
        .px(px(16.0))
        .pt(px(14.0))
        .pb(px(4.0))
        .text_size(ui_text_sm(cx))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(t.text_secondary))
        .child(title)
}

fn note(text: &'static str, t: &ThemeColors, cx: &App) -> impl IntoElement {
    div()
        .px(px(16.0))
        .py(px(12.0))
        .text_size(ui_text(13.0, cx))
        .text_color(rgb(t.text_muted))
        .child(text)
}

okena_ui::impl_focusable!(AiProxyActivity);

#[cfg(test)]
mod tests {
    use super::{AI_ACTIVITY_MINUTES, event_text, route_rows};
    use okena_core::ai_proxy::{
        AiAccount, AiActivity, AiActivityKind, AiProxyStatus, AiRouteActivity,
    };

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

    fn route(account_id: Option<&str>, last_minute: u32) -> AiRouteActivity {
        let mut per_minute = vec![0; AI_ACTIVITY_MINUTES];
        per_minute[AI_ACTIVITY_MINUTES - 1] = last_minute;
        AiRouteActivity {
            account_id: account_id.map(str::to_owned),
            per_minute,
            errors: 0,
            rate_limited: 0,
        }
    }

    #[test]
    fn rows_list_every_account_then_removed_ones_then_the_own_login() {
        let status = AiProxyStatus {
            accounts: vec![account("a"), account("b")],
            activity: AiActivity {
                since_ms: 0,
                routes: vec![route(None, 1), route(Some("b"), 2), route(Some("gone"), 3)],
                events: Vec::new(),
            },
            ..AiProxyStatus::default()
        };
        let rows: Vec<_> = route_rows(&status)
            .into_iter()
            .map(|row| (row.label, row.route.per_minute[AI_ACTIVITY_MINUTES - 1]))
            .collect();
        assert_eq!(
            rows,
            [
                ("A".to_owned(), 0),
                ("B".to_owned(), 2),
                ("Removed account".to_owned(), 3),
                ("Terminal's own login".to_owned(), 1),
            ]
        );
    }

    #[test]
    fn events_name_the_account_and_the_error() {
        let accounts = [account("a")];
        let failed = AiActivityKind::Failed {
            account_id: Some("a".into()),
            status: Some(529),
            message: Some("Overloaded".into()),
        };
        assert_eq!(event_text(&failed, &accounts), "A: 529 Overloaded");
        let unreachable = AiActivityKind::Failed {
            account_id: None,
            status: None,
            message: Some("No response from the provider".into()),
        };
        assert_eq!(
            event_text(&unreachable, &accounts),
            "Terminal's own login: No response from the provider"
        );
        let failover = AiActivityKind::RateLimited {
            account_id: "gone".into(),
        };
        assert_eq!(
            event_text(&failover, &accounts),
            "Removed account rate limited, moved to the next account"
        );
    }
}
