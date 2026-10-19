//! Usage of the AI proxy's account pool. While a host routes new shells through
//! the proxy, these accounts serve `claude`, not the `claude` login itself.

use crate::ui_helpers::capitalize_first;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::ai_proxy::{AiAccount, AiProxyCommand, AiProxyState, AiUsage};
use okena_core::api::{ActionRequest, ApiUsageWindow};
use okena_extensions::{ExtensionActions, ThemeColors};
use okena_transport::LOCAL_DAEMON_CONNECTION_ID;
use okena_ui::tokens::{ui_text_ms, ui_text_sm, ui_text_xs};
use okena_usage::history::{History, Provider};
use okena_usage::{
    SegmentUnit, Segments, TriggerItem, UsageRow, WorkingDays, effective_time_pct,
    format_reset_time_epoch, headline_color, history_points, now_secs, render_usage_bar,
    render_usage_row, reset_aligned_segments, usage_body_container, usage_trigger_items,
};
use okena_workspace::remote_sync::HostAiAccounts;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct AccountKey {
    pub connection_id: String,
    pub account_id: String,
}

impl AccountKey {
    pub fn new(host: &HostAiAccounts, account: &AiAccount) -> Self {
        Self {
            connection_id: host.connection_id.clone(),
            account_id: account.id.clone(),
        }
    }

    pub fn element_id(&self) -> String {
        format!(
            "claude-pool:{}:{}{}",
            self.connection_id.len(),
            self.connection_id,
            self.account_id
        )
    }
}

struct GlobalPoolHistory(WeakEntity<PoolHistory>);
impl Global for GlobalPoolHistory {}

#[derive(Default)]
pub(crate) struct AccountHistory {
    pub history: History,
    pub error: Option<String>,
    fetched_at: Option<Instant>,
    request: Option<Task<()>>,
}

/// Usage history of the pool accounts whose detail is open, fetched on demand
/// and shared across windows.
#[derive(Default)]
pub(crate) struct PoolHistory {
    pub entries: BTreeMap<AccountKey, AccountHistory>,
}

impl PoolHistory {
    pub fn shared(cx: &mut App) -> Entity<Self> {
        if let Some(existing) = cx
            .try_global::<GlobalPoolHistory>()
            .and_then(|g| g.0.upgrade())
        {
            return existing;
        }
        let entity = cx.new(|_| Self::default());
        cx.set_global(GlobalPoolHistory(entity.downgrade()));
        entity
    }

    pub fn request(&mut self, key: AccountKey, cx: &mut Context<Self>) {
        let entry = self.entries.entry(key.clone()).or_default();
        if entry.request.is_some()
            || entry
                .fetched_at
                .is_some_and(|at| at.elapsed() < Duration::from_secs(60))
        {
            return;
        }
        let response = ExtensionActions::send(
            &key.connection_id,
            ActionRequest::AiProxy {
                request: AiProxyCommand::UsageHistory {
                    account_id: key.account_id.clone(),
                },
            },
            cx,
        );
        let request_key = key.clone();
        let task = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let result = response
                .await
                .and_then(|value| parse_history(value, &key.account_id));
            let _ = this.update(cx, |this, cx| {
                let entry = this.entries.entry(key).or_default();
                entry.request = None;
                entry.fetched_at = Some(Instant::now());
                match result {
                    Ok(history) => {
                        entry.error = None;
                        entry.history = history;
                    }
                    Err(error) => {
                        entry.error = Some(error);
                    }
                }
                cx.notify();
            });
        });
        self.entries.entry(request_key).or_default().request = Some(task);
    }
}

fn parse_history(value: serde_json::Value, account_id: &str) -> Result<History, String> {
    let state: AiProxyState =
        serde_json::from_value(value).map_err(|error| format!("Invalid usage history: {error}"))?;
    if let Some(failure) = state.failure {
        return Err(failure);
    }
    let history = state
        .status
        .and_then(|status| status.usage_history)
        .ok_or("Usage history is unavailable from this proxy")?;
    if history.account_id != account_id
        || history
            .samples
            .iter()
            .any(|sample| sample.source != account_id || sample.provider != Provider::Claude)
    {
        return Err("Usage history belongs to another account".into());
    }
    Ok(History::from_samples(history.samples))
}

const FIVE_HOUR_SECS: f64 = 5.0 * 3600.0;
const SEVEN_DAY_SECS: f64 = 7.0 * 86400.0;

pub(crate) fn render_trigger(
    account: &AiAccount,
    working: WorkingDays,
    t: &ThemeColors,
    cx: &App,
) -> AnyElement {
    let now_ms = jiff::Timestamp::now().as_millisecond();
    let status = if account.needs_login || !account.enabled {
        account_status(account, now_ms)
    } else {
        account
            .cooldown_until_ms
            .filter(|until| *until > now_ms && account.exhausted_period(now_ms).is_none())
            .map(|_| "429 cooldown".to_string())
    };
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

/// One labeled bar per known window, with the grid and time marker of the
/// account detail, for the pool popover's account list.
pub(crate) fn window_bars(
    five_hour: Option<&ApiUsageWindow>,
    seven_day: Option<&ApiUsageWindow>,
    working: WorkingDays,
    marker: &str,
    t: &ThemeColors,
    cx: &App,
) -> Vec<AnyElement> {
    [
        ("5h", five_hour, SegmentUnit::Hour),
        ("7d", seven_day, SegmentUnit::Day),
    ]
    .into_iter()
    .filter_map(|(period, window, unit)| {
        let window = window?;
        let seconds = window.window_seconds;
        let segments = window.reset_at.map_or_else(Segments::default, |reset| {
            reset_aligned_segments(reset, seconds, unit, working)
        });
        let time = segments
            .time_pct
            .or_else(|| okena_usage::linear_time_pct(window.reset_at?, seconds, now_secs()?));
        Some(
            h_flex()
                .gap(px(8.0))
                .items_center()
                .child(
                    div()
                        .w(px(18.0))
                        .text_size(ui_text_xs(cx))
                        .text_color(rgb(t.text_muted))
                        .child(period),
                )
                .child(div().flex_1().child(render_usage_bar(
                    t,
                    window.used_percent,
                    time,
                    format!("{marker}-{period}-bar"),
                    &segments,
                )))
                .child(
                    div()
                        .w(px(34.0))
                        .flex()
                        .justify_end()
                        .text_size(ui_text_xs(cx))
                        .text_color(rgb(headline_color(t, window.used_percent, time)))
                        .child(format!("{:.0}%", window.used_percent)),
                )
                .into_any_element(),
        )
    })
    .collect()
}

pub(crate) fn render_body(
    host: &HostAiAccounts,
    account: &AiAccount,
    history: &History,
    working: WorkingDays,
    chart_state: &Entity<okena_usage::HistoryChartState>,
    t: &ThemeColors,
    cx: &App,
) -> Div {
    let host_name =
        (host.connection_id != LOCAL_DAEMON_CONNECTION_ID).then_some(host.host_name.as_str());
    let marker = AccountKey::new(host, account).element_id();
    let now_ms = jiff::Timestamp::now().as_millisecond();
    let mut title = account.label.clone();
    if let Some(host_name) = host_name {
        title = format!("{host_name} / {title}");
    }
    let plan = account.plan.as_deref().map(capitalize_first);
    let status = account_status(account, now_ms);
    let row = |label: &str, period: &str, usage: &AiUsage, period_secs: f64, unit: SegmentUnit| {
        let usage_row = UsageRow {
            label: label.to_owned().into(),
            period: period.to_owned().into(),
            pct: usage.utilization,
            time_pct: linear_time_pct(usage, period_secs),
            reset_epoch: reset_epoch(usage),
            period_secs,
            unit: Some(unit),
            marker_id: format!("{marker}-{period}").into(),
        };
        render_usage_row(
            t,
            cx,
            &usage_row,
            working,
            history_points(history, period, &usage_row),
            chart_state,
        )
        .into_any_element()
    };

    usage_body_container().child(
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
            }),
    )
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
    use super::{AccountKey, account_status, parse_history, trigger_items};
    use okena_core::ai_proxy::{AiAccount, AiUsage};
    use okena_usage::WorkingDays;

    fn account() -> AiAccount {
        AiAccount {
            id: "a".into(),
            label: "Work".into(),
            email: None,
            plan: None,
            rate_limit_tier: None,
            enabled: true,
            needs_login: false,
            cooldown_until_ms: None,
            five_hour: None,
            seven_day: None,
            requests: 0,
            last_used_ms: None,
        }
    }

    #[gpui::test]
    fn history_requests_are_shared_and_isolated_by_host(cx: &mut gpui::TestAppContext) {
        use super::PoolHistory;
        use gpui::Task;
        use okena_core::ai_proxy::AiProxyCommand;
        use okena_core::api::ActionRequest;
        use okena_extensions::ExtensionActions;
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let (first, second) = cx.update(|cx| {
            cx.set_global(ExtensionActions::new(move |connection, action, _| {
                counter.fetch_add(1, Ordering::SeqCst);
                let ActionRequest::AiProxy { request: AiProxyCommand::UsageHistory { account_id } } = action else {
                    panic!("expected a usage history request");
                };
                let percent = if connection == "local" { 23.0 } else { 71.0 };
                Task::ready(Ok(serde_json::json!({
                    "status": {
                        "version": "test", "inflight": 0, "accounts": [], "logins": [],
                        "usage_history": { "account_id": account_id, "samples": [{
                            "version": 1, "recorded_at": 1500.0, "provider": "claude", "source": account_id, "plan": null,
                            "limits": [{ "name": "5h", "used_percent": percent, "window_seconds": 18000.0, "reset_at": 19000.0 }]
                        }] }
                    }, "failure": null
                })))
            }));
            (PoolHistory::shared(cx), PoolHistory::shared(cx))
        });
        assert_eq!(first, second);
        let local = AccountKey {
            connection_id: "local".into(),
            account_id: "a".into(),
        };
        let remote = AccountKey {
            connection_id: "remote".into(),
            account_id: "a".into(),
        };
        cx.update(|cx| {
            first.update(cx, |history, cx| history.request(local.clone(), cx));
            second.update(cx, |history, cx| history.request(local.clone(), cx));
            second.update(cx, |history, cx| history.request(remote.clone(), cx));
        });
        cx.run_until_parked();
        cx.update(|cx| {
            second.update(cx, |history, cx| history.request(local.clone(), cx));
            let history = first.read(cx);
            for (key, percent) in [(local, 23.0), (remote, 71.0)] {
                let entry = &history.entries[&key];
                assert!(entry.error.is_none());
                assert_eq!(
                    entry.history.points("5h", 19000.0, 18000.0, 1600.0)[0].used_percent,
                    percent
                );
            }
        });
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn history_response_keeps_account_and_window_boundaries() {
        let response = serde_json::json!({
            "status": {
                "version": "test", "inflight": 0, "accounts": [], "logins": [],
                "usage_history": { "account_id": "a", "samples": [{
                    "version": 1, "recorded_at": 1500.0, "provider": "claude", "source": "a", "plan": null,
                    "limits": [{ "name": "5h", "used_percent": 23.0, "window_seconds": 18000.0, "reset_at": 19000.0 }]
                }] }
            }, "failure": null
        });
        let history = parse_history(response.clone(), "a").unwrap();
        assert_eq!(
            history.points("5h", 19000.0, 18000.0, 1600.0)[0].used_percent,
            23.0
        );
        assert!(history.points("7d", 19000.0, 604800.0, 1600.0).is_empty());
        assert!(parse_history(response.clone(), "b").is_err());
        let mut mixed = response.clone();
        mixed["status"]["usage_history"]["samples"][0]["source"] = "b".into();
        assert!(parse_history(mixed, "a").is_err());
        let mut legacy = response;
        legacy["status"]["usage_history"] = serde_json::Value::Null;
        assert!(parse_history(legacy, "a").is_err());
        assert!(
            parse_history(
                serde_json::json!({"status": null, "failure": "Offline"}),
                "a"
            )
            .is_err()
        );
    }

    #[test]
    fn account_keys_do_not_mix_hosts_or_ambiguous_ids() {
        let first = AccountKey {
            connection_id: "ab".into(),
            account_id: "c".into(),
        };
        let second = AccountKey {
            connection_id: "a".into(),
            account_id: "bc".into(),
        };
        assert_ne!(first, second);
        assert_ne!(first.element_id(), second.element_id());
        let remote = AccountKey {
            connection_id: "remote".into(),
            account_id: first.account_id.clone(),
        };
        assert_ne!(first, remote);
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
