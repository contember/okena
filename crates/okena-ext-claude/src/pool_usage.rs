//! Usage of the AI proxy's account pool. While a host routes new shells through
//! the proxy, these accounts serve `claude`, not the `claude` login itself.

use crate::ui_helpers::capitalize_first;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::ai_proxy::{AiAccount, AiUsage};
use okena_extensions::ThemeColors;
use okena_transport::LOCAL_DAEMON_CONNECTION_ID;
use okena_ui::tokens::{ui_text_ms, ui_text_sm, ui_text_xs};
use okena_usage::{
    SegmentUnit, TriggerItem, UsageRow, WorkingDays, effective_time_pct, format_reset_time_epoch,
    now_secs, render_usage_row, usage_body_container, usage_divider, usage_trigger_items,
};
use okena_workspace::remote_sync::HostAiAccounts;

const FIVE_HOUR_SECS: f64 = 5.0 * 3600.0;
const SEVEN_DAY_SECS: f64 = 7.0 * 86400.0;

/// The 5-hour usage of every account the proxy may route to, or the account
/// count while none has reported usage yet.
pub(crate) fn render_trigger(
    hosts: &[HostAiAccounts],
    working: WorkingDays,
    t: &ThemeColors,
    cx: &App,
) -> Vec<AnyElement> {
    let items: Vec<TriggerItem> = hosts
        .iter()
        .flat_map(|host| &host.accounts)
        .filter(|account| account.enabled && !account.needs_login)
        .filter_map(|account| {
            let usage = account.five_hour.as_ref()?;
            Some((
                account.label.clone().into(),
                usage.utilization,
                time_pct(usage, FIVE_HOUR_SECS, SegmentUnit::Hour, working),
            ))
        })
        .collect();
    if !items.is_empty() {
        return usage_trigger_items(t, cx, &items);
    }
    let count: usize = hosts.iter().map(|host| host.accounts.len()).sum();
    vec![
        div()
            .text_size(ui_text_sm(cx))
            .text_color(rgb(t.text_muted))
            .child(format!(
                "Claude · {count} account{}",
                if count == 1 { "" } else { "s" }
            ))
            .into_any_element(),
    ]
}

pub(crate) fn render_body(
    hosts: &[HostAiAccounts],
    working: WorkingDays,
    chart_state: &Entity<okena_usage::HistoryChartState>,
    t: &ThemeColors,
    cx: &App,
) -> Div {
    let name_hosts = hosts.len() > 1
        || hosts
            .iter()
            .any(|host| host.connection_id != LOCAL_DAEMON_CONNECTION_ID);
    let now_ms = jiff::Timestamp::now().as_millisecond();
    let mut body = usage_body_container();
    for (index, (host, account)) in hosts
        .iter()
        .flat_map(|host| host.accounts.iter().map(move |account| (host, account)))
        .enumerate()
    {
        if index > 0 {
            body = body.child(usage_divider(t));
        }
        let host_name = name_hosts.then_some(host.host_name.as_str());
        let marker = format!("claude-pool-{}-{}", host.connection_id, account.id);
        body = body.child(render_account(
            account,
            host_name,
            &marker,
            now_ms,
            working,
            chart_state,
            t,
            cx,
        ));
    }
    body
}

#[allow(clippy::too_many_arguments)] // one account row's inputs, all needed to render it
fn render_account(
    account: &AiAccount,
    host_name: Option<&str>,
    marker: &str,
    now_ms: i64,
    working: WorkingDays,
    chart_state: &Entity<okena_usage::HistoryChartState>,
    t: &ThemeColors,
    cx: &App,
) -> impl IntoElement {
    let mut title = account.label.clone();
    if let Some(host_name) = host_name {
        title = format!("{host_name} / {title}");
    }
    let plan = account.plan.as_deref().map(capitalize_first);
    let status = account_status(account, now_ms);
    let row = |label: &str, period: &str, usage: &AiUsage, period_secs: f64, unit: SegmentUnit| {
        render_usage_row(
            t,
            cx,
            &UsageRow {
                label: label.to_owned().into(),
                period: period.to_owned().into(),
                pct: usage.utilization,
                time_pct: linear_time_pct(usage, period_secs),
                reset_epoch: reset_epoch(usage),
                period_secs,
                unit: Some(unit),
                marker_id: format!("{marker}-{period}").into(),
            },
            working,
            Vec::new(),
            chart_state,
        )
        .into_any_element()
    };

    v_flex()
        .gap(px(7.0))
        .child(
            h_flex()
                .items_baseline()
                .justify_between()
                .gap(px(8.0))
                .child(
                    h_flex()
                        .gap(px(6.0))
                        .items_baseline()
                        .child(
                            div()
                                .text_size(ui_text_ms(cx))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(t.text_primary))
                                .child(title),
                        )
                        .when_some(plan, |el, plan| {
                            el.child(
                                div()
                                    .text_size(ui_text_xs(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(plan),
                            )
                        }),
                )
                .when_some(status, |el, status| {
                    el.child(
                        div()
                            .text_size(ui_text_xs(cx))
                            .text_color(rgb(t.text_muted))
                            .child(status),
                    )
                }),
        )
        .when_some(account.five_hour.as_ref(), |el, usage| {
            el.child(row(
                "Session",
                "5h",
                usage,
                FIVE_HOUR_SECS,
                SegmentUnit::Hour,
            ))
        })
        .when_some(account.seven_day.as_ref(), |el, usage| {
            el.child(row("Weekly", "7d", usage, SEVEN_DAY_SECS, SegmentUnit::Day))
        })
}

/// Why the proxy does not route to the account right now, if it does not.
fn account_status(account: &AiAccount, now_ms: i64) -> Option<String> {
    if account.needs_login {
        return Some("Needs login".into());
    }
    if !account.enabled {
        return Some("Disabled".into());
    }
    let until_ms = account.cooldown_until_ms.filter(|until| *until > now_ms)?;
    Some(format!(
        "Rate limited until {}",
        format_reset_time_epoch(until_ms as f64 / 1000.0, false)
    ))
}

fn reset_epoch(usage: &AiUsage) -> Option<f64> {
    usage.resets_at_ms.map(|ms| ms as f64 / 1000.0)
}

fn linear_time_pct(usage: &AiUsage, period_secs: f64) -> Option<f64> {
    okena_usage::linear_time_pct(reset_epoch(usage)?, period_secs, now_secs()?)
}

fn time_pct(
    usage: &AiUsage,
    period_secs: f64,
    unit: SegmentUnit,
    working: WorkingDays,
) -> Option<f64> {
    effective_time_pct(
        reset_epoch(usage),
        period_secs,
        Some(unit),
        working,
        linear_time_pct(usage, period_secs),
    )
}

#[cfg(test)]
mod tests {
    use super::account_status;
    use okena_core::ai_proxy::AiAccount;

    fn account() -> AiAccount {
        AiAccount {
            id: "a".into(),
            label: "Work".into(),
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

    #[test]
    fn status_names_why_an_account_is_skipped() {
        assert_eq!(account_status(&account(), 1_000), None);
        let cooled_down = AiAccount {
            cooldown_until_ms: Some(500),
            ..account()
        };
        assert_eq!(account_status(&cooled_down, 1_000), None);
        let disabled = AiAccount {
            enabled: false,
            ..account()
        };
        assert_eq!(
            account_status(&disabled, 1_000).as_deref(),
            Some("Disabled")
        );
        let logged_out = AiAccount {
            needs_login: true,
            enabled: false,
            ..account()
        };
        assert_eq!(
            account_status(&logged_out, 1_000).as_deref(),
            Some("Needs login")
        );
        let limited = AiAccount {
            cooldown_until_ms: Some(2_000),
            ..account()
        };
        assert!(
            account_status(&limited, 1_000)
                .is_some_and(|status| status.starts_with("Rate limited until"))
        );
    }
}
