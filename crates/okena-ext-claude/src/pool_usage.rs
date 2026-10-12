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

/// Both quota windows and availability for every enabled pool account.
pub(crate) fn render_trigger(
    hosts: &[HostAiAccounts],
    working: WorkingDays,
    t: &ThemeColors,
    cx: &App,
) -> Vec<AnyElement> {
    let now_ms = jiff::Timestamp::now().as_millisecond();
    let accounts: Vec<AnyElement> = hosts
        .iter()
        .flat_map(|host| &host.accounts)
        .filter(|account| account.enabled && !account.needs_login)
        .map(|account| {
            let status = account
                .cooldown_until_ms
                .filter(|until| *until > now_ms && account.exhausted_period(now_ms).is_none())
                .map(|_| "429 cooldown");
            let items = trigger_items(account, working);
            h_flex()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.text_muted))
                        .child(account.label.clone()),
                )
                .when_some(status, |el, status| {
                    el.child(
                        div()
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.warning))
                            .child(status),
                    )
                })
                .children(usage_trigger_items(t, cx, &items))
                .when(items.is_empty(), |el| {
                    el.child(
                        div()
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_muted))
                            .child("Usage unknown"),
                    )
                })
                .into_any_element()
        })
        .collect();
    if !accounts.is_empty() {
        return accounts;
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

fn trigger_items(account: &AiAccount, working: WorkingDays) -> Vec<TriggerItem> {
    [
        (
            "5h",
            account.five_hour.as_ref(),
            FIVE_HOUR_SECS,
            SegmentUnit::Hour,
        ),
        (
            "7d",
            account.seven_day.as_ref(),
            SEVEN_DAY_SECS,
            SegmentUnit::Day,
        ),
    ]
    .into_iter()
    .filter_map(|(period, usage, seconds, unit)| {
        let usage = usage?;
        Some((
            period.into(),
            usage.utilization,
            time_pct(usage, seconds, unit, working),
        ))
    })
    .collect()
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
    if let Some(period) = account.exhausted_period(now_ms) {
        return Some(format!("{period} limit reached"));
    }
    let until_ms = account.cooldown_until_ms.filter(|until| *until > now_ms)?;
    Some(format!(
        "429 cooldown until {}",
        format_reset_time_epoch(until_ms as f64 / 1000.0, true)
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
    use super::{account_status, trigger_items};
    use okena_core::ai_proxy::{AiAccount, AiUsage};
    use okena_usage::WorkingDays;

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
                .is_some_and(|status| status.starts_with("429 cooldown until"))
        );
    }

    #[test]
    fn zero_session_usage_does_not_hide_an_exhausted_weekly_quota() {
        let mut account = AiAccount {
            five_hour: Some(AiUsage {
                utilization: 0.0,
                resets_at_ms: None,
            }),
            seven_day: Some(AiUsage {
                utilization: 100.0,
                resets_at_ms: Some(2_000),
            }),
            ..account()
        };
        let items = trigger_items(&account, WorkingDays::all());
        assert_eq!(
            items
                .iter()
                .map(|(label, percent, _)| (label.as_ref(), *percent))
                .collect::<Vec<_>>(),
            [("5h", 0.0), ("7d", 100.0)]
        );
        assert_eq!(
            account_status(&account, 1_000).as_deref(),
            Some("7d limit reached")
        );
        assert_eq!(account_status(&account, 2_000), None);
        account.five_hour = None;
        assert_eq!(trigger_items(&account, WorkingDays::all()).len(), 1);
    }
}
